//! Bevy plugin for Apple MetalFX upscaling and frame interpolation.
//!
//! Uses `objc2-metal-fx` for MetalFX framework bindings and integrates
//! as a render graph node replacing Bevy's built-in upscaling.
//!
//! ## Supported Modes
//! - **Spatial**: Single-frame ML upscaling (macOS 13+)
//! - **Temporal**: Multi-frame temporal upscaling with motion vectors (macOS 13+)
//! - **FrameInterpolation**: Generate intermediate frames (macOS 26+, Metal 4)
//!
//! Each mode has a matching cargo feature, and the features are cumulative:
//! `frame-interpolation` implies `temporal` implies `spatial`. They gate both
//! this crate's encode paths and the `objc2-metal-fx` bindings behind them, so
//! a narrower feature set really does compile a narrower surface.
//!
//! ## What is and is not verified
//!
//! Spatial and temporal upscaling are complete and stable. Frame interpolation
//! computes a correct intermediate frame and, with [`present::MetalFxDualPresent`]
//! enabled, presents it — at twice the accepted-present rate of a single
//! present, with the render rate unchanged. Whether the extra frame reaches the
//! display is **unverified**: see [`present`] for what that depends on.

#[cfg(target_os = "macos")]
mod platform;

#[cfg(target_os = "macos")]
mod node;

#[cfg(target_os = "macos")]
pub use node::{MetalFxConfig, MetalFxFrameTiming, MetalFxUpscaleNode};

#[cfg(not(target_os = "macos"))]
mod stub {
    use super::MetalFxMode;
    use bevy::prelude::*;

    /// Render-world configuration (stub for non-macOS platforms).
    ///
    /// Field visibility mirrors the macOS definition so cross-platform code
    /// sees one shape, not two.
    #[derive(Resource, Clone, Copy, bevy::render::extract_resource::ExtractResource)]
    pub struct MetalFxConfig {
        pub(crate) render_scale: f32,
        pub(crate) mode: MetalFxMode,
        pub(crate) dynamic_res_range: Option<(f32, f32)>,
    }

    /// Render graph node (stub for non-macOS platforms — does nothing).
    #[derive(Default)]
    pub struct MetalFxUpscaleNode;
}

#[cfg(not(target_os = "macos"))]
pub use stub::{MetalFxConfig, MetalFxUpscaleNode};

#[cfg(all(target_os = "macos", feature = "temporal"))]
mod jitter;

#[cfg(target_os = "macos")]
pub mod gpu_timing;

#[cfg(target_os = "macos")]
pub use gpu_timing::{GpuTimingSink, GpuTimingStats};

/// Display-timed dual presentation — the half of frame interpolation that
/// lives below the render graph. Only meaningful when interpolation is built.
#[cfg(all(target_os = "macos", feature = "frame-interpolation"))]
pub mod present;

#[cfg(all(target_os = "macos", feature = "frame-interpolation"))]
pub use present::{display_awake, PresentSink, PresentStats};

/// Shared GPU-timing sink, cloned into both the main world (for the debug
/// server to read) and the render world (for the upscale node to push into).
/// Holds recent per-command-buffer GPU-elapsed samples for Phase 0 bound-ness
/// analysis. See [`gpu_timing`] for the metric caveat.
#[cfg(target_os = "macos")]
#[derive(bevy::prelude::Resource, Clone)]
pub struct GpuTimingDiag(pub std::sync::Arc<GpuTimingSink>);

/// Check whether MetalFX is available on this system at runtime.
///
/// Returns `false` on non-macOS platforms or when the MetalFX framework
/// is not present (macOS < 13).
pub fn is_available() -> bool {
    #[cfg(target_os = "macos")]
    {
        platform::is_available_impl()
    }
    #[cfg(not(target_os = "macos"))]
    {
        false
    }
}

