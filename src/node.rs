//! MetalFX spatial upscaling render graph node.
//!
//! Implements `ViewNode` to run at `Node3d::Upscaling`. Creates its own
//! output texture at full resolution, uses MetalFX to upscale from
//! `main_texture` (low-res) into it, then blits to the swapchain via
//! a render pass on `ViewTarget::out_texture()`.
//!
//! ## Architecture
//!
//! ```text
//! main_texture (low-res)
//!   → MetalFX spatial upscale (raw Metal encode)
//!     → metalfx_output (full-res, our texture)
//!       → blit render pass → out_texture (swapchain)
//! ```

use std::ffi::c_void;
use std::sync::Mutex;

use bevy::prelude::*;
use bevy::render::render_graph::{NodeRunError, RenderGraphContext, ViewNode};
use bevy::render::render_resource::{
    Extent3d, TextureDescriptor, TextureDimension, TextureUsages, TextureView,
};
use bevy::render::renderer::RenderContext;
use bevy::render::view::ViewTarget;
use foreign_types::ForeignType;
use objc2::rc::Retained;
use objc2::runtime::ProtocolObject;
use objc2_metal_fx::MTLFXSpatialScaler;

use crate::{encode_spatial_upscale, try_create_spatial_scaler_from_raw, wgpu_format_to_mtl};

/// Resource holding the MetalFX render configuration.
#[derive(Resource, Clone, Copy)]
pub struct MetalFxConfig {
    pub render_scale: f32,
}

/// Thread-safe wrapper for the MetalFX spatial scaler.
///
/// Metal objects are thread-safe by design — the Metal runtime serializes
/// access internally. objc2 doesn't mark protocol objects as Send/Sync
/// because it can't verify this for arbitrary protocols, but MTLFXSpatialScaler
/// is guaranteed safe by Apple's documentation.
struct SendScaler(Retained<ProtocolObject<dyn MTLFXSpatialScaler>>);

// Safety: MTLFXSpatialScaler is a Metal framework object. All Metal objects
// are thread-safe per Apple's Metal Best Practices Guide.
unsafe impl Send for SendScaler {}
unsafe impl Sync for SendScaler {}

/// Cached state for the MetalFX upscale node.
struct CachedState {
    scaler: SendScaler,
    /// Our full-resolution output texture (MetalFX writes here).
    output_texture: bevy::render::render_resource::Texture,
    #[allow(dead_code)]
    output_view: TextureView,
    input_w: u32,
    input_h: u32,
    output_w: u32,
    output_h: u32,
}

/// MetalFX spatial upscaling ViewNode.
///
/// Runs at `Node3d::Upscaling`, replacing Bevy's built-in blit upscaler
/// with Apple's ML-accelerated spatial upscaling.
#[derive(Default)]
pub struct MetalFxUpscaleNode {
    cached: Mutex<Option<CachedState>>,
}

impl ViewNode for MetalFxUpscaleNode {
    type ViewQuery = &'static ViewTarget;

