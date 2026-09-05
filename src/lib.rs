//! Bevy plugin for Apple MetalFX upscaling and frame interpolation.
//!
//! Uses `objc2-metal-fx` for MetalFX framework bindings and integrates
//! through Bevy 0.19's per-view render schedules. Spatial and Temporal reconstruct
//! scene color before postprocessing and native-resolution UI.
//!
//! <div class="warning">
//!
//! **docs.rs shows the portable API built on Linux.**
//!
//! The controller, effect observations, and history requests are available there.
//! The macOS renderer, timing, and presentation modules and platform-specific
//! [`MetalFxPlugin`] fields require native documentation built on a Mac.
//!
//! Pinning docs.rs to an Apple target does not work: it cross-compiles from a
//! Linux container and must still build the dependency graph, and `blake3`
//! (via `bevy_asset`) has a C build script that needs an Apple toolchain. That
//! was tried in 0.2.0 and produced no docs page at all.
//!
//! For the real API: `cargo doc --open -p bevy_metalfx --all-features` on a
//! Mac, or read the source.
//!
//! </div>
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
//! ## Observation and validation
//!
//! [`MetalFxEffectStatus`] reports per-view render decisions with explicit frame
//! identity and freshness. `OutputWritten` proves that commands to copy the
//! reconstructed image were encoded; it does not prove GPU completion or panel
//! delivery. Requested configuration alone never proves an active effect.
//!
//! Adaptive operation accepts only explicitly validated GPU frame-cost samples.
//! Without them it holds quality and reports unavailable timing. CPU frame-loop
//! intervals, dedicated Metal command-buffer durations, and the opt-in marker
//! experiment are not interchangeable with validated frame cost.
//!
//! Frame interpolation retains an experimental late composition/presentation
//! path. Historical presentation results are dated evidence; they do not certify
//! current native UI, HDR composition, latency, or physical panel delivery.
//! See the repository smoke fixture and research reports for reproducible checks.

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
    ///
    /// The fields are deliberately unread here: nothing on a non-macOS target
    /// consumes them, and the struct exists so cross-platform code can name and
    /// insert the resource without `#[cfg]`. They stopped being `pub` in 0.2,
    /// which is what makes the lint notice — it is the point of the type, not a
    /// gap in it. Without this the crate warns on every docs.rs build, which is
    /// a Linux build of the default features.
    #[allow(dead_code)]
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

// Not platform-gated: the Halton sequence and the jitter system are plain
// Bevy, and the call site in `build` is gated on the feature alone. Adding
// `target_os = "macos"` here made `--features temporal` fail to compile on
// every other platform — the module was configured out while its caller was
// not. Shipped broken in 0.1.0; caught by cross-checking the docs.rs target.
#[cfg(feature = "temporal")]
mod jitter;

#[cfg(target_os = "macos")]
pub mod gpu_timing;

#[cfg(target_os = "macos")]
pub use gpu_timing::{GpuTimingSink, GpuTimingStats};

pub mod effect;
pub use effect::{
    MetalFxEffectObservation, MetalFxEffectReason, MetalFxEffectState, MetalFxEffectStatus,
};

pub mod adaptive;
pub mod adaptive_runtime;
#[cfg(test)]
mod lifecycle_tests;
pub use adaptive_runtime::{
    MetalFxAdaptiveConfig, MetalFxAdaptiveContext, MetalFxAdaptiveReason, MetalFxAdaptiveStatus,
    MetalFxFrameCostInput, ValidatedGpuFrameCost,
};
#[cfg(target_os = "macos")]
mod control_upscale;
mod effect_runtime;
#[cfg(target_os = "macos")]
pub mod frame_timing;
pub use effect_runtime::MetalFxObservationFrame;

/// Display-timed dual presentation — the half of frame interpolation that
/// lives below the render graph. Only meaningful when interpolation is built.
#[cfg(all(target_os = "macos", feature = "frame-interpolation"))]
pub mod present;

#[cfg(all(target_os = "macos", feature = "frame-interpolation"))]
pub use present::{display_awake, PresentSink, PresentStats};

/// Shared GPU-timing sink, cloned into both the main world (for the debug
/// server to read) and the render world (for the upscale node to push into).
/// Holds dedicated command-buffer elapsed samples, including upstream waits.
/// These are diagnostic intervals, not isolated MetalFX cost or total frame cost.
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
    /// Bypass MetalFX. Native scale uses Bevy's normal output; a reduced scale
    /// uses the cropped bilinear control before postprocessing and native UI.
    Disabled,
}

