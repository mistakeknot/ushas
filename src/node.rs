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
//! ## Frame Interpolation
//!
//! `FrameInterpolation` is *additive* to temporal upscaling, not an alternative
//! to it. `MTLFXFrameInterpolator` consumes two consecutive **upscaled** frames,
//! so the mode holds both objects and encodes two stages per frame:
//!
//! ```text
//! main_texture (low-res) ─┬→ temporal upscale ──→ metalfx_output (full-res)
//!                         │                          │        │
//!   content depth+motion ─┴──────────────→ interpolate│        └→ swapchain
//!                                              ↑      ↓
//!                     metalfx_prev_color ──────┘  metalfx_interp_output
//!                            ↖───────── copy ────────┘  (computed, unpresented)
//! ```
//!
//! The descriptor's `inputWidth`/`inputHeight` describe only the depth and
//! motion textures; the color textures are at **output** size. Sizing them to
//! the render resolution trips a MetalFX debug-layer assertion ("Color texture
//! width mismatch from descriptor").
//!
//! Presenting the interpolated frame needs a second present per update, which a
//! Bevy render graph does not do. `present.rs` implements that on a
//! `CAMetalLayer` of its own; it is opt-in and **not yet validated** — every
//! measurement so far ran on a locked, display-asleep machine, which cannot
//! present frames at all. See `docs/m5-max-performance-research.md`.
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
use crate::gpu_timing::add_gpu_timing_handler;
use crate::{GpuTimingDiag, MetalFxMode};

/// Resource holding the MetalFX render configuration.
/// Extracted from main world each frame via `ExtractResourcePlugin`.
#[derive(Resource, Clone, Copy, bevy::render::extract_resource::ExtractResource)]
pub struct MetalFxConfig {
    pub render_scale: f32,
    pub mode: MetalFxMode,
    /// When `Some((min, max))`, the temporal scaler is created with true dynamic
    /// resolution enabled spanning that render-scale range, so an adaptive
    /// governor can flex `render_scale` within `[min, max]` without rebuilding
    /// the scaler. `None` = fixed-scale scaler (recreated only on window resize).
    pub dynamic_res_range: Option<(f32, f32)>,
}

/// Per-frame wall-clock delta, mirrored into the render world.
///
/// `MTLFXFrameInterpolator::setDeltaTime` wants "the length of the time
/// interval, in seconds, between time of current and previous frame". The
/// render world has no `Time` resource of its own (`bevy_time` only plumbs an
/// `Instant` channel between the worlds), so the main world copies its delta
/// into this resource and `ExtractResourcePlugin` carries it across.
///
/// Only the frame-interpolation path reads it; other modes ignore it.
#[derive(Resource, Clone, Copy, bevy::render::extract_resource::ExtractResource)]
pub struct MetalFxFrameTiming {
    /// Seconds elapsed since the previous frame.
    pub delta_seconds: f32,
}

impl Default for MetalFxFrameTiming {
    fn default() -> Self {
        // 60 Hz until the first real frame delta arrives.
        Self {
            delta_seconds: 1.0 / 60.0,
        }
    }
}

/// Thread-safe wrapper for MetalFX scalers/interpolators.
pub(crate) enum SendScaler {
    Spatial(Retained<ProtocolObject<dyn MTLFXSpatialScaler>>),
    Temporal(Retained<ProtocolObject<dyn MTLFXTemporalScaler>>),
    /// Frame interpolation is a *two-stage* pipeline, not an alternative to
    /// upscaling: the temporal scaler produces the full-res frame, and the
    /// interpolator synthesises an intermediate frame from two consecutive
    /// full-res frames. Both objects are held together for the life of the node.
    FrameInterpolator {
        scaler: Retained<ProtocolObject<dyn MTLFXTemporalScaler>>,
        interpolator: Retained<ProtocolObject<dyn MTLFXFrameInterpolator>>,
    },
}

// Safety: Metal framework objects are thread-safe per Apple's Metal Best
// Practices Guide § "Metal and Multithread Safety".
unsafe impl Send for SendScaler {}
unsafe impl Sync for SendScaler {}

/// Pixel format for the owned presentation layer and its staging textures.
///
/// `CAMetalLayer` accepts BGRA channel order only — setting it to the view's
/// RGBA format makes CoreAnimation accept presents and then silently skip them.
#[cfg(feature = "frame-interpolation")]
const PRESENT_FORMAT: bevy::render::render_resource::TextureFormat =
    bevy::render::render_resource::TextureFormat::Bgra8UnormSrgb;