/// MetalFX operating mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum MetalFxMode {
    /// Single-frame spatial upscaling. Needs only color input.
    /// Available on macOS 13+ with Apple Silicon.
    #[default]
    Spatial,
    /// Temporal upscaling with motion vectors + jitter.
    /// Better quality than spatial but requires MotionVectorPrepass.
    Temporal,
    /// Frame interpolation — generates intermediate frames between rendered frames.
    /// Requires macOS 26+ (Metal 4). Adds +1 frame of input latency.
    FrameInterpolation,
    /// Bypass MetalFX — render at full res with Bevy's default upscaling.
    /// Useful for A/B benchmarking.
    Disabled,
}

/// Configuration for the MetalFX plugin.
pub struct MetalFxPlugin {
    /// Render scale factor (0.25–1.0). Default 0.5 = half-res render.
    pub render_scale: f32,
    /// Which MetalFX mode to use.
    pub mode: MetalFxMode,
    /// Enable adaptive render scale — dynamically adjusts scale based on P99 frame time.
    /// The initial `render_scale` is snapped to the nearest supported step (0.5 or 0.75).
    /// The system will not scale outside this range. `MetalFxRenderScale` becomes mutable.
    pub adaptive: bool,
    /// Optional externally-owned GPU-timing sink (Phase 0 bench). When the host
    /// (e.g. `src-tauri`) wants to read GPU timings over the debug server, it
    /// constructs the sink, passes a clone here, and keeps a clone for the diag
    /// provider — avoiding a process-global. `None` ⇒ the plugin makes its own.
    #[cfg(target_os = "macos")]
    pub gpu_timing_sink: Option<std::sync::Arc<GpuTimingSink>>,
    /// Dual presentation for `FrameInterpolation`: present the synthesised
    /// frame on its own drawable, ahead of the real one, so interpolation
    /// actually raises the displayed frame rate.
    ///
    /// Ignored in every other mode. Turning it off keeps the interpolated frame
    /// computed-but-unpresented, which is only useful as a measurement control
    /// — it costs the full interpolation pass and shows nothing for it.
    #[cfg(all(target_os = "macos", feature = "frame-interpolation"))]
    pub dual_present: Option<present::MetalFxDualPresent>,
}

impl Default for MetalFxPlugin {
    fn default() -> Self {
        Self {
            render_scale: 0.5,
            mode: MetalFxMode::Spatial,
            adaptive: false,
            #[cfg(target_os = "macos")]
            gpu_timing_sink: None,
            #[cfg(all(target_os = "macos", feature = "frame-interpolation"))]
            dual_present: None,
        }
    }
}

/// Main-world resource holding the render scale for resolution override systems.
#[derive(bevy::prelude::Resource, Clone, Copy)]
pub struct MetalFxRenderScale(pub f32);

