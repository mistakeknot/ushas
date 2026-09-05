//! MetalFX upscaling pass (spatial + temporal).
//!
//! Spatial and Temporal run after the main pass and before postprocessing.
//! They reconstruct the reduced content into the next full-resolution main
//! texture, then Bevy applies tonemapping, native-resolution UI, and its final
//! window blit. FrameInterpolation retains its experimental late output path.
//!
//! Through 0.3 this was a render-graph `ViewNode`. Bevy 0.19 removed the render
//! graph in favour of ECS schedules, so the entry point is now the
//! [`metalfx_upscale`] system.
//!
//! ## Architecture
//!
//! ```text
//! main_texture (low-res)
//!   → MetalFX upscale (spatial or temporal, raw Metal encode)
//!     → metalfx_output (full-res, our texture)
//!       → next main texture → postprocessing → native UI → Bevy window blit
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
//!                            ↖───────── copy ────────┘
//! ```
//!
//! The descriptor's `inputWidth`/`inputHeight` describe only the depth and
//! motion textures; the color textures are at **output** size. Sizing them to
//! the render resolution trips a MetalFX debug-layer assertion ("Color texture
//! width mismatch from descriptor").
//!
//! Presenting the synthesised frame needs a second present per update, which
//! Bevy does not do. The [`crate::present`] module does it on a `CAMetalLayer`
//! of its own, and Phase D of `run` feeds it: both frames are converted to BGRA
//! staging textures here, and presented from the frame command buffer's
//! completion handler there.
//!
//! FrameInterpolation still runs after Bevy's final blit and presents its own
//! textures. Its native UI and HDR composition are not covered by the early
//! Spatial/Temporal path: moving the owned-layer presentation earlier without
//! a separate composition step would bypass tonemapping and omit UI entirely.
//!
//! It is opt-in (`MetalFxDualPresent::enabled`). Historical experiments recorded
//! differing drawable callback behavior across display conditions. Current
//! presentation ordering, image content, and panel delivery require independent
//! validation; encoding alone does not establish them. See [`crate::present`]
//! and `docs/m5-max-performance-research.md`.
//!
//! ## Where the phases live
//!
//! `run` orchestrates; the phases that carry weight are child modules, which
//! can still reach this module's private fields:
//!
//! | Phase | Module | What it does |
//! |-------|--------|--------------|
//! | A     | [`scaler`] | scaler lifecycle + the textures sized to it |
//! | B0.5  | [`resolve`] | depth + motion prepass resolve to content size |
//! | B     | [`encode`] | the MetalFX encode, one arm per mode |
//! | C/D   | here | main-texture reconstruction, or experimental interpolation output/present |
//!
//! Phases A and B can decline to run — a temporal scaler may still be
//! compiling, or a raw Metal handle may be unavailable — and return `false`
//! rather than a `NodeRunError`, so the decision to skip a frame stays in
//! `run`.
//!
//! ## Temporal Scaler Threading
//!
//! The temporal scaler's `newTemporalScalerWithDevice:` compiles ML pipelines
//! internally and can take several seconds. To avoid blocking the render thread,
//! scaler creation is dispatched to a background OS thread. The render node
//! polls for readiness each frame. The crop-aware control pass expands the
//! reduced scene while Spatial/Temporal are unavailable, preserving the
//! explicit pending/failure observation.

mod encode;
mod resolve;
mod scaler;

#[cfg(feature = "frame-interpolation")]
use std::ffi::c_void;
use std::sync::Mutex;

use bevy::camera::MainPassResolutionOverride;
use bevy::core_pipeline::blit::{BlitPipeline, BlitPipelineKey};
use bevy::core_pipeline::prepass::ViewPrepassTextures;
use bevy::prelude::*;
use bevy::render::camera::{ExtractedCamera, TemporalJitter};
use bevy::render::render_resource::{
    BindGroup, CachedRenderPipelineId, Extent3d, LoadOp, Operations, PipelineCache,
    RenderPassColorAttachment, RenderPassDescriptor, SpecializedRenderPipeline, StoreOp,
    TextureView, TextureViewId,
};
use bevy::render::sync_world::MainEntity;
// Gated to match its only use site — the dual-present block below is
// `frame-interpolation`-only, and an import wider than its use is an unused
// import on every other feature combination.
#[cfg(feature = "frame-interpolation")]
use bevy::render::render_resource::CommandEncoderDescriptor;
use bevy::render::renderer::{RenderContext, ViewQuery};
use bevy::render::view::ViewTarget;
// Unconditional: `SendScaler::Spatial` holds a `Retained`, so every feature
// combination needs this — including the default, spatial-only build.
use objc2::rc::Retained;
use objc2::runtime::ProtocolObject;
#[cfg(feature = "frame-interpolation")]
use objc2_metal_fx::MTLFXFrameInterpolator;
use objc2_metal_fx::MTLFXSpatialScaler;
#[cfg(feature = "temporal")]
use objc2_metal_fx::MTLFXTemporalScaler;