/// Configuration for the MetalFX plugin.
pub struct MetalFxPlugin {
    /// Render scale factor (0.1–1.0), subject to the selected mode/device band.
    /// Default 0.5 renders half the output width and height.
    pub render_scale: f32,
    /// Which MetalFX mode to use.
    pub mode: MetalFxMode,
    /// Enable adaptive render scale using fresh, validated GPU frame-cost samples.
    ///
    /// The governor climbs the [`MetalFxQuality`] ladder the device admits
    /// (see [`MetalFxDeviceScaleBand`]); the initial `render_scale` is snapped
    /// to the nearest rung. On an M5 that ladder reaches one third resolution;
    /// on earlier chips it stops at one half. The default quality floor is 0.5,
    /// so lower rungs require an explicit floor change. `MetalFxRenderScale` becomes
    /// mutable. Configure the target and quality floor with [`MetalFxAdaptiveConfig`].
    /// Without a validated frame signal, adaptation holds scale and reports why;
    /// app cadence and the MetalFX command-buffer timer are not substitutes.
    pub adaptive: bool,
    /// Optional externally-owned GPU-timing sink (Phase 0 bench). When the host
    /// (e.g. `src-tauri`) wants to read GPU timings over the debug server, it
    /// constructs the sink, passes a clone here, and keeps a clone for the diag
    /// provider — avoiding a process-global. `None` ⇒ the plugin makes its own.
    #[cfg(target_os = "macos")]
    pub gpu_timing_sink: Option<std::sync::Arc<GpuTimingSink>>,
    /// Dual presentation for `FrameInterpolation`: present the synthesised
    /// frame on its own drawable ahead of the real one. This submits two images;
    /// displayed cadence, image ordering, and latency require separate validation.
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

/// The render-scale band the active mode will accept on *this device*, as
/// fractions of the output resolution.
///
/// This is the floor the hardware reports, which is a different thing from
/// [`MetalFxScaleRange`]: that one is the band the plugin was *configured*
/// with, this one is the band it *could* be configured with. The distinction
/// matters because the floor moved. Every Apple Silicon generation before M5
/// reports a maximum temporal upscale ratio of `2.0` — half resolution — and
/// the M5 family reports `3.0`, one third in each dimension, nine times fewer
/// rasterized pixels than native. A ladder that stops at `0.5` because it was
/// written on an M1 leaves that on the table, and nothing complains.
///
/// For [`MetalFxMode::Temporal`] and [`MetalFxMode::FrameInterpolation`] the
/// band is read from `MTLFXTemporalScalerDescriptor` once the render device
/// exists, in the plugin's `finish`. Until then, and on any machine where the
/// query is unavailable, it is the assumed pre-M5 band `0.5..=1.0`, and
/// [`Self::is_from_device`] says which you are looking at. Spatial and
/// disabled modes have no device floor at all — the spatial scaler accepts any
/// ratio — so their band is the plugin's own `0.1..=1.0` and is likewise not a
/// device fact.
#[derive(bevy::prelude::Resource, Debug, Clone, Copy, PartialEq)]
pub struct MetalFxDeviceScaleBand {
    min: f32,
    max: f32,
    from_device: bool,
}

impl MetalFxDeviceScaleBand {
    /// The band every Apple Silicon device before M5 reports for temporal
    /// scaling, used until the device has been asked.
    pub const ASSUMED_TEMPORAL: Self = Self {
        min: 0.5,
        max: 1.0,
        from_device: false,
    };

    /// The plugin's own accepted range, for modes with no device floor.
    pub const PLUGIN_RANGE: Self = Self {
        min: 0.1,
        max: 1.0,
        from_device: false,
    };

    /// Build the band from what MetalFX reports: upscale ratios, `output /
    /// input`, so the ends swap under the reciprocal. `(1.0, 3.0)` becomes
    /// `0.333..=1.0`.
    pub fn from_upscale_ratios(min_ratio: f32, max_ratio: f32) -> Self {
        Self {
            min: (1.0 / max_ratio).clamp(0.1, 1.0),
            max: (1.0 / min_ratio).clamp(0.1, 1.0),
            from_device: true,
        }
    }

    /// The band for a mode, given the device's temporal ratios if they were
    /// read.
    pub fn for_mode(mode: MetalFxMode, temporal_ratios: Option<(f32, f32)>) -> Self {
        match mode {
            MetalFxMode::Temporal | MetalFxMode::FrameInterpolation => match temporal_ratios {
                Some((min_ratio, max_ratio)) => Self::from_upscale_ratios(min_ratio, max_ratio),
                None => Self::ASSUMED_TEMPORAL,
            },
            MetalFxMode::Spatial | MetalFxMode::Disabled => Self::PLUGIN_RANGE,
        }
    }

    /// Smallest render scale the device will reconstruct from.
    pub fn min(&self) -> f32 {
        self.min
    }

    /// Largest render scale, which is native.
    pub fn max(&self) -> f32 {
        self.max
    }

    /// Whether this came from the device or is the assumed band.
    pub fn is_from_device(&self) -> bool {
        self.from_device
    }

    /// The band as a range of output-resolution fractions.
    pub fn as_range(&self) -> core::ops::RangeInclusive<f32> {
        self.min..=self.max
    }

    /// Whether a render scale is inside the band, with a small tolerance so
    /// that one third compares equal to the device's `1.0 / 3.0`.
    pub fn contains(&self, render_scale: f32) -> bool {
        render_scale >= self.min - 1e-4 && render_scale <= self.max + 1e-4
    }
}

/// Quality presets named the way players already know them, pinned to the
/// render scales those names have meant since DLSS fixed them.
///
/// A preset is a render scale and nothing more; the plugin does not change
/// sharpening, jitter or anything else per preset. What the names buy is a
/// ladder whose rungs are the ones a settings menu would show, filtered by
/// what the device can actually do — see [`Self::ladder`].
///
/// | Preset | Render scale | Rasterized pixels |
/// |---|---|---|
/// | `UltraPerformance` | 1/3 | 11% |
/// | `Performance` | 1/2 | 25% |
/// | `Balanced` | 0.58 | 34% |
/// | `Quality` | 2/3 | 44% |
/// | `Native` | 1.0 | 100% — MetalFX acts as temporal anti-aliasing |
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum MetalFxQuality {
    /// One third resolution. Reported available on M5; not on earlier chips.
    UltraPerformance,
    /// Half resolution. Every supported device.
    Performance,
    /// 0.58 of output.
    Balanced,
    /// Two thirds of output.
    Quality,
    /// Native resolution, no upscale — temporal accumulation only.
    Native,
}

impl MetalFxQuality {
    /// Every preset, from the lowest render scale to native.
    pub const ALL: [MetalFxQuality; 5] = [
        MetalFxQuality::UltraPerformance,
        MetalFxQuality::Performance,
        MetalFxQuality::Balanced,
        MetalFxQuality::Quality,
        MetalFxQuality::Native,
    ];

