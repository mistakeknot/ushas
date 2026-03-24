//! MetalFX upscaling render graph node (spatial + temporal).
//!
//! Runs after `Node3d::Upscaling`. Creates its own output texture at full
//! resolution, uses MetalFX to upscale from `main_texture` (low-res) into it,
//! then blits to the swapchain via a render pass on `ViewTarget::out_texture()`.
//!
//! ## Architecture
//!
//! ```text
//! main_texture (low-res)
//!   → MetalFX upscale (spatial or temporal, raw Metal encode)
//!     → metalfx_output (full-res, our texture)
//!       → blit render pass → out_texture (swapchain)
//! ```
//!
//! ## Temporal Scaler Threading
//!
//! The temporal scaler's `newTemporalScalerWithDevice:` compiles ML pipelines
//! internally and can take several seconds. To avoid blocking the render thread,
//! scaler creation is dispatched to a background OS thread. The render node
//! polls for readiness each frame and falls through to Bevy's bilinear upscaling
//! until the scaler is ready.

use std::ffi::c_void;
use std::sync::Mutex;

use bevy::core_pipeline::blit::{BlitPipeline, BlitPipelineKey};
use bevy::core_pipeline::prepass::ViewPrepassTextures;
use bevy::prelude::*;
use bevy::render::camera::TemporalJitter;
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
use objc2_metal_fx::{MTLFXFrameInterpolator, MTLFXSpatialScaler, MTLFXTemporalScaler};

use crate::platform::{
    encode_spatial_upscale, encode_temporal_upscale, try_create_spatial_scaler_from_raw,
    wgpu_format_to_mtl,
};
use crate::MetalFxMode;

/// Resource holding the MetalFX render configuration.
#[derive(Resource, Clone, Copy)]
pub struct MetalFxConfig {
    pub render_scale: f32,
    pub mode: MetalFxMode,
}

/// Thread-safe wrapper for MetalFX scalers/interpolators.
pub(crate) enum SendScaler {
    Spatial(Retained<ProtocolObject<dyn MTLFXSpatialScaler>>),
    Temporal(Retained<ProtocolObject<dyn MTLFXTemporalScaler>>),
    FrameInterpolator(Retained<ProtocolObject<dyn MTLFXFrameInterpolator>>),
}

// Safety: Metal framework objects are thread-safe per Apple's Metal Best
// Practices Guide § "Metal and Multithread Safety".
unsafe impl Send for SendScaler {}
unsafe impl Sync for SendScaler {}

/// Cached state for the MetalFX upscale node.
struct CachedState {
    scaler: SendScaler,
    output_texture: bevy::render::render_resource::Texture,
    output_view: TextureView,
    /// Previous frame color texture for frame interpolation (ring buffer A).
    prev_color_texture: Option<bevy::render::render_resource::Texture>,
    input_w: u32,
    input_h: u32,
    output_w: u32,
    output_h: u32,
    frame_count: u64,
}

/// Pending temporal scaler creation on a background thread.
struct PendingScaler {
    receiver: std::sync::mpsc::Receiver<Option<SendScaler>>,
    input_w: u32,
    input_h: u32,
    output_w: u32,
    output_h: u32,
}

/// MetalFX upscaling ViewNode (spatial + temporal).
#[derive(Default)]
pub struct MetalFxUpscaleNode {
    cached: Mutex<Option<CachedState>>,
    pending: Mutex<Option<PendingScaler>>,
    cached_bind_group: Mutex<Option<(TextureViewId, BindGroup)>>,
    cached_pipeline: Mutex<Option<CachedRenderPipelineId>>,
}

impl ViewNode for MetalFxUpscaleNode {
    type ViewQuery = (
        &'static ViewTarget,
        Option<&'static ViewPrepassTextures>,
        Option<&'static TemporalJitter>,
    );