    fn run<'w>(
        &self,
        _graph: &mut RenderGraphContext,
        render_context: &mut RenderContext<'w>,
        target: bevy::ecs::query::QueryItem<'w, '_, Self::ViewQuery>,
        world: &'w World,
    ) -> Result<(), NodeRunError> {
        let main_tex = target.main_texture();
        let main_size = main_tex.size();
        let input_w = main_size.width;
        let input_h = main_size.height;
        let main_format = main_tex.format();

        let Some(color_mtl_fmt) = wgpu_format_to_mtl(main_format) else {
            log::error!("MetalFxUpscaleNode: unsupported format {:?}", main_format);
            return Ok(());
        };

        // Infer output dimensions: input = render_scale * output
        let config = world.get_resource::<MetalFxConfig>();
        let render_scale = config.map_or(0.5, |c| c.render_scale);
        let output_w = (input_w as f32 / render_scale).round() as u32;
        let output_h = (input_h as f32 / render_scale).round() as u32;

        // --- Phase A: Get or create scaler + output texture ---
        let device = render_context.render_device().clone();
        let mut cached = self.cached.lock().unwrap();

        let needs_recreate = cached.as_ref().map_or(true, |c| {
            c.input_w != input_w
                || c.input_h != input_h
                || c.output_w != output_w
                || c.output_h != output_h
        });

        if needs_recreate {
            log::info!(
                "MetalFxUpscaleNode: creating scaler {input_w}x{input_h} -> {output_w}x{output_h}"
            );

            // Get raw Metal device pointer.
            let wgpu_dev = device.wgpu_device();
            let Some(hal_dev) = (unsafe { wgpu_dev.as_hal::<wgpu_hal::metal::Api>() }) else {
                log::error!("MetalFxUpscaleNode: no Metal HAL device");
                return Ok(());
            };
            let dev_lock = hal_dev.raw_device().lock();
            let device_ptr = dev_lock.as_ptr() as *mut c_void;

            let scaler = unsafe {
                try_create_spatial_scaler_from_raw(
                    device_ptr,
                    input_w as usize,
                    input_h as usize,
                    output_w as usize,
                    output_h as usize,
                    color_mtl_fmt,
                    color_mtl_fmt,
                )
            };
            let Some(scaler) = scaler else {
                log::error!("MetalFxUpscaleNode: failed to create spatial scaler");
                return Ok(());
            };

            // Create our output texture at full resolution.
            let output_texture = device.create_texture(&TextureDescriptor {
                label: Some("metalfx_output"),
                size: Extent3d {
                    width: output_w,
                    height: output_h,
                    depth_or_array_layers: 1,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: TextureDimension::D2,
                format: main_format,
                usage: TextureUsages::RENDER_ATTACHMENT
                    | TextureUsages::TEXTURE_BINDING
                    | TextureUsages::STORAGE_BINDING,
                view_formats: &[],
            });
            let output_view = output_texture.create_view(
                &bevy::render::render_resource::TextureViewDescriptor::default(),
            );

            *cached = Some(CachedState {
                scaler: SendScaler(scaler),
                output_texture,
                output_view,
                input_w,
                input_h,
                output_w,
                output_h,
            });
        }

        let state = cached.as_ref().unwrap();

        // --- Phase B: MetalFX encode ---
        // Extract raw Metal texture pointers.
        let Some(hal_main_tex) =
            (unsafe { main_tex.as_hal::<wgpu_hal::metal::Api>() })
        else {
            log::error!("MetalFxUpscaleNode: no Metal HAL for main texture");
            return Ok(());
        };
        let main_tex_ptr = unsafe { hal_main_tex.raw_handle().as_ptr() } as *mut c_void;

        let Some(hal_out_tex) =
            (unsafe { state.output_texture.as_hal::<wgpu_hal::metal::Api>() })
        else {
            log::error!("MetalFxUpscaleNode: no Metal HAL for output texture");
            return Ok(());
        };
        let out_tex_ptr = unsafe { hal_out_tex.raw_handle().as_ptr() } as *mut c_void;

        // Get the command encoder and encode the MetalFX upscale.
        let encoder = render_context.command_encoder();
        unsafe {
            encoder.as_hal_mut::<wgpu_hal::metal::Api, _, ()>(|hal_encoder| {
                let Some(enc) = hal_encoder else {
                    log::error!("MetalFxUpscaleNode: no Metal command encoder");
                    return;
                };
                let Some(cmd_buf) = enc.raw_command_buffer() else {
                    log::error!("MetalFxUpscaleNode: no raw command buffer");
                    return;
                };
                let cmd_buf_ptr = cmd_buf.as_ptr() as *mut c_void;

                encode_spatial_upscale(
                    &state.scaler.0,
                    main_tex_ptr,
                    out_tex_ptr,
                    cmd_buf_ptr,
                    input_w as usize,
                    input_h as usize,
                );
            });
        }

        // Drop the cached lock before any further rendering.
        drop(cached);

        // --- Phase C: Blit upscaled texture → swapchain ---
        // The existing UpscalingNode runs after us and blits main_texture
        // to out_texture. In Phase 2b we'll replace it entirely with our
        // own blit from metalfx_output → out_texture.

        Ok(())
    }
}