use crate::platform::wgpu_format_to_mtl;
use crate::{
    MetalFxEffectObservation, MetalFxEffectReason, MetalFxEffectState, MetalFxEffectStatus,
    MetalFxMode, MetalFxObservationFrame,
};

/// Resource holding the MetalFX render configuration.
/// Extracted from main world each frame via `ExtractResourcePlugin`.
///
/// Fields are crate-private: this is a mirror the plugin maintains, not a
/// control surface. Drive the render scale through [`crate::MetalFxRenderScale`]
/// and the mode through [`crate::MetalFxPlugin::mode`].
#[derive(Resource, Clone, Copy, bevy::render::extract_resource::ExtractResource)]
pub struct MetalFxConfig {
    pub(crate) render_scale: f32,
    pub(crate) mode: MetalFxMode,
    /// When `Some((min, max))`, the temporal scaler is created with true dynamic
    /// resolution enabled spanning that render-scale range, so an adaptive
    /// governor can flex `render_scale` within `[min, max]` without rebuilding
    /// the scaler. `None` = fixed-scale scaler (recreated only on window resize).
    pub(crate) dynamic_res_range: Option<(f32, f32)>,
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
    pub(crate) delta_seconds: f32,
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
    #[cfg(feature = "temporal")]
    Temporal(Retained<ProtocolObject<dyn MTLFXTemporalScaler>>),
    /// Frame interpolation is a *two-stage* pipeline, not an alternative to
    /// upscaling: the temporal scaler produces the full-res frame, and the
    /// interpolator synthesises an intermediate frame from two consecutive
    /// full-res frames. Both objects are held together for the life of the node.
    #[cfg(feature = "frame-interpolation")]
    FrameInterpolator {
        scaler: Retained<ProtocolObject<dyn MTLFXTemporalScaler>>,
        interpolator: Retained<ProtocolObject<dyn MTLFXFrameInterpolator>>,
    },
}

// Safety: Metal framework objects are thread-safe per Apple's Metal Best
// Practices Guide § "Metal and Multithread Safety".
unsafe impl Send for SendScaler {}
unsafe impl Sync for SendScaler {}

impl SendScaler {
    fn mode(&self) -> MetalFxMode {
        match self {
            Self::Spatial(_) => MetalFxMode::Spatial,
            #[cfg(feature = "temporal")]
            Self::Temporal(_) => MetalFxMode::Temporal,
            #[cfg(feature = "frame-interpolation")]
            Self::FrameInterpolator { .. } => MetalFxMode::FrameInterpolation,
        }
    }

    /// Whether this scaler consumes depth and motion vectors, and so needs the
    /// prepass resolve passes. False on a `spatial`-only build, where neither
    /// variant that answers true is compiled in.
    fn is_temporal_like(&self) -> bool {
        match self {
            SendScaler::Spatial(_) => false,
            #[cfg(feature = "temporal")]
            SendScaler::Temporal(_) => true,
            #[cfg(feature = "frame-interpolation")]
            SendScaler::FrameInterpolator { .. } => true,
        }
    }
}

/// Pixel format for the owned presentation layer and its staging textures.
///
/// `CAMetalLayer` accepts BGRA channel order only — setting it to the view's
/// RGBA format makes CoreAnimation accept presents and then silently skip them.
#[cfg(feature = "frame-interpolation")]
const PRESENT_FORMAT: bevy::render::render_resource::TextureFormat =
    bevy::render::render_resource::TextureFormat::Bgra8UnormSrgb;

