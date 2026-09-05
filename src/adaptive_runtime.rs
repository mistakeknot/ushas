//! Bevy adapter for adaptive resolution using explicitly validated GPU inputs.
//!
//! No GPU measurement is installed by default. App cadence, virtual time, and
//! MetalFX command-buffer elapsed time are not substituted for frame GPU cost.

use crate::adaptive::{
    AdaptiveConfig, AdaptiveConfigError, AdaptiveController, AdaptiveDecision, AdaptiveObservation,
    AdaptiveReason, AdaptiveSampleValidity,
};
use crate::{
    MetalFxEffectState, MetalFxEffectStatus, MetalFxMode, MetalFxModeResource,
    MetalFxObservationFrame, MetalFxRenderScale,
};
use bevy::camera::RenderTarget;
use bevy::prelude::*;
use bevy::window::{Monitor, OnMonitor, PrimaryWindow, WindowRef};
use std::collections::BTreeMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// Single authority for the adaptive frame budget. Neither option paces frames.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub enum MetalFxAdaptiveTarget {
    /// Use the primary window's current monitor metadata, or a labelled 60 FPS
    /// fallback. Reported refresh is not measured VRR or presentation cadence.
    #[default]
    Monitor,
    /// The application's intended rendered FPS budget. Invalid values are
    /// reported as configuration errors rather than replaced by a fallback.
    Explicit(f64),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MetalFxAdaptiveTargetFallback {
    PrimaryWindowUnavailable,
    CurrentMonitorUnavailable,
    RefreshUnavailable,
}

/// Provenance of the resolved budget, independent of GPU sample provenance.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum MetalFxAdaptiveTargetSource {
    /// No adaptive update has run (for example, adaptive is not enabled).
    #[default]
    Unresolved,
    Explicit,
    /// Bevy's OS-reported metadata for the window's associated monitor. Bevy
    /// 0.19 may retain this value after an in-place display-mode change.
    MonitorReportedRefresh {
        window_id: u64,
        monitor_id: u64,
    },
    Fallback60(MetalFxAdaptiveTargetFallback),
}

/// Policy for the opt-in adaptive plugin. `target` is its only FPS input;
/// the remaining fields configure quality, evidence and freshness.
///
/// ```
/// use bevy_metalfx::{MetalFxAdaptiveConfig, MetalFxAdaptiveTarget};
/// let config = MetalFxAdaptiveConfig {
///     target: MetalFxAdaptiveTarget::Explicit(60.0),
///     minimum_scale: 0.5,
///     ..Default::default()
/// };
/// assert_eq!(config.target, MetalFxAdaptiveTarget::Explicit(60.0));
/// ```
#[derive(Resource, Clone, Debug, PartialEq)]
pub struct MetalFxAdaptiveConfig {
    pub target: MetalFxAdaptiveTarget,
    /// Lowest acceptable render scale. Default 0.5.
    pub minimum_scale: f32,
    /// Exponential smoothing time constant in wall-clock time.
    pub smoothing_time: Duration,
    /// Continuous smoothed overload before one downward step.
    pub over_budget_for: Duration,
    /// Continuous smoothed headroom before one upward step.
    pub headroom_for: Duration,
    /// Fresh, ready evidence required after a transition.
    pub settling_time: Duration,
    /// Maximum wall-clock age and gap between usable GPU samples.
    pub max_sample_age: Duration,
    /// Overload threshold as a multiple of the resolved frame budget.
    pub over_budget_ratio: f64,
    /// Headroom threshold as a fraction of the resolved frame budget.
    pub headroom_ratio: f64,
    /// Required fractional GPU-cost reduction after a downward step.
    pub minimum_downshift_benefit: f64,
    /// Main-world camera entity bits. `None` selects the only active 3D camera.
    pub primary_view: Option<u64>,
    pub max_effect_age_frames: u64,
    pub max_effect_wall_age: Duration,
    pub max_sample_age_frames: u64,
}

impl Default for MetalFxAdaptiveConfig {
    fn default() -> Self {
        let policy = AdaptiveConfig::default();
        Self {
            target: MetalFxAdaptiveTarget::Monitor,
            minimum_scale: policy.minimum_scale,
            smoothing_time: policy.smoothing_time,
            over_budget_for: policy.over_budget_for,
            headroom_for: policy.headroom_for,
            settling_time: policy.settling_time,
            max_sample_age: policy.max_sample_age,
            over_budget_ratio: policy.over_budget_ratio,
            headroom_ratio: policy.headroom_ratio,
            minimum_downshift_benefit: policy.minimum_downshift_benefit,
            primary_view: None,
            max_effect_age_frames: 2,
            max_effect_wall_age: Duration::from_millis(250),
            max_sample_age_frames: 4,
        }
    }
}

