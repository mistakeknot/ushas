//! MetalFX spatial upscaling render graph node.
//!
//! Runs after `Node3d::Upscaling`. Creates its own output texture at full
//! resolution, uses MetalFX to upscale from `main_texture` (low-res) into it,
//! then blits to the swapchain via a render pass on `ViewTarget::out_texture()`.
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

use bevy::core_pipeline::blit::{BlitPipeline, BlitPipelineKey};
use bevy::prelude::*;
use bevy::render::render_graph::{NodeRunError, RenderGraphContext, ViewNode};
use bevy::render::render_resource::{
    BindGroup, CachedRenderPipelineId, Extent3d, PipelineCache, RenderPassDescriptor,
    SpecializedRenderPipeline, TextureDescriptor, TextureDimension, TextureUsages, TextureView,
    TextureViewId,
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
struct SendScaler(Retained<ProtocolObject<dyn MTLFXSpatialScaler>>);

// Safety: MTLFXSpatialScaler is a Metal framework object. All Metal objects
// are thread-safe per Apple's Metal Best Practices Guide.
unsafe impl Send for SendScaler {}
unsafe impl Sync for SendScaler {}

/// Cached state for the MetalFX upscale node.
struct CachedState {
    scaler: SendScaler,
    output_texture: bevy::render::render_resource::Texture,
    output_view: TextureView,
    input_w: u32,
    input_h: u32,
    output_w: u32,
    output_h: u32,
}

/// MetalFX spatial upscaling ViewNode.
///
/// Runs after `Node3d::Upscaling`, overwriting the bilinear-upscaled result
/// with Apple's ML-accelerated spatial upscaling.
#[derive(Default)]
pub struct MetalFxUpscaleNode {
    cached: Mutex<Option<CachedState>>,
    cached_bind_group: Mutex<Option<(TextureViewId, BindGroup)>>,
    cached_pipeline: Mutex<Option<CachedRenderPipelineId>>,
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

            // Invalidate cached bind group and pipeline since texture changed.
            *self.cached_bind_group.lock().unwrap() = None;
            *self.cached_pipeline.lock().unwrap() = None;

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

        // --- Phase C: Blit metalfx_output → out_texture (swapchain) ---
        let pipeline_cache = world.resource::<PipelineCache>();
        let blit_pipeline = world.resource::<BlitPipeline>();

        // Specialize + queue the blit pipeline for the output format.
        let mut cached_pipeline = self.cached_pipeline.lock().unwrap();
        let pipeline_id = match *cached_pipeline {
            Some(id) => id,
            None => {
                let key = BlitPipelineKey {
                    texture_format: target.out_texture_view_format(),
                    blend_state: None,
                    samples: 1,
                };
                let descriptor = blit_pipeline.specialize(key);
                let id = pipeline_cache.queue_render_pipeline(descriptor);
                *cached_pipeline = Some(id);
                id
            }
        };

        let Some(pipeline) = pipeline_cache.get_render_pipeline(pipeline_id) else {
            log::warn!("MetalFxUpscaleNode: blit pipeline not ready yet");
            drop(cached);
            return Ok(());
        };

        // Get or create bind group for our metalfx_output texture.
        let output_view = &state.output_view;
        let mut cached_bg = self.cached_bind_group.lock().unwrap();
        let bind_group = match &mut *cached_bg {
            Some((id, bg)) if output_view.id() == *id => bg,
            slot => {
                let bg = blit_pipeline.create_bind_group(
                    render_context.render_device(),
                    output_view,
                    pipeline_cache,
                );
                let (_, bg) = slot.insert((output_view.id(), bg));
                bg
            }
        };

        let pass_descriptor = RenderPassDescriptor {
            label: Some("metalfx_blit"),
            color_attachments: &[Some(target.out_texture_color_attachment(None))],
            depth_stencil_attachment: None,
            timestamp_writes: None,
            occlusion_query_set: None,
        };

        // Drop locks before starting the render pass (borrow checker).
        drop(cached);
        drop(cached_pipeline);

        let mut render_pass = render_context
            .command_encoder()
            .begin_render_pass(&pass_descriptor);

        render_pass.set_pipeline(pipeline);
        render_pass.set_bind_group(0, bind_group, &[]);
        render_pass.draw(0..3, 0..1);

        drop(render_pass);
        drop(cached_bg);

        Ok(())
    }
}