    /// The render scale this preset means, as a fraction of the output.
    pub const fn render_scale(self) -> f32 {
        match self {
            MetalFxQuality::UltraPerformance => 1.0 / 3.0,
            MetalFxQuality::Performance => 0.5,
            MetalFxQuality::Balanced => 0.58,
            MetalFxQuality::Quality => 2.0 / 3.0,
            MetalFxQuality::Native => 1.0,
        }
    }

    /// The same value the way MetalFX takes it: an upscale ratio.
    pub fn upscale_ratio(self) -> f32 {
        1.0 / self.render_scale()
    }

    /// Whether the device band admits this preset.
    pub fn is_available_on(self, band: &MetalFxDeviceScaleBand) -> bool {
        band.contains(self.render_scale())
    }

    /// The render scales of every preset the band admits, ascending — the
    /// rungs the adaptive governor climbs. Never empty: a band that admits no
    /// preset yields its own ends.
    pub fn ladder(band: &MetalFxDeviceScaleBand) -> Vec<f32> {
        let mut steps: Vec<f32> = Self::ALL
            .iter()
            .map(|q| q.render_scale())
            .filter(|scale| band.contains(*scale))
            .collect();
        if steps.is_empty() {
            steps.push(band.min());
            if band.max() > band.min() {
                steps.push(band.max());
            }
        }
        steps
    }
}

/// The render-scale band MetalFX is configured to accept, inclusive.
///
/// Inserted by [`MetalFxPlugin`], so a consumer can ask what it is allowed to
/// write into [`MetalFxRenderScale`] instead of discovering the answer as a
/// scaler that silently fails to build.
///
/// **Why this needs an accessor at all.** A render scale here is a fraction of
/// the output resolution (`0.5` = half-res). MetalFX is configured in the other
/// direction, as *upscale ratios* — `output / input`, always `>= 1.0`. The two
/// are reciprocals, so converting also swaps the ends: the smallest render
/// scale is the largest upscale ratio. Handing MetalFX a fraction where it
/// wants a ratio makes `newTemporalScalerWithDevice` return `nil` and report
/// nothing further, which is exactly the failure this type exists to prevent.
/// See [`Self::as_upscale_ratios`].
#[derive(bevy::prelude::Resource, Debug, Clone, Copy, PartialEq)]
pub struct MetalFxScaleRange {
    min: f32,
    max: f32,
}

impl MetalFxScaleRange {
    /// Smallest render scale (largest upscale) MetalFX will accept here.
    pub fn min(&self) -> f32 {
        self.min
    }

    /// Largest render scale (smallest upscale) MetalFX will accept here.
    pub fn max(&self) -> f32 {
        self.max
    }

    /// The band as a range of output-resolution fractions.
    pub fn as_range(&self) -> core::ops::RangeInclusive<f32> {
        self.min..=self.max
    }

    /// The same band expressed the way MetalFX takes it: upscale ratios,
    /// `output / input`, always `>= 1.0`.
    ///
    /// Reciprocal of [`Self::as_range`], with the ends swapped — a `0.5` render
    /// scale is a `2.0` upscale, and a `0.75` render scale is a `~1.33` one, so
    /// the *minimum* scale produces the *maximum* ratio. These are the values
    /// handed to `setInputContentMinScale` / `setInputContentMaxScale`.
    pub fn as_upscale_ratios(&self) -> core::ops::RangeInclusive<f32> {
        1.0 / self.max..=1.0 / self.min
    }

    /// Whether `render_scale` is inside the band.
    ///
    /// The checkable condition: test before writing [`MetalFxRenderScale`].
    pub fn contains(&self, render_scale: f32) -> bool {
        self.as_range().contains(&render_scale)
    }
}

mod history;
pub use history::MetalFxHistoryReset;

#[cfg(target_os = "macos")]
impl MetalFxPlugin {
    /// Put the shared [`GpuTimingDiag`] in both worlds: the main world, where a
    /// debug server or harness reads `stats()`, and the render world, where the
    /// node pushes samples in.
    ///
    /// Reuses a host-provided sink when given, so a caller holding its own
    /// `Arc` reads the exact same ring the render world writes to; otherwise it
    /// creates a sink populated by completed active or timing-control encodes.
    fn install_gpu_timing(&self, app: &mut bevy::app::App) {
        use bevy::render::RenderApp;

        let timing = GpuTimingDiag(self.gpu_timing_sink.clone().unwrap_or_default());
        app.insert_resource(timing.clone());
        if let Some(render_app) = app.get_sub_app_mut(RenderApp) {
            render_app.insert_resource(timing);
        }
    }
}

impl bevy::app::Plugin for MetalFxPlugin {
    /// Ask the device what `build` could not.
    ///
    /// There is no render device in `build`; there is one here. Everything
    /// below corrects a provisional value inserted in `build`, so nothing
    /// depends on this running — an app that never reaches `finish` keeps the
    /// assumed band and the ladder built from it.
    fn finish(&self, app: &mut bevy::app::App) {
        #[cfg(target_os = "macos")]
        {
            if !is_available() || self.mode == MetalFxMode::Disabled {
                return;
            }
            let Some(render_device) = app
                .world()
                .get_resource::<bevy::render::renderer::RenderDevice>()
                .cloned()
            else {
                log::warn!("MetalFX: no RenderDevice at finish — keeping the assumed scale band");
                return;
            };
            let band = device_scale_band(&render_device, effect_runtime::compiled_mode(self.mode));
            log::info!(
                "MetalFX: device scale band {:.3}..={:.3} ({})",
                band.min(),
                band.max(),
                if band.is_from_device() {
                    "reported by the device"
                } else {
                    "assumed, not reported"
                }
            );
            app.insert_resource(band);

            if self.adaptive {
                // The ladder, the scaler's dynamic band and the reported range
                // are one fact stated three ways; all three move together.
                let steps = MetalFxQuality::ladder(&band);
                let (lo, hi) = (steps[0], steps[steps.len() - 1]);
                adaptive_runtime::configure_ladder(app, steps.clone(), self.render_scale);
                if let Some(mut config) = app.world_mut().get_resource_mut::<MetalFxConfig>() {
                    config.dynamic_res_range = Some((lo, hi));
                }
                app.insert_resource(MetalFxScaleRange { min: lo, max: hi });
                log::info!("MetalFX adaptive: ladder {steps:?}");
            }
        }
        #[cfg(not(target_os = "macos"))]
        {
            let _ = app;
        }
    }