/// Cached state for the MetalFX upscale node.
struct CachedState {
    scaler: SendScaler,
    /// Content-sized input texture (copied from main_texture's top-left region).
    input_texture: bevy::render::render_resource::Texture,
    output_texture: bevy::render::render_resource::Texture,
    output_view: TextureView,
    /// Previous frame's *upscaled* color, at output resolution — the history
    /// input for frame interpolation.
    prev_color_texture: Option<bevy::render::render_resource::Texture>,
    /// Destination for the synthesised intermediate frame (frame interpolation
    /// only), at output resolution. Kept separate from `output_texture` so the
    /// real upscaled frame survives for presentation and for the history copy.
    interp_output_texture: Option<bevy::render::render_resource::Texture>,
    /// Stable view of the synthesised frame, used as the blit source when it is
    /// drawn into its own drawable for presentation.
    interp_output_view: Option<TextureView>,
    /// BGRA staging copies of the two frames, for the owned-layer present.
    ///
    /// `CAMetalLayer` supports BGRA channel order only, while MetalFX writes in
    /// the view's format (RGBA here). A blit copy cannot convert channel order,
    /// so each frame is first drawn into a BGRA texture by the same fullscreen
    /// blit pass Bevy uses for its own swapchain — which does the conversion for
    /// free — and the drawable copy is then format-identical.
    interp_bgra: Option<bevy::render::render_resource::Texture>,
    interp_bgra_view: Option<TextureView>,
    real_bgra: Option<bevy::render::render_resource::Texture>,
    real_bgra_view: Option<TextureView>,
    /// Content-sized Depth32Float texture for temporal mode (written by depth resolve pass).
    content_depth_texture: Option<bevy::render::render_resource::Texture>,
    /// Stable view for the content depth texture (avoids per-frame view creation).
    content_depth_view: Option<TextureView>,
    /// Content-sized RG16Float texture for temporal mode (written by motion resolve pass).
    content_motion_texture: Option<bevy::render::render_resource::Texture>,
    /// Stable view for the content motion texture.
    content_motion_view: Option<TextureView>,
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

/// Render pipeline + bind group layout for prepass texture resolve.
struct ResolvePipeline {
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
    depth_resolve: Mutex<Option<ResolvePipeline>>,
    /// Cached bind group for depth resolve (keyed on src + dst TextureViewId).
    depth_resolve_bind_group: Mutex<Option<(TextureViewId, TextureViewId, wgpu::BindGroup)>>,
    /// Motion vector resolve render pipeline (lazy-init, resolution-independent).
    motion_resolve: Mutex<Option<ResolvePipeline>>,
    /// Cached bind group for motion resolve (keyed on src TextureViewId).
    motion_resolve_bind_group: Mutex<Option<(TextureViewId, wgpu::BindGroup)>>,
    /// Cached bind group sampling the *interpolated* frame, for the extra
    /// present. Separate from `cached_bind_group`, which samples the real one.
    #[cfg(feature = "frame-interpolation")]
    cached_interp_bind_group: Mutex<Option<(TextureViewId, BindGroup)>>,
    /// Cached bind group sampling the real frame for the owned-layer present.
    /// Separate from `cached_bind_group`, which serves Bevy's swapchain blit.
    #[cfg(feature = "frame-interpolation")]
    cached_real_present_bind_group: Mutex<Option<(TextureViewId, BindGroup)>>,
    /// Blit pipeline specialised for [`PRESENT_FORMAT`].
    #[cfg(feature = "frame-interpolation")]
    cached_present_pipeline: Mutex<Option<CachedRenderPipelineId>>,
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
            motion_resolve: Mutex::new(None),
            motion_resolve_bind_group: Mutex::new(None),
            #[cfg(feature = "frame-interpolation")]
            cached_interp_bind_group: Mutex::new(None),
            #[cfg(feature = "frame-interpolation")]
            cached_real_present_bind_group: Mutex::new(None),
            #[cfg(feature = "frame-interpolation")]
            cached_present_pipeline: Mutex::new(None),
        }
    }
}

impl ViewNode for MetalFxUpscaleNode {
    type ViewQuery = (
        &'static ViewTarget,
        Option<&'static ViewPrepassTextures>,
        Option<&'static TemporalJitter>,
        // `extract_cameras` clones `Projection` onto the render-world view
        // entity, so the camera frustum is readable here without a bespoke
        // extract system. Frame interpolation needs FOV/near/far from it.
        Option<&'static Projection>,
    );

    fn run<'w>(
        &self,
        _graph: &mut RenderGraphContext,
        render_context: &mut RenderContext<'w>,
        (target, prepass_textures, temporal_jitter, projection): bevy::ecs::query::QueryItem<
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
        let dynamic_res_range = config.and_then(|c| c.dynamic_res_range);

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
        // Per-frame content dimensions follow the *current* render scale.
        let input_w = (full_w as f32 * render_scale).round() as u32;
        let input_h = (full_h as f32 * render_scale).round() as u32;
        let output_w = full_w;
        let output_h = full_h;
        let content_w = input_w;
        let content_h = input_h;

        // Dimensions the scaler is *created* at. With dynamic resolution enabled,
        // MetalFX requires the descriptor's input size to equal the output size
        // (the input texture is allocated full-size and the usable content region
        // flexes within it via inputContentMin/MaxScale + the per-frame
        // setInputContentWidth/Height). Without dynamic res, the scaler is created
        // at the current fixed input size.
        let (scaler_input_w, scaler_input_h) = match dynamic_res_range {
            Some(_) => (output_w, output_h),
            None => (input_w, input_h),
        };

        // --- Phase A: Get or create scaler + output texture ---
        let device = render_context.render_device().clone();
        let mut cached = self.cached.lock().unwrap();

        // Recreate only when the *scaler* input dimensions change (i.e. a window
        // resize, or the dynamic-res max). Per-frame render-scale changes move
        // `input_w/h` but not `scaler_input_w/h`, so under dynamic resolution they
        // no longer force a rebuild — the scaler flexes via setInputContentWidth.
        let needs_recreate = cached.as_ref().is_none_or(|c| {
            c.input_w != scaler_input_w
                || c.input_h != scaler_input_h
                || c.output_w != output_w
                || c.output_h != output_h
        });