impl bevy::app::Plugin for MetalFxPlugin {
    fn build(&self, app: &mut bevy::app::App) {
        assert!(
            (0.1..=1.0).contains(&self.render_scale),
            "MetalFxPlugin: render_scale must be in [0.1, 1.0], got {}",
            self.render_scale
        );

        if !is_available() {
            log::warn!("MetalFX is not available on this system — plugin disabled");
            return;
        }

        if self.mode == MetalFxMode::Disabled {
            // Even in Disabled mode, apply resolution override if scale < 1.0.
            // This lets Bevy's built-in bilinear upscaler handle the upscale,
            // serving as a control condition for MetalFX benchmarks.
            if self.render_scale < 1.0 {
                log::info!(
                    "MetalFX Disabled + scale={} — applying resolution override only (Bevy bilinear upscaler)",
                    self.render_scale
                );
                app.insert_resource(MetalFxRenderScale(self.render_scale));
                app.add_systems(bevy::app::PostStartup, apply_resolution_override);
                app.add_systems(bevy::app::Update, update_resolution_on_resize);
            } else {
                log::info!("MetalFX mode is Disabled at full resolution — bypassing");
            }
            return;
        }

        log::info!(
            "MetalFX plugin initialized: mode={:?}, render_scale={}",
            self.mode,
            self.render_scale
        );

        // Main-world: insert render scale resource and resolution override systems.
        app.insert_resource(MetalFxRenderScale(self.render_scale));
        app.insert_resource(MetalFxModeResource(self.mode));
        // Main-world MetalFxConfig for render-world extraction. In adaptive mode
        // the temporal scaler is built with dynamic resolution spanning the
        // governor's scale range, so scale changes flex without a scaler rebuild.
        let dynamic_res_range = if self.adaptive {
            Some((SCALE_STEPS[0], SCALE_STEPS[SCALE_STEPS.len() - 1]))
        } else {
            None
        };
        app.insert_resource(MetalFxConfig {
            render_scale: self.render_scale,
            mode: self.mode,
            dynamic_res_range,
        });
        app.add_systems(bevy::app::PostStartup, apply_resolution_override);
        app.add_systems(bevy::app::Update, update_resolution_on_resize);

        // Adaptive render scale (opt-in).
        if self.adaptive {
            app.insert_resource(AdaptiveScaleState::new(self.render_scale));
            app.add_systems(
                bevy::app::Update,
                (
                    adaptive_scale_system,
                    sync_config_scale,
                    update_resolution_on_scale_change,
                )
                    .chain(),
            );
        } else {
            // Even without adaptive, keep config in sync for manual scale changes.
            app.add_systems(
                bevy::app::Update,
                (sync_config_scale, update_resolution_on_scale_change).chain(),
            );
        }

        // Temporal + FrameInterpolation modes: add prepass components and jitter system.
        #[cfg(feature = "temporal")]
        if self.mode == MetalFxMode::Temporal || self.mode == MetalFxMode::FrameInterpolation {
            app.add_systems(bevy::app::PostStartup, setup_temporal_camera);
            app.add_systems(bevy::app::Update, jitter::update_jitter);
        }
        #[cfg(not(feature = "temporal"))]
        if self.mode == MetalFxMode::Temporal || self.mode == MetalFxMode::FrameInterpolation {
            log::warn!(
                "MetalFX: {:?} mode requested but 'temporal' feature not enabled — falling back to Spatial",
                self.mode
            );
            app.insert_resource(MetalFxModeResource(MetalFxMode::Spatial));
        }

        // Extract MetalFxConfig from main world to render world each frame.
        // Must be added to main app — the plugin internally finds the RenderApp sub-app.
        #[cfg(target_os = "macos")]
        app.add_plugins(bevy::render::extract_resource::ExtractResourcePlugin::<
            MetalFxConfig,
        >::default());

        // Frame interpolation needs the real inter-frame interval; mirror the
        // main world's `Time` delta into the render world (which has no `Time`).
        #[cfg(target_os = "macos")]
        if self.mode == MetalFxMode::FrameInterpolation {
            app.insert_resource(MetalFxFrameTiming::default());
            app.add_systems(bevy::app::Update, update_frame_timing);
            app.add_plugins(bevy::render::extract_resource::ExtractResourcePlugin::<
                MetalFxFrameTiming,
            >::default());
        }

        // Dual presentation — the half of interpolation that lives below the
        // render graph. The layer can only be found on the main thread, and the
        // extra present is encoded in the render world, so the pointer is
        // captured here and extracted across.
        #[cfg(all(target_os = "macos", feature = "frame-interpolation"))]
        if self.mode == MetalFxMode::FrameInterpolation {
            app.insert_resource(self.dual_present.clone().unwrap_or_default());
            app.add_systems(bevy::app::Update, present::capture_metal_layer);
            app.add_plugins(bevy::render::extract_resource::ExtractResourcePlugin::<
                present::MetalFxDualPresent,
            >::default());
        }

        #[cfg(target_os = "macos")]
        {
            use bevy::core_pipeline::core_3d::graph::{Core3d, Node3d};
            use bevy::render::render_graph::{RenderGraphExt, ViewNodeRunner};
            use bevy::render::RenderApp;

            // Shared GPU-timing sink: same Arc in main world (debug server reads)
            // and render world (upscale node pushes). Phase 0 bound-ness bench.
            // Reuse a host-provided sink if given (so the debug provider shares
            // the exact Arc), else make our own.
            let timing = GpuTimingDiag(self.gpu_timing_sink.clone().unwrap_or_default());
            app.insert_resource(timing.clone());

            if let Some(render_app) = app.get_sub_app_mut(RenderApp) {
                render_app.insert_resource(timing);
                render_app
                    .add_render_graph_node::<ViewNodeRunner<MetalFxUpscaleNode>>(
                        Core3d,
                        MetalFxLabel,
                    )
                    // Run MetalFX after Bevy's UpscalingNode — we overwrite
                    // out_texture with the ML-upscaled result via Metal blit.
                    .add_render_graph_edges(Core3d, (Node3d::Upscaling, MetalFxLabel));
            }
        }
    }
}