impl MetalFxAdaptiveConfig {
    fn controller_policy(&self, target_fps: f64) -> AdaptiveConfig {
        AdaptiveConfig {
            target_fps,
            minimum_scale: self.minimum_scale,
            smoothing_time: self.smoothing_time,
            over_budget_for: self.over_budget_for,
            headroom_for: self.headroom_for,
            settling_time: self.settling_time,
            max_sample_age: self.max_sample_age,
            over_budget_ratio: self.over_budget_ratio,
            headroom_ratio: self.headroom_ratio,
            minimum_downshift_benefit: self.minimum_downshift_benefit,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct ResolvedTarget {
    fps: f64,
    source: MetalFxAdaptiveTargetSource,
}

fn resolve_target(world: &mut World, target: MetalFxAdaptiveTarget) -> ResolvedTarget {
    if let MetalFxAdaptiveTarget::Explicit(fps) = target {
        return ResolvedTarget {
            fps,
            source: MetalFxAdaptiveTargetSource::Explicit,
        };
    }
    let fallback = |reason| ResolvedTarget {
        fps: 60.0,
        source: MetalFxAdaptiveTargetSource::Fallback60(reason),
    };
    let mut windows =
        world.query_filtered::<(Entity, Option<&OnMonitor>), (With<Window>, With<PrimaryWindow>)>();
    let Ok((window, monitor_link)) = windows.single(world) else {
        return fallback(MetalFxAdaptiveTargetFallback::PrimaryWindowUnavailable);
    };
    let Some(monitor_entity) = monitor_link.map(|link| link.0) else {
        return fallback(MetalFxAdaptiveTargetFallback::CurrentMonitorUnavailable);
    };
    let Some(monitor) = world.get::<Monitor>(monitor_entity) else {
        return fallback(MetalFxAdaptiveTargetFallback::CurrentMonitorUnavailable);
    };
    let Some(rate) = monitor.refresh_rate_millihertz.filter(|rate| *rate > 0) else {
        return fallback(MetalFxAdaptiveTargetFallback::RefreshUnavailable);
    };
    ResolvedTarget {
        fps: f64::from(rate) / 1000.0,
        source: MetalFxAdaptiveTargetSource::MonitorReportedRefresh {
            window_id: window.to_bits(),
            monitor_id: monitor_entity.to_bits(),
        },
    }
}

/// The frame/view/configuration identity a validated measurement must carry.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct MetalFxAdaptiveContextSnapshot {
    pub epoch: u64,
    pub view_id: Option<u64>,
    pub render_scale: f32,
    pub output_size: [u32; 2],
    pub mode: MetalFxMode,
    pub target_fps: f64,
}

impl Default for MetalFxAdaptiveContextSnapshot {
    fn default() -> Self {
        Self {
            epoch: 0,
            view_id: None,
            render_scale: 1.0,
            output_size: [0, 0],
            mode: MetalFxMode::Disabled,
            target_fps: 60.0,
        }
    }
}

/// Measurement context, frozen separately for each render-world extraction.
/// Capture the render world's context when recording a frame, not later when
/// its asynchronous GPU callback completes. Main-world clones share updates.
#[derive(Resource, Clone, Default)]
pub struct MetalFxAdaptiveContext {
    snapshot: Arc<Mutex<MetalFxAdaptiveContextSnapshot>>,
    reset_generation: Arc<AtomicU64>,
}

impl MetalFxAdaptiveContext {
    pub fn snapshot(&self) -> MetalFxAdaptiveContextSnapshot {
        *self.snapshot.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Invalidate measurements after a camera cut, workload discontinuity,
    /// or a change to the timing instrument or its validation protocol.
    /// Resize, view, mode, and adaptive policy changes reset automatically.
    /// A normal adaptive scale change must not reset the benefit comparison.
    pub fn request_reset(&self) {
        self.reset_generation.fetch_add(1, Ordering::Relaxed);
    }
}

impl bevy::render::extract_resource::ExtractResource for MetalFxAdaptiveContext {
    type Source = Self;

    fn extract_resource(source: &Self) -> Self {
        Self {
            snapshot: Arc::new(Mutex::new(source.snapshot())),
            reset_generation: source.reset_generation.clone(),
        }
    }
}

/// A frame GPU-cost sample from an externally validated measurement adapter.
///
/// `source` names the instrument; `validation` identifies the trace or protocol
/// establishing GPU semantics and full frame coverage for this configuration.
/// Neither field validates the instrument automatically. Supplying a pass-only
/// timer, CPU duration, presentation interval, or unvalidated signal violates
/// this contract. Values are milliseconds; identity is captured at recording.
#[derive(Clone, Copy, Debug)]
pub struct ValidatedGpuFrameCost {
    pub frame_id: u64,
    pub view_id: u64,
    pub epoch: u64,
    pub sampled_at: Instant,
    pub sampled_scale: f32,
    pub gpu_ms: f64,
    pub source: &'static str,
    pub validation: &'static str,
}

/// Shared, bounded latest-per-view input. Empty by default, including on Metal.
#[derive(Resource, Clone, Default)]
pub struct MetalFxFrameCostInput(Arc<Mutex<BTreeMap<u64, ValidatedGpuFrameCost>>>);

impl MetalFxFrameCostInput {
    pub const MAX_VIEWS: usize = 64;

    /// Attest that the adapter has validated frame coverage, then publish.
    /// Invalid values, empty provenance, and old/duplicate samples are rejected.
    pub fn publish_validated(&self, sample: ValidatedGpuFrameCost) -> bool {
        if !sample.gpu_ms.is_finite()
            || sample.gpu_ms <= 0.0
            || !sample.sampled_scale.is_finite()
            || sample.sampled_scale <= 0.0
            || sample.sampled_scale > 1.0
            || sample.source.trim().is_empty()
            || sample.validation.trim().is_empty()
        {
            return false;
        }
        let mut samples = self.0.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(previous) = samples.get(&sample.view_id) {
            if sample.epoch < previous.epoch
                || (sample.epoch == previous.epoch
                    && (sample.frame_id <= previous.frame_id
                        || sample.sampled_at < previous.sampled_at))
            {
                return false;
            }
        } else if samples.len() >= Self::MAX_VIEWS {
            let (&oldest_view, oldest) = samples
                .iter()
                .min_by_key(|(_, s)| s.sampled_at)
                .expect("full input is nonempty");
            if sample.sampled_at <= oldest.sampled_at {
                return false;
            }
            samples.remove(&oldest_view);
        }
        samples.insert(sample.view_id, sample);
        true
    }

    pub fn latest(&self, view_id: u64) -> Option<ValidatedGpuFrameCost> {
        self.0
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(&view_id)
            .copied()
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MetalFxAdaptiveReason {
    Disabled,
    InvalidConfiguration(AdaptiveConfigError),
    NoRenderView,
    MultipleViewsUnsupported,
    UnsupportedTarget,
    UnsupportedViewport,
    EffectStale,
    EffectNotReady(MetalFxEffectState),
    EffectConfigurationMismatch,
    TimingUnavailable,
    SampleEpochMismatch,
    SampleFrameMismatch,
    Controller(AdaptiveReason),
}

/// Observable adaptive outcome. This is a controller decision, not proof of
/// image quality, GPU completion, or presentation.
#[derive(Resource, Clone, Debug)]
pub struct MetalFxAdaptiveStatus {
    pub reason: MetalFxAdaptiveReason,
    pub decision: Option<AdaptiveDecision>,
    pub scale: f32,
    pub epoch: u64,
    pub view_id: Option<u64>,
    pub sample_frame: Option<u64>,
    pub source: Option<&'static str>,
    pub validation: Option<&'static str>,
    /// Resolved budget, or the invalid explicit value when configuration fails.
    pub target_fps: f64,
    pub target_source: MetalFxAdaptiveTargetSource,
}

impl Default for MetalFxAdaptiveStatus {
    fn default() -> Self {
        Self {
            reason: MetalFxAdaptiveReason::Disabled,
            decision: None,
            scale: 1.0,
            epoch: 0,
            view_id: None,
            sample_frame: None,
            source: None,
            validation: None,
            target_fps: 60.0,
            target_source: MetalFxAdaptiveTargetSource::Unresolved,
        }
    }
}

#[derive(Resource)]
struct AdaptiveRuntime {
    enabled: bool,
    ladder: Vec<f32>,
    starting_scale: f32,
    origin: Instant,
    controller: Option<AdaptiveController>,
    applied_config: Option<MetalFxAdaptiveConfig>,
    applied_target: Option<ResolvedTarget>,
    fingerprint: Option<ViewFingerprint>,
    last_mode: Option<MetalFxMode>,
    reset_generation: u64,
    epoch: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct ViewFingerprint {
    view_id: u64,
    window_id: u64,
    output_size: [u32; 2],
}

pub(crate) fn install(app: &mut App, enabled: bool) {
    app.init_resource::<MetalFxAdaptiveConfig>()
        .init_resource::<MetalFxAdaptiveContext>()
        .init_resource::<MetalFxFrameCostInput>()
        .init_resource::<MetalFxAdaptiveStatus>()
        .insert_resource(AdaptiveRuntime {
            enabled,
            ladder: vec![0.5, 1.0],
            starting_scale: 1.0,
            origin: Instant::now(),
            controller: None,
            applied_config: None,
            applied_target: None,
            fingerprint: None,
            last_mode: None,
            reset_generation: 0,
            epoch: 0,
        });
    let context = app.world().resource::<MetalFxAdaptiveContext>().clone();
    let input = app.world().resource::<MetalFxFrameCostInput>().clone();
    if let Some(render_app) = app.get_sub_app_mut(bevy::render::RenderApp) {
        use bevy::render::extract_resource::{extract_resource, ExtractResource};
        render_app
            .insert_resource(MetalFxAdaptiveContext::extract_resource(&context))
            .insert_resource(input)
            .add_systems(
                bevy::render::ExtractSchedule,
                extract_resource::<MetalFxAdaptiveContext, ()>,
            );
    }
    if enabled {
        app.world_mut()
            .resource_mut::<MetalFxAdaptiveStatus>()
            .reason = MetalFxAdaptiveReason::TimingUnavailable;
    }
}

pub(crate) fn configure_ladder(app: &mut App, ladder: Vec<f32>, starting_scale: f32) {
    let mut runtime = app.world_mut().resource_mut::<AdaptiveRuntime>();
    runtime.ladder = ladder;
    runtime.starting_scale = starting_scale;
    runtime.controller = None;
    runtime.applied_config = None;
    runtime.applied_target = None;
}

pub(crate) fn adaptive_scale_system(world: &mut World) {
    update_at(world, Instant::now());
}

fn update_at(world: &mut World, now: Instant) {
    let config = world.resource::<MetalFxAdaptiveConfig>().clone();
    let target = resolve_target(world, config.target);
    let policy = config.controller_policy(target.fps);
    let mode = world
        .get_resource::<MetalFxModeResource>()
        .map(MetalFxModeResource::get)
        .unwrap_or(MetalFxMode::Disabled);
    let current_frame = world
        .get_resource::<MetalFxObservationFrame>()
        .map_or(0, |f| f.0);
    let scale = world
        .get_resource::<MetalFxRenderScale>()
        .map_or(1.0, |s| s.0);
    let selected = select_view(world, config.primary_view);
    let effects = world
        .get_resource::<MetalFxEffectStatus>()
        .cloned()
        .unwrap_or_default();
    let input = world.resource::<MetalFxFrameCostInput>().clone();
    let context = world.resource::<MetalFxAdaptiveContext>().clone();
    world.resource_scope(|world, mut runtime: Mut<AdaptiveRuntime>| {
        let mut status = MetalFxAdaptiveStatus {
            scale,
            target_fps: target.fps,
            target_source: target.source,
            ..Default::default()
        };
        if !runtime.enabled || mode == MetalFxMode::Disabled {
            if runtime.last_mode != Some(mode) {
                runtime.epoch = runtime
                    .epoch
                    .checked_add(1)
                    .expect("adaptive epoch exhausted");
                runtime.last_mode = Some(mode);
            }
            if let Some(controller) = runtime.controller.as_mut() {
                controller.reset();
            }
            publish_context(
                world,
                MetalFxAdaptiveContextSnapshot {
                    epoch: runtime.epoch,
                    render_scale: scale,
                    mode,
                    target_fps: target.fps,
                    ..Default::default()
                },
            );
            status.epoch = runtime.epoch;
            *world.resource_mut::<MetalFxAdaptiveStatus>() = status;
            return;
        }

        let config_changed = runtime.applied_config.as_ref() != Some(&config)
            || runtime.applied_target != Some(target);
        let configuration = if let Some(controller) = runtime.controller.as_mut() {
            controller.update_config(policy)
        } else {
            AdaptiveController::new(policy, runtime.ladder.clone(), runtime.starting_scale).map(
                |controller| {
                    runtime.controller = Some(controller);
                },
            )
        };
        if let Err(error) = configuration {
            if config_changed {
                runtime.epoch = runtime
                    .epoch
                    .checked_add(1)
                    .expect("adaptive epoch exhausted");
                runtime.applied_config = Some(config.clone());
                runtime.applied_target = Some(target);
            }
            if let Some(controller) = runtime.controller.as_mut() {
                controller.reset();
            }
            publish_context(
                world,
                MetalFxAdaptiveContextSnapshot {
                    epoch: runtime.epoch,
                    render_scale: scale,
                    mode,
                    target_fps: target.fps,
                    ..Default::default()
                },
            );
            status.reason = MetalFxAdaptiveReason::InvalidConfiguration(error);
            status.epoch = runtime.epoch;
            status.view_id = runtime.fingerprint.map(|f| f.view_id);
            *world.resource_mut::<MetalFxAdaptiveStatus>() = status;
            return;
        }
        let fingerprint = selected.as_ref().ok().copied();
        let reset_generation = context.reset_generation.load(Ordering::Relaxed);
        if config_changed
            || runtime.fingerprint != fingerprint
            || runtime.last_mode != Some(mode)
            || runtime.reset_generation != reset_generation
        {
            runtime.epoch = runtime
                .epoch
                .checked_add(1)
                .expect("adaptive epoch exhausted");
            runtime
                .controller
                .as_mut()
                .expect("validated controller")
                .reset();
            runtime.applied_config = Some(config.clone());
            runtime.applied_target = Some(target);
            runtime.fingerprint = fingerprint;
            runtime.last_mode = Some(mode);
            runtime.reset_generation = reset_generation;
        }
        let active_scale = runtime
            .controller
            .as_ref()
            .expect("validated controller")
            .current_scale();
        if let Some(mut scale) = world.get_resource_mut::<MetalFxRenderScale>() {
            if scale.0 != active_scale {
                scale.0 = active_scale;
            }
        }
        let mut snapshot = MetalFxAdaptiveContextSnapshot {
            epoch: runtime.epoch,
            view_id: fingerprint.map(|f| f.view_id),
            render_scale: active_scale,
            output_size: fingerprint.map_or([0, 0], |f| f.output_size),
            mode,
            target_fps: target.fps,
        };
        publish_context(world, snapshot);
        status.scale = active_scale;
        status.epoch = snapshot.epoch;
        status.view_id = snapshot.view_id;

        let mut sample = None;
        let mut effect_ready = false;
        let held = match selected {
            Err(reason) => Some(reason),
            Ok(view) => {
                let effect = effects.snapshot(view.view_id, current_frame);
                if !effect.is_fresh(config.max_effect_age_frames, config.max_effect_wall_age) {
                    Some(MetalFxAdaptiveReason::EffectStale)
                } else {
                    let observation = effect
                        .last_observation
                        .as_ref()
                        .expect("fresh observation exists");
                    if observation.state != MetalFxEffectState::OutputWritten {
                        Some(MetalFxAdaptiveReason::EffectNotReady(observation.state))
                    } else if observation.effective_mode != mode
                        || (observation.requested_scale - active_scale).abs() > 1e-4
                        || !observation.requested_scale.is_finite()
                        || observation.output_size != view.output_size
                        || observation.content_size
                            != view
                                .output_size
                                .map(|dimension| (dimension as f32 * active_scale).round() as u32)
                        || observation.content_size.contains(&0)
                    {
                        Some(MetalFxAdaptiveReason::EffectConfigurationMismatch)
                    } else {
                        effect_ready = true;
                        sample = input.latest(view.view_id);
                        match sample {
                            None => Some(MetalFxAdaptiveReason::TimingUnavailable),
                            Some(sample) if sample.epoch != snapshot.epoch => {
                                Some(MetalFxAdaptiveReason::SampleEpochMismatch)
                            }
                            Some(sample)
                                if sample.frame_id > current_frame
                                    || current_frame - sample.frame_id
                                        > config.max_sample_age_frames =>
                            {
                                Some(MetalFxAdaptiveReason::SampleFrameMismatch)
                            }
                            _ => None,
                        }
                    }
                }
            }
        };
        let elapsed = now
            .checked_duration_since(runtime.origin)
            .unwrap_or_default();
        let mut observation = AdaptiveObservation {
            now: elapsed,
            epoch: snapshot.epoch,
            frame_id: current_frame,
            sampled_at: elapsed,
            sampled_scale: active_scale,
            gpu_ms: None,
            validity: AdaptiveSampleValidity::Unavailable,
            effect_ready,
        };
        let mut reason = held;
        if held.is_none() {
            let sample = sample.expect("unheld sample exists");
            status.sample_frame = Some(sample.frame_id);
            status.source = Some(sample.source);
            status.validation = Some(sample.validation);
            if let Some(sampled_at) = sample.sampled_at.checked_duration_since(runtime.origin) {
                observation.frame_id = sample.frame_id;
                observation.sampled_at = sampled_at;
                observation.sampled_scale = sample.sampled_scale;
                observation.gpu_ms = Some(sample.gpu_ms);
                observation.validity = AdaptiveSampleValidity::ValidatedFrameCost;
            } else {
                reason = Some(MetalFxAdaptiveReason::Controller(
                    AdaptiveReason::InvalidSample,
                ));
            }
        }
        let decision = runtime
            .controller
            .as_mut()
            .expect("validated controller")
            .observe(observation);
        if let Some(mut scale) = world.get_resource_mut::<MetalFxRenderScale>() {
            if scale.0 != decision.scale {
                scale.0 = decision.scale;
            }
        }
        snapshot.render_scale = decision.scale;
        publish_context(world, snapshot);
        status.reason = reason.unwrap_or(MetalFxAdaptiveReason::Controller(decision.reason));
        status.decision = Some(decision);
        status.scale = decision.scale;
        *world.resource_mut::<MetalFxAdaptiveStatus>() = status;
    });
}

fn publish_context(world: &mut World, snapshot: MetalFxAdaptiveContextSnapshot) {
    let mut context = world.resource_mut::<MetalFxAdaptiveContext>();
    *context.snapshot.lock().unwrap_or_else(|e| e.into_inner()) = snapshot;
    // The payload uses interior mutability for main-world readers. Explicitly
    // mark its Bevy resource so ExtractResource copies this frame's snapshot.
    context.set_changed();
}

fn select_view(
    world: &mut World,
    primary: Option<u64>,
) -> Result<ViewFingerprint, MetalFxAdaptiveReason> {
    let mut cameras = world.query_filtered::<(Entity, &Camera, &RenderTarget), With<Camera3d>>();
    let active: Vec<_> = cameras
        .iter(world)
        .filter(|(_, camera, _)| camera.is_active)
        .map(|(entity, camera, target)| (entity, camera.viewport.clone(), target.clone()))
        .collect();
    if active.len() > 1 {
        return Err(MetalFxAdaptiveReason::MultipleViewsUnsupported);
    }
    let Some((entity, viewport, target)) = active.into_iter().next() else {
        return Err(MetalFxAdaptiveReason::NoRenderView);
    };
    if primary.is_some_and(|primary| primary != entity.to_bits()) {
        return Err(MetalFxAdaptiveReason::NoRenderView);
    }
    let window = match target {
        RenderTarget::Window(WindowRef::Entity(entity)) => entity,
        RenderTarget::Window(WindowRef::Primary) => {
            let mut windows = world.query_filtered::<Entity, With<PrimaryWindow>>();
            windows
                .single(world)
                .map_err(|_| MetalFxAdaptiveReason::UnsupportedTarget)?
        }
        _ => return Err(MetalFxAdaptiveReason::UnsupportedTarget),
    };
    let window_size = world
        .get::<Window>(window)
        .map(|w| [w.physical_width(), w.physical_height()])
        .filter(|size| !size.contains(&0))
        .ok_or(MetalFxAdaptiveReason::UnsupportedTarget)?;
    if let Some(viewport) = viewport {
        if viewport.physical_position != UVec2::ZERO
            || viewport.physical_size.to_array() != window_size
        {
            return Err(MetalFxAdaptiveReason::UnsupportedViewport);
        }
    }
    Ok(ViewFingerprint {
        view_id: entity.to_bits(),
        window_id: window.to_bits(),
        output_size: window_size,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::MetalFxEffectObservation;
    use bevy::window::PrimaryWindow;

    fn app() -> (App, u64, Instant) {
        let mut app = App::new();
        app.insert_resource(MetalFxRenderScale(1.0))
            .insert_resource(MetalFxModeResource(MetalFxMode::Temporal))
            .insert_resource(MetalFxObservationFrame(0))
            .init_resource::<MetalFxEffectStatus>();
        install(&mut app, true);
        configure_ladder(&mut app, vec![0.5, 0.67, 0.77, 1.0], 1.0);
        app.world_mut().spawn((Window::default(), PrimaryWindow));
        let view = app.world_mut().spawn(Camera3d::default()).id().to_bits();
        let origin = app.world().resource::<AdaptiveRuntime>().origin;
        update_at(app.world_mut(), origin);
        (app, view, origin)
    }

    fn ready(app: &mut App, view: u64, frame: u64) {
        app.world_mut().resource_mut::<MetalFxObservationFrame>().0 = frame;
        let scale = app.world().resource::<MetalFxRenderScale>().0;
        let size = app
            .world()
            .resource::<MetalFxAdaptiveContext>()
            .snapshot()
            .output_size;
        app.world()
            .resource::<MetalFxEffectStatus>()
            .publish(MetalFxEffectObservation::new(
                frame,
                view,
                MetalFxMode::Temporal,
                MetalFxMode::Temporal,
                scale,
                [
                    (size[0] as f32 * scale).round() as u32,
                    (size[1] as f32 * scale).round() as u32,
                ],
                size,
                MetalFxEffectState::OutputWritten,
                None,
            ));
    }

    fn gpu(app: &App, view: u64, frame: u64, at: Instant, gpu_ms: f64) -> ValidatedGpuFrameCost {
        let context = app.world().resource::<MetalFxAdaptiveContext>().snapshot();
        ValidatedGpuFrameCost {
            frame_id: frame,
            view_id: view,
            epoch: context.epoch,
            sampled_at: at,
            sampled_scale: context.render_scale,
            gpu_ms,
            source: "test-frame-instrument",
            validation: "synthetic-controller-fixture",
        }
    }

    fn feed(app: &mut App, view: u64, frame: u64, at: Instant, ms: f64) {
        ready(app, view, frame);
        let sample = gpu(app, view, frame, at, ms);
        assert!(app
            .world()
            .resource::<MetalFxFrameCostInput>()
            .publish_validated(sample));
        update_at(app.world_mut(), at);
    }

    fn monitor(app: &mut App, rate: Option<u32>) -> Entity {
        app.world_mut()
            .spawn(bevy::window::Monitor {
                name: Some("test monitor".into()),
                physical_width: 1920,
                physical_height: 1080,
                physical_position: IVec2::ZERO,
                refresh_rate_millihertz: rate,
                scale_factor: 1.0,
                // A supported mode is not the current reported refresh.
                video_modes: vec![bevy::window::VideoMode {
                    physical_size: UVec2::new(1920, 1080),
                    bit_depth: 24,
                    refresh_rate_millihertz: 240_000,
                }],
            })
            .id()
    }

    #[test]
    fn default_target_uses_the_primary_windows_current_monitor() {
        let (mut app, _, origin) = app();
        let other = monitor(&mut app, Some(60_000));
        app.world_mut()
            .entity_mut(other)
            .insert(bevy::window::PrimaryMonitor);
        let current = monitor(&mut app, Some(119_880));
        let window = app
            .world_mut()
            .query_filtered::<Entity, With<PrimaryWindow>>()
            .single(app.world())
            .unwrap();
        app.world_mut()
            .entity_mut(window)
            .insert(bevy::window::OnMonitor(current));
        let previous = app.world().resource::<MetalFxAdaptiveContext>().snapshot();
        update_at(app.world_mut(), origin + Duration::from_millis(17));
        let snapshot = app.world().resource::<MetalFxAdaptiveContext>().snapshot();
        assert_eq!(snapshot.target_fps, 119.88);
        assert!(snapshot.epoch > previous.epoch);
        assert_eq!(
            app.world()
                .resource::<MetalFxAdaptiveStatus>()
                .target_source,
            MetalFxAdaptiveTargetSource::MonitorReportedRefresh {
                window_id: window.to_bits(),
                monitor_id: current.to_bits(),
            }
        );
        assert_eq!(app.world().resource::<MetalFxRenderScale>().0, 1.0);
    }

    #[test]
    fn unavailable_monitor_metadata_reports_the_specific_60_fps_fallback() {
        let (mut app, _, origin) = app();
        assert_eq!(
            app.world()
                .resource::<MetalFxAdaptiveStatus>()
                .target_source,
            MetalFxAdaptiveTargetSource::Fallback60(
                MetalFxAdaptiveTargetFallback::CurrentMonitorUnavailable
            )
        );
        let current = monitor(&mut app, None);
        let window = app
            .world_mut()
            .query_filtered::<Entity, With<PrimaryWindow>>()
            .single(app.world())
            .unwrap();
        app.world_mut()
            .entity_mut(window)
            .insert(OnMonitor(current));
        for rate in [None, Some(0)] {
            app.world_mut()
                .get_mut::<Monitor>(current)
                .unwrap()
                .refresh_rate_millihertz = rate;
            update_at(app.world_mut(), origin + Duration::from_millis(17));
            let status = app.world().resource::<MetalFxAdaptiveStatus>();
            assert_eq!(status.target_fps, 60.0);
            assert_eq!(
                status.target_source,
                MetalFxAdaptiveTargetSource::Fallback60(
                    MetalFxAdaptiveTargetFallback::RefreshUnavailable
                )
            );
        }
        app.world_mut().despawn(window);
        update_at(app.world_mut(), origin + Duration::from_millis(34));
        let status = app.world().resource::<MetalFxAdaptiveStatus>();
        assert_eq!(status.target_fps, 60.0);
        assert_eq!(
            status.target_source,
            MetalFxAdaptiveTargetSource::Fallback60(
                MetalFxAdaptiveTargetFallback::PrimaryWindowUnavailable
            )
        );
    }

    #[test]
    fn explicit_60_fps_wins_over_monitor_and_only_explicit_changes_reset_evidence() {
        let (mut app, _, origin) = app();
        let current = monitor(&mut app, Some(120_000));
        let window = app
            .world_mut()
            .query_filtered::<Entity, With<PrimaryWindow>>()
            .single(app.world())
            .unwrap();
        app.world_mut()
            .entity_mut(window)
            .insert(OnMonitor(current));
        app.world_mut()
            .resource_mut::<MetalFxAdaptiveConfig>()
            .target = MetalFxAdaptiveTarget::Explicit(60.0);
        update_at(app.world_mut(), origin + Duration::from_millis(17));
        let before = app.world().resource::<MetalFxAdaptiveContext>().snapshot();
        assert_eq!(before.target_fps, 60.0);
        assert_eq!(
            app.world()
                .resource::<MetalFxAdaptiveStatus>()
                .target_source,
            MetalFxAdaptiveTargetSource::Explicit
        );
        app.world_mut()
            .get_mut::<Monitor>(current)
            .unwrap()
            .refresh_rate_millihertz = Some(144_000);
        update_at(app.world_mut(), origin + Duration::from_millis(34));
        assert_eq!(
            app.world()
                .resource::<MetalFxAdaptiveContext>()
                .snapshot()
                .epoch,
            before.epoch
        );
        app.world_mut()
            .resource_mut::<MetalFxAdaptiveConfig>()
            .target = MetalFxAdaptiveTarget::Explicit(90.0);
        update_at(app.world_mut(), origin + Duration::from_millis(51));
        let after = app.world().resource::<MetalFxAdaptiveContext>().snapshot();
        assert_eq!(after.target_fps, 90.0);
        assert!(after.epoch > before.epoch);
        assert_eq!(app.world().resource::<MetalFxRenderScale>().0, 1.0);
    }

    #[test]
    fn monitor_refresh_change_invalidates_a_pending_gpu_sample() {
        let (mut app, view, origin) = app();
        let current = monitor(&mut app, Some(60_000));
        let window = app
            .world_mut()
            .query_filtered::<Entity, With<PrimaryWindow>>()
            .single(app.world())
            .unwrap();
        app.world_mut()
            .entity_mut(window)
            .insert(bevy::window::OnMonitor(current));
        update_at(app.world_mut(), origin + Duration::from_millis(1));
        ready(&mut app, view, 2);
        let sample = gpu(&app, view, 2, origin + Duration::from_millis(17), 40.0);
        assert!(app
            .world()
            .resource::<MetalFxFrameCostInput>()
            .publish_validated(sample));
        app.world_mut()
            .get_mut::<bevy::window::Monitor>(current)
            .unwrap()
            .refresh_rate_millihertz = Some(120_000);
        update_at(app.world_mut(), origin + Duration::from_millis(34));
        let snapshot = app.world().resource::<MetalFxAdaptiveContext>().snapshot();
        assert_eq!(snapshot.target_fps, 120.0);
        assert!(snapshot.epoch > sample.epoch);
        assert_eq!(
            app.world().resource::<MetalFxAdaptiveStatus>().reason,
            MetalFxAdaptiveReason::SampleEpochMismatch
        );
        assert_eq!(app.world().resource::<MetalFxRenderScale>().0, 1.0);
    }

    #[test]
    fn default_timing_is_unavailable_and_app_delta_cannot_lower_quality() {
        let (mut app, view, origin) = app();
        let mut virtual_time = Time::<Virtual>::default();
        virtual_time.set_relative_speed(100.0);
        app.insert_resource(virtual_time);
        for frame in 1..600 {
            app.world_mut()
                .resource_mut::<Time<Virtual>>()
                .advance_by(Duration::from_secs(2));
            ready(&mut app, view, frame);
            update_at(app.world_mut(), origin + Duration::from_millis(frame * 17));
            assert_eq!(app.world().resource::<MetalFxRenderScale>().0, 1.0);
        }
        assert_eq!(
            app.world().resource::<MetalFxAdaptiveStatus>().reason,
            MetalFxAdaptiveReason::TimingUnavailable
        );
    }

    #[test]
    fn validated_gpu_decisions_ignore_virtual_time_pause_and_speed() {
        for speed in [0.0, 0.25, 100.0] {
            let (mut app, view, origin) = app();
            let mut virtual_time = Time::<Virtual>::default();
            if speed == 0.0 {
                virtual_time.pause();
            } else {
                virtual_time.set_relative_speed(speed);
            }
            app.insert_resource(virtual_time);
            for frame in 1..900 {
                let scale = app.world().resource::<MetalFxRenderScale>().0;
                feed(
                    &mut app,
                    view,
                    frame,
                    origin + Duration::from_secs_f64(frame as f64 / 60.0),
                    30.0 * f64::from(scale).powi(2),
                );
            }
            assert_eq!(app.world().resource::<MetalFxRenderScale>().0, 0.67);
        }
    }

    #[test]
    fn pending_and_stale_effects_hold_even_with_validated_high_gpu_cost() {
        let (mut app, view, origin) = app();
        for frame in 1..600 {
            app.world_mut().resource_mut::<MetalFxObservationFrame>().0 = frame;
            let mut observation = MetalFxEffectObservation::new(
                frame,
                view,
                MetalFxMode::Temporal,
                MetalFxMode::Temporal,
                1.0,
                [1280, 720],
                [1280, 720],
                MetalFxEffectState::Pending,
                None,
            );
            if frame > 300 {
                observation.frame_id = 1;
                observation.state = MetalFxEffectState::OutputWritten;
            }
            app.world()
                .resource::<MetalFxEffectStatus>()
                .publish(observation);
            let sample = gpu(
                &app,
                view,
                frame,
                origin + Duration::from_millis(frame * 17),
                40.0,
            );
            app.world()
                .resource::<MetalFxFrameCostInput>()
                .publish_validated(sample);
            update_at(app.world_mut(), sample.sampled_at);
            assert_eq!(app.world().resource::<MetalFxRenderScale>().0, 1.0);
        }
        assert_eq!(
            app.world().resource::<MetalFxAdaptiveStatus>().reason,
            MetalFxAdaptiveReason::EffectStale
        );
    }

    #[test]
    fn policy_floor_syncs_scale_even_without_a_new_gpu_sample() {
        let (mut app, view, origin) = app();
        for frame in 1..300 {
            let scale = app.world().resource::<MetalFxRenderScale>().0;
            feed(
                &mut app,
                view,
                frame,
                origin + Duration::from_millis(frame * 17),
                30.0 * f64::from(scale).powi(2),
            );
        }
        assert!(app.world().resource::<MetalFxRenderScale>().0 < 1.0);
        app.world_mut()
            .resource_mut::<MetalFxAdaptiveConfig>()
            .minimum_scale = 0.9;
        update_at(app.world_mut(), origin + Duration::from_secs(6));
        assert_eq!(app.world().resource::<MetalFxRenderScale>().0, 1.0);
    }

    #[test]
    fn native_content_under_a_half_resolution_request_cannot_drive_adaptation() {
        let (mut app, view, origin) = app();
        configure_ladder(&mut app, vec![0.5, 0.67, 0.77, 1.0], 0.5);
        update_at(app.world_mut(), origin + Duration::from_millis(1));
        ready(&mut app, view, 1);
        let context = app.world().resource::<MetalFxAdaptiveContext>().snapshot();
        assert_eq!(context.render_scale, 0.5);
        app.world()
            .resource::<MetalFxEffectStatus>()
            .publish(MetalFxEffectObservation::new(
                1,
                view,
                MetalFxMode::Temporal,
                MetalFxMode::Temporal,
                0.5,
                context.output_size,
                context.output_size,
                MetalFxEffectState::OutputWritten,
                None,
            ));
        let at = origin + Duration::from_millis(17);
        let sample = gpu(&app, view, 1, at, 30.0);
        assert!(app
            .world()
            .resource::<MetalFxFrameCostInput>()
            .publish_validated(sample));
        update_at(app.world_mut(), at);
        assert_eq!(
            app.world().resource::<MetalFxAdaptiveStatus>().reason,
            MetalFxAdaptiveReason::EffectConfigurationMismatch
        );
        assert_eq!(app.world().resource::<MetalFxRenderScale>().0, 0.5);
    }

    #[test]
    fn resize_and_explicit_reset_invalidate_old_sample_epoch() {
        let (mut app, view, origin) = app();
        ready(&mut app, view, 1);
        let old = gpu(&app, view, 1, origin, 40.0);
        let before = old.epoch;
        let window = app
            .world_mut()
            .query_filtered::<Entity, With<Window>>()
            .single(app.world())
            .unwrap();
        app.world_mut()
            .get_mut::<Window>(window)
            .unwrap()
            .resolution
            .set_physical_resolution(900, 600);
        update_at(app.world_mut(), origin + Duration::from_millis(1));
        let resized = app.world().resource::<MetalFxAdaptiveContext>().snapshot();
        assert!(resized.epoch > before);
        assert_eq!(resized.output_size, [900, 600]);
        ready(&mut app, view, 2);
        app.world()
            .resource::<MetalFxFrameCostInput>()
            .publish_validated(old);
        update_at(app.world_mut(), origin + Duration::from_millis(2));
        assert_eq!(
            app.world().resource::<MetalFxAdaptiveStatus>().reason,
            MetalFxAdaptiveReason::SampleEpochMismatch
        );
        app.world()
            .resource::<MetalFxAdaptiveContext>()
            .request_reset();
        update_at(app.world_mut(), origin + Duration::from_millis(3));
        assert!(
            app.world()
                .resource::<MetalFxAdaptiveContext>()
                .snapshot()
                .epoch
                > resized.epoch
        );
    }

    #[test]
    fn input_rejects_invalid_provenance_and_orders_each_view_independently() {
        let (app, view, origin) = app();
        let input = app.world().resource::<MetalFxFrameCostInput>();
        let mut sample = gpu(&app, view, 2, origin, 30.0);
        assert!(input.publish_validated(sample));
        assert!(!input.publish_validated(sample));
        sample.frame_id = 1;
        assert!(!input.publish_validated(sample));
        sample.view_id += 1;
        assert!(input.publish_validated(sample));
        sample.frame_id = 3;
        sample.validation = "";
        assert!(!input.publish_validated(sample));
        sample.validation = "fixture";
        sample.gpu_ms = f64::NAN;
        assert!(!input.publish_validated(sample));
        sample.gpu_ms = 0.0;
        assert!(!input.publish_validated(sample));
        assert_eq!(input.latest(view).unwrap().frame_id, 2);
    }

    #[test]
    fn render_frame_context_does_not_follow_the_next_main_frame() {
        use bevy::render::extract_resource::ExtractResource;
        let (mut app, _, origin) = app();
        let extracted = MetalFxAdaptiveContext::extract_resource(
            app.world().resource::<MetalFxAdaptiveContext>(),
        );
        let before = extracted.snapshot();
        app.world()
            .resource::<MetalFxAdaptiveContext>()
            .request_reset();
        update_at(app.world_mut(), origin + Duration::from_millis(17));
        assert!(
            app.world()
                .resource::<MetalFxAdaptiveContext>()
                .snapshot()
                .epoch
                > before.epoch
        );
        assert_eq!(extracted.snapshot(), before);
    }

    #[test]
    fn disabled_mode_invalidates_measurements_before_reenable() {
        let (mut app, view, origin) = app();
        let before = app.world().resource::<MetalFxAdaptiveContext>().snapshot();
        app.world_mut().resource_mut::<MetalFxModeResource>().0 = MetalFxMode::Disabled;
        update_at(app.world_mut(), origin + Duration::from_millis(17));
        let disabled = app.world().resource::<MetalFxAdaptiveContext>().snapshot();
        assert!(disabled.epoch > before.epoch);
        assert_eq!(disabled.mode, MetalFxMode::Disabled);
        assert_eq!(
            app.world().resource::<MetalFxAdaptiveStatus>().reason,
            MetalFxAdaptiveReason::Disabled
        );
        app.world_mut().resource_mut::<MetalFxModeResource>().0 = MetalFxMode::Temporal;
        update_at(app.world_mut(), origin + Duration::from_millis(34));
        ready(&mut app, view, 3);
        let mut sample = gpu(&app, view, 3, origin + Duration::from_millis(34), 40.0);
        sample.epoch = before.epoch;
        app.world()
            .resource::<MetalFxFrameCostInput>()
            .publish_validated(sample);
        update_at(app.world_mut(), origin + Duration::from_millis(35));
        assert_eq!(
            app.world().resource::<MetalFxAdaptiveStatus>().reason,
            MetalFxAdaptiveReason::SampleEpochMismatch
        );
        assert_eq!(app.world().resource::<MetalFxRenderScale>().0, 1.0);
    }

    #[test]
    fn invalid_policy_holds_scale_and_invalidates_old_measurements() {
        let (mut app, _, origin) = app();
        let before = app.world().resource::<MetalFxAdaptiveContext>().snapshot();
        app.world_mut()
            .resource_mut::<MetalFxAdaptiveConfig>()
            .target = MetalFxAdaptiveTarget::Explicit(0.0);
        update_at(app.world_mut(), origin + Duration::from_millis(17));
        assert_eq!(
            app.world().resource::<MetalFxAdaptiveStatus>().reason,
            MetalFxAdaptiveReason::InvalidConfiguration(AdaptiveConfigError::InvalidTarget)
        );
        assert_eq!(
            app.world().resource::<MetalFxAdaptiveStatus>().target_fps,
            0.0
        );
        assert_eq!(
            app.world()
                .resource::<MetalFxAdaptiveStatus>()
                .target_source,
            MetalFxAdaptiveTargetSource::Explicit
        );
        let invalid = app.world().resource::<MetalFxAdaptiveContext>().snapshot();
        assert!(invalid.epoch > before.epoch);
        assert_eq!(invalid.view_id, None);
        assert_eq!(app.world().resource::<MetalFxRenderScale>().0, 1.0);
        app.world_mut()
            .resource_mut::<MetalFxAdaptiveConfig>()
            .target = MetalFxAdaptiveTarget::Explicit(60.0);
        update_at(app.world_mut(), origin + Duration::from_millis(34));
        assert!(
            app.world()
                .resource::<MetalFxAdaptiveContext>()
                .snapshot()
                .epoch
                > invalid.epoch
        );
    }

    #[test]
    fn multiple_active_views_do_not_drive_one_global_scale() {
        let (mut app, view, origin) = app();
        app.world_mut().spawn(Camera3d::default());
        ready(&mut app, view, 1);
        update_at(app.world_mut(), origin);
        assert_eq!(
            app.world().resource::<MetalFxAdaptiveStatus>().reason,
            MetalFxAdaptiveReason::MultipleViewsUnsupported
        );
        assert_eq!(app.world().resource::<MetalFxRenderScale>().0, 1.0);
    }
}