        if needs_recreate {
            // Check if a background scaler creation is pending.
            let mut pending = self.pending.lock().unwrap();
            if let Some(p) = pending.as_ref() {
                // Check if dimensions match what we need (scaler-creation dims).
                if p.input_w == scaler_input_w && p.input_h == scaler_input_h
                    && p.output_w == output_w && p.output_h == output_h
                {
                    // Try to receive the scaler (non-blocking).
                    match p.receiver.try_recv() {
                        Ok(Some(scaler)) => {
                            log::info!(
                                "MetalFxUpscaleNode: background scaler ready {input_w}x{input_h} -> {output_w}x{output_h}"
                            );
                            *pending = None;

                            // Sized to the scaler's max input so the same textures
                            // serve every frame under dynamic resolution (the
                            // per-frame content copy fills only the top-left region).
                            let input_texture = device.create_texture(&TextureDescriptor {
                                label: Some("metalfx_input"),
                                size: Extent3d {
                                    width: scaler_input_w,
                                    height: scaler_input_h,
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
                            let (content_depth_texture, content_depth_view) = if matches!(scaler, SendScaler::Temporal(_) | SendScaler::FrameInterpolator { .. }) {
                                let tex = device.create_texture(&TextureDescriptor {
                                    label: Some("metalfx_content_depth"),
                                    size: Extent3d {
                                        width: scaler_input_w,
                                        height: scaler_input_h,
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
                            // Written via motion resolve render pass (Bevy's prepass
                            // textures lack COPY_SRC, so copy_texture_to_texture fails).
                            let (content_motion_texture, content_motion_view) = if matches!(scaler, SendScaler::Temporal(_) | SendScaler::FrameInterpolator { .. }) {
                                let tex = device.create_texture(&TextureDescriptor {
                                    label: Some("metalfx_content_motion"),
                                    size: Extent3d {
                                        width: scaler_input_w,
                                        height: scaler_input_h,
                                        depth_or_array_layers: 1,
                                    },
                                    mip_level_count: 1,
                                    sample_count: 1,
                                    dimension: TextureDimension::D2,
                                    format: bevy::render::render_resource::TextureFormat::Rg16Float,
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
                                // COPY_SRC: frame interpolation snapshots the upscaled
                                // frame into `metalfx_prev_color` as history.
                                usage: TextureUsages::RENDER_ATTACHMENT
                                    | TextureUsages::TEXTURE_BINDING
                                    | TextureUsages::STORAGE_BINDING
                                    | TextureUsages::COPY_SRC,
                                view_formats: &[],
                            });
                            let output_view = output_texture.create_view(
                                &bevy::render::render_resource::TextureViewDescriptor::default(),
                            );

                            *self.cached_bind_group.lock().unwrap() = None;
                            *self.cached_pipeline.lock().unwrap() = None;
                            *self.depth_resolve_bind_group.lock().unwrap() = None;
                            *self.motion_resolve_bind_group.lock().unwrap() = None;

                            *cached = Some(CachedState {
                                scaler,
                                input_texture,
                                output_texture,
                                output_view,
                                prev_color_texture: None,
                            interp_output_texture: None,
                            interp_output_view: None,
                            interp_bgra: None,
                            interp_bgra_view: None,
                            real_bgra: None,
                            real_bgra_view: None,
                                content_depth_texture,
                                content_depth_view,
                                content_motion_texture,
                                content_motion_view,
                                input_w: scaler_input_w,
                                input_h: scaler_input_h,
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
                    // Dimensions changed: discard the pending creation AND the
                    // stale cached scaler so the block below starts a fresh one.
                    *pending = None;
                    *cached = None;
                }
            }

            // Start a new creation only when we have neither a usable scaler nor
            // one in flight. Guarding on `cached.is_none()` is essential: the
            // receive branch above sets `*cached = Some(..)` and `*pending = None`
            // in the same frame, so a `pending.is_none()`-only guard would fall
            // straight through here and immediately discard the scaler we just
            // received — rebuilding forever (6zit.12). The dimensions-changed
            // path nulls `cached` above so a genuine resize still recreates.
            if cached.is_none() && pending.is_none() {
                log::info!(
                    "MetalFxUpscaleNode: creating {:?} scaler {scaler_input_w}x{scaler_input_h} -> {output_w}x{output_h} (dynamic_res={dynamic_res_range:?}, cur_input={input_w}x{input_h})",
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
                            // COPY_SRC: see the sibling `metalfx_output` above.
                            usage: TextureUsages::RENDER_ATTACHMENT
                                | TextureUsages::TEXTURE_BINDING
                                | TextureUsages::STORAGE_BINDING
                                | TextureUsages::COPY_SRC,
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
                            interp_output_texture: None,
                            interp_output_view: None,
                            interp_bgra: None,
                            interp_bgra_view: None,
                            real_bgra: None,
                            real_bgra_view: None,
                            content_depth_texture: None,
                            content_depth_view: None,
                            content_motion_texture: None,
                            content_motion_view: None,
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

                        // MTLPixelFormat is a #[repr(transparent)] newtype over
                        // NSUInteger (= usize on 64-bit macOS); read its
                        // discriminant via the field rather than transmuting.
                        let color_fmt_raw: usize = color_mtl_fmt.0;
                        match mode {
                            MetalFxMode::Temporal => unsafe {
                                // Create temporal scaler at the max input dimensions
                                // (== current dims when dynamic res is off). Depth and
                                // motion vectors are resolved to content-sized textures
                                // before being passed to MetalFX. `dynamic_res_range`, if
                                // set, enables true dynamic resolution so scale changes
                                // flex without rebuilding this scaler.
                                crate::platform::spawn_temporal_scaler_thread(
                                    device_ptr,
                                    scaler_input_w as usize, scaler_input_h as usize,
                                    output_w as usize, output_h as usize,
                                    color_fmt_raw, dynamic_res_range, tx,
                                );
                            },
                            MetalFxMode::FrameInterpolation => unsafe {
                                crate::platform::spawn_frame_interpolator_thread(
                                    device_ptr,
                                    scaler_input_w as usize, scaler_input_h as usize,
                                    output_w as usize, output_h as usize,
                                    color_fmt_raw, tx,
                                );
                            },
                            _ => unreachable!(),
                        }

                        *pending = Some(PendingScaler {
                            receiver: rx,
                            input_w: scaler_input_w,
                            input_h: scaler_input_h,
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
            SendScaler::Temporal(_) | SendScaler::FrameInterpolator { .. }
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

            // Resolve motion vectors to content-sized RG16Float via render pass.
            // Bevy's prepass textures lack COPY_SRC, so copy_texture_to_texture fails.
            let content_motion_view = state.content_motion_view.as_ref().unwrap();
            {
                let mut mr = self.motion_resolve.lock().unwrap();
                if mr.is_none() {
                    let wgpu_dev = device.wgpu_device();
                    let shader = wgpu_dev.create_shader_module(wgpu::ShaderModuleDescriptor {
                        label: Some("motion_resolve_shader"),
                        source: wgpu::ShaderSource::Wgsl(
                            include_str!("motion_resolve.wgsl").into(),
                        ),
                    });
                    let bgl = wgpu_dev.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                        label: Some("motion_resolve_bgl"),
                        entries: &[wgpu::BindGroupLayoutEntry {
                            binding: 0,
                            visibility: wgpu::ShaderStages::FRAGMENT,
                            ty: wgpu::BindingType::Texture {
                                sample_type: wgpu::TextureSampleType::Float { filterable: false },
                                view_dimension: wgpu::TextureViewDimension::D2,
                                multisampled: false,
                            },
                            count: None,
                        }],
                    });
                    let pipeline_layout = wgpu_dev.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                        label: Some("motion_resolve_layout"),
                        bind_group_layouts: &[&bgl],
                        push_constant_ranges: &[],
                    });
                    let pipeline = wgpu_dev.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                        label: Some("motion_resolve_pipeline"),
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
                            targets: &[Some(wgpu::ColorTargetState {
                                format: wgpu::TextureFormat::Rg16Float,
                                blend: None,
                                write_mask: wgpu::ColorWrites::ALL,
                            })],
                            compilation_options: Default::default(),
                        }),
                        primitive: wgpu::PrimitiveState {
                            topology: wgpu::PrimitiveTopology::TriangleList,
                            ..Default::default()
                        },
                        depth_stencil: None,
                        multisample: Default::default(),
                        multiview: None,
                        cache: None,
                    });
                    *mr = Some(ResolvePipeline {
                        pipeline,
                        bind_group_layout: bgl,
                    });
                }
                let mr_ref = mr.as_ref().unwrap();

                let src_motion_view = motion_attachment.texture.texture.create_view(
                    &bevy::render::render_resource::TextureViewDescriptor::default(),
                );

                // Get or create cached bind group for motion resolve.
                let mut mr_bg = self.motion_resolve_bind_group.lock().unwrap();
                let need_new = match &*mr_bg {
                    Some((src_id, _)) if *src_id == src_motion_view.id() => false,
                    _ => true,
                };
                if need_new {
                    let wgpu_dev = device.wgpu_device();
                    let src_view_wgpu: &wgpu::TextureView = &src_motion_view;
                    let bg = wgpu_dev.create_bind_group(&wgpu::BindGroupDescriptor {
                        label: Some("motion_resolve_bg"),
                        layout: &mr_ref.bind_group_layout,
                        entries: &[wgpu::BindGroupEntry {
                            binding: 0,
                            resource: wgpu::BindingResource::TextureView(src_view_wgpu),
                        }],
                    });
                    *mr_bg = Some((src_motion_view.id(), bg));
                }
                let bind_group = &mr_bg.as_ref().unwrap().1;

                let mut pass = render_context.command_encoder().begin_render_pass(
                    &RenderPassDescriptor {
                        label: Some("metalfx_motion_resolve"),
                        color_attachments: &[Some(
                            wgpu::RenderPassColorAttachment {
                                view: content_motion_view,
                                resolve_target: None,
                                ops: wgpu::Operations {
                                    load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT),
                                    store: wgpu::StoreOp::Store,
                                },
                                depth_slice: None,
                            },
                        )],
                        depth_stencil_attachment: None,
                        timestamp_writes: None,
                        occlusion_query_set: None,
                    },
                );
                pass.set_pipeline(&mr_ref.pipeline);
                pass.set_bind_group(0, bind_group, &[]);
                pass.set_viewport(0.0, 0.0, content_w as f32, content_h as f32, 0.0, 1.0);
                pass.draw(0..3, 0..1);
            }

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
                    *dr = Some(ResolvePipeline {
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
                                    // Clear to 1.0 (near plane in Bevy's reversed-Z).
                                    // Safe default: out-of-viewport fragments read as near-plane
                                    // rather than far-plane (infinity), preventing edge ghosting.
                                    load: bevy::render::render_resource::LoadOp::Clear(1.0),
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
        // Frame interpolation needs two extra full-resolution textures: the
        // previous upscaled frame (history) and somewhere to put the synthesised
        // frame. Both sit at *output* size — the interpolator consumes upscaled
        // color, so `inputWidth/Height` on its descriptor describe only the
        // depth/motion textures. Sizing these to the input instead trips
        // MetalFX's "Color texture width mismatch from descriptor" assertion.
        let interp_ptrs = match &state.scaler {
            SendScaler::FrameInterpolator { .. } => {
                if state.prev_color_texture.is_none() {
                    state.prev_color_texture = Some(device.create_texture(&TextureDescriptor {
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
                        usage: TextureUsages::TEXTURE_BINDING | TextureUsages::COPY_DST,
                        view_formats: &[],
                    }));
                }
                if state.interp_output_texture.is_none() {
                    let tex = device.create_texture(&TextureDescriptor {
                        label: Some("metalfx_interp_output"),
                        size: Extent3d {
                            width: output_w,
                            height: output_h,
                            depth_or_array_layers: 1,
                        },
                        mip_level_count: 1,
                        sample_count: 1,
                        dimension: TextureDimension::D2,
                        format: main_format,
                        usage: TextureUsages::TEXTURE_BINDING
                            | TextureUsages::STORAGE_BINDING
                            | TextureUsages::COPY_SRC,
                        view_formats: &[],
                    });
                    state.interp_output_view = Some(tex.create_view(
                        &bevy::render::render_resource::TextureViewDescriptor::default(),
                    ));
                    state.interp_output_texture = Some(tex);

                    let mut stage = |label: &'static str| {
                        let t = device.create_texture(&TextureDescriptor {
                            label: Some(label),
                            size: Extent3d {
                                width: output_w,
                                height: output_h,
                                depth_or_array_layers: 1,
                            },
                            mip_level_count: 1,
                            sample_count: 1,
                            dimension: TextureDimension::D2,
                            format: PRESENT_FORMAT,
                            usage: TextureUsages::RENDER_ATTACHMENT
                                | TextureUsages::COPY_SRC,
                            view_formats: &[],
                        });
                        let v = t.create_view(
                            &bevy::render::render_resource::TextureViewDescriptor::default(),
                        );
                        (t, v)
                    };
                    let (t, v) = stage("metalfx_interp_bgra");
                    state.interp_bgra = Some(t);
                    state.interp_bgra_view = Some(v);
                    let (t, v) = stage("metalfx_real_bgra");
                    state.real_bgra = Some(t);
                    state.real_bgra_view = Some(v);
                }

                let prev_tex = state.prev_color_texture.as_ref().unwrap();
                let Some(hal) = (unsafe { prev_tex.as_hal::<wgpu_hal::metal::Api>() }) else {
                    log::error!("MetalFxUpscaleNode: no Metal HAL for prev color texture");
                    return Ok(());
                };
                let prev_ptr = unsafe { hal.raw_handle().as_ptr() as *mut c_void };
                drop(hal);

                let interp_tex = state.interp_output_texture.as_ref().unwrap();
                let Some(hal) = (unsafe { interp_tex.as_hal::<wgpu_hal::metal::Api>() }) else {
                    log::error!("MetalFxUpscaleNode: no Metal HAL for interp output texture");
                    return Ok(());
                };
                let interp_ptr = unsafe { hal.raw_handle().as_ptr() as *mut c_void };

                Some((prev_ptr, interp_ptr))
            }
            _ => None,
        };

        // GPU-timing sink (Phase 0 bound-ness bench): clone the Arc before
        // borrowing the encoder, so the completion handler can capture it.
        let timing_sink = world
            .get_resource::<GpuTimingDiag>()
            .map(|d| d.0.clone());

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

                        if let Some(sink) = timing_sink.clone() {
                            // Borrowed cmd buffer, registered pre-commit (Codex review A/B/D).
                            add_gpu_timing_handler(cmd_buf_ptr, sink);
                        }
                    });
                }
            }
            SendScaler::FrameInterpolator {
                scaler,
                interpolator,
            } => {
                let (depth_ptr, motion_ptr) = temporal_ptrs.unwrap();
                let (prev_color_ptr, interp_out_ptr) = interp_ptrs.unwrap();
                // Bevy's `TemporalJitter.offset` is a pixel offset in [-0.5, 0.5]
                // whose Y is flipped when it enters clip space (see
                // `TemporalJitter::jitter_projection`: `offset * vec2(2, -2)`).
                // MetalFX's `jitterOffsetX/Y` wants the pixel offset that returns
                // the sample to the reference frame — the same X, but Y negated
                // to match MetalFX's (un-flipped) pixel space.
                let jitter = temporal_jitter
                    .map(|j| Vec2::new(j.offset.x, -j.offset.y))
                    .unwrap_or(Vec2::ZERO);
                let motion_scale_x = -(input_w as f32);
                let motion_scale_y = -(input_h as f32);

                // Camera params, read from the render world rather than guessed.
                //
                // `MTLFXFrameInterpolator` uses these to unproject depth when it
                // synthesises the intermediate frame, so wrong values distort
                // the motion field rather than failing loudly.
                //
                // Unit trap: MetalFX documents `fieldOfView` as the *vertical*
                // FOV in DEGREES; Bevy's `PerspectiveProjection::fov` is
                // vertical FOV in RADIANS. Passing it through unconverted is a
                // silent ~57x error.
                let (field_of_view, aspect_ratio, near_plane, far_plane) = match projection {
                    Some(Projection::Perspective(p)) => (
                        p.fov.to_degrees(),
                        p.aspect_ratio,
                        p.near,
                        p.far,
                    ),
                    // Orthographic/custom projections have no meaningful FOV.
                    // MetalFX frame interpolation assumes a perspective frustum,
                    // so fall back to a neutral one and let the caller know.
                    _ => {
                        // Once, not per-frame — this runs in the render loop.
                        static WARNED: std::sync::Once = std::sync::Once::new();
                        WARNED.call_once(|| {
                            log::warn!(
                                "MetalFxUpscaleNode: frame interpolation expects a perspective \
                                 Projection on the view; falling back to 45deg/0.1/1000"
                            );
                        });
                        (
                            45.0_f32,
                            output_w as f32 / output_h as f32,
                            0.1_f32,
                            1000.0_f32,
                        )
                    }
                };

                // Real inter-frame interval, mirrored from the main world's
                // `Time` (the render world has none). Defaults to 60Hz if the
                // resource is missing, which only happens when the plugin was
                // built in a non-interpolation mode.
                let delta_time = world
                    .get_resource::<MetalFxFrameTiming>()
                    .map_or(1.0 / 60.0, |t| t.delta_seconds);

                unsafe {
                    encoder.as_hal_mut::<wgpu_hal::metal::Api, _, ()>(|hal_encoder| {
                        let Some(enc) = hal_encoder else { return };
                        let Some(cmd_buf) = enc.raw_command_buffer() else { return };
                        let cmd_buf_ptr = cmd_buf.as_ptr() as *mut c_void;

                        // Stage 1 — upscale the low-res render to output size.
                        // This is the *real* frame, and it is what gets
                        // presented; it also becomes next frame's history.
                        encode_temporal_upscale(
                            scaler,
                            input_tex_ptr,
                            depth_ptr,
                            motion_ptr,
                            out_tex_ptr,
                            cmd_buf_ptr,
                            content_w as usize,
                            content_h as usize,
                            jitter.x,
                            jitter.y,
                            motion_scale_x,
                            motion_scale_y,
                            is_first_frame,
                        );

                        // Stage 2 — synthesise the intermediate frame between
                        // the previous upscaled frame and this one. Both color
                        // inputs are full-res; depth/motion stay content-sized.
                        crate::platform::encode_frame_interpolation(
                            interpolator,
                            out_tex_ptr,
                            prev_color_ptr,
                            depth_ptr,
                            motion_ptr,
                            interp_out_ptr,
                            cmd_buf_ptr,
                            jitter.x,
                            jitter.y,
                            motion_scale_x,
                            motion_scale_y,
                            delta_time,
                            field_of_view,
                            aspect_ratio,
                            near_plane,
                            far_plane,
                            is_first_frame,
                        );

                        if let Some(sink) = timing_sink.clone() {
                            add_gpu_timing_handler(cmd_buf_ptr, sink);
                        }
                    });
                }

                // Snapshot this frame's upscaled color into the history buffer.
                //
                // MetalFX's contract: whenever `shouldResetHistory` is false,
                // `prevColorTexture` must contain the data that was in
                // `colorTexture` during the *previous* `encodeToCommandBuffer:`.
                // Without this copy the history buffer stayed uninitialised for
                // the life of the process, so every frame after the first was
                // interpolated against garbage (6zit.8).
                //
                // Encoded *after* the interpolation pass on purpose: Metal
                // executes commands in encode order within a command buffer, so
                // the pass above still reads the genuine previous frame and this
                // copy only lands afterwards.
                let prev_tex = state.prev_color_texture.as_ref().unwrap();
                encoder.copy_texture_to_texture(
                    state.output_texture.as_image_copy(),
                    prev_tex.as_image_copy(),
                    Extent3d {
                        width: output_w,
                        height: output_h,
                        depth_or_array_layers: 1,
                    },
                );
            }
            SendScaler::Temporal(scaler) => {
                let (depth_ptr, motion_ptr) = temporal_ptrs.unwrap();
                // Negate Y to convert Bevy's clip-space jitter convention to
                // MetalFX's pixel-space one — see the FrameInterpolator branch.
                let jitter = temporal_jitter
                    .map(|j| Vec2::new(j.offset.x, -j.offset.y))
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
                            jitter.x,
                            jitter.y,
                            motion_scale_x,
                            motion_scale_y,
                            is_first_frame,
                        );

                        if let Some(sink) = timing_sink.clone() {
                            add_gpu_timing_handler(cmd_buf_ptr, sink);
                        }
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

        // Cloned out before the state lock is released, so the dual-present
        // path can still reach these views after `cached` is gone.
        // `TextureView` is a refcounted handle, so this is a refcount bump.
        #[cfg(feature = "frame-interpolation")]
        let interp_view_for_present = state.interp_output_view.clone();
        #[cfg(feature = "frame-interpolation")]
        let real_view_for_present = state.output_view.clone();
        #[cfg(feature = "frame-interpolation")]
        let staging = match (
            state.interp_bgra_view.clone(),
            state.real_bgra_view.clone(),
            state.interp_bgra.clone(),
            state.real_bgra.clone(),
        ) {
            (Some(iv), Some(rv), Some(it), Some(rt)) => Some((iv, rv, it, rt)),
            _ => None,
        };

        // Which frame goes into Bevy's swapchain image?
        //
        // Bevy presents that image untimed, after the graph, so it always lands
        // on the *earlier* of the two vsyncs. The interpolated frame depicts the
        // earlier moment, so under dual presentation it is the one that belongs
        // there — and the real frame is the one this node presents itself, held
        // back by one refresh interval.
        //
        // The reverse cannot work, which is worth stating because it is the
        // obvious first design: two untimed presents issued microseconds apart
        // both target the same vsync, so CoreAnimation discards the earlier one
        // outright (its presented-handler never fires). Delaying ours to
        // separate them would then display the interpolated frame *after* the
        // real frame it was built from — a backwards step in time.
        #[cfg(feature = "frame-interpolation")]
        let dual_active = interp_view_for_present.is_some()
            && world
                .get_resource::<crate::present::MetalFxDualPresent>()
                .and_then(|d| d.layer())
                .is_some();

        // Bevy's swapchain image always carries the real frame. Under dual
        // presentation it is not what the user sees — our own layer sits above
        // wgpu's — so there is nothing to gain from putting the interpolated
        // frame here, and keeping it uniform means the non-dual path is byte
        // for byte unchanged.
        let swapchain_view = &state.output_view;

        let mut cached_bg = self.cached_bind_group.lock().unwrap();
        let bind_group = match &mut *cached_bg {
            Some((id, bg)) if swapchain_view.id() == *id => bg,
            slot => {
                let bg = blit_pipeline.create_bind_group(
                    render_context.render_device(),
                    swapchain_view,
                    pipeline_cache,
                );
                let (_, bg) = slot.insert((swapchain_view.id(), bg));
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

        // --- Phase D: present the real frame, one refresh behind ---
        //
        // Bevy has just been handed the *interpolated* frame in its swapchain
        // image and will present it untimed. This second present carries the
        // real frame and is held back a refresh interval, so the two land on
        // consecutive vsyncs in the order they depict.
        // --- Phase D: present both frames from our own layer ---
        //
        // Two steps, and the split is forced by Metal's rules. First each frame
        // is drawn into a BGRA staging texture with the same fullscreen blit
        // pass Bevy uses for its swapchain: `CAMetalLayer` accepts BGRA channel
        // order only, MetalFX writes the view's RGBA format, and a blit copy
        // cannot convert between them. Then the presents themselves happen in
        // the graph command buffer's completion handler, on a command buffer we
        // own — acquiring the drawables fresh at that moment, because a drawable
        // acquired mid-graph has been recycled by the time a later commit lands
        // and its present is silently discarded.
        #[cfg(feature = "frame-interpolation")]
        if dual_active {
            if let (Some(dual), Some((interp_src, real_src, interp_tex, real_tex))) = (
                world.get_resource::<crate::present::MetalFxDualPresent>(),
                staging,
            ) {
                let mut present_pipeline = self.cached_present_pipeline.lock().unwrap();
                let present_id = match *present_pipeline {
                    Some(id) => id,
                    None => {
                        let id = pipeline_cache.queue_render_pipeline(blit_pipeline.specialize(
                            BlitPipelineKey {
                                texture_format: PRESENT_FORMAT,
                                blend_state: None,
                                samples: 1,
                            },
                        ));
                        *present_pipeline = Some(id);
                        id
                    }
                };
                drop(present_pipeline);

                if let (Some(present_pipe), Some(layer), Some(queue)) = (
                    pipeline_cache.get_render_pipeline(present_id),
                    dual.layer(),
                    dual.queue(),
                ) {
                    self.convert_for_present(
                        render_context,
                        blit_pipeline,
                        pipeline_cache,
                        present_pipe,
                        interp_view_for_present.as_ref().unwrap(),
                        &interp_src,
                        true,
                    );
                    self.convert_for_present(
                        render_context,
                        blit_pipeline,
                        pipeline_cache,
                        present_pipe,
                        &real_view_for_present,
                        &real_src,
                        false,
                    );

                    // Raw handles must be taken before `as_hal_mut` — wgpu's
                    // snatch lock forbids overlapping texture and encoder access.
                    let ptrs = unsafe {
                        let i = interp_tex.as_hal::<wgpu_hal::metal::Api>();
                        let r = real_tex.as_hal::<wgpu_hal::metal::Api>();
                        match (i, r) {
                            (Some(i), Some(r)) => Some((
                                i.raw_handle().as_ptr() as *mut c_void,
                                r.raw_handle().as_ptr() as *mut c_void,
                            )),
                            _ => None,
                        }
                    };

                    if let Some((interp_ptr, real_ptr)) = ptrs {
                        // SAFETY: the encoder's command buffer is live and
                        // uncommitted, and both conversion passes are encoded on
                        // it, so the staging textures are final by the time the
                        // completion handler copies them.
                        unsafe {
                            render_context
                                .command_encoder()
                                .as_hal_mut::<wgpu_hal::metal::Api, _, ()>(|hal_encoder| {
                                    let Some(enc) = hal_encoder else { return };
                                    let Some(cmd_buf) = enc.raw_command_buffer() else {
                                        return;
                                    };
                                    crate::present::present_pair_deferred(
                                        cmd_buf.as_ptr() as *mut c_void,
                                        layer,
                                        queue,
                                        interp_ptr,
                                        real_ptr,
                                        dual.refresh_interval,
                                        &dual.sink,
                                    );
                                });
                        }
                    }
                }
            }
        }

        Ok(())
    }
}

#[cfg(feature = "frame-interpolation")]
impl MetalFxUpscaleNode {
    /// Present both the interpolated and the real frame on our own layer.
    ///
    /// This is the half of frame interpolation that lives below the render
    /// graph. Bevy presents one swapchain image per `App::update()`, so the
    /// synthesised frame has nowhere to go; and a second drawable taken from
    /// *wgpu's* layer is never displayed, because wgpu acquires that layer's
    /// drawable before the graph runs and holds it all frame. So both frames go
    /// to a `CAMetalLayer` we own and stack above wgpu's.
    ///
    /// Order matters and is load-bearing: the interpolated frame depicts the
    /// moment between the previous real frame and this one, so it is presented
    /// first, and the real frame is held back one refresh interval with
    /// `presentDrawable:afterMinimumDuration:`. Without that hold both presents
    /// target the same vsync and CoreAnimation keeps only the later one.
    ///
    /// Both presents are encoded onto the render graph's own command buffer,
    /// after the passes that fill them, so a frame can never be presented
    /// before it is drawn.
    #[allow(clippy::too_many_arguments)]
    fn present_dual_frames(
        &self,
        world: &World,
        render_context: &mut RenderContext,
        interp_view: &TextureView,
        real_view: &TextureView,
        pipeline: &bevy::render::render_resource::RenderPipeline,
        blit_pipeline: &BlitPipeline,
        pipeline_cache: &PipelineCache,
        format: bevy::render::render_resource::TextureFormat,
        output_w: u32,
        output_h: u32,
    ) {
        use crate::present::{acquire_drawable, present_drawable};

        let Some(dual) = world.get_resource::<crate::present::MetalFxDualPresent>() else {
            return;
        };
        let Some(layer) = dual.layer() else {
            return;
        };

        // Take both drawables up front. The layer allows three, so holding two
        // is within budget — and acquiring them together means a shortage costs
        // the whole pair rather than presenting the real frame without its
        // interpolated partner, which would read as a stutter.
        //
        // SAFETY: `layer` is our own `CAMetalLayer`, valid for the window's life.
        let (Some(interp_drawable), Some(real_drawable)) =
            (unsafe { acquire_drawable(layer) }, unsafe { acquire_drawable(layer) })
        else {
            dual.sink.push_dropped();
            return;
        };

        // Draw each frame into its drawable. The layer is `framebufferOnly`, so
        // these must be render passes rather than blits.
        let ok = self.draw_into_drawable(
            render_context, blit_pipeline, pipeline_cache, pipeline,
            interp_view, &interp_drawable, format, output_w, output_h, true,
        ) && self.draw_into_drawable(
            render_context, blit_pipeline, pipeline_cache, pipeline,
            real_view, &real_drawable, format, output_w, output_h, false,
        );
        if !ok {
            dual.sink.push_dropped();
            return;
        }

        // SAFETY: the encoder's command buffer is live and uncommitted, and both
        // render passes above are already encoded onto it.
        unsafe {
            render_context
                .command_encoder()
                .as_hal_mut::<wgpu_hal::metal::Api, _, ()>(|hal_encoder| {
                    let Some(enc) = hal_encoder else { return };
                    let Some(cmd_buf) = enc.raw_command_buffer() else {
                        return;
                    };
                    let ptr = cmd_buf.as_ptr() as *mut c_void;

                    // Interpolated frame first — it depicts the earlier moment.
                    present_drawable(
                        ptr,
                        interp_drawable,
                        dual.timing,
                        dual.refresh_interval,
                        &dual.sink,
                        dual.queue(),
                    );
                    // Real frame one refresh later, so the two occupy
                    // consecutive intervals instead of collapsing into one.
                    present_drawable(
                        ptr,
                        real_drawable,
                        dual.timing,
                        dual.refresh_interval,
                        &dual.sink,
                        dual.queue(),
                    );
                });
        }
    }

    /// Draw `source_view` into `drawable` with the fullscreen blit pipeline.
    ///
    /// Returns false if the drawable could not be wrapped as a render target.
    #[allow(clippy::too_many_arguments)]
    fn draw_into_drawable(
        &self,
        render_context: &mut RenderContext,
        blit_pipeline: &BlitPipeline,
        pipeline_cache: &PipelineCache,
        pipeline: &bevy::render::render_resource::RenderPipeline,
        source_view: &TextureView,
        drawable: &crate::present::AcquiredDrawable,
        format: bevy::render::render_resource::TextureFormat,
        output_w: u32,
        output_h: u32,
        is_interpolated: bool,
    ) -> bool {
        // SAFETY: the drawable is live and the dimensions are the layer's
        // drawable size, which we set to match wgpu's.
        let drawable_texture = unsafe {
            crate::present::wrap_drawable_texture(
                render_context.render_device().wgpu_device(),
                drawable,
                format,
                output_w,
                output_h,
            )
        };
        let drawable_view = drawable_texture
            .create_view(&bevy::render::render_resource::TextureViewDescriptor::default());

        // Two caches, one per source, so alternating between the interpolated
        // and the real frame each pass does not thrash a single slot.
        let mut slot = if is_interpolated {
            self.cached_interp_bind_group.lock().unwrap()
        } else {
            self.cached_real_present_bind_group.lock().unwrap()
        };
        let bind_group = match &mut *slot {
            Some((id, bg)) if source_view.id() == *id => bg,
            s => {
                let bg = blit_pipeline.create_bind_group(
                    render_context.render_device(),
                    source_view,
                    pipeline_cache,
                );
                let (_, bg) = s.insert((source_view.id(), bg));
                bg
            }
        };

        {
            // `Clear` rather than `Load`: a freshly acquired drawable has
            // undefined contents, and on a tile-based GPU a clear load action is
            // cheaper than reading them back.
            let mut pass = render_context
                .command_encoder()
                .begin_render_pass(&RenderPassDescriptor {
                    label: Some(if is_interpolated {
                        "metalfx_present_interp"
                    } else {
                        "metalfx_present_real"
                    }),
                    color_attachments: &[Some(
                        bevy::render::render_resource::RenderPassColorAttachment {
                            view: &drawable_view,
                            resolve_target: None,
                            depth_slice: None,
                            ops: bevy::render::render_resource::Operations {
                                load: bevy::render::render_resource::LoadOp::Clear(
                                    bevy::color::LinearRgba::BLACK.into(),
                                ),
                                store: bevy::render::render_resource::StoreOp::Store,
                            },
                        },
                    )],
                    depth_stencil_attachment: None,
                    timestamp_writes: None,
                    occlusion_query_set: None,
                });

            pass.set_pipeline(pipeline);
            pass.set_bind_group(0, bind_group, &[]);
            pass.draw(0..3, 0..1);
        }

        true
    }
}

#[cfg(feature = "frame-interpolation")]
impl MetalFxUpscaleNode {
    /// Draw `source` into `target` with the fullscreen blit pipeline,
    /// converting RGBA to the layer's BGRA channel order on the way.
    #[allow(clippy::too_many_arguments)]
    fn convert_for_present(
        &self,
        render_context: &mut RenderContext,
        blit_pipeline: &BlitPipeline,
        pipeline_cache: &PipelineCache,
        pipeline: &bevy::render::render_resource::RenderPipeline,
        source: &TextureView,
        target: &TextureView,
        is_interpolated: bool,
    ) {
        // Two caches, one per source, so alternating between the interpolated
        // and the real frame each frame does not thrash a single slot.
        let mut slot = if is_interpolated {
            self.cached_interp_bind_group.lock().unwrap()
        } else {
            self.cached_real_present_bind_group.lock().unwrap()
        };
        let bind_group = match &mut *slot {
            Some((id, bg)) if source.id() == *id => bg,
            s => {
                let bg = blit_pipeline.create_bind_group(
                    render_context.render_device(),
                    source,
                    pipeline_cache,
                );
                let (_, bg) = s.insert((source.id(), bg));
                bg
            }
        };

        let mut pass = render_context
            .command_encoder()
            .begin_render_pass(&RenderPassDescriptor {
                label: Some(if is_interpolated {
                    "metalfx_present_convert_interp"
                } else {
                    "metalfx_present_convert_real"
                }),
                color_attachments: &[Some(
                    bevy::render::render_resource::RenderPassColorAttachment {
                        view: target,
                        resolve_target: None,
                        depth_slice: None,
                        ops: bevy::render::render_resource::Operations {
                            // The fullscreen triangle covers every pixel, and a
                            // clear load action is free on a tile-based GPU.
                            load: bevy::render::render_resource::LoadOp::Clear(
                                bevy::color::LinearRgba::BLACK.into(),
                            ),
                            store: bevy::render::render_resource::StoreOp::Store,
                        },
                    },
                )],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
            });

        pass.set_pipeline(pipeline);
        pass.set_bind_group(0, bind_group, &[]);
        pass.draw(0..3, 0..1);
    }
}