/// Cached state for the MetalFX upscale node.
struct CachedState {
    key: scaler::ScalerKey,
    scaler: SendScaler,
    /// Content-sized input texture (copied from main_texture's top-left region).
    input_texture: bevy::render::render_resource::Texture,
    output_texture: bevy::render::render_resource::Texture,
    output_view: TextureView,
    /// Previous frame's *upscaled* color, at output resolution — the history
    /// input for frame interpolation.
    #[cfg(feature = "frame-interpolation")]
    prev_color_texture: Option<bevy::render::render_resource::Texture>,
    /// Destination for the synthesised intermediate frame (frame interpolation
    /// only), at output resolution. Kept separate from `output_texture` so the
    /// real upscaled frame survives for presentation and for the history copy.
    #[cfg(feature = "frame-interpolation")]
    interp_output_texture: Option<bevy::render::render_resource::Texture>,
    /// Stable view of the synthesised frame, used as the blit source when it is
    /// drawn into its own drawable for presentation.
    #[cfg(feature = "frame-interpolation")]
    interp_output_view: Option<TextureView>,
    /// BGRA staging copies of the two frames, for the owned-layer present.
    ///
    /// `CAMetalLayer` supports BGRA channel order only, while MetalFX writes in
    /// the view's format (RGBA here). A blit copy cannot convert channel order,
    /// so each frame is first drawn into a BGRA texture by the same fullscreen
    /// blit pass Bevy uses for its own swapchain — which does the conversion for
    /// free — and the drawable copy is then format-identical.
    #[cfg(feature = "frame-interpolation")]
    interp_bgra: Option<bevy::render::render_resource::Texture>,
    #[cfg(feature = "frame-interpolation")]
    interp_bgra_view: Option<TextureView>,
    #[cfg(feature = "frame-interpolation")]
    real_bgra: Option<bevy::render::render_resource::Texture>,
    #[cfg(feature = "frame-interpolation")]
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
    key: scaler::ScalerKey,
    receiver: std::sync::mpsc::Receiver<Option<SendScaler>>,
    /// Keeps only a diagnostic held attempt connected. Dropped on generation
    /// change along with its receiver; ordinary creation never retains this.
    #[cfg(feature = "diagnostic-fault-injection")]
    _diagnostic_keepalive: Option<std::sync::mpsc::Sender<Option<SendScaler>>>,
    /// When the background creation started, so a creation that never returns
    /// can be told apart from one that is merely slow.
    ///
    /// It cannot, otherwise: the receive loop treats `TryRecvError::Empty` as
    /// "still creating" and skips the frame, which is correct for the ~1s a
    /// cold MPSGraph compile takes and indistinguishable from a permanent hang.
    /// Measured against a locked session, `newTemporalScalerWithDevice:` did not
    /// return in 121s across 36,052 rendered frames -- MetalFX silently never
    /// engaged, and the only trace was one INFO line saying it had started.
    started: std::time::Instant,
    /// Latched so the warning below is printed once per creation, not per frame.
    /// `Cell` because the receive loop holds `&PendingScaler` and reassigns the
    /// enclosing `Option` in sibling arms, so it cannot take `&mut` here.
    warned: std::cell::Cell<bool>,
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
    cached_pipeline: Mutex<
        Option<(
            bevy::render::render_resource::TextureFormat,
            CachedRenderPipelineId,
        )>,
    >,
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

/// The MetalFX upscale pass.
///
/// Bevy 0.19 removed the render graph and drives rendering from ECS *schedules*
/// instead: a pass is an ordinary system that reaches the current view through
/// [`ViewQuery`] and the frame's encoder through [`RenderContext`]. Spatial and
/// Temporal are registered before EarlyPostProcess; experimental interpolation
/// is registered after Bevy's window upscaling.
///
/// The caches live in a [`Local`], with one scaler/history set for this system.
/// The current implementation accepts a single active 3D view covering its
/// full render target. Additional views and offset/subrectangle viewports fail
/// closed with an effect-status reason, leaving Bevy's output in place.
#[allow(clippy::type_complexity)]
pub fn metalfx_upscale(
    view: ViewQuery<(
        &'static MainEntity,
        &'static ExtractedCamera,
        &'static ViewTarget,
        Option<&'static MainPassResolutionOverride>,
        Option<&'static ViewPrepassTextures>,
        Option<&'static TemporalJitter>,
        // `extract_cameras` clones `Projection` onto the render-world view
        // entity, so the camera frustum is readable here without a bespoke
        // extract system. Frame interpolation needs FOV/near/far from it.
        Option<&'static Projection>,
    )>,
    cameras: Query<(&ExtractedCamera, &ViewTarget), With<Camera3d>>,
    state: Local<MetalFxUpscaleNode>,
    mut render_context: RenderContext,
    world: &World,
) {
    // CurrentView selects the render entity; MainEntity in the query provides
    // the stable public identity shared with main-world camera consumers.
    state.run(
        &mut render_context,
        view.into_inner(),
        cameras.iter().take(2).count(),
        world,
    );
}

/// Timing-only pass for [`MetalFxMode::Disabled`].
///
/// Submits an empty command buffer with a completion timestamp handler. This
/// characterizes the timer floor and dependency behavior, not a native frame's
/// GPU cost. The enabled MetalFX buffer can include upstream waits, so these
/// intervals must not be subtracted as though they were isolated pass costs.
///
/// Registered only when the host explicitly supplied a
/// [`MetalFxPlugin::gpu_timing_sink`](crate::MetalFxPlugin::gpu_timing_sink).
/// A plain `Disabled` consumer submits nothing extra and pays nothing.
pub fn metalfx_timing_only(mut render_context: RenderContext, world: &World) {
    let Some(sink) = world
        .get_resource::<crate::GpuTimingDiag>()
        .map(|d| d.0.clone())
    else {
        return;
    };

    let device = render_context.render_device().clone();
    let mut encoder =
        device.create_command_encoder(&bevy::render::render_resource::CommandEncoderDescriptor {
            label: Some("metalfx_timing_only"),
        });

    // SAFETY: mirrors the borrow discipline in `encode.rs`. This encoder carries
    // no work, so no texture guard is live and wgpu's snatch lock is untouched.
    // The command-buffer pointer is read inside the closure, handed straight to
    // `addCompletedHandler:`, and never retained or stored.
    unsafe {
        encoder.as_hal_mut::<wgpu_hal::metal::Api, _, ()>(|hal_encoder| {
            let Some(enc) = hal_encoder else { return };
            let Some(cmd_buf) = enc.raw_command_buffer() else {
                return;
            };
            crate::gpu_timing::add_gpu_timing_handler(
                cmd_buf as *const _ as *mut std::ffi::c_void,
                sink,
            );
        });
    }

    render_context.add_command_buffer(encoder.finish());
}

impl MetalFxUpscaleNode {
    // Reduced builds legitimately compute values whose only consumers are
    // gated out — jitter, projection, the prepass pointers. The
    // `frame-interpolation` build compiles every path and is *not* excepted
    // here, so it stays the one that catches a genuinely unused binding.
    #[cfg_attr(not(feature = "frame-interpolation"), allow(unused_variables))]
    #[allow(clippy::type_complexity)]
    fn run(
        &self,
        render_context: &mut RenderContext,
        (
            main_entity,
            camera,
            target,
            resolution_override,
            prepass_textures,
            temporal_jitter,
            projection,
        ): (
            &MainEntity,
            &ExtractedCamera,
            &ViewTarget,
            Option<&MainPassResolutionOverride>,
            Option<&ViewPrepassTextures>,
            Option<&TemporalJitter>,
            Option<&Projection>,
        ),
        active_views: usize,
        world: &World,
    ) {
        let main_tex = target.main_texture();
        let main_size = main_tex.size();
        let main_format = main_tex.format();

        let config = world.get_resource::<MetalFxConfig>();
        let render_scale = config.map_or(0.5, |c| c.render_scale);
        let mode =
            crate::effect_runtime::compiled_mode(config.map_or(MetalFxMode::Spatial, |c| c.mode));
        let requested_mode = world
            .get_resource::<crate::effect_runtime::MetalFxRequestedEffect>()
            .map_or(mode, |request| request.mode);
        let view_id = main_entity.id().to_bits();
        let output = [main_size.width, main_size.height];
        let observed_content = resolution_override
            .map(|resolution| resolution.0.to_array())
            .unwrap_or(output);
        let publish = |effective_mode, state, reason| {
            if let (Some(status), Some(frame)) = (
                world.get_resource::<MetalFxEffectStatus>(),
                world.get_resource::<MetalFxObservationFrame>(),
            ) {
                status.publish(MetalFxEffectObservation::new(
                    frame.0,
                    view_id,
                    requested_mode,
                    effective_mode,
                    render_scale,
                    observed_content,
                    output,
                    state,
                    reason,
                ));
            }
        };
        // Publish only the terminal decision for this invocation. The main
        // world may read concurrently, so intermediate work must not replace
        // the last completed frame with a transient pending/encoded state.
        if let Some(reason) = crate::effect_runtime::view_scope_error(active_views) {
            publish(
                MetalFxMode::Disabled,
                if active_views == 0 {
                    MetalFxEffectState::NoRender
                } else {
                    MetalFxEffectState::Unavailable
                },
                Some(reason),
            );
            return;
        }
        let content = match crate::effect_runtime::observed_content_size(
            output,
            resolution_override.map(|resolution| resolution.0.to_array()),
            camera.viewport.as_ref().map(|viewport| {
                (
                    viewport.physical_position.to_array(),
                    viewport.physical_size.to_array(),
                )
            }),
        ) {
            Ok(content) => content,
            Err(reason) => {
                publish(
                    MetalFxMode::Disabled,
                    MetalFxEffectState::Unavailable,
                    Some(reason),
                );
                return;
            }
        };
        if mode == MetalFxMode::Disabled {
            publish(
                MetalFxMode::Disabled,
                MetalFxEffectState::Disabled,
                Some(MetalFxEffectReason::ModeDisabled),
            );
            return;
        }
        let Some(color_mtl_fmt) = wgpu_format_to_mtl(main_format) else {
            log::error!("MetalFxUpscaleNode: unsupported format {:?}", main_format);
            publish(
                MetalFxMode::Disabled,
                MetalFxEffectState::Unavailable,
                Some(MetalFxEffectReason::UnsupportedFormat),
            );
            return;
        };
        // The experimental interpolation path remains after tonemapping/UI
        // until its owned-layer composition is redesigned. Spatial/Temporal
        // instead operate on the scene before postprocessing and must describe
        // the input texture's actual color contract.
        let color_processing = if mode == MetalFxMode::FrameInterpolation {
            objc2_metal_fx::MTLFXSpatialScalerColorProcessingMode::Perceptual
        } else {
            match crate::platform::scaler_color_processing(main_format, target.compositing_space) {
                Ok(processing) => processing,
                Err(reason) => {
                    publish(
                        MetalFxMode::Disabled,
                        MetalFxEffectState::Unavailable,
                        Some(reason),
                    );
                    return;
                }
            }
        };
        // Only temporal scalers implement a variable content region. Spatial
        // and interpolation descriptors are built at the actual input size.
        let dynamic_res_range = if mode == MetalFxMode::Temporal {
            config.and_then(|c| c.dynamic_res_range)
        } else {
            None
        };

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
        let input_w = content[0];
        let input_h = content[1];
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
        let effective_mode = match self.ensure_scaler(
            &device,
            &mut cached,
            scaler::ScalerDims {
                scaler_input_w,
                scaler_input_h,
                input_w,
                input_h,
                output_w,
                output_h,
            },
            view_id,
            mode,
            main_format,
            color_mtl_fmt,
            color_processing,
            dynamic_res_range,
            #[cfg(feature = "diagnostic-fault-injection")]
            world
                .get_resource::<crate::MetalFxDiagnosticFault>()
                .map_or_else(Default::default, |fault| fault.snapshot()),
        ) {
            Ok(mode) => mode,
            Err(reason) => {
                let state = if matches!(
                    reason,
                    MetalFxEffectReason::ScalerPending | MetalFxEffectReason::ScalerCreationSlow
                ) {
                    MetalFxEffectState::Pending
                } else {
                    MetalFxEffectState::Failed
                };
                publish(MetalFxMode::Disabled, state, Some(reason));
                return;
            }
        };
        let fallback_reason =
            (effective_mode != requested_mode).then_some(MetalFxEffectReason::FeatureUnavailable);

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
        let is_temporal_like = state.scaler.is_temporal_like();

        if is_temporal_like {
            let Some(prepass) = prepass_textures else {
                log::warn!("MetalFxUpscaleNode: temporal mode but no prepass textures");
                publish(
                    MetalFxMode::Disabled,
                    MetalFxEffectState::Failed,
                    Some(MetalFxEffectReason::MissingPrepass),
                );
                return;
            };
            let Some(depth_attachment) = &prepass.depth else {
                log::warn!("MetalFxUpscaleNode: no depth prepass texture");
                publish(
                    MetalFxMode::Disabled,
                    MetalFxEffectState::Failed,
                    Some(MetalFxEffectReason::MissingPrepass),
                );
                return;
            };
            let Some(motion_attachment) = &prepass.motion_vectors else {
                log::warn!("MetalFxUpscaleNode: no motion vector prepass texture");
                publish(
                    MetalFxMode::Disabled,
                    MetalFxEffectState::Failed,
                    Some(MetalFxEffectReason::MissingPrepass),
                );
                return;
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
            self.resolve_motion(
                &device,
                render_context,
                &motion_attachment.texture.texture,
                content_motion_view,
                content_w,
                content_h,
            );

            // Resolve depth to content-sized Depth32Float via fragment shader render pass.
            // This block must be a separate scope — render pass guard must drop before
            // as_hal_mut is called for the MetalFX encode.
            let content_depth_view = state.content_depth_view.as_ref().unwrap();
            self.resolve_depth(
                &device,
                render_context,
                &depth_attachment.texture.texture,
                content_depth_view,
                content_w,
                content_h,
            );
            // dr and dr_bg guards also dropped here
        }

        if !self.encode_metalfx(
            world,
            &device,
            render_context,
            state,
            is_temporal_like,
            temporal_jitter,
            projection,
            main_format,
            content_w,
            content_h,
            input_w,
            input_h,
            output_w,
            output_h,
        ) {
            publish(
                MetalFxMode::Disabled,
                MetalFxEffectState::Failed,
                Some(MetalFxEffectReason::MetalHandleUnavailable),
            );
            return;
        }
        // --- Phase C: Reconstruct the main texture before postprocessing/UI ---
        // The interpolation branch preserves its separate late output path.
        let pipeline_cache = world.resource::<PipelineCache>();
        let blit_pipeline = world.resource::<BlitPipeline>();

        let reconstruct_main = mode != MetalFxMode::FrameInterpolation;
        let target_format = if reconstruct_main {
            Some(target.main_texture_format())
        } else {
            target.out_texture_view_format()
        };
        let Some(target_format) = target_format else {
            publish(
                effective_mode,
                MetalFxEffectState::Encoded,
                Some(MetalFxEffectReason::MissingOutput),
            );
            return;
        };
        let mut cached_pipeline = self.cached_pipeline.lock().unwrap();
        let pipeline_id = match *cached_pipeline {
            Some((format, id)) if format == target_format => id,
            _ => {
                // wgpu 29 / Bevy 0.19: the output format is now optional,
                // because a view target can exist without a resolved output.
                // Skip the frame rather than guessing a format.
                let key = BlitPipelineKey {
                    target_format,
                    blend_state: None,
                    samples: 1,
                    // Source and destination have identical main texture/color
                    // semantics. Bevy's final window blit owns color conversion.
                    // The legacy interpolation output also preserves its path.
                    source_space: None,
                };
                let descriptor = blit_pipeline.specialize(key);
                let id = pipeline_cache.queue_render_pipeline(descriptor);
                *cached_pipeline = Some((target_format, id));
                id
            }
        };

        let Some(pipeline) = pipeline_cache.get_render_pipeline(pipeline_id) else {
            let reason = if matches!(
                pipeline_cache.get_render_pipeline_state(pipeline_id),
                bevy::render::render_resource::CachedPipelineState::Err(_),
            ) {
                MetalFxEffectReason::BlitPipelineFailed
            } else {
                MetalFxEffectReason::BlitPipelinePending
            };
            publish(effective_mode, MetalFxEffectState::Encoded, Some(reason));
            drop(cached);
            return;
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

        #[cfg(feature = "frame-interpolation")]
        let dual_active = interp_view_for_present.is_some()
            && world
                .get_resource::<crate::present::MetalFxDualPresent>()
                .and_then(|d| d.layer())
                .is_some();

        // Bevy always receives the real reconstructed frame. The experimental
        // owned layer, when active, independently presents the interpolated
        // and real images above Bevy's window surface.
        let reconstructed_view = &state.output_view;

        let mut cached_bg = self.cached_bind_group.lock().unwrap();
        let bind_group = match &mut *cached_bg {
            Some((id, bg)) if reconstructed_view.id() == *id => bg,
            slot => {
                let bg = blit_pipeline.create_bind_group(
                    render_context.render_device(),
                    reconstructed_view,
                    pipeline_cache,
                );
                let (_, bg) = slot.insert((reconstructed_view.id(), bg));
                bg
            }
        };

        let output_attachment = if reconstruct_main {
            // This call flips Bevy's current main texture immediately. All
            // readiness and failure checks must precede it; after the flip we
            // must encode a complete full-resolution destination write.
            let output = target.post_process_write();
            Some(RenderPassColorAttachment {
                view: output.destination,
                depth_slice: None,
                resolve_target: None,
                ops: Operations {
                    load: LoadOp::Clear(Default::default()),
                    store: StoreOp::Store,
                },
            })
        } else {
            target.out_texture_color_attachment(None)
        };
        let Some(output_attachment) = output_attachment else {
            publish(
                effective_mode,
                MetalFxEffectState::Encoded,
                Some(MetalFxEffectReason::MissingOutput),
            );
            return;
        };
        let pass_descriptor = RenderPassDescriptor {
            label: Some(if reconstruct_main {
                "metalfx_reconstruct_main"
            } else {
                "metalfx_interpolation_output"
            }),
            // Bevy 0.19 returns the attachment already wrapped in `Option`.
            color_attachments: &[Some(output_attachment)],
            depth_stencil_attachment: None,
            timestamp_writes: None,
            occlusion_query_set: None,
            multiview_mask: None,
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
        publish(
            effective_mode,
            MetalFxEffectState::OutputWritten,
            fallback_reason,
        );

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
                                target_format: PRESENT_FORMAT,
                                blend_state: None,
                                samples: 1,
                                // 0.18's key had no colour-space knob and blitted
                                // the source through unchanged; `None` is that
                                // behaviour, not a new choice.
                                source_space: None,
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
                        // SAFETY: both staging textures are owned by CachedState and live for
                        // the frame. Taken here, before the `as_hal_mut` below, because the
                        // two cannot overlap (snatch lock).
                        let i = interp_tex.as_hal::<wgpu_hal::metal::Api>();
                        let r = real_tex.as_hal::<wgpu_hal::metal::Api>();
                        match (i, r) {
                            (Some(i), Some(r)) => Some((
                                i.raw_handle() as *const _ as *mut c_void,
                                r.raw_handle() as *const _ as *mut c_void,
                            )),
                            _ => None,
                        }
                    };

                    if let Some((interp_ptr, real_ptr)) = ptrs {
                        // A dedicated encoder for the raw work, for the same
                        // reason as the MetalFX encode in `encode.rs`: wgpu 29
                        // panics if one command encoder sees both the wgpu API
                        // and `as_hal_mut`, and the two `convert_for_present`
                        // calls above are render passes on the context's
                        // encoder.
                        let mut raw_encoder =
                            device.create_command_encoder(&CommandEncoderDescriptor {
                                label: Some("metalfx_dual_present"),
                            });
                        // SAFETY: both conversion passes are already encoded, and
                        // `add_command_buffer` below queues this buffer after
                        // them. Metal executes command buffers on a queue in
                        // commit order, so this buffer's completion handler
                        // cannot run until the conversions have finished and the
                        // staging textures are final.
                        unsafe {
                            raw_encoder.as_hal_mut::<wgpu_hal::metal::Api, _, ()>(|hal_encoder| {
                                let Some(enc) = hal_encoder else { return };
                                let Some(cmd_buf) = enc.raw_command_buffer() else {
                                    return;
                                };
                                crate::present::present_pair_deferred(
                                    cmd_buf as *const _ as *mut c_void,
                                    layer,
                                    queue,
                                    interp_ptr,
                                    real_ptr,
                                    dual.refresh_interval,
                                    &dual.sink,
                                    dual.single_present,
                                );
                            });
                        }
                        render_context.add_command_buffer(raw_encoder.finish());
                    }
                }
            }
        }
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
                multiview_mask: None,
            });

        pass.set_pipeline(pipeline);
        pass.set_bind_group(0, bind_group, &[]);
        pass.draw(0..3, 0..1);
    }
}
