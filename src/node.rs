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
    /// Content-sized input texture (copied from main_texture's top-left region).
    input_texture: bevy::render::render_resource::Texture,
    output_texture: bevy::render::render_resource::Texture,
    output_view: TextureView,
    /// Previous frame color texture for frame interpolation (ring buffer A).
    prev_color_texture: Option<bevy::render::render_resource::Texture>,
    /// Content-sized Depth32Float texture for temporal mode (written by depth resolve pass).
    content_depth_texture: Option<bevy::render::render_resource::Texture>,
    /// Stable view for the content depth texture (avoids per-frame view creation).
    content_depth_view: Option<TextureView>,
    /// Content-sized RG16Float texture for temporal mode (written by copy_texture_to_texture).
    content_motion_texture: Option<bevy::render::render_resource::Texture>,
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

/// Depth resolve render pipeline + bind group layout (single Mutex to prevent lock ordering issues).
struct DepthResolvePipeline {
    pipeline: wgpu::RenderPipeline,
    bind_group_layout: wgpu::BindGroupLayout,
}

/// MetalFX upscaling ViewNode (spatial + temporal).
pub struct MetalFxUpscaleNode {
    cached: Mutex<Option<CachedState>>,
    pending: Mutex<Option<PendingScaler>>,
    cached_bind_group: Mutex<Option<(TextureViewId, BindGroup)>>,
    cached_pipeline: Mutex<Option<CachedRenderPipelineId>>,
    /// Depth resolve render pipeline (lazy-init, resolution-independent).
    depth_resolve: Mutex<Option<DepthResolvePipeline>>,
    /// Cached bind group for depth resolve (keyed on src + dst TextureViewId).
    depth_resolve_bind_group: Mutex<Option<(TextureViewId, TextureViewId, wgpu::BindGroup)>>,
}

impl Default for MetalFxUpscaleNode {
    fn default() -> Self {
        Self {
            cached: Mutex::new(None),
            pending: Mutex::new(None),
            cached_bind_group: Mutex::new(None),
            cached_pipeline: Mutex::new(None),
            depth_resolve: Mutex::new(None),
            depth_resolve_bind_group: Mutex::new(None),
        }
    }
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

        // main_texture is full physical resolution (e.g., 3024x1800 on Retina).
        // MainPassResolutionOverride renders content at half-res in the top-left corner.
        //
        // MetalFX spatial scaler requires inputWidth to match the texture it reads from.
        // We create a content-sized input texture, GPU-copy the rendered region from
        // main_texture into it, then pass it to MetalFX for true upscaling:
        //   - input_texture: content_w × content_h (e.g., 1512×900)
        //   - output_texture: full_w × full_h (e.g., 3024×1800)
        //   - Scaler upscales input → output (2× ML upscale)
        let full_w = main_size.width;
        let full_h = main_size.height;
        let input_w = (full_w as f32 * render_scale).round() as u32;
        let input_h = (full_h as f32 * render_scale).round() as u32;
        let output_w = full_w;
        let output_h = full_h;
        let content_w = input_w;
        let content_h = input_h;

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

                            let input_texture = device.create_texture(&TextureDescriptor {
                                label: Some("metalfx_input"),
                                size: Extent3d {
                                    width: input_w,
                                    height: input_h,
                                    depth_or_array_layers: 1,
                                },
                                mip_level_count: 1,
                                sample_count: 1,
                                dimension: TextureDimension::D2,
                                format: main_format,
                                usage: TextureUsages::COPY_DST
                                    | TextureUsages::TEXTURE_BINDING,
                                view_formats: &[],
                            });

                            // Content-sized depth texture for temporal mode.
                            // Depth32Float — matches scaler's setDepthTextureFormat.
                            // Written via depth resolve render pass (@builtin(frag_depth)).
                            let (content_depth_texture, content_depth_view) = if matches!(scaler, SendScaler::Temporal(_) | SendScaler::FrameInterpolator(_)) {
                                let tex = device.create_texture(&TextureDescriptor {
                                    label: Some("metalfx_content_depth"),
                                    size: Extent3d {
                                        width: input_w,
                                        height: input_h,
                                        depth_or_array_layers: 1,
                                    },
                                    mip_level_count: 1,
                                    sample_count: 1,
                                    dimension: TextureDimension::D2,
                                    format: bevy::render::render_resource::TextureFormat::Depth32Float,
                                    usage: TextureUsages::RENDER_ATTACHMENT
                                        | TextureUsages::TEXTURE_BINDING,
                                    view_formats: &[],
                                });
                                let view = tex.create_view(
                                    &bevy::render::render_resource::TextureViewDescriptor::default(),
                                );
                                (Some(tex), Some(view))
                            } else {
                                (None, None)
                            };