use bevy::camera::MainPassResolutionOverride;
use bevy::prelude::*;
use bevy::render::camera::MipBias;

/// Texture LOD bias to apply when rendering below native resolution.
///
/// MetalFX (and Bevy's bilinear fallback) render the scene at `render_scale`
/// of the output resolution, so PBR textures are sampled at a coarser mip
/// level than the final image warrants. Biasing sampling by `log2(scale)`
/// (negative for `scale < 1.0`) pulls in a sharper mip, restoring texture
/// detail the upscaler would otherwise have to hallucinate. At `scale = 0.5`
/// this is `-1.0` — exactly one mip level sharper.
fn mip_bias_for_scale(scale: f32) -> f32 {
    scale.clamp(0.1, 1.0).log2()
}

#[cfg(feature = "temporal")]
use bevy::core_pipeline::prepass::{DepthPrepass, MotionVectorPrepass};
#[cfg(feature = "temporal")]
use bevy::render::camera::TemporalJitter;

/// Main-world resource reporting the MetalFX mode actually in effect.
///
/// Read this rather than assuming the mode you configured: the plugin falls
/// back to [`MetalFxMode::Spatial`] when a requested mode is unavailable at
/// runtime, and this resource is what says so.
///
/// Readable, not writable — an app cannot decide which mode is active, so the
/// field is crate-private and reached through [`MetalFxModeResource::get`].
#[derive(Resource, Clone, Copy)]
pub struct MetalFxModeResource(pub(crate) MetalFxMode);

impl MetalFxModeResource {
    /// The mode the plugin settled on.
    pub fn get(&self) -> MetalFxMode {
        self.0
    }
}

// --- Adaptive render scale ---

/// Scale steps from lowest quality (best perf) to highest quality (worst perf).
const SCALE_STEPS: [f32; 2] = [0.5, 0.75];
/// Number of frames in the rolling window (~2 seconds at 60fps).
const WINDOW_SIZE: usize = 120;
/// P99 threshold to trigger scale-down (60fps = 16.67ms).
const P99_SCALE_DOWN_MS: f32 = 16.67;
/// P99 threshold to trigger scale-up (generous margin below 60fps).
const P99_SCALE_UP_MS: f32 = 12.0;
/// Consecutive windows over threshold before scaling down.
const WINDOWS_TO_SCALE_DOWN: u32 = 3;
/// Consecutive windows under threshold before scaling up.
const WINDOWS_TO_SCALE_UP: u32 = 5;
/// Cooldown after a scale change (seconds). 10s covers temporal scaler background creation.
const SCALE_CHANGE_COOLDOWN: f32 = 10.0;
/// Evaluate P99 every N frames (half the window — overlapping evaluation).
const EVAL_CADENCE_FRAMES: u32 = 60;

/// Adaptive render scale state — tracks frame times and manages scale transitions.
#[derive(Resource)]
pub struct AdaptiveScaleState {
    /// Rolling buffer of recent frame times (milliseconds).
    frame_times: [f32; WINDOW_SIZE],
    /// Write index into `frame_times` (circular buffer).
    write_idx: usize,
    /// Number of valid samples (grows until buffer is full).
    sample_count: usize,
    /// Current scale step index into `SCALE_STEPS`.
    current_step: usize,
    /// Consecutive evaluation windows where P99 exceeded the scale-down threshold.
    consecutive_over: u32,
    /// Consecutive evaluation windows where P99 was under the scale-up threshold.
    consecutive_under: u32,
    /// Cooldown timer (seconds remaining). No scale changes while > 0.
    cooldown: f32,
    /// Frame counter for evaluation cadence.
    frames_since_eval: u32,
}

