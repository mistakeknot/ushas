//! Bounded lifecycle exercises using real window/camera mutations and render observations.

use bevy::prelude::*;
use bevy::render::view::screenshot::{Screenshot, ScreenshotCaptured};
use bevy::window::PrimaryWindow;
use bevy_metalfx::{
    MetalFxAdaptiveContext, MetalFxEffectReason, MetalFxEffectState, MetalFxEffectStatus,
    MetalFxHistoryReset, MetalFxMode, MetalFxModeResource, MetalFxObservationFrame,
    MetalFxRenderScale,
};
use serde_json::{json, Value};
use std::time::{Duration, Instant};

const READY_FRAMES: usize = 20;
const DEADLINE: Duration = Duration::from_secs(25);
const MAX_OBSERVATIONS: usize = 4096;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LifecycleExercise {
    Resize,
    CameraCut,
    LateCamera,
    MultipleViews,
    InactiveCutResume,
}

impl LifecycleExercise {
    pub fn parse(value: &str) -> Result<Self, String> {
        match value {
            "resize" => Ok(Self::Resize),
            "camera-cut" => Ok(Self::CameraCut),
            "late-camera" => Ok(Self::LateCamera),
            "multiple-views" => Ok(Self::MultipleViews),
            "inactive-cut-resume" => Ok(Self::InactiveCutResume),
            _ => Err("lifecycle must be resize, camera-cut, late-camera, multiple-views, or inactive-cut-resume".into()),
        }
    }
}

pub struct LifecyclePlugin(LifecycleExercise);

impl LifecyclePlugin {
    pub fn new(exercise: LifecycleExercise) -> Self {
        Self(exercise)
    }
}

impl Plugin for LifecyclePlugin {
    fn build(&self, app: &mut App) {
        app.insert_resource(LifecycleRun::new(self.0))
            .add_systems(Update, exercise);
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Phase {
    Initial,
    Changed,
    Restored,
}

impl Phase {
    fn name(self) -> &'static str {
        match self {
            Self::Initial => "initial",
            Self::Changed => "changed",
            Self::Restored => "restored",
        }
    }
}

#[derive(Resource)]
pub struct LifecycleRun {
    exercise: LifecycleExercise,
    started: Instant,
    phase_started: Instant,
    phase: Phase,
    phase_frame: u64,
    outcome: Option<bool>,
    error: Option<String>,
    initial_size: Option<[u32; 2]>,
    initial_scale: Option<f32>,
    epoch_before_change: Option<u64>,
    primary: Option<Entity>,
    secondary: Option<Entity>,
    replaced: Option<Entity>,
    stable: StableFrames,
    capture_pending: bool,
    phase_capture: Option<bool>,
    reset_was_pending: bool,
    events: Vec<Value>,
    observations: Vec<Value>,
    captures: Vec<Value>,
    dropped_observations: usize,
    last_logged: Option<(Phase, Vec<ViewEvidence>, bool)>,
}

impl LifecycleRun {
    fn new(exercise: LifecycleExercise) -> Self {
        let now = Instant::now();
        Self {
            exercise,
            started: now,
            phase_started: now,
            phase: Phase::Initial,
            phase_frame: 0,
            outcome: None,
            error: None,
            initial_size: None,
            initial_scale: None,
            epoch_before_change: None,
            primary: None,
            secondary: None,
            replaced: None,
            stable: StableFrames::default(),
            capture_pending: false,
            phase_capture: None,
            reset_was_pending: false,
            events: vec![],
            observations: vec![],
            captures: vec![],
            dropped_observations: 0,
            last_logged: None,
        }
    }

    pub fn exercise(&self) -> LifecycleExercise {
        self.exercise
    }
    pub fn finished(&self) -> bool {
        self.outcome.is_some()
    }
    pub fn passed(&self) -> bool {
        self.outcome == Some(true)
    }
    pub fn report(&self) -> Value {
        json!({"exercise":format!("{:?}",self.exercise),"valid":self.outcome,"error":self.error,
            "scope":"real lifecycle mutations, render-path observations and captured pixels; not GPU completion or panel delivery",
            "phase":self.phase.name(),"wall_elapsed_s":self.started.elapsed().as_secs_f64(),
            "initial_size":self.initial_size,"initial_scale":self.initial_scale,
            "events":self.events,"observations":self.observations,"captures":self.captures,
            "dropped_observations":self.dropped_observations})
    }