    fn build(&self, app: &mut bevy::app::App) {
        assert!(
            (0.1..=1.0).contains(&self.render_scale),
            "MetalFxPlugin: render_scale must be in [0.1, 1.0], got {}",
            self.render_scale
        );

        effect_runtime::install(app, self.mode, self.render_scale);
        adaptive_runtime::install(
            app,
            self.adaptive && is_available() && self.mode != MetalFxMode::Disabled,
        );
        let effective_mode = effect_runtime::compiled_mode(self.mode);

        // GPU timing is installed before either early return, for the same
        // reason `MetalFxRenderScale` is: a resource that only exists on the
        // active path is a resource that is missing precisely where a benchmark
        // needs it. `Disabled` is not an absence of measurement — it is the
        // control arm, and a control that reports nothing is indistinguishable
        // from a control that reports zero.
        #[cfg(target_os = "macos")]
        self.install_gpu_timing(app);

        // `MetalFxRenderScale` and `MetalFxModeResource` are this plugin's
        // "what actually happened" surface, and the README promises the plugin
        // disables itself gracefully with no `#[cfg]` guards in app code. That
        // promise only holds if the resources exist on the paths where it
        // disables itself. They did not, so any consumer reading
        // `Res<MetalFxRenderScale>` panicked with "Resource does not exist" on
        // precisely the machines and configurations the graceful path was
        // written for — a non-macOS build, or `MetalFxMode::Disabled`.
        //
        // Both report the *effective* values, not the requested ones: with the
        // plugin inactive no resolution override is applied, so the scale really
        // is 1.0, and saying otherwise would mislead an adaptive governor into
        // thinking it had headroom to reclaim.
        if !is_available() {
            log::warn!("MetalFX is not available on this system — plugin disabled");
            app.insert_resource(MetalFxRenderScale(1.0));
            app.insert_resource(MetalFxModeResource(MetalFxMode::Disabled));
            return;
        }

        if self.mode == MetalFxMode::Disabled {
            app.insert_resource(MetalFxModeResource(MetalFxMode::Disabled));
            // Even in Disabled mode, apply resolution override if scale < 1.0.
            // The cropped bilinear pass expands content before postprocessing,
            // serving as a control condition for MetalFX benchmarks.
            if self.render_scale < 1.0 {
                log::info!(
                    "MetalFX Disabled + scale={} — applying cropped bilinear control",
                    self.render_scale
                );
                app.insert_resource(MetalFxRenderScale(self.render_scale));
                register_resolution_override(app);
                #[cfg(target_os = "macos")]
                control_upscale::install(app);
            } else {
                log::info!("MetalFX mode is Disabled at full resolution — bypassing");
                app.insert_resource(MetalFxRenderScale(1.0));
            }

            // Opt-in only: a host that asked for a timing sink is running a
            // benchmark and wants the control arm instrumented. Everyone else
            // gets the historical behaviour, submitting nothing extra.
            #[cfg(target_os = "macos")]
            if self.gpu_timing_sink.is_some() {
                use bevy::core_pipeline::schedule::Core3d;
                use bevy::core_pipeline::upscaling::upscaling;
                use bevy::render::RenderApp;

                if let Some(render_app) = app.get_sub_app_mut(RenderApp) {
                    render_app.add_systems(
                        Core3d,
                        node::metalfx_timing_only
                            .in_set(MetalFxLabel)
                            .after(upscaling),
                    );
                    log::info!(
                        "MetalFX Disabled + gpu_timing_sink — submitting an empty timed \
                         command buffer per frame to measure the GPU-timing floor"
                    );
                }
            }
            return;
        }

        log::info!(
            "MetalFX plugin initialized: mode={:?}, render_scale={}",
            effective_mode,
            self.render_scale
        );

        // Main-world: insert render scale resource and resolution override systems.
        app.insert_resource(MetalFxRenderScale(self.render_scale));
        app.insert_resource(MetalFxModeResource(effective_mode));
        // The device cannot be asked yet — there is no render device in
        // `build` — so the band and the ladder built from it are provisional
        // here and corrected in `finish`. The provisional band is the assumed
        // pre-M5 one, which is the safe direction: it can only widen.
        let provisional_band = MetalFxDeviceScaleBand::for_mode(effective_mode, None);
        let provisional_ladder = MetalFxQuality::ladder(&provisional_band);
        app.insert_resource(provisional_band);
        // Main-world MetalFxConfig for render-world extraction. In adaptive mode
        // the temporal scaler is built with dynamic resolution spanning the
        // governor's scale range, so scale changes flex without a scaler rebuild.
        let dynamic_res_range = if self.adaptive {
            Some((
                provisional_ladder[0],
                provisional_ladder[provisional_ladder.len() - 1],
            ))
        } else {
            None
        };
        // Derived from `dynamic_res_range`, not restated alongside it: this is
        // the same band the temporal scaler is actually created with, so the
        // two cannot drift. Without adaptive scaling there is no band — the
        // scaler is rebuilt whenever the scale moves — so the range collapses
        // to the configured scale, which is the honest answer to "what will be
        // accepted without a rebuild".
        let (range_min, range_max) =
            dynamic_res_range.unwrap_or((self.render_scale, self.render_scale));
        app.insert_resource(MetalFxScaleRange {
            min: range_min,
            max: range_max,
        });
        app.insert_resource(MetalFxConfig {
            render_scale: self.render_scale,
            mode: effective_mode,
            dynamic_res_range,
        });
        // Registered unconditionally, including off macOS: the crate promises a
        // consumer needs no `#[cfg]` guards of its own, and that only holds if
        // the resource it writes to exists everywhere.
        app.init_resource::<MetalFxHistoryReset>();
        app.add_plugins(bevy::render::extract_resource::ExtractResourcePlugin::<
            MetalFxHistoryReset,
        >::default());
        register_resolution_override(app);

        // Adaptive render scale (opt-in).
        if self.adaptive {
            adaptive_runtime::configure_ladder(app, provisional_ladder.clone(), self.render_scale);
            app.add_systems(
                bevy::app::Update,
                (
                    adaptive_runtime::adaptive_scale_system,
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
        if effective_mode == MetalFxMode::Temporal
            || effective_mode == MetalFxMode::FrameInterpolation
        {
            app.add_systems(bevy::app::PostStartup, setup_temporal_camera);
            app.add_systems(
                bevy::app::Update,
                (setup_temporal_camera, jitter::update_jitter).chain(),
            );
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
        if effective_mode == MetalFxMode::FrameInterpolation {
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
        if effective_mode == MetalFxMode::FrameInterpolation {
            app.insert_resource(self.dual_present.clone().unwrap_or_default());
            app.add_systems(bevy::app::Update, present::capture_metal_layer);
            app.add_plugins(bevy::render::extract_resource::ExtractResourcePlugin::<
                present::MetalFxDualPresent,
            >::default());
        }

        #[cfg(target_os = "macos")]
        {
            use bevy::core_pipeline::schedule::Core3d;
            use bevy::core_pipeline::upscaling::upscaling;
            use bevy::render::RenderApp;

            // The shared GPU-timing sink is installed at the top of `build`, so
            // it exists on the Disabled path too. Nothing to do here but wire
            // the node that pushes into it.
            if let Some(render_app) = app.get_sub_app_mut(RenderApp) {
                if effective_mode == MetalFxMode::FrameInterpolation {
                    // Experimental interpolation retains its legacy late output path.
                    render_app.add_systems(
                        Core3d,
                        node::metalfx_upscale.in_set(MetalFxLabel).after(upscaling),
                    );
                } else {
                    // Reconstruct scene color before postprocessing and native UI.
                    use bevy::core_pipeline::schedule::Core3dSystems;
                    render_app.add_systems(
                        Core3d,
                        node::metalfx_upscale
                            .in_set(MetalFxLabel)
                            .after(Core3dSystems::MainPass)
                            .before(Core3dSystems::EarlyPostProcess),
                    );
                }
            }
            if effective_mode != MetalFxMode::FrameInterpolation {
                control_upscale::install(app);
            }
        }
    }
}

use bevy::camera::MainPassResolutionOverride;
use bevy::prelude::*;
use bevy::render::camera::MipBias;
use bevy::render::sync_world::RenderEntity;
use bevy::render::Extract;

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

/// Register the resolution-override systems as a single unit.
///
/// The main-world insert and the render-world extract go in together and never
/// separately. They *were* separate — the main-world half existed alone for the
/// entire life of this crate, which is the bug documented on
/// [`extract_resolution_override`]. Registering them apart again would restore a
/// plugin that reports a render scale it does not apply.
fn register_resolution_override(app: &mut App) {
    app.add_systems(bevy::app::PostStartup, apply_resolution_override);
    app.add_systems(
        bevy::app::Update,
        (apply_resolution_override, update_resolution_on_resize)
            .chain()
            .before(sync_config_scale),
    );

    let Some(render_app) = app.get_sub_app_mut(bevy::render::RenderApp) else {
        // Deliberately loud. Skipping quietly is what the old code effectively
        // did, and the resulting failure presents as "MetalFX won us nothing"
        // rather than "the render scale never reached the GPU" — a wrong
        // conclusion about the upscaler instead of a bug report about wiring.
        log::error!(
            "MetalFX: no RenderApp when registering the resolution override. The render scale \
             will NOT be applied — MainPassResolutionOverride is read in the render world only. \
             Add MetalFxPlugin after DefaultPlugins."
        );
        return;
    };
    render_app.add_systems(
        bevy::render::ExtractSchedule,
        extract_resolution_override.after(bevy::render::camera::extract_cameras),
    );
}

/// Copy `MainPassResolutionOverride` onto the camera's **render-world** entity.
///
/// Bevy reads this component off the render-world view entity, never the main
/// world one: `main_opaque_pass_3d`, the prepass node and the view-uniform
/// writer all take it as `Option<&MainPassResolutionOverride>` on the entity
/// `extract_cameras` builds, and feed it to `Viewport::from_viewport_and_
/// override`. Its own doc comment states the contract — "Insert this component
/// on a 3d camera entity in the render world."
///
/// Nothing carries it across the world boundary for you. `extract_cameras`
/// extracts a *fixed* list — Hdr, CompositingSpace, ColorGrading, Exposure,
/// TemporalJitter, MipBias, RenderLayers, Projection, NoIndirectDrawing — and
/// this component is not on it. There is no `ExtractComponentPlugin` for it
/// either, and there cannot be a derived one: it is not `Clone`.
///
/// So a main-world-only insert is inert, in the most expensive way available.
/// The component reads back correctly from the main world, the logs report the
/// intended resolution, and the GPU rasterizes every pixel at full size.
/// Measured with gpu-load-bench at 8000 serial-ALU fragment iterations: a
/// native 640x360 window ran 3.6-3.8x faster than native 1280x720, while the
/// same 640x360 asked for through this override ran at full-resolution speed.
///
/// What hid it for so long is that `apply_resolution_override` inserts `MipBias`
/// on the same line — and `MipBias` *is* on the extract list. Half the pair
/// worked, so nothing downstream looked disconnected.
// Keep the ECS query filters visible beside this system.
#[allow(clippy::type_complexity)]
fn extract_resolution_override(
    mut commands: Commands,
    cameras: Extract<Query<(&RenderEntity, Option<&MainPassResolutionOverride>), With<Camera3d>>>,
) {
    for (render_entity, resolution) in cameras.iter() {
        // A camera that has not synced yet has no render entity to write to.
        let Ok(mut entity) = commands.get_entity(render_entity.id()) else {
            continue;
        };
        match resolution {
            Some(resolution) => {
                entity.insert(MainPassResolutionOverride(**resolution));
            }
            // Mirror absence, not just presence — `extract_cameras` does the
            // same for MipBias and for the same reason. A stale override left
            // on the render entity would go on shrinking the main pass after
            // the main world stopped asking for it.
            None => {
                entity.remove::<MainPassResolutionOverride>();
            }
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
        if res_override.0 == UVec2::new(override_w, override_h) {
            continue;
        }
        log::info!("MetalFX: resize -> MainPassResolutionOverride {override_w}x{override_h}");
        res_override.0 = UVec2::new(override_w, override_h);
    }
}

/// Update resolution override when the render scale changes (adaptive mode).
fn update_resolution_on_scale_change(
    mut cameras: Query<(&mut MainPassResolutionOverride, &mut MipBias), With<Camera3d>>,
    windows: Query<&Window>,
    scale: Res<MetalFxRenderScale>,
) {
    if !scale.is_changed() {
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
    if config.render_scale != scale.0 {
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
// Keep the ECS query filters visible beside this system.
#[allow(clippy::type_complexity)]
fn setup_temporal_camera(
    mut commands: Commands,
    cameras: Query<
        Entity,
        (
            With<Camera3d>,
            Or<(
                Without<MotionVectorPrepass>,
                Without<DepthPrepass>,
                Without<TemporalJitter>,
            )>,
        ),
    >,
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

/// System set containing the MetalFX upscale pass.
///
/// Through 0.3 this was a render *graph* label, because Bevy had a render graph
/// and the pass was a node in it. Bevy 0.19 drives rendering from schedules, so
/// the thing you order against is a set rather than a node — the type keeps its
/// name. Spatial/Temporal run before EarlyPostProcess; experimental interpolation
/// and the Disabled timing control run after Bevy upscaling. Order against the
/// selected mode when combining this set with other render systems.
#[derive(SystemSet, Debug, Hash, PartialEq, Eq, Clone)]
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
            // wgpu-hal 29 hands back the objc2 device directly; through 28 it
            // was behind a Mutex, hence the lock this used to take.
            //
            // SAFETY: scaler creation retains what it needs, and the pointer
            // does not outlive this function.
            &**hal_dev.raw_device() as *const _ as *mut c_void
        };

        let fmt = bevy::render::render_resource::TextureFormat::Bgra8Unorm;
        let Some(color_fmt) = platform::wgpu_format_to_mtl(fmt) else {
            return false;
        };

        let scaler = unsafe {
            platform::try_create_spatial_scaler_from_raw(
                device_ptr,
                800,
                450,
                1600,
                900,
                color_fmt,
                color_fmt,
                objc2_metal_fx::MTLFXSpatialScalerColorProcessingMode::Linear,
            )
        };
        scaler.is_some()
    }
    #[cfg(not(target_os = "macos"))]
    {
        false
    }
}

/// The render-scale band the device admits for a mode, asked directly.
///
/// The plugin calls this in `finish` and publishes the answer as
/// [`MetalFxDeviceScaleBand`]; it is public so an integration test, or an app
/// that builds its own settings menu before adding the plugin, can ask the
/// same question. Modes without a device floor, and platforms without MetalFX,
/// get the non-device band from [`MetalFxDeviceScaleBand::for_mode`].
pub fn device_scale_band(
    _render_device: &bevy::render::renderer::RenderDevice,
    mode: MetalFxMode,
) -> MetalFxDeviceScaleBand {
    #[cfg(all(target_os = "macos", feature = "temporal"))]
    {
        if matches!(
            mode,
            MetalFxMode::Temporal | MetalFxMode::FrameInterpolation
        ) && is_available()
        {
            use std::ffi::c_void;

            let wgpu_dev = _render_device.wgpu_device();
            // SAFETY: a query on a live render device, outside the render
            // graph, so no encoder `as_hal_mut` can be in flight to contend
            // for the snatch lock — the same footing as `probe_spatial_scaler`.
            if let Some(hal_dev) = unsafe { wgpu_dev.as_hal::<wgpu_hal::metal::Api>() } {
                // SAFETY: the pointer does not outlive this function, and the
                // query retains nothing.
                let device_ptr = &**hal_dev.raw_device() as *const _ as *mut c_void;
                let ratios = unsafe { platform::temporal_upscale_ratio_band_from_raw(device_ptr) };
                return MetalFxDeviceScaleBand::for_mode(mode, ratios);
            }
        }
    }
    MetalFxDeviceScaleBand::for_mode(mode, None)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The adaptive band must BE the governor's ladder, not a copy of it.
    ///
    /// This is the whole point of deriving the range from the ladder: if the
    /// device widens the ladder and the reported band does not follow,
    /// consumers clamp to a stale window and the bug is invisible.
    #[test]
    fn adaptive_range_tracks_the_governor_ladder() {
        let band = MetalFxDeviceScaleBand::from_upscale_ratios(1.0, 3.0);
        let steps = MetalFxQuality::ladder(&band);
        let range = MetalFxScaleRange {
            min: steps[0],
            max: steps[steps.len() - 1],
        };
        assert_eq!(range.min(), steps[0]);
        assert_eq!(range.max(), steps[steps.len() - 1]);
        assert!(range.min() < range.max(), "steps must be ascending");
        assert!(steps.windows(2).all(|w| w[0] < w[1]), "{steps:?}");
    }

    /// The presets are the DLSS factors, ascending, ending at native.
    #[test]
    fn presets_are_the_dlss_factors() {
        let scales: Vec<f32> = MetalFxQuality::ALL
            .iter()
            .map(|q| q.render_scale())
            .collect();
        assert!((scales[0] - 1.0 / 3.0).abs() < 1e-6, "{scales:?}");
        assert_eq!(scales[1], 0.5);
        assert_eq!(scales[2], 0.58);
        assert!((scales[3] - 2.0 / 3.0).abs() < 1e-6, "{scales:?}");
        assert_eq!(scales[4], 1.0);
        assert!(scales.windows(2).all(|w| w[0] < w[1]), "{scales:?}");
        assert!((MetalFxQuality::UltraPerformance.upscale_ratio() - 3.0).abs() < 1e-5);
    }

    /// The device reports upscale ratios; the band is their reciprocal with
    /// the ends swapped. `(1.0, 3.0)` is the M5 answer and it means one third.
    #[test]
    fn device_band_is_the_reciprocal_of_the_reported_ratios() {
        let m5 = MetalFxDeviceScaleBand::from_upscale_ratios(1.0, 3.0);
        assert!((m5.min() - 1.0 / 3.0).abs() < 1e-6, "{m5:?}");
        assert_eq!(m5.max(), 1.0);
        assert!(m5.is_from_device());
        assert!(m5.contains(1.0 / 3.0));
        assert!(!m5.contains(0.3));

        let pre_m5 = MetalFxDeviceScaleBand::from_upscale_ratios(1.0, 2.0);
        assert_eq!(pre_m5.min(), 0.5);
        assert!(!pre_m5.contains(1.0 / 3.0));
    }

    /// Before the device is asked, the band is the pre-M5 one and says so.
    /// Assuming the wider band would be the wrong direction: a scaler asked
    /// for a ratio its device rejects is nil, silently.
    #[test]
    fn the_assumed_band_is_the_narrow_one_and_is_labelled() {
        let assumed = MetalFxDeviceScaleBand::for_mode(MetalFxMode::Temporal, None);
        assert_eq!(assumed, MetalFxDeviceScaleBand::ASSUMED_TEMPORAL);
        assert!(!assumed.is_from_device());
        assert_eq!(assumed.min(), 0.5);

        // Spatial has no device floor; its band is the plugin's own range.
        let spatial = MetalFxDeviceScaleBand::for_mode(MetalFxMode::Spatial, Some((1.0, 3.0)));
        assert_eq!(spatial, MetalFxDeviceScaleBand::PLUGIN_RANGE);
        assert!(!spatial.is_from_device());
    }

    /// The ladder is the presets the band admits — no more, and never none.
    #[test]
    fn the_ladder_is_clipped_to_the_band() {
        let m5 = MetalFxDeviceScaleBand::from_upscale_ratios(1.0, 3.0);
        assert_eq!(
            MetalFxQuality::ladder(&m5).len(),
            5,
            "M5 admits every preset"
        );

        let pre_m5 = MetalFxDeviceScaleBand::ASSUMED_TEMPORAL;
        let ladder = MetalFxQuality::ladder(&pre_m5);
        assert_eq!(ladder.len(), 4, "{ladder:?}");
        assert_eq!(ladder[0], 0.5, "one third is not admitted: {ladder:?}");
        assert_eq!(*ladder.last().unwrap(), 1.0);

        // A band that admits no preset still yields rungs, its own ends.
        let odd = MetalFxDeviceScaleBand::from_upscale_ratios(1.05, 1.1);
        let ladder = MetalFxQuality::ladder(&odd);
        assert_eq!(ladder.len(), 2, "{ladder:?}");
        assert!(ladder[0] < ladder[1]);
    }

    /// Device admission and user quality floor jointly determine the start.
    #[test]
    fn adaptive_state_snaps_to_the_nearest_allowed_rung() {
        use adaptive::{AdaptiveConfig, AdaptiveController};
        let config = AdaptiveConfig {
            minimum_scale: 1.0 / 3.0,
            ..Default::default()
        };
        let narrow =
            AdaptiveController::new(config.clone(), vec![0.5, 0.58, 2.0 / 3.0, 1.0], 0.4).unwrap();
        assert_eq!(narrow.current_scale(), 0.5);
        let wide = AdaptiveController::new(config, vec![1.0 / 3.0, 0.5, 0.58, 2.0 / 3.0, 1.0], 0.4)
            .unwrap();
        assert!((wide.current_scale() - 1.0 / 3.0).abs() < 1e-6);
    }

    #[test]
    fn explicitly_disabled_mode_does_not_wait_for_adaptive_timing() {
        let mut app = App::new();
        app.add_plugins(MetalFxPlugin {
            mode: MetalFxMode::Disabled,
            adaptive: true,
            ..default()
        });
        assert_eq!(
            app.world().resource::<MetalFxAdaptiveStatus>().reason,
            MetalFxAdaptiveReason::Disabled
        );
    }

    /// Render scale -> MetalFX upscale ratio is a reciprocal, so the ends swap.
    ///
    /// Pinned because getting it backwards does not fail loudly: MetalFX takes
    /// ratios >= 1.0, and a render fraction < 1.0 makes
    /// `newTemporalScalerWithDevice` return nil with no diagnostic.
    #[test]
    fn upscale_ratios_are_the_reciprocal_with_ends_swapped() {
        let range = MetalFxScaleRange {
            min: 0.5,
            max: 0.75,
        };
        let ratios = range.as_upscale_ratios();

        // min scale 0.5 -> max ratio 2.0; max scale 0.75 -> min ratio ~1.333.
        assert!((ratios.start() - 1.0 / 0.75).abs() < 1e-6, "{ratios:?}");
        assert!((ratios.end() - 2.0).abs() < 1e-6, "{ratios:?}");

        // Every ratio MetalFX is handed must be a genuine upscale.
        assert!(
            *ratios.start() >= 1.0,
            "an upscale ratio below 1.0 makes the scaler nil: {ratios:?}"
        );
    }

    /// Native render scale is the degenerate case: ratio exactly 1.0, not 0.
    #[test]
    fn native_scale_is_a_unit_ratio() {
        let range = MetalFxScaleRange { min: 1.0, max: 1.0 };
        let ratios = range.as_upscale_ratios();
        assert!((ratios.start() - 1.0).abs() < 1e-6);
        assert!((ratios.end() - 1.0).abs() < 1e-6);
    }

    /// The checkable condition an out-of-band scale used to lack.
    #[test]
    fn contains_rejects_out_of_band_scales() {
        let range = MetalFxScaleRange {
            min: 0.5,
            max: 0.75,
        };
        assert!(range.contains(0.5), "inclusive at the bottom");
        assert!(range.contains(0.75), "inclusive at the top");
        assert!(range.contains(0.6));
        assert!(!range.contains(0.49));
        assert!(!range.contains(0.76));
        // The case that produced a silent nil scaler: asking for more than
        // native, i.e. an upscale ratio below 1.0.
        assert!(!range.contains(1.5));
    }

    #[test]
    fn history_reset_defaults_to_not_requested() {
        let reset = MetalFxHistoryReset::default();
        assert!(
            !reset.is_requested(),
            "a default reset would throw away history on every startup frame"
        );
    }

    #[test]
    fn history_reset_survives_app_updates_without_a_render_acknowledgement() {
        let mut app = bevy::app::App::new();
        app.init_resource::<MetalFxHistoryReset>();

        // Frame N: a consumer requests after `First` has already run.
        app.update();
        app.world_mut()
            .resource_mut::<MetalFxHistoryReset>()
            .request();
        assert!(
            app.world().resource::<MetalFxHistoryReset>().is_requested(),
            "the request must still be set when the render world extracts at end of frame"
        );

        // No renderer has encoded this request. Advancing the app cannot consume it.
        app.update();
        assert!(
            app.world().resource::<MetalFxHistoryReset>().is_requested(),
            "a request must survive until a temporal encode acknowledges it"
        );
    }

    /// The README promises the plugin "disables itself gracefully — no `#[cfg]`
    /// guards needed in your app code". That is only true if its public
    /// resources exist on the disabled paths too.
    ///
    /// They did not, so an app reading `Res<MetalFxRenderScale>` — the
    /// documented way to drive render scale — panicked with "Resource does not
    /// exist" on exactly the configurations the graceful path was written for.
    /// Caught by running the renderer with `--metalfx=off`, which no test
    /// covered because every test built the plugin in an enabled mode.
    #[test]
    fn disabled_mode_still_publishes_its_public_resources() {
        for scale in [1.0_f32, 0.5] {
            let mut app = bevy::app::App::new();
            app.add_plugins(MetalFxPlugin {
                render_scale: scale,
                mode: MetalFxMode::Disabled,
                ..Default::default()
            });

            let world = app.world();
            assert!(
                world.get_resource::<MetalFxRenderScale>().is_some(),
                "MetalFxRenderScale must exist at scale {scale} in Disabled mode — \
                 consumers read it unconditionally"
            );
            assert_eq!(
                world.get_resource::<MetalFxModeResource>().map(|m| m.get()),
                Some(MetalFxMode::Disabled),
                "the reported mode must say Disabled, not the requested mode"
            );
            #[cfg(target_os = "macos")]
            assert!(
                world.get_resource::<GpuTimingDiag>().is_some(),
                "GpuTimingDiag must exist at scale {scale} in Disabled mode — this is \
                 the control arm of every MetalFX benchmark, and a control that reports \
                 no samples is indistinguishable from one that reports zero"
            );
        }
    }

    /// The bug this crate shipped with for its entire life.
    ///
    /// `MainPassResolutionOverride` is read off the **render**-world view
    /// entity, and Bevy does not carry it across the world boundary for you: it
    /// is absent from `extract_cameras`' fixed extract list, and it cannot have
    /// an `ExtractComponentPlugin` because it is not `Clone`. Registering only
    /// the main-world half therefore produced a plugin that logged the right
    /// resolution, read the component back correctly, and rendered every pixel
    /// at full size anyway.
    ///
    /// Note what this test can and cannot do. It asserts the render-world half
    /// is REGISTERED. It cannot assert the override is EFFECTIVE — that needs a
    /// GPU, and is what `gpu-load-bench` measures by comparing the override
    /// against a NATIVE window at the same pixel count. Reading the component
    /// back proves only that it was applied, which is the distinction that let
    /// this survive an earlier investigation.
    #[test]
    fn the_resolution_override_is_registered_in_the_render_world_too() {
        use bevy::render::{ExtractSchedule, RenderApp};

        let mut app = bevy::app::App::new();
        let mut render_app = bevy::app::SubApp::new();
        render_app.add_schedule(bevy::ecs::schedule::Schedule::new(ExtractSchedule));
        app.insert_sub_app(RenderApp, render_app);

        register_resolution_override(&mut app);

        let extract = app
            .get_sub_app(RenderApp)
            .expect("render sub-app was inserted above")
            .get_schedule(ExtractSchedule)
            .expect("ExtractSchedule was added above");
        assert_eq!(
            extract.systems_len(),
            1,
            "register_resolution_override must add extract_resolution_override to the render \
             world's ExtractSchedule. Without it the main-world component is inert, and the \
             plugin reports a render scale it never applies."
        );
    }

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