impl AdaptiveScaleState {
    fn new(initial_scale: f32) -> Self {
        let current_step = SCALE_STEPS
            .iter()
            .enumerate()
            .min_by(|(_, a), (_, b)| {
                (*a - initial_scale)
                    .abs()
                    .total_cmp(&(*b - initial_scale).abs())
            })
            .map(|(i, _)| i)
            .unwrap_or(0);

        Self {
            frame_times: [0.0; WINDOW_SIZE],
            write_idx: 0,
            sample_count: 0,
            current_step,
            consecutive_over: 0,
            consecutive_under: 0,
            cooldown: 0.0,
            frames_since_eval: 0,
        }
    }
}

/// Insert `MainPassResolutionOverride` on all Camera3d entities at startup.
fn apply_resolution_override(
    mut commands: Commands,
    cameras: Query<Entity, (With<Camera3d>, Without<MainPassResolutionOverride>)>,
    windows: Query<&Window>,
    scale: Res<MetalFxRenderScale>,
) {
    let Ok(window) = windows.single() else {
        return;
    };
    let w = window.physical_width();
    let h = window.physical_height();
    if w == 0 || h == 0 {
        return;
    }
    let override_w = (w as f32 * scale.0).round() as u32;
    let override_h = (h as f32 * scale.0).round() as u32;
    let mip_bias = mip_bias_for_scale(scale.0);

    for entity in cameras.iter() {
        log::info!(
            "MetalFX: setting MainPassResolutionOverride {override_w}x{override_h} \
             (window {w}x{h}, scale {}, mip_bias {mip_bias:.3})",
            scale.0
        );
        commands.entity(entity).insert((
            MainPassResolutionOverride(UVec2::new(override_w, override_h)),
            MipBias(mip_bias),
        ));
    }
}

/// Update resolution override when the window size changes.
fn update_resolution_on_resize(
    mut cameras: Query<&mut MainPassResolutionOverride, With<Camera3d>>,
    windows: Query<&Window, Changed<Window>>,
    scale: Res<MetalFxRenderScale>,
) {
    let Ok(window) = windows.single() else {
        return;
    };
    let w = window.physical_width();
    let h = window.physical_height();
    if w == 0 || h == 0 {
        return;
    }
    let override_w = (w as f32 * scale.0).round() as u32;
    let override_h = (h as f32 * scale.0).round() as u32;

    for mut res_override in cameras.iter_mut() {
        log::info!("MetalFX: resize -> MainPassResolutionOverride {override_w}x{override_h}");
        res_override.0 = UVec2::new(override_w, override_h);
    }
}