                            // Content-sized motion vector texture for temporal mode.
                            // RG16Float supports copy_texture_to_texture.
                            let content_motion_texture = if matches!(scaler, SendScaler::Temporal(_) | SendScaler::FrameInterpolator(_)) {
                                Some(device.create_texture(&TextureDescriptor {
                                    label: Some("metalfx_content_motion"),
                                    size: Extent3d {
                                        width: input_w,
                                        height: input_h,
                                        depth_or_array_layers: 1,
                                    },
                                    mip_level_count: 1,
                                    sample_count: 1,
                                    dimension: TextureDimension::D2,
                                    format: bevy::render::render_resource::TextureFormat::Rg16Float,
                                    usage: TextureUsages::COPY_DST
                                        | TextureUsages::TEXTURE_BINDING,
                                    view_formats: &[],
                                }))
                            } else {
                                None
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
                            *self.depth_resolve_bind_group.lock().unwrap() = None;

                            *cached = Some(CachedState {
                                scaler,
                                input_texture,
                                output_texture,
                                output_view,
                                prev_color_texture: None,
                                content_depth_texture,
                                content_depth_view,
                                content_motion_texture,
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

                        // Content-sized input texture for MetalFX (GPU-copied from main_texture).
                        let input_texture = device.create_texture(&TextureDescriptor {
                            label: Some("metalfx_input"),
                            size: Extent3d {
                                width: input_w,
                                height: input_h,
                                depth_or_array_layers: 1,
                            },
                            mip_level_count: 1,
                            sample_count: 1,
                            dimension: TextureDimension::D2,
                            format: main_format,
                            usage: TextureUsages::COPY_DST
                                | TextureUsages::TEXTURE_BINDING
                                | TextureUsages::STORAGE_BINDING,
                            view_formats: &[],
                        });

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
                            input_texture,
                            output_texture,
                            output_view,
                            prev_color_texture: None,
                            content_depth_texture: None,
                            content_depth_view: None,
                            content_motion_texture: None,
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
                                // Create temporal scaler at content dimensions (not full-res).
                                // Depth and motion vectors are resolved to content-sized
                                // textures before being passed to MetalFX.
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

        // --- Phase B0: GPU-copy color content region into content-sized input texture ---
        // All modes now use the same path: copy the top-left content region from
        // main_texture into the content-sized input texture.
        render_context.command_encoder().copy_texture_to_texture(
            main_tex.as_image_copy(),
            state.input_texture.as_image_copy(),
            Extent3d {
                width: content_w,
                height: content_h,
                depth_or_array_layers: 1,
            },
        );

        // --- Phase B0.5: Temporal/FrameInterp — resolve depth + copy motion vectors ---
        // Bevy's prepass renders depth/motion at full physical resolution. We resolve
        // them into content-sized textures before passing to MetalFX.
        let is_temporal_like = matches!(
            state.scaler,
            SendScaler::Temporal(_) | SendScaler::FrameInterpolator(_)
        );

        if is_temporal_like {
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

            // Log prepass and content-sized dimensions on first frame.
            if state.frame_count == 0 {
                let depth_size = depth_attachment.texture.texture.size();
                let motion_size = motion_attachment.texture.texture.size();
                log::info!(
                    "MetalFxUpscaleNode temporal: prepass depth={}x{} ({:?}), motion={}x{} ({:?}), \
                     content-sized={}x{}, scaler input={}x{} -> output={}x{}",
                    depth_size.width, depth_size.height,
                    depth_attachment.texture.texture.format(),
                    motion_size.width, motion_size.height,
                    motion_attachment.texture.texture.format(),
                    content_w, content_h,
                    state.input_w, state.input_h,
                    state.output_w, state.output_h,
                );
            }

            // Copy motion vectors to content-sized texture (RG16Float supports copy).
            let content_motion = state.content_motion_texture.as_ref().unwrap();
            render_context.command_encoder().copy_texture_to_texture(
                motion_attachment.texture.texture.as_image_copy(),
                content_motion.as_image_copy(),
                Extent3d {
                    width: content_w,
                    height: content_h,
                    depth_or_array_layers: 1,
                },
            );

            // Resolve depth to content-sized Depth32Float via fragment shader render pass.
            // This block must be a separate scope — render pass guard must drop before
            // as_hal_mut is called for the MetalFX encode.
            let content_depth_view = state.content_depth_view.as_ref().unwrap();
            {
                // Lazy-init depth resolve render pipeline.
                let mut dr = self.depth_resolve.lock().unwrap();
                if dr.is_none() {
                    let wgpu_dev = device.wgpu_device();
                    let shader = wgpu_dev.create_shader_module(wgpu::ShaderModuleDescriptor {
                        label: Some("depth_resolve_shader"),
                        source: wgpu::ShaderSource::Wgsl(
                            include_str!("depth_resolve.wgsl").into(),
                        ),
                    });
                    let bgl = wgpu_dev.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                        label: Some("depth_resolve_bgl"),
                        entries: &[wgpu::BindGroupLayoutEntry {
                            binding: 0,
                            visibility: wgpu::ShaderStages::FRAGMENT,
                            ty: wgpu::BindingType::Texture {
                                sample_type: wgpu::TextureSampleType::Depth,
                                view_dimension: wgpu::TextureViewDimension::D2,
                                multisampled: false,
                            },
                            count: None,
                        }],
                    });
                    let pipeline_layout = wgpu_dev.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                        label: Some("depth_resolve_layout"),
                        bind_group_layouts: &[&bgl],
                        push_constant_ranges: &[],
                    });
                    let pipeline = wgpu_dev.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                        label: Some("depth_resolve_pipeline"),
                        layout: Some(&pipeline_layout),
                        vertex: wgpu::VertexState {
                            module: &shader,
                            entry_point: Some("vs_main"),
                            buffers: &[],
                            compilation_options: Default::default(),
                        },
                        fragment: Some(wgpu::FragmentState {
                            module: &shader,
                            entry_point: Some("fs_main"),
                            targets: &[],
                            compilation_options: Default::default(),
                        }),
                        primitive: wgpu::PrimitiveState {
                            topology: wgpu::PrimitiveTopology::TriangleList,
                            ..Default::default()
                        },
                        depth_stencil: Some(wgpu::DepthStencilState {
                            format: wgpu::TextureFormat::Depth32Float,
                            depth_write_enabled: true,
                            depth_compare: wgpu::CompareFunction::Always,
                            stencil: Default::default(),
                            bias: Default::default(),
                        }),
                        multisample: Default::default(),
                        multiview: None,
                        cache: None,
                    });
                    *dr = Some(DepthResolvePipeline {
                        pipeline,
                        bind_group_layout: bgl,
                    });
                }
                let dr_ref = dr.as_ref().unwrap();

                // Create source depth view (prepass texture — changes if prepass is recreated).
                // Destination view is stored in CachedState (stable across frames).
                let src_depth_view = depth_attachment.texture.texture.create_view(
                    &bevy::render::render_resource::TextureViewDescriptor::default(),
                );

                // Get or create cached bind group (keyed on src + dst TextureViewId).
                // dst_id is stable (stored in CachedState), src_id changes on prepass recreation.
                let mut dr_bg = self.depth_resolve_bind_group.lock().unwrap();
                let need_new_bg = match &*dr_bg {
                    Some((src_id, dst_id, _))
                        if *src_id == src_depth_view.id() && *dst_id == content_depth_view.id() =>
                    {
                        false
                    }
                    _ => true,
                };
                if need_new_bg {
                    let wgpu_dev = device.wgpu_device();
                    // Extract the raw wgpu TextureView from Bevy's wrapped type.
                    let src_view_wgpu: &wgpu::TextureView = &src_depth_view;
                    let bg = wgpu_dev.create_bind_group(&wgpu::BindGroupDescriptor {
                        label: Some("depth_resolve_bg"),
                        layout: &dr_ref.bind_group_layout,
                        entries: &[wgpu::BindGroupEntry {
                            binding: 0,
                            resource: wgpu::BindingResource::TextureView(src_view_wgpu),
                        }],
                    });
                    *dr_bg = Some((src_depth_view.id(), content_depth_view.id(), bg));
                }
                let bind_group = &dr_bg.as_ref().unwrap().2;

                // Dispatch depth resolve render pass.
                let mut pass = render_context.command_encoder().begin_render_pass(
                    &RenderPassDescriptor {
                        label: Some("metalfx_depth_resolve"),
                        color_attachments: &[],
                        depth_stencil_attachment: Some(
                            bevy::render::render_resource::RenderPassDepthStencilAttachment {
                                view: content_depth_view,
                                depth_ops: Some(bevy::render::render_resource::Operations {
                                    load: bevy::render::render_resource::LoadOp::Clear(0.0),
                                    store: bevy::render::render_resource::StoreOp::Store,
                                }),
                                stencil_ops: None,
                            },
                        ),
                        timestamp_writes: None,
                        occlusion_query_set: None,
                    },
                );
                pass.set_pipeline(&dr_ref.pipeline);
                pass.set_bind_group(0, bind_group, &[]);
                pass.set_viewport(
                    0.0, 0.0,
                    content_w as f32, content_h as f32,
                    0.0, 1.0,
                );
                pass.draw(0..3, 0..1);
                // pass drops here → render encoder ends
            }
            // dr and dr_bg guards also dropped here
        }

        // --- Phase B: MetalFX encode ---
        // CRITICAL: Extract ALL raw texture pointers in isolated scopes BEFORE
        // calling encoder.as_hal_mut(). wgpu uses a "snatch lock" internally;
        // calling as_hal() on textures while as_hal_mut() is active (or vice
        // versa) causes a recursive lock panic.

        let input_tex_ptr = {
            let Some(hal) = (unsafe { state.input_texture.as_hal::<wgpu_hal::metal::Api>() }) else {
                log::error!("MetalFxUpscaleNode: no Metal HAL for input texture");
                return Ok(());
            };
            unsafe { hal.raw_handle().as_ptr() as *mut c_void }
        };

        let out_tex_ptr = {
            let Some(hal) = (unsafe { state.output_texture.as_hal::<wgpu_hal::metal::Api>() }) else {
                log::error!("MetalFxUpscaleNode: no Metal HAL for output texture");
                return Ok(());
            };
            unsafe { hal.raw_handle().as_ptr() as *mut c_void }
        };

        let is_first_frame = state.frame_count == 0;
        state.frame_count += 1;

        // Extract temporal texture pointers (content-sized depth + motion).
        let temporal_ptrs = if is_temporal_like {
            let content_depth = state.content_depth_texture.as_ref().unwrap();
            let content_motion = state.content_motion_texture.as_ref().unwrap();

            let depth_ptr = {
                let Some(hal) = (unsafe { content_depth.as_hal::<wgpu_hal::metal::Api>() }) else {
                    log::error!("MetalFxUpscaleNode: no Metal HAL for content depth texture");
                    return Ok(());
                };
                unsafe { hal.raw_handle().as_ptr() as *mut c_void }
            };

            let motion_ptr = {
                let Some(hal) = (unsafe { content_motion.as_hal::<wgpu_hal::metal::Api>() }) else {
                    log::error!("MetalFxUpscaleNode: no Metal HAL for content motion texture");
                    return Ok(());
                };
                unsafe { hal.raw_handle().as_ptr() as *mut c_void }
            };

            Some((depth_ptr, motion_ptr))
        } else {
            None
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
                            input_tex_ptr,
                            out_tex_ptr,
                            cmd_buf_ptr,
                            content_w as usize,
                            content_h as usize,
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
                            input_tex_ptr,
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
                            input_tex_ptr,
                            depth_ptr,
                            motion_ptr,
                            out_tex_ptr,
                            cmd_buf_ptr,
                            content_w as usize,
                            content_h as usize,
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
