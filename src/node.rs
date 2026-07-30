//! MetalFX upscaling pass (spatial + temporal).
//!
//! Runs after Bevy's own `upscaling` system. Creates its own output texture at
//! full resolution, uses MetalFX to upscale from `main_texture` (low-res) into
//! it, then blits to the swapchain via a render pass on
//! `ViewTarget::out_texture()`.
//!
//! Through 0.3 this was a render-graph `ViewNode`. Bevy 0.19 removed the render
//! graph in favour of ECS schedules, so the entry point is now the
//! [`metalfx_upscale`] system; everything below it is unchanged, because the
//! MetalFX work only ever needed a command encoder and a pair of textures.
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
//! It is opt-in (`MetalFxDualPresent::enabled`). Presents are accepted at
//! twice the single-present rate; whether they reach the panel is unverified,
//! because `MTLDrawable.presentedTime` does not populate on the development
//! machine for any program. See [`crate::present`] and
//! `docs/m5-max-performance-research.md`.
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
//! | C/D   | here | swapchain blit, then the optional second present |
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
//! polls for readiness each frame and falls through to Bevy's bilinear upscaling
//! until the scaler is ready.

mod encode;
mod resolve;
mod scaler;

#[cfg(feature = "frame-interpolation")]
use std::ffi::c_void;
use std::sync::Mutex;

use bevy::core_pipeline::blit::{BlitPipeline, BlitPipelineKey};
use bevy::core_pipeline::prepass::ViewPrepassTextures;
use bevy::prelude::*;
use bevy::render::camera::TemporalJitter;
use bevy::render::render_resource::{
    BindGroup, CachedRenderPipelineId, Extent3d, PipelineCache, RenderPassDescriptor,
    SpecializedRenderPipeline, TextureView, TextureViewId,
};
use bevy::render::renderer::{RenderContext, ViewQuery};
use bevy::render::view::ViewTarget;
#[cfg(feature = "frame-interpolation")]
use objc2::rc::Retained;
use objc2::runtime::ProtocolObject;
#[cfg(feature = "frame-interpolation")]
use objc2_metal_fx::MTLFXFrameInterpolator;
use objc2_metal_fx::MTLFXSpatialScaler;
#[cfg(feature = "temporal")]
use objc2_metal_fx::MTLFXTemporalScaler;

use crate::platform::wgpu_format_to_mtl;
use crate::MetalFxMode;

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

/// The MetalFX upscale pass.
///
/// Bevy 0.19 removed the render graph and drives rendering from ECS *schedules*
/// instead: a pass is an ordinary system that reaches the current view through
/// [`ViewQuery`] and the frame's encoder through [`RenderContext`]. That is the
/// entire integration change. The MetalFX work below is untouched by it,
/// because it only ever needed a command encoder and a pair of textures — the
/// graph was scaffolding around that, not part of it.
///
/// The per-frame caches that used to live in the graph node live in a [`Local`],
/// which gives them the same per-system persistence the node had.
#[allow(clippy::type_complexity)]
pub fn metalfx_upscale(
    view: ViewQuery<(
        &'static ViewTarget,
        Option<&'static ViewPrepassTextures>,
        Option<&'static TemporalJitter>,
        // `extract_cameras` clones `Projection` onto the render-world view
        // entity, so the camera frustum is readable here without a bespoke
        // extract system. Frame interpolation needs FOV/near/far from it.
        Option<&'static Projection>,
    )>,
    state: Local<MetalFxUpscaleNode>,
    mut render_context: RenderContext,
    world: &World,
) {
    state.run(&mut render_context, view.into_inner(), world);
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
        (target, prepass_textures, temporal_jitter, projection): (
            &ViewTarget,
            Option<&ViewPrepassTextures>,
            Option<&TemporalJitter>,
            Option<&Projection>,
        ),
        world: &World,
    ) {
        let main_tex = target.main_texture();
        let main_size = main_tex.size();
        let main_format = main_tex.format();

        let Some(color_mtl_fmt) = wgpu_format_to_mtl(main_format) else {
            log::error!("MetalFxUpscaleNode: unsupported format {:?}", main_format);
            return;
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
        if !self.ensure_scaler(
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
            mode,
            main_format,
            color_mtl_fmt,
            dynamic_res_range,
        ) {
            return;
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
        let is_temporal_like = state.scaler.is_temporal_like();

        if is_temporal_like {
            let Some(prepass) = prepass_textures else {
                log::warn!("MetalFxUpscaleNode: temporal mode but no prepass textures");
                return;
            };
            let Some(depth_attachment) = &prepass.depth else {
                log::warn!("MetalFxUpscaleNode: no depth prepass texture");
                return;
            };
            let Some(motion_attachment) = &prepass.motion_vectors else {
                log::warn!("MetalFxUpscaleNode: no motion vector prepass texture");
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
            return;
        }

        // --- Phase C: Blit metalfx_output → out_texture (swapchain) ---
        let pipeline_cache = world.resource::<PipelineCache>();
        let blit_pipeline = world.resource::<BlitPipeline>();

        let mut cached_pipeline = self.cached_pipeline.lock().unwrap();
        let pipeline_id = match *cached_pipeline {
            Some(id) => id,
            None => {
                // wgpu 29 / Bevy 0.19: the output format is now optional,
                // because a view target can exist without a resolved output.
                // Skip the frame rather than guessing a format.
                let Some(target_format) = target.out_texture_view_format() else {
                    log::error!("MetalFxUpscaleNode: view target has no output format");
                    return;
                };
                let key = BlitPipelineKey {
                    target_format,
                    blend_state: None,
                    samples: 1,
                    // 0.18's key had no colour-space knob and blitted the source
                    // through unchanged; `None` is that behaviour, not a new choice.
                    source_space: None,
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
            // Bevy 0.19 returns the attachment already wrapped in `Option`.
            color_attachments: &[target.out_texture_color_attachment(None)],
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