/// Adaptive render scale system — adjusts scale based on P99 frame time.
fn adaptive_scale_system(
    time: Res<Time>,
    mut state: ResMut<AdaptiveScaleState>,
    mut scale: ResMut<MetalFxRenderScale>,
) {
    // Record frame time.
    let dt_ms = time.delta_secs() * 1000.0;
    let idx = state.write_idx;
    state.frame_times[idx] = dt_ms;
    state.write_idx = (idx + 1) % WINDOW_SIZE;
    if state.sample_count < WINDOW_SIZE {
        state.sample_count += 1;
    }

    // Tick cooldown.
    if state.cooldown > 0.0 {
        state.cooldown -= time.delta_secs();
        if state.cooldown > 0.0 {
            state.frames_since_eval = 0;
            return;
        }
        state.consecutive_over = 0;
        state.consecutive_under = 0;
        state.frames_since_eval = 0;
    }

    // Check evaluation cadence.
    state.frames_since_eval += 1;
    if state.frames_since_eval < EVAL_CADENCE_FRAMES {
        return;
    }
    state.frames_since_eval = 0;

    // Need enough samples.
    if state.sample_count < WINDOW_SIZE / 2 {
        return;
    }

    // Compute P99 from rolling window.
    let count = state.sample_count;
    let mut sorted = state.frame_times;
    sorted[..count].sort_by(|a, b| a.total_cmp(b));
    let p99_idx = ((count as f32 * 0.99) as usize).min(count - 1);
    let p99 = sorted[p99_idx];

    // Evaluate thresholds with proper hysteresis.
    // Dead zone (P99_SCALE_UP_MS..=P99_SCALE_DOWN_MS): neither counter advances or resets.
    // This prevents jitter near the threshold from resetting accumulated evidence.
    if p99 > P99_SCALE_DOWN_MS {
        state.consecutive_over += 1;
        state.consecutive_under = 0;
    } else if p99 < P99_SCALE_UP_MS {
        state.consecutive_under += 1;
        state.consecutive_over = 0;
    }
    // Dead zone: no action — counters hold their values.

    // Scale down: move to lower step.
    if state.consecutive_over >= WINDOWS_TO_SCALE_DOWN && state.current_step > 0 {
        let old = SCALE_STEPS[state.current_step];
        state.current_step -= 1;
        let new_scale = SCALE_STEPS[state.current_step];
        log::info!(
            "MetalFX adaptive: scale DOWN {old} -> {new_scale} (P99={p99:.2}ms > {P99_SCALE_DOWN_MS}ms)"
        );
        scale.0 = new_scale;
        state.cooldown = SCALE_CHANGE_COOLDOWN;
        state.consecutive_over = 0;
        state.consecutive_under = 0;
    } else if state.consecutive_under >= WINDOWS_TO_SCALE_UP
        && state.current_step < SCALE_STEPS.len() - 1
    {
        // Scale up: move to higher step.
        let old = SCALE_STEPS[state.current_step];
        state.current_step += 1;
        let new_scale = SCALE_STEPS[state.current_step];
        log::info!(
            "MetalFX adaptive: scale UP {old} -> {new_scale} (P99={p99:.2}ms < {P99_SCALE_UP_MS}ms)"
        );
        scale.0 = new_scale;
        state.cooldown = SCALE_CHANGE_COOLDOWN;
        state.consecutive_over = 0;
        state.consecutive_under = 0;
    }
}

/// Update resolution override when the render scale changes (adaptive mode).
fn update_resolution_on_scale_change(
    mut cameras: Query<(&mut MainPassResolutionOverride, &mut MipBias), With<Camera3d>>,
    windows: Query<&Window>,
    scale: Res<MetalFxRenderScale>,
) {
    if !scale.is_changed() || scale.is_added() {
        return;
    }
    let Ok(window) = windows.single() else {
        return;
    };
    let w = window.physical_width();
    let h = window.physical_height();
    if w == 0 || h == 0 {
        return;
    }
    let override_w = (w as f32 * scale.0).round() as u32;
    let override_h = (h as f32 * scale.0).round() as u32;
    let mip_bias = mip_bias_for_scale(scale.0);

    for (mut res_override, mut bias) in cameras.iter_mut() {
        log::info!(
            "MetalFX: scale change -> MainPassResolutionOverride {override_w}x{override_h} \
             (scale={}, mip_bias {mip_bias:.3})",
            scale.0
        );
        res_override.0 = UVec2::new(override_w, override_h);
        bias.0 = mip_bias;
    }
}

/// Keep main-world MetalFxConfig.render_scale in sync with MetalFxRenderScale.
fn sync_config_scale(scale: Res<MetalFxRenderScale>, mut config: ResMut<MetalFxConfig>) {
    if scale.is_changed() && !scale.is_added() {
        config.render_scale = scale.0;
    }
}