    fn event(&mut self, now: Instant, frame: u64, name: &str, detail: Value) {
        self.events.push(json!({"event":name,"frame":frame,"elapsed_s":now.duration_since(self.started).as_secs_f64(),"detail":detail}));
    }

    fn transition(&mut self, phase: Phase, now: Instant, frame: u64) {
        self.phase = phase;
        self.phase_started = now;
        self.phase_frame = frame;
        self.stable = StableFrames::default();
        self.capture_pending = false;
        self.phase_capture = None;
    }

    fn finish(&mut self, now: Instant, frame: u64, error: Option<String>) {
        self.outcome = Some(error.is_none());
        self.error = error;
        self.event(
            now,
            frame,
            "finished",
            json!({"passed":self.passed(),"error":self.error}),
        );
    }

    fn record(
        &mut self,
        now: Instant,
        frame: u64,
        views: &[ViewEvidence],
        reset_pending: bool,
        epoch: u64,
        scale: f32,
    ) {
        let key = (self.phase, views.to_vec(), reset_pending);
        if self.last_logged.as_ref() == Some(&key) {
            return;
        }
        self.last_logged = Some(key);
        if self.observations.len() >= MAX_OBSERVATIONS {
            self.dropped_observations += 1;
            return;
        }
        self.observations.push(json!({"app_frame":frame,"elapsed_s":now.duration_since(self.started).as_secs_f64(),
            "phase":self.phase.name(),"reset_pending":reset_pending,"adaptive_epoch":epoch,"scale":scale,
            "views":views.iter().map(|v|json!({"entity":v.entity,"active":v.active,"fresh":v.fresh,
                "frame":v.frame,"state":format!("{:?}",v.state),"reason":v.reason.map(|r|format!("{r:?}")),
                "content_size":v.content,"output_size":v.output,"effective_mode":format!("{:?}",v.mode),"requested_scale":v.scale})).collect::<Vec<_>>()}));
    }
}

#[derive(Clone, PartialEq)]
struct ViewEvidence {
    entity: u64,
    active: bool,
    fresh: bool,
    frame: Option<u64>,
    state: MetalFxEffectState,
    reason: Option<MetalFxEffectReason>,
    content: [u32; 2],
    output: [u32; 2],
    mode: MetalFxMode,
    scale: f32,
}

fn ready_view(
    view: &ViewEvidence,
    mode: MetalFxMode,
    output: [u32; 2],
    scale: f32,
    since: u64,
) -> bool {
    view.active
        && view.fresh
        && view.frame.is_some_and(|f| f >= since)
        && view.state == MetalFxEffectState::OutputWritten
        && view.mode == mode
        && view.output == output
        && view.content == output.map(|d| (d as f32 * scale).round() as u32)
        && (view.scale - scale).abs() <= 1e-4
}

fn rejected_views(views: &[ViewEvidence], since: u64) -> bool {
    let active: Vec<_> = views.iter().filter(|v| v.active).collect();
    active.len() == 2
        && active.iter().all(|v| {
            v.fresh
                && v.frame.is_some_and(|f| f >= since)
                && v.state == MetalFxEffectState::Unavailable
                && v.reason == Some(MetalFxEffectReason::MultipleViewsUnsupported)
        })
}

#[derive(Default)]
struct StableFrames {
    count: usize,
    last_signature: Option<Vec<(u64, u64)>>,
}

impl StableFrames {
    fn observe(&mut self, ready: bool, views: &[ViewEvidence]) {
        if !ready {
            *self = Self::default();
            return;
        }
        let mut signature: Vec<_> = views
            .iter()
            .filter_map(|v| v.frame.map(|frame| (v.entity, frame)))
            .collect();
        signature.sort_unstable();
        if !signature.is_empty() && self.last_signature.as_ref() != Some(&signature) {
            self.count += 1;
            self.last_signature = Some(signature);
        }
    }
}

#[derive(Component)]
struct LifecycleCapture {
    phase: Phase,
    expected_size: [u32; 2],
}

fn capture_phase(
    event: On<ScreenshotCaptured>,
    config: Res<crate::RunConfig>,
    purpose: Query<&LifecycleCapture>,
    mut run: ResMut<LifecycleRun>,
) {
    let Ok(purpose) = purpose.get(event.entity) else {
        return;
    };
    let path = format!("{}.lifecycle-{}.png", config.0.output, purpose.phase.name());
    let result = (|| -> Result<Value, String> {
        if let Some(parent) = std::path::Path::new(&path)
            .parent()
            .filter(|p| !p.as_os_str().is_empty())
        {
            std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
        }
        let dynamic = event
            .image
            .clone()
            .try_into_dynamic()
            .map_err(|e| e.to_string())?;
        let rgba = dynamic.to_rgba8();
        let mut proof = crate::metrics::image_proof(rgba.as_raw(), rgba.width(), rgba.height());
        dynamic.save(&path).map_err(|e| e.to_string())?;
        let valid =
            proof["nonuniform"] == true && [rgba.width(), rgba.height()] == purpose.expected_size;
        proof["valid"] = json!(valid);
        proof["path"] = json!(path);
        proof["width"] = json!(rgba.width());
        proof["height"] = json!(rgba.height());
        proof["phase"] = json!(purpose.phase.name());
        Ok(proof)
    })();
    let proof = result.unwrap_or_else(|error| json!({"valid":false,"error":error,"path":path}));
    if purpose.phase == run.phase {
        run.phase_capture = Some(proof["valid"] == true);
    }
    run.captures.push(proof);
}

fn require_capture(run: &mut LifecycleRun, commands: &mut Commands, size: [u32; 2]) -> bool {
    if run.phase_capture == Some(true) {
        return true;
    }
    if !run.capture_pending {
        run.capture_pending = true;
        commands
            .spawn((
                Screenshot::primary_window(),
                LifecycleCapture {
                    phase: run.phase,
                    expected_size: size,
                },
            ))
            .observe(capture_phase);
    }
    false
}

#[allow(clippy::too_many_arguments)]
fn exercise(
    mut commands: Commands,
    mut run: ResMut<LifecycleRun>,
    config: Res<crate::RunConfig>,
    frame: Res<MetalFxObservationFrame>,
    status: Res<MetalFxEffectStatus>,
    scale: Res<MetalFxRenderScale>,
    mode: Res<MetalFxModeResource>,
    context: Res<MetalFxAdaptiveContext>,
    mut history: Option<ResMut<MetalFxHistoryReset>>,
    mut windows: Query<&mut Window, With<PrimaryWindow>>,
    mut cameras: Query<(Entity, &mut Camera, &mut Transform), With<Camera3d>>,
) {
    if run.finished() {
        return;
    }
    let now = Instant::now();
    let Ok(mut window) = windows.single_mut() else {
        run.finish(
            now,
            frame.0,
            Some("lifecycle requires one primary window".into()),
        );
        return;
    };
    let size = [window.physical_width(), window.physical_height()];
    run.initial_size.get_or_insert(size);
    let snapshot = context.snapshot();
    let reset_pending = history.as_ref().is_some_and(|r| r.is_requested());
    let mut views: Vec<_> = cameras
        .iter()
        .map(|(entity, camera, _)| {
            let effect = status.snapshot(entity.to_bits(), frame.0);
            let observed = effect.last_observation.as_ref();
            ViewEvidence {
                entity: entity.to_bits(),
                active: camera.is_active,
                fresh: effect.is_fresh(2, Duration::from_millis(500)),
                frame: observed.map(|o| o.frame_id),
                state: observed.map_or(MetalFxEffectState::NoRender, |o| o.state),
                reason: observed.and_then(|o| o.reason),
                content: observed.map_or([0, 0], |o| o.content_size),
                output: observed.map_or([0, 0], |o| o.output_size),
                mode: observed.map_or(MetalFxMode::Disabled, |o| o.effective_mode),
                scale: observed.map_or(0.0, |o| o.requested_scale),
            }
        })
        .collect();
    views.sort_by_key(|v| v.entity);
    run.record(now, frame.0, &views, reset_pending, snapshot.epoch, scale.0);
    if run.reset_was_pending && !reset_pending {
        run.event(
            now,
            frame.0,
            "reset_acknowledged",
            json!({"scope":"temporal reset commands encoded; GPU completion not asserted"}),
        );
        run.reset_was_pending = false;
    }
    let needs_reset = matches!(
        run.exercise,
        LifecycleExercise::CameraCut | LifecycleExercise::InactiveCutResume
    );
    let failure = if now.duration_since(run.started) > DEADLINE {
        Some(format!(
            "lifecycle deadline exceeded in {}",
            run.phase.name()
        ))
    } else if mode.get() == MetalFxMode::Disabled {
        Some("lifecycle exercises require an active MetalFX mode".into())
    } else if needs_reset
        && (!matches!(
            mode.get(),
            MetalFxMode::Temporal | MetalFxMode::FrameInterpolation
        ) || history.is_none())
    {
        Some("camera-cut exercises require temporal history support".into())
    } else if run.phase_capture == Some(false) {
        Some(format!(
            "{} capture did not prove matching rendered content",
            run.phase.name()
        ))
    } else if run
        .initial_scale
        .is_some_and(|initial| (initial - scale.0).abs() > 1e-4)
    {
        Some("render scale changed without a validated GPU measurement".into())
    } else {
        None
    };
    if let Some(error) = failure {
        restore(&mut run, &mut commands, &mut window, &mut cameras);
        run.finish(now, frame.0, Some(error));
        return;
    }
    let active: Vec<_> = views.iter().filter(|v| v.active).collect();
    if run.exercise == LifecycleExercise::LateCamera && run.phase == Phase::Initial {
        if !views.is_empty() {
            run.finish(
                now,
                frame.0,
                Some("late-camera exercise already had a camera at startup".into()),
            );
            return;
        }
        if now.duration_since(run.phase_started) >= Duration::from_millis(250) {
            run.initial_scale = Some(scale.0);
            run.epoch_before_change = Some(snapshot.epoch);
            let camera = crate::scene::spawn_camera(&mut commands);
            run.primary = Some(camera);
            run.event(
                now,
                frame.0,
                "camera_spawned_after_startup",
                json!({"view":camera.to_bits()}),
            );
            run.transition(Phase::Changed, now, frame.0);
        }
        return;
    }
    if run.primary.is_none() && active.len() == 1 {
        run.primary = Some(Entity::from_bits(active[0].entity));
    }
    let epoch_ready = !config.0.adaptive
        || run
            .epoch_before_change
            .is_none_or(|old| snapshot.epoch > old);
    if run.exercise == LifecycleExercise::InactiveCutResume && run.phase == Phase::Changed {
        if !reset_pending {
            restore(&mut run, &mut commands, &mut window, &mut cameras);
            run.finish(
                now,
                frame.0,
                Some("history reset was consumed while the camera was inactive".into()),
            );
            return;
        }
        let inactive_observed = views.len() == 1
            && !views[0].active
            && views[0].fresh
            && views[0].frame.is_some_and(|f| f >= run.phase_frame)
            && views[0].state == MetalFxEffectState::NoRender;
        if inactive_observed
            && frame.0.saturating_sub(run.phase_frame) >= 6
            && now.duration_since(run.phase_started) >= Duration::from_millis(300)
        {
            if let Some(primary) = run.primary {
                if let Ok((_, mut camera, _)) = cameras.get_mut(primary) {
                    camera.is_active = true;
                }
            }
            run.epoch_before_change = Some(snapshot.epoch);
            run.event(
                now,
                frame.0,
                "camera_resumed_with_reset_pending",
                json!({"reset_pending":true}),
            );
            run.transition(Phase::Restored, now, frame.0);
        }
        return;
    }
    let ready = if run.exercise == LifecycleExercise::MultipleViews && run.phase == Phase::Changed {
        rejected_views(&views, run.phase_frame)
    } else {
        active.len() == 1
            && run.primary.is_some_and(|e| e.to_bits() == active[0].entity)
            && ready_view(active[0], mode.get(), size, scale.0, run.phase_frame)
            && (run.phase == Phase::Initial || epoch_ready)
            && !(needs_reset && run.phase != Phase::Initial && reset_pending)
    };
    run.stable.observe(ready, &views);
    if run.stable.count < READY_FRAMES {
        return;
    }
    let unsupported_phase =
        run.exercise == LifecycleExercise::MultipleViews && run.phase == Phase::Changed;
    if !unsupported_phase && !require_capture(&mut run, &mut commands, size) {
        return;
    }
    if run.phase == Phase::Initial {
        run.initial_scale = Some(scale.0);
        run.epoch_before_change = Some(snapshot.epoch);
        match run.exercise {
            LifecycleExercise::Resize => {
                let changed = [(size[0] * 3 / 4).max(64), (size[1] * 3 / 4).max(64)];
                window
                    .resolution
                    .set_physical_resolution(changed[0], changed[1]);
                run.event(
                    now,
                    frame.0,
                    "window_resized",
                    json!({"from":size,"to":changed}),
                );
            }
            LifecycleExercise::CameraCut | LifecycleExercise::InactiveCutResume => {
                if let Some(primary) = run.primary {
                    if let Ok((_, mut camera, mut transform)) = cameras.get_mut(primary) {
                        transform.rotate_y(0.25);
                        if run.exercise == LifecycleExercise::InactiveCutResume {
                            camera.is_active = false;
                        }
                    }
                }
                if let Some(history) = history.as_mut() {
                    history.request();
                }
                context.request_reset();
                run.reset_was_pending = true;
                let inactive = run.exercise == LifecycleExercise::InactiveCutResume;
                run.event(
                    now,
                    frame.0,
                    "partial_camera_cut_requested",
                    json!({"yaw_radians":0.25,
                    "inactive":inactive,"reset_pending":true}),
                );
            }
            LifecycleExercise::MultipleViews => {
                let extra = crate::scene::spawn_camera(&mut commands);
                commands.entity(extra).insert(Camera {
                    order: 1,
                    ..default()
                });
                run.secondary = Some(extra);
                run.event(
                    now,
                    frame.0,
                    "second_active_camera_spawned",
                    json!({"view":extra.to_bits()}),
                );
            }
            LifecycleExercise::LateCamera => unreachable!("late camera starts without a view"),
        }
        run.transition(Phase::Changed, now, frame.0);
    } else if run.phase == Phase::Changed && run.exercise != LifecycleExercise::CameraCut {
        run.epoch_before_change = Some(snapshot.epoch);
        match run.exercise {
            LifecycleExercise::Resize => {
                let original = run.initial_size.unwrap();
                window
                    .resolution
                    .set_physical_resolution(original[0], original[1]);
                run.event(
                    now,
                    frame.0,
                    "window_size_restored",
                    json!({"from":size,"to":original}),
                );
            }
            LifecycleExercise::LateCamera => {
                if let Some(old) = run.primary {
                    commands.entity(old).despawn();
                    run.replaced = Some(old);
                }
                let replacement = crate::scene::spawn_camera(&mut commands);
                run.primary = Some(replacement);
                let old_view = run.replaced.map(Entity::to_bits);
                run.event(
                    now,
                    frame.0,
                    "camera_replaced",
                    json!({"new_view":replacement.to_bits(),"old_view":old_view}),
                );
            }
            LifecycleExercise::MultipleViews => {
                if let Some(extra) = run.secondary.take() {
                    commands.entity(extra).despawn();
                }
                run.event(
                    now,
                    frame.0,
                    "second_camera_removed",
                    json!({"all_active_views_reported":"MultipleViewsUnsupported"}),
                );
            }
            _ => unreachable!("reset exercises finish after acknowledged recovery"),
        }
        run.transition(Phase::Restored, now, frame.0);
    } else {
        let old_view_stale = run.replaced.is_none_or(|old| {
            !status
                .snapshot(old.to_bits(), frame.0)
                .is_fresh(2, Duration::from_millis(500))
        });
        if !old_view_stale {
            return;
        }
        run.finish(now, frame.0, None);
    }
}

fn restore(
    run: &mut LifecycleRun,
    commands: &mut Commands,
    window: &mut Window,
    cameras: &mut Query<(Entity, &mut Camera, &mut Transform), With<Camera3d>>,
) {
    if let Some(size) = run.initial_size {
        window.resolution.set_physical_resolution(size[0], size[1]);
    }
    if let Some(extra) = run.secondary.take() {
        commands.entity(extra).despawn();
    }
    if let Some(primary) = run.primary {
        if let Ok((_, mut camera, _)) = cameras.get_mut(primary) {
            camera.is_active = true;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn view() -> ViewEvidence {
        ViewEvidence {
            entity: 7,
            active: true,
            fresh: true,
            frame: Some(50),
            state: MetalFxEffectState::OutputWritten,
            reason: None,
            content: [640, 360],
            output: [1280, 720],
            mode: MetalFxMode::Temporal,
            scale: 0.5,
        }
    }

    #[test]
    fn readiness_requires_current_geometry_and_configuration() {
        let original = view();
        assert!(ready_view(
            &original,
            MetalFxMode::Temporal,
            [1280, 720],
            0.5,
            50
        ));
        for bad in [
            ViewEvidence {
                fresh: false,
                ..original.clone()
            },
            ViewEvidence {
                content: [1280, 720],
                ..original.clone()
            },
            ViewEvidence {
                output: [960, 540],
                ..original.clone()
            },
            ViewEvidence {
                frame: Some(49),
                ..original.clone()
            },
            ViewEvidence {
                mode: MetalFxMode::Spatial,
                ..original.clone()
            },
            ViewEvidence {
                scale: 0.67,
                ..original.clone()
            },
            ViewEvidence {
                state: MetalFxEffectState::Pending,
                ..original.clone()
            },
            ViewEvidence {
                active: false,
                ..original.clone()
            },
        ] {
            assert!(!ready_view(
                &bad,
                MetalFxMode::Temporal,
                [1280, 720],
                0.5,
                50
            ));
        }
    }

    #[test]
    fn multiple_views_must_report_the_explicit_failure_for_both_views() {
        let rejected = ViewEvidence {
            state: MetalFxEffectState::Unavailable,
            reason: Some(MetalFxEffectReason::MultipleViewsUnsupported),
            ..view()
        };
        assert!(rejected_views(
            &[
                rejected.clone(),
                ViewEvidence {
                    entity: 8,
                    ..rejected.clone()
                }
            ],
            50
        ));
        assert!(!rejected_views(&[rejected.clone()], 50));
        assert!(!rejected_views(&[rejected.clone(), view()], 50));
        assert!(!rejected_views(
            &[
                rejected.clone(),
                ViewEvidence {
                    entity: 8,
                    fresh: false,
                    ..rejected
                }
            ],
            50
        ));
    }

    #[test]
    fn repeated_old_observations_cannot_satisfy_a_settling_window() {
        let mut stable = StableFrames::default();
        for _ in 0..20 {
            stable.observe(true, &[view()]);
        }
        assert_eq!(stable.count, 1);
        stable.observe(
            true,
            &[ViewEvidence {
                frame: Some(51),
                ..view()
            }],
        );
        assert_eq!(stable.count, 2);
        stable.observe(false, &[view()]);
        assert_eq!(stable.count, 0);
    }
}