    fn run<'w>(
        &self,
        _graph: &mut RenderGraphContext,
        render_context: &mut RenderContext<'w>,
        (target, prepass_textures, temporal_jitter): bevy::ecs::query::QueryItem<
            'w,
            '_,
            Self::ViewQuery,
        >,
        world: &'w World,
    ) -> Result<(), NodeRunError> {
        let main_tex = target.main_texture();
        let main_size = main_tex.size();
        let main_format = main_tex.format();

        let Some(color_mtl_fmt) = wgpu_format_to_mtl(main_format) else {
            log::error!("MetalFxUpscaleNode: unsupported format {:?}", main_format);
            return Ok(());
        };

        let config = world.get_resource::<MetalFxConfig>();
        let render_scale = config.map_or(0.5, |c| c.render_scale);
        let mode = config.map_or(MetalFxMode::Spatial, |c| c.mode);

        // main_texture is at LOW resolution (set by MainPassResolutionOverride).
        // The full window resolution = main_texture_size / render_scale.
        // MetalFX upscales from main_texture size → full window size.
        let input_w = main_size.width;
        let input_h = main_size.height;
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
            // Check if a background scaler creation is pending.
            let mut pending = self.pending.lock().unwrap();
            if let Some(p) = pending.as_ref() {
                // Check if dimensions match what we need.
                if p.input_w == input_w && p.input_h == input_h
                    && p.output_w == output_w && p.output_h == output_h
                {
                    // Try to receive the scaler (non-blocking).
                    match p.receiver.try_recv() {
                        Ok(Some(scaler)) => {
                            log::info!(
                                "MetalFxUpscaleNode: background scaler ready {input_w}x{input_h} -> {output_w}x{output_h}"
                            );
                            *pending = None;

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

                            *self.cached_bind_group.lock().unwrap() = None;
                            *self.cached_pipeline.lock().unwrap() = None;

                            *cached = Some(CachedState {
                                scaler,
                                output_texture,
                                output_view,
                                prev_color_texture: None,
                                input_w,
                                input_h,
                                output_w,
                                output_h,
                                frame_count: 0,
                            });
                        }
                        Ok(None) => {
                            log::warn!("MetalFxUpscaleNode: background scaler creation failed");
                            *pending = None;
                            return Ok(());
                        }
                        Err(std::sync::mpsc::TryRecvError::Empty) => {
                            // Still creating — skip this frame.
                            return Ok(());
                        }
                        Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                            log::error!("MetalFxUpscaleNode: background thread panicked");
                            *pending = None;
                            return Ok(());
                        }
                    }
                } else {
                    // Dimensions changed, discard pending and start new.
                    *pending = None;
                }
            }

            // If no cached scaler and no pending creation, start one.
            if cached.is_none() && pending.is_none() {
                log::info!(
                    "MetalFxUpscaleNode: creating {:?} scaler {input_w}x{input_h} -> {output_w}x{output_h}",
                    mode
                );

                let wgpu_dev = device.wgpu_device();
                let Some(hal_dev) = (unsafe { wgpu_dev.as_hal::<wgpu_hal::metal::Api>() }) else {
                    log::error!("MetalFxUpscaleNode: no Metal HAL device");
                    return Ok(());
                };
                let device_ptr = {
                    let dev_lock = hal_dev.raw_device().lock();
                    dev_lock.as_ptr() as *mut c_void
                };

                match mode {
                    MetalFxMode::Spatial => {
                        // Spatial is fast — create synchronously.
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

                        *self.cached_bind_group.lock().unwrap() = None;
                        *self.cached_pipeline.lock().unwrap() = None;

                        *cached = Some(CachedState {
                            scaler: SendScaler::Spatial(scaler),
                            output_texture,
                            output_view,
                            prev_color_texture: None,
                            input_w,
                            input_h,
                            output_w,
                            output_h,
                            frame_count: 0,
                        });
                    }
                    MetalFxMode::Temporal | MetalFxMode::FrameInterpolation => {
                        // Temporal + FrameInterpolation are slow — create on background thread.
                        let (tx, rx) = std::sync::mpsc::channel();

                        let color_fmt_raw: usize = unsafe { std::mem::transmute(color_mtl_fmt) };
                        match mode {
                            MetalFxMode::Temporal => unsafe {
                                crate::platform::spawn_temporal_scaler_thread(
                                    device_ptr,
                                    input_w as usize, input_h as usize,
                                    output_w as usize, output_h as usize,
                                    color_fmt_raw, tx,
                                );
                            },
                            MetalFxMode::FrameInterpolation => unsafe {
                                crate::platform::spawn_frame_interpolator_thread(
                                    device_ptr,
                                    input_w as usize, input_h as usize,
                                    output_w as usize, output_h as usize,
                                    color_fmt_raw, tx,
                                );
                            },
                            _ => unreachable!(),
                        }

                        *pending = Some(PendingScaler {
                            receiver: rx,
                            input_w,
                            input_h,
                            output_w,
                            output_h,
                        });

                        return Ok(());
                    }
                    _ => {
                        log::warn!("MetalFxUpscaleNode: unsupported mode {:?}", mode);
                        return Ok(());
                    }
                }
            }

            // Still no cached scaler after all attempts — skip this frame.
            if cached.is_none() {
                return Ok(());
            }
        }

        let state = cached.as_mut().unwrap();

        // --- Phase B: MetalFX encode ---
        // CRITICAL: Extract ALL raw texture pointers in isolated scopes BEFORE
        // calling encoder.as_hal_mut(). wgpu uses a "snatch lock" internally;
        // calling as_hal() on textures while as_hal_mut() is active (or vice
        // versa) causes a recursive lock panic.
        let main_tex_ptr = {
            let Some(hal) = (unsafe { main_tex.as_hal::<wgpu_hal::metal::Api>() }) else {
                log::error!("MetalFxUpscaleNode: no Metal HAL for main texture");
                return Ok(());
            };
            unsafe { hal.raw_handle().as_ptr() as *mut c_void }
        }; // hal guard dropped here

        let out_tex_ptr = {
            let Some(hal) = (unsafe { state.output_texture.as_hal::<wgpu_hal::metal::Api>() }) else {
                log::error!("MetalFxUpscaleNode: no Metal HAL for output texture");
                return Ok(());
            };
            unsafe { hal.raw_handle().as_ptr() as *mut c_void }
        }; // hal guard dropped here

        let is_first_frame = state.frame_count == 0;
        state.frame_count += 1;

        // For temporal mode, also extract depth + motion vector pointers.
        let temporal_ptrs = match &state.scaler {
            SendScaler::Temporal(_) => {
                let Some(prepass) = prepass_textures else {
                    log::warn!("MetalFxUpscaleNode: temporal mode but no prepass textures");
                    return Ok(());
                };
                let Some(depth_attachment) = &prepass.depth else {
                    log::warn!("MetalFxUpscaleNode: no depth prepass texture");
                    return Ok(());
                };
                let Some(motion_attachment) = &prepass.motion_vectors else {
                    log::warn!("MetalFxUpscaleNode: no motion vector prepass texture");
                    return Ok(());
                };

                let depth_ptr = {
                    let Some(hal) = (unsafe { depth_attachment.texture.texture.as_hal::<wgpu_hal::metal::Api>() }) else {
                        log::error!("MetalFxUpscaleNode: no Metal HAL for depth texture");
                        return Ok(());
                    };
                    unsafe { hal.raw_handle().as_ptr() as *mut c_void }
                };

                let motion_ptr = {
                    let Some(hal) = (unsafe { motion_attachment.texture.texture.as_hal::<wgpu_hal::metal::Api>() }) else {
                        log::error!("MetalFxUpscaleNode: no Metal HAL for motion texture");
                        return Ok(());
                    };
                    unsafe { hal.raw_handle().as_ptr() as *mut c_void }
                };

                Some((depth_ptr, motion_ptr))
            }
            _ => None,
        };

        // For frame interpolation, extract prev color ptr (must be before as_hal_mut).
        let prev_color_ptr = match &state.scaler {
            SendScaler::FrameInterpolator(_) => {
                if state.prev_color_texture.is_none() {
                    let prev_tex = device.create_texture(&TextureDescriptor {
                        label: Some("metalfx_prev_color"),
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
                            | TextureUsages::COPY_DST,
                        view_formats: &[],
                    });
                    state.prev_color_texture = Some(prev_tex);
                }
                let prev_tex = state.prev_color_texture.as_ref().unwrap();
                let Some(hal) = (unsafe { prev_tex.as_hal::<wgpu_hal::metal::Api>() }) else {
                    log::error!("MetalFxUpscaleNode: no Metal HAL for prev color texture");
                    return Ok(());
                };
                Some(unsafe { hal.raw_handle().as_ptr() as *mut c_void })
            }
            _ => None,
        };

        // Now safe to acquire encoder's as_hal_mut — all texture guards dropped.
        let encoder = render_context.command_encoder();

        match &state.scaler {
            SendScaler::Spatial(scaler) => {
                unsafe {
                    encoder.as_hal_mut::<wgpu_hal::metal::Api, _, ()>(|hal_encoder| {
                        let Some(enc) = hal_encoder else { return };
                        let Some(cmd_buf) = enc.raw_command_buffer() else { return };
                        let cmd_buf_ptr = cmd_buf.as_ptr() as *mut c_void;

                        encode_spatial_upscale(
                            scaler,
                            main_tex_ptr,
                            out_tex_ptr,
                            cmd_buf_ptr,
                            input_w as usize,
                            input_h as usize,
                        );
                    });
                }
            }
            SendScaler::FrameInterpolator(interpolator) => {
                let (depth_ptr, motion_ptr) = temporal_ptrs.unwrap();
                let prev_color_ptr = prev_color_ptr.unwrap();
                let jitter_offset = temporal_jitter
                    .map(|j| j.offset)
                    .unwrap_or(Vec2::ZERO);
                let motion_scale_x = -(input_w as f32);
                let motion_scale_y = -(input_h as f32);

                // Camera params — use defaults for strategy game.
                // TODO: Extract from Bevy Projection component when available in ViewQuery.
                let delta_time = 1.0 / 60.0; // Approximate
                let field_of_view = 45.0_f32; // Degrees
                let aspect_ratio = output_w as f32 / output_h as f32;
                let near_plane = 0.1_f32;
                let far_plane = 1000.0_f32;

                unsafe {
                    encoder.as_hal_mut::<wgpu_hal::metal::Api, _, ()>(|hal_encoder| {
                        let Some(enc) = hal_encoder else { return };
                        let Some(cmd_buf) = enc.raw_command_buffer() else { return };
                        let cmd_buf_ptr = cmd_buf.as_ptr() as *mut c_void;

                        crate::platform::encode_frame_interpolation(
                            interpolator,
                            main_tex_ptr,
                            prev_color_ptr,
                            depth_ptr,
                            motion_ptr,
                            out_tex_ptr,
                            cmd_buf_ptr,
                            jitter_offset.x,
                            jitter_offset.y,
                            motion_scale_x,
                            motion_scale_y,
                            delta_time,
                            field_of_view,
                            aspect_ratio,
                            near_plane,
                            far_plane,
                            is_first_frame,
                        );
                    });
                }

                // After encoding, copy current color to prev for next frame.
                // The encoder will handle this as a GPU copy.
                // TODO: Implement GPU blit from main_texture to prev_color_texture.
            }
            SendScaler::Temporal(scaler) => {
                let (depth_ptr, motion_ptr) = temporal_ptrs.unwrap();
                let jitter_offset = temporal_jitter
                    .map(|j| j.offset)
                    .unwrap_or(Vec2::ZERO);

                let motion_scale_x = -(input_w as f32);
                let motion_scale_y = -(input_h as f32);

                unsafe {
                    encoder.as_hal_mut::<wgpu_hal::metal::Api, _, ()>(|hal_encoder| {
                        let Some(enc) = hal_encoder else { return };
                        let Some(cmd_buf) = enc.raw_command_buffer() else { return };
                        let cmd_buf_ptr = cmd_buf.as_ptr() as *mut c_void;

                        encode_temporal_upscale(
                            scaler,
                            main_tex_ptr,
                            depth_ptr,
                            motion_ptr,
                            out_tex_ptr,
                            cmd_buf_ptr,
                            input_w as usize,
                            input_h as usize,
                            jitter_offset.x,
                            jitter_offset.y,
                            motion_scale_x,
                            motion_scale_y,
                            is_first_frame,
                        );
                    });
                }
            }
        }

        // --- Phase C: Blit metalfx_output → out_texture (swapchain) ---
        let pipeline_cache = world.resource::<PipelineCache>();
        let blit_pipeline = world.resource::<BlitPipeline>();

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