/// Mirror the main-world frame delta into [`MetalFxFrameTiming`] for extraction.
///
/// Clamped to a sane interval: MetalFX divides by `deltaTime` internally, so a
/// zero (first frame, or a paused clock) would poison the interpolation, and a
/// multi-second hitch would extrapolate motion absurdly far. The bounds span
/// ~1000 Hz down to ~5 Hz.
#[cfg(target_os = "macos")]
fn update_frame_timing(time: Res<Time>, mut timing: ResMut<MetalFxFrameTiming>) {
    timing.delta_seconds = time.delta_secs().clamp(0.001, 0.2);
}

/// Insert prepass components and jitter on Camera3d for temporal mode.
#[cfg(feature = "temporal")]
fn setup_temporal_camera(
    mut commands: Commands,
    cameras: Query<Entity, (With<Camera3d>, Without<MotionVectorPrepass>)>,
) {
    for entity in cameras.iter() {
        log::info!("MetalFX temporal: adding MotionVectorPrepass + DepthPrepass + TemporalJitter + Msaa::Off");
        commands.entity(entity).insert((
            MotionVectorPrepass,
            DepthPrepass,
            TemporalJitter::default(),
            // Disable MSAA — MetalFX temporal provides anti-aliasing via jittered
            // accumulation. MSAA multisampled depth textures can't be partially
            // copied to content-sized input buffers for MetalFX.
            bevy::render::view::Msaa::Off,
        ));
    }
}

/// Render graph label for the MetalFX upscale node.
#[derive(Debug, Hash, PartialEq, Eq, Clone, bevy::render::render_graph::RenderLabel)]
pub struct MetalFxLabel;

/// Probe whether a spatial scaler can be created for the given render device.
///
/// Extracts the raw Metal device from Bevy's `RenderDevice`, attempts to create
/// a spatial scaler at 800x450 → 1600x900 (Bgra8Unorm), and returns `true` on success.
/// Returns `false` on non-macOS or if scaler creation fails.
///
/// Intended for integration testing — not needed at runtime (the plugin handles
/// scaler creation internally).
pub fn probe_spatial_scaler(_render_device: &bevy::render::renderer::RenderDevice) -> bool {
    #[cfg(target_os = "macos")]
    {
        use foreign_types::ForeignType;
        use std::ffi::c_void;

        if !is_available() {
            return false;
        }

        let wgpu_dev = _render_device.wgpu_device();
        // SAFETY: a probe on a live render device, outside the render graph, so
        // no encoder `as_hal_mut` can be in flight to contend for the snatch lock.
        let Some(hal_dev) = (unsafe { wgpu_dev.as_hal::<wgpu_hal::metal::Api>() }) else {
            return false;
        };
        let device_ptr = {
            // SAFETY: read while the lock is held; scaler creation retains what
            // it needs, and the pointer does not outlive this function.
            let dev_lock = hal_dev.raw_device().lock();
            dev_lock.as_ptr() as *mut c_void
        };

        let fmt = bevy::render::render_resource::TextureFormat::Bgra8Unorm;
        let Some(color_fmt) = platform::wgpu_format_to_mtl(fmt) else {
            return false;
        };

        let scaler = unsafe {
            platform::try_create_spatial_scaler_from_raw(
                device_ptr, 800, 450, 1600, 900, color_fmt, color_fmt,
            )
        };
        scaler.is_some()
    }
    #[cfg(not(target_os = "macos"))]
    {
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mip_bias_matches_log2_scale() {
        // Half-res render → one mip level sharper.
        assert!((mip_bias_for_scale(0.5) - (-1.0)).abs() < 1e-6);
        // Quarter-res → two levels sharper.
        assert!((mip_bias_for_scale(0.25) - (-2.0)).abs() < 1e-6);
        // Native res → no bias.
        assert!((mip_bias_for_scale(1.0) - 0.0).abs() < 1e-6);
    }

    #[test]
    fn mip_bias_clamps_degenerate_scales() {
        // Out-of-range scales must not produce NaN/±inf bias.
        assert!(mip_bias_for_scale(0.0).is_finite());
        assert!(mip_bias_for_scale(-1.0).is_finite());
        assert!((mip_bias_for_scale(2.0) - 0.0).abs() < 1e-6);
    }
}
