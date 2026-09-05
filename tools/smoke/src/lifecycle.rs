//! Bounded lifecycle exercises using actual targets, mutations and render observations.

use bevy::camera::RenderTarget;
use bevy::prelude::*;
use bevy::render::render_resource::{TextureDimension, TextureUsages};
use bevy::render::view::screenshot::ScreenshotCaptured;
use bevy::window::PrimaryWindow;
use bevy_metalfx::{
    diagnostic_fault::ScalerFaultSnapshot, MetalFxAdaptiveContext, MetalFxDiagnosticFault,
    MetalFxEffectReason, MetalFxEffectState, MetalFxEffectStatus, MetalFxHistoryReset, MetalFxMode,
    MetalFxModeResource, MetalFxObservationFrame, MetalFxRenderScale, ScalerCreationFault,
};
use serde_json::{json, Value};
use std::collections::VecDeque;
use std::time::{Duration, Instant};

const READY_FRAMES: usize = 20;
const DEADLINE: Duration = Duration::from_secs(25);
const OBSERVATIONS_PER_PHASE: usize = 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LifecycleExercise {
    Resize,
    CameraCut,
    LateCamera,
    MultipleViews,
    InactiveCutResume,
    CreationFailure,
    CreationSlow,
    WindowMinimize,
    OsSleepResume,
}

impl LifecycleExercise {
    pub fn parse(value: &str) -> Result<Self, String> {
        match value {
            "resize" => Ok(Self::Resize),
            "camera-cut" => Ok(Self::CameraCut),
            "late-camera" => Ok(Self::LateCamera),
            "multiple-views" => Ok(Self::MultipleViews),
            "inactive-cut-resume" => Ok(Self::InactiveCutResume),
            "creation-failure" => Ok(Self::CreationFailure),
            "creation-slow" => Ok(Self::CreationSlow),
            "window-minimize" => Ok(Self::WindowMinimize),
            "os-sleep-resume" => Ok(Self::OsSleepResume),
            _ => Err("lifecycle must be resize, camera-cut, late-camera, multiple-views, inactive-cut-resume, creation-failure, creation-slow, window-minimize, or os-sleep-resume".into()),
        }
    }

    fn creation_fault(self) -> Option<ScalerCreationFault> {
        match self {
            Self::CreationFailure => Some(ScalerCreationFault::ReturnNone),
            Self::CreationSlow => Some(ScalerCreationFault::HoldPending),
            _ => None,
        }
    }

    fn native_lifecycle(self) -> bool {
        matches!(self, Self::WindowMinimize | Self::OsSleepResume)
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
        if self.0.native_lifecycle() {
            app.add_plugins(crate::window_lifecycle::WindowLifecyclePlugin);
        }
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
    const ALL: [Self; 3] = [Self::Initial, Self::Changed, Self::Restored];

    fn index(self) -> usize {
        match self {
            Self::Initial => 0,
            Self::Changed => 1,
            Self::Restored => 2,
        }
    }

    fn name(self) -> &'static str {
        match self {
            Self::Initial => "initial",
            Self::Changed => "changed",
            Self::Restored => "restored",
        }
    }
}

#[derive(Default)]
struct ObservationLedger {
    phases: [VecDeque<Value>; 3],
    totals: [usize; 3],
    evicted: [usize; 3],
}

impl ObservationLedger {
    fn record(&mut self, phase: Phase, observation: Value) {
        let index = phase.index();
        let records = &mut self.phases[index];
        self.totals[index] += 1;
        if records.len() == OBSERVATIONS_PER_PHASE {
            records.pop_front();
            self.evicted[index] += 1;
        }
        records.push_back(observation);
    }

    fn observations(&self) -> Vec<Value> {
        self.phases
            .iter()
            .flat_map(|records| records.iter().cloned())
            .collect()
    }

    fn evicted(&self) -> usize {
        self.evicted.iter().sum()
    }

    fn report(&self) -> Value {
        let phases: serde_json::Map<String, Value> = Phase::ALL
            .into_iter()
            .map(|phase| {
                let index = phase.index();
                let records = &self.phases[index];
                (
                    phase.name().into(),
                    json!({"total":self.totals[index],"retained":records.len(),
                "evicted":self.evicted[index],
                "first_retained_app_frame":records.front().map(|record|&record["app_frame"]),
                "last_retained_app_frame":records.back().map(|record|&record["app_frame"])}),
                )
            })
            .collect();
        json!({"kind":"per_phase_recent_ring","max_records_per_phase":OBSERVATIONS_PER_PHASE,
            "evicted":self.evicted(),"phases":phases,
            "scope":"bounded recent observations within each phase; evicted records are not a complete transition history; transition events and captures are retained separately"})
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
    initial_target: Option<LifecycleTarget>,
    expected_size: Option<[u32; 2]>,
    initial_scale: Option<f32>,
    epoch_before_change: Option<u64>,
    primary: Option<Entity>,
    secondary: Option<Entity>,
    replaced: Option<Entity>,
    stable: StableFrames,
    capture_pending: bool,
    phase_capture: Option<bool>,
    reset_was_pending: bool,
    fault_generation: Option<u64>,
    creation_reason_seen: bool,
    native_cursor: Option<u64>,
    native_window: Option<Entity>,
    native_restore_requested: bool,
    native_hidden_since: Option<Instant>,
    native_cycle: Option<(u64, u64)>,
    native_evidence: Option<Value>,
    events: Vec<Value>,
    observations: ObservationLedger,
    captures: Vec<Value>,
    last_logged: Option<(Phase, Vec<ViewEvidence>, bool, ScalerFaultSnapshot)>,
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
            initial_target: None,
            expected_size: None,
            initial_scale: None,
            epoch_before_change: None,
            primary: None,
            secondary: None,
            replaced: None,
            stable: StableFrames::default(),
            capture_pending: false,
            phase_capture: None,
            reset_was_pending: false,
            fault_generation: None,
            creation_reason_seen: false,
            native_cursor: None,
            native_window: None,
            native_restore_requested: false,
            native_hidden_since: None,
            native_cycle: None,
            native_evidence: None,
            events: vec![],
            observations: ObservationLedger::default(),
            captures: vec![],
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
    fn ready_view(
        &self,
        view: &ViewEvidence,
        mode: MetalFxMode,
        window: [u32; 2],
        scale: f32,
    ) -> bool {
        self.expected_size.is_some_and(|expected| {
            window == expected && ready_view(view, mode, expected, scale, self.phase_frame)
        })
    }
    pub fn report(&self) -> Value {
        json!({"exercise":format!("{:?}",self.exercise),"valid":self.outcome,"error":self.error,
            "scope":"real lifecycle mutations, render-path observations and captured pixels; not GPU completion or panel delivery",
            "phase":self.phase.name(),"wall_elapsed_s":self.started.elapsed().as_secs_f64(),
            "wall_elapsed_clock":"wall_elapsed_s and event elapsed_s use Instant, which may exclude OS sleep on macOS; native_lifecycle.wall_elapsed_seconds uses sleep-inclusive SystemTime",
            "initial_size":self.initial_size,"expected_size":self.expected_size,"initial_scale":self.initial_scale,
            "target":self.initial_target.map(LifecycleTarget::report),
            "fault_generation":self.fault_generation,"creation_reason_seen":self.creation_reason_seen,
            "native_lifecycle":self.native_evidence,
            "creation_fault_scope":self.exercise.creation_fault().map(|_|"simulated creation completion only; not a reproduced driver failure or OS sleep; changed-phase pixels exercise the real bilinear fallback"),
            "events":self.events,"observations":self.observations.observations(),"captures":self.captures,
            "observation_retention":self.observations.report(),
            "dropped_observations":self.observations.evicted()})
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

    // Keep the observed frame, geometry, reset and diagnostic generation together.
    #[allow(clippy::too_many_arguments)]
    fn record(
        &mut self,
        now: Instant,
        frame: u64,
        views: &[ViewEvidence],
        reset_pending: bool,
        epoch: u64,
        scale: f32,
        fault: ScalerFaultSnapshot,
    ) {
        let key = (self.phase, views.to_vec(), reset_pending, fault);
        if self.last_logged.as_ref() == Some(&key) {
            return;
        }
        self.last_logged = Some(key);
        self.observations.record(self.phase, json!({"app_frame":frame,"elapsed_s":now.duration_since(self.started).as_secs_f64(),
            "phase":self.phase.name(),"reset_pending":reset_pending,"adaptive_epoch":epoch,"scale":scale,
            "diagnostic_fault":{"generation":fault.generation,"mode":format!("{:?}",fault.fault),"scope":"main-world control snapshot; effect frame may lag extraction"},
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

fn inactive_view_settled(views: &[ViewEvidence], since: u64, current_frame: u64) -> bool {
    current_frame.saturating_sub(since) >= 6
        && views.len() == 1
        && !views[0].active
        && ((views[0].fresh && views[0].state == MetalFxEffectState::NoRender)
            || (!views[0].fresh && views[0].frame.is_none_or(|f| f < since)))
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TargetIdentity {
    Window(Entity),
    Image(AssetId<Image>),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct LifecycleTarget {
    identity: TargetIdentity,
    size: [u32; 2],
}

impl LifecycleTarget {
    fn resolve(
        exercise: LifecycleExercise,
        capture: &crate::offscreen::CaptureTarget,
        images: &Assets<Image>,
        window: Option<(Entity, [u32; 2])>,
    ) -> Result<Self, String> {
        match capture {
            crate::offscreen::CaptureTarget::Window => {
                let (entity, size) = window.ok_or("lifecycle requires one primary window")?;
                Ok(Self {
                    identity: TargetIdentity::Window(entity),
                    size,
                })
            }
            crate::offscreen::CaptureTarget::Image(handle) => {
                if exercise.creation_fault().is_none() {
                    return Err(
                        "only creation-failure and creation-slow support image lifecycle targets"
                            .into(),
                    );
                }
                let image = images
                    .get(handle)
                    .ok_or("lifecycle image target is missing")?;
                let descriptor = &image.texture_descriptor;
                if descriptor.dimension != TextureDimension::D2
                    || descriptor.size.depth_or_array_layers != 1
                    || descriptor.sample_count != 1
                    || descriptor.size.width == 0
                    || descriptor.size.height == 0
                    || !descriptor
                        .usage
                        .contains(TextureUsages::RENDER_ATTACHMENT | TextureUsages::COPY_SRC)
                {
                    return Err("lifecycle image requires a nonempty single-sample 2D render and readback texture".into());
                }
                Ok(Self {
                    identity: TargetIdentity::Image(handle.id()),
                    size: [descriptor.size.width, descriptor.size.height],
                })
            }
        }
    }

    fn matches_camera_target(self, target: &RenderTarget) -> bool {
        match self.identity {
            TargetIdentity::Image(id) => target.as_image().is_some_and(|image| image.id() == id),
            TargetIdentity::Window(primary) => match target {
                RenderTarget::Window(bevy::window::WindowRef::Primary) => true,
                RenderTarget::Window(bevy::window::WindowRef::Entity(entity)) => *entity == primary,
                _ => false,
            },
        }
    }

    fn validate_continuity(self, initial: Self) -> Result<(), String> {
        if self.identity != initial.identity {
            Err("lifecycle target was replaced".into())
        } else if matches!(self.identity, TargetIdentity::Image(_)) && self.size != initial.size {
            Err("lifecycle image dimensions changed during a fixed-size creation exercise".into())
        } else {
            Ok(())
        }
    }

    fn report(self) -> Value {
        match self.identity {
            TargetIdentity::Window(entity) => {
                json!({"kind":"window","entity":entity.to_bits(),"size":self.size,
                "scope":"window target only; no native visibility or OS transition inferred from pixels"})
            }
            TargetIdentity::Image(id) => {
                json!({"kind":"image","asset_id":format!("{id:?}"),"size":self.size,
                "geometry_source":"actual Image texture descriptor",
                "scope":"offscreen image creation-fault exercise; no native window, swapchain, OS sleep or panel claim"})
            }
        }
    }
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
        let valid = capture_is_valid(&proof, [rgba.width(), rgba.height()], purpose.expected_size);
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

fn require_capture(
    run: &mut LifecycleRun,
    commands: &mut Commands,
    size: [u32; 2],
    target: &crate::offscreen::CaptureTarget,
) -> bool {
    if run.phase_capture == Some(true) {
        return true;
    }
    if !run.capture_pending {
        run.capture_pending = true;
        commands
            .spawn((
                target.screenshot(),
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
    mut fault: ResMut<MetalFxDiagnosticFault>,
    native: Option<Res<crate::window_lifecycle::WindowLifecycle>>,
    capture_target: Res<crate::offscreen::CaptureTarget>,
    images: Res<Assets<Image>>,
    mut windows: Query<(Entity, &mut Window), With<PrimaryWindow>>,
    mut cameras: Query<(Entity, &mut Camera, &mut Transform, &RenderTarget), With<Camera3d>>,
) {
    if run.finished() {
        return;
    }
    let now = Instant::now();
    let mut window = windows.single_mut().ok();
    let resolved = LifecycleTarget::resolve(
        run.exercise,
        &capture_target,
        &images,
        window
            .as_ref()
            .map(|(entity, window)| (*entity, [window.physical_width(), window.physical_height()])),
    );
    let target = match resolved.and_then(|target| {
        if let Some(initial) = run.initial_target {
            target.validate_continuity(initial)?;
        }
        if cameras.iter().any(|(_, camera, _, camera_target)| {
            camera.is_active && !target.matches_camera_target(camera_target)
        }) {
            return Err(
                "active lifecycle camera does not render the selected capture target".into(),
            );
        }
        Ok(target)
    }) {
        Ok(target) => target,
        Err(error) => {
            fault.clear();
            restore(
                &mut run,
                &mut commands,
                window.as_mut().map(|(_, window)| &mut **window),
                &mut cameras,
            );
            run.finish(now, frame.0, Some(error));
            return;
        }
    };
    run.initial_target.get_or_insert(target);
    let size = target.size;
    let native_mode = run.exercise.native_lifecycle();
    if native_mode {
        run.native_evidence = native.as_ref().map(|observer| observer.report());
    }
    run.initial_size.get_or_insert(size);
    run.expected_size.get_or_insert(size);
    let snapshot = context.snapshot();
    let reset_pending = history.as_ref().is_some_and(|r| r.is_requested());
    let mut views: Vec<_> = cameras
        .iter()
        .map(|(entity, camera, _, _)| {
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
    let fault_snapshot = fault.snapshot();
    run.record(
        now,
        frame.0,
        &views,
        reset_pending,
        snapshot.epoch,
        scale.0,
        fault_snapshot,
    );
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
        LifecycleExercise::CameraCut
            | LifecycleExercise::InactiveCutResume
            | LifecycleExercise::CreationFailure
            | LifecycleExercise::CreationSlow
            | LifecycleExercise::WindowMinimize
            | LifecycleExercise::OsSleepResume
    );
    let creation_fault = run.exercise.creation_fault();
    let native_elapsed = native.as_ref().and_then(|observer| observer.wall_elapsed());
    let failure = if native_mode
        && native
            .as_ref()
            .and_then(|observer| observer.cursor())
            .is_none()
    {
        Some("native lifecycle event ledger is unavailable, poisoned, or overflowed".into())
    } else if native_mode && native_elapsed.is_none() {
        Some("native lifecycle wall clock moved backwards or is unavailable".into())
    } else if run.exercise == LifecycleExercise::OsSleepResume
        && !native
            .as_ref()
            .is_some_and(|observer| observer.native_sleep_available())
    {
        Some("native NSWorkspace system sleep/wake observer is unavailable".into())
    } else if native_mode
        && run
            .native_window
            .is_some_and(|original| Some(original) != window.as_ref().map(|(entity, _)| *entity))
    {
        Some("the native lifecycle test window was replaced".into())
    } else if (native_mode
        && native_elapsed.is_some_and(|elapsed| elapsed > Duration::from_secs(60)))
        || (!native_mode && now.duration_since(run.started) > DEADLINE)
    {
        Some(format!(
            "lifecycle deadline exceeded in {}",
            run.phase.name()
        ))
    } else if mode.get() == MetalFxMode::Disabled {
        Some("lifecycle exercises require an active MetalFX mode".into())
    } else if (creation_fault.is_some() || native_mode) && mode.get() != MetalFxMode::Temporal {
        Some("creation fault and native lifecycle exercises require Temporal mode".into())
    } else if native_mode && fault_snapshot.fault != ScalerCreationFault::Off {
        Some("native lifecycle exercises require diagnostic creation faults to remain off".into())
    } else if creation_fault.is_some()
        && run.phase == Phase::Initial
        && fault_snapshot.fault != ScalerCreationFault::Off
    {
        Some("creation fault must be disabled during initial readiness".into())
    } else if creation_fault.is_some()
        && run.phase == Phase::Changed
        && (Some(fault_snapshot.fault) != creation_fault
            || Some(fault_snapshot.generation) != run.fault_generation)
    {
        Some("injected creation fault changed before fallback capture".into())
    } else if creation_fault.is_some() && run.phase == Phase::Changed && !reset_pending {
        Some("history reset was consumed before injected creation completed".into())
    } else if creation_fault.is_some()
        && run.phase == Phase::Changed
        && views.iter().any(|view| {
            view.fresh
                && view.frame.is_some_and(|f| f >= run.phase_frame)
                && matches!(
                    view.state,
                    MetalFxEffectState::Encoded | MetalFxEffectState::OutputWritten
                )
        })
    {
        Some("injected creation was incorrectly reported as active MetalFX".into())
    } else if creation_fault.is_some()
        && run.phase == Phase::Restored
        && (fault_snapshot.fault != ScalerCreationFault::Off
            || run
                .fault_generation
                .is_none_or(|old| fault_snapshot.generation <= old))
    {
        Some("creation recovery did not release the injected generation".into())
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
        fault.clear();
        restore(
            &mut run,
            &mut commands,
            window.as_mut().map(|(_, window)| &mut **window),
            &mut cameras,
        );
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
    if native_mode && run.phase == Phase::Changed {
        let (window_entity, window) = window.as_mut().expect("native lifecycle requires a window");
        let window_entity = *window_entity;
        let observer = native.as_ref().expect("native observer checked above");
        let cursor = run
            .native_cursor
            .expect("initial native phase records a cursor");
        let recovered = if run.exercise == LifecycleExercise::WindowMinimize {
            if !run.native_restore_requested {
                if observer.window_state_since(cursor, window_entity, true) {
                    if run.native_hidden_since.is_none() {
                        run.native_hidden_since = Some(now);
                        run.event(now, frame.0, "window_minimized_observed", json!({
                            "window":window_entity.to_bits(),"after_sequence":cursor,
                            "proof":"WindowOccluded(true) and native is_minimized Some(true) observed after the request"}));
                    }
                    if run.native_hidden_since.is_some_and(|started| {
                        now.duration_since(started) >= Duration::from_millis(500)
                    }) {
                        run.native_cursor =
                            observer.record_minimize_request(window_entity, false, frame.0);
                        window.set_minimized(false);
                        run.native_restore_requested = true;
                        run.event(now, frame.0, "window_restore_requested", json!({"window":window_entity.to_bits(),"scope":"request only; native restore observations still required"}));
                    }
                } else {
                    run.native_hidden_since = None;
                }
                false
            } else {
                observer.window_state_since(cursor, window_entity, false)
            }
        } else if let Some(cycle) = observer.sleep_cycle_since(cursor) {
            run.native_cycle = Some(cycle);
            run.event(now, frame.0, "system_sleep_wake_observed", json!({
                "will_sleep_sequence":cycle.0,"did_wake_sequence":cycle.1,
                "scope":"native NSWorkspace system notifications; no power request was made by this fixture"}));
            true
        } else {
            false
        };
        if recovered {
            history
                .as_mut()
                .expect("Temporal history checked above")
                .request();
            context.request_reset();
            run.reset_was_pending = true;
            run.epoch_before_change = Some(snapshot.epoch);
            run.event(now, frame.0, "native_recovery_reset_requested", json!({
                "window":window_entity.to_bits(),"reset_pending":true,
                "scope":"native exit transition observed; fresh output, reset acknowledgement and restored pixels still required"}));
            run.transition(Phase::Restored, now, frame.0);
        }
        // No screenshot or generic render-readiness inference substitutes for
        // native enter/exit events. Rendering is allowed to continue while hidden.
        return;
    }
    if run.exercise == LifecycleExercise::InactiveCutResume && run.phase == Phase::Changed {
        if !reset_pending {
            restore(
                &mut run,
                &mut commands,
                window.as_mut().map(|(_, window)| &mut **window),
                &mut cameras,
            );
            run.finish(
                now,
                frame.0,
                Some("history reset was consumed while the camera was inactive".into()),
            );
            return;
        }
        if inactive_view_settled(&views, run.phase_frame, frame.0)
            && now.duration_since(run.phase_started) >= Duration::from_millis(300)
        {
            if let Some(primary) = run.primary {
                if let Ok((_, mut camera, _, _)) = cameras.get_mut(primary) {
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
    let native_restored = if native_mode && run.phase == Phase::Restored {
        let window_entity = window
            .as_ref()
            .expect("native lifecycle requires a window")
            .0;
        let observer = native.as_ref().expect("native observer checked above");
        let cursor = run
            .native_cursor
            .expect("initial native phase records a cursor");
        match run.exercise {
            LifecycleExercise::WindowMinimize => {
                observer.window_state_since(cursor, window_entity, false)
            }
            LifecycleExercise::OsSleepResume => {
                run.native_cycle.is_some() && observer.sleep_cycle_since(cursor) == run.native_cycle
            }
            _ => unreachable!(),
        }
    } else {
        true
    };
    let ready = if run.exercise == LifecycleExercise::MultipleViews && run.phase == Phase::Changed {
        rejected_views(&views, run.phase_frame)
    } else if creation_fault.is_some() && run.phase == Phase::Changed {
        active.len() == 1
            && run.primary.is_some_and(|e| e.to_bits() == active[0].entity)
            && run.expected_size == Some(size)
            && creation_fallback(active[0], size, scale.0, run.phase_frame)
    } else {
        active.len() == 1
            && run.primary.is_some_and(|e| e.to_bits() == active[0].entity)
            && run.ready_view(active[0], mode.get(), size, scale.0)
            && (run.phase == Phase::Initial || epoch_ready)
            && native_restored
            && !(needs_reset && run.phase != Phase::Initial && reset_pending)
    };
    run.stable.observe(ready, &views);
    if creation_fault.is_some() && run.phase == Phase::Changed && ready {
        let expected_reason = if run.exercise == LifecycleExercise::CreationFailure {
            MetalFxEffectReason::ScalerCreationFailed
        } else {
            MetalFxEffectReason::ScalerCreationSlow
        };
        if active[0].reason == Some(expected_reason) && !run.creation_reason_seen {
            run.creation_reason_seen = true;
            let phase_elapsed_s = now.duration_since(run.phase_started).as_secs_f64();
            run.event(
                now,
                frame.0,
                "injected_creation_reason_observed",
                json!({
                "reason":format!("{expected_reason:?}"),"observed_frame":active[0].frame,
                "generation":fault_snapshot.generation,"reset_pending":reset_pending,
                "phase_elapsed_s":phase_elapsed_s}),
            );
        }
    }
    if run.stable.count < READY_FRAMES {
        return;
    }
    if creation_fault.is_some()
        && run.phase == Phase::Changed
        && (!run.creation_reason_seen
            || (run.exercise == LifecycleExercise::CreationSlow
                && now.duration_since(run.phase_started) < Duration::from_secs(10)))
    {
        return;
    }
    let unsupported_phase =
        run.exercise == LifecycleExercise::MultipleViews && run.phase == Phase::Changed;
    let expected_size = run.expected_size.unwrap();
    if !unsupported_phase
        && !require_capture(&mut run, &mut commands, expected_size, &capture_target)
    {
        return;
    }
    if run.phase == Phase::Initial {
        run.initial_scale = Some(scale.0);
        run.epoch_before_change = Some(snapshot.epoch);
        match run.exercise {
            LifecycleExercise::Resize => {
                let changed = [(size[0] * 3 / 4).max(64), (size[1] * 3 / 4).max(64)];
                run.expected_size = Some(changed);
                window
                    .as_mut()
                    .expect("resize requires a window")
                    .1
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
                    if let Ok((_, mut camera, mut transform, _)) = cameras.get_mut(primary) {
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
            LifecycleExercise::CreationFailure | LifecycleExercise::CreationSlow => {
                let selected = creation_fault.expect("creation exercise has a fault");
                fault.set(selected);
                run.fault_generation = Some(fault.snapshot().generation);
                history
                    .as_mut()
                    .expect("temporal history checked above")
                    .request();
                context.request_reset();
                run.reset_was_pending = true;
                run.event(now, frame.0, "creation_fault_injected", json!({
                    "mode":format!("{selected:?}"),"generation":fault.snapshot().generation,
                    "reset_pending":true,"scope":"simulated creation result; no driver fault induced"}));
            }
            LifecycleExercise::WindowMinimize | LifecycleExercise::OsSleepResume => {
                let (window_entity, window) =
                    window.as_mut().expect("native lifecycle requires a window");
                let window_entity = *window_entity;
                let observer = native.as_ref().expect("native observer checked above");
                run.native_window = Some(window_entity);
                if run.exercise == LifecycleExercise::WindowMinimize {
                    run.native_cursor =
                        observer.record_minimize_request(window_entity, true, frame.0);
                    window.set_minimized(true);
                    run.event(now, frame.0, "window_minimize_requested", json!({"window":window_entity.to_bits(),"scope":"request only; actual native minimize observations still required"}));
                } else {
                    run.native_cursor = observer.cursor();
                    run.event(now, frame.0, "awaiting_external_system_sleep", json!({"window":window_entity.to_bits(),"scope":"fixture observes only; an externally initiated system sleep and wake must occur before the 60-second wall-clock deadline"}));
                }
            }
            LifecycleExercise::LateCamera => unreachable!("late camera starts without a view"),
        }
        run.transition(Phase::Changed, now, frame.0);
    } else if run.phase == Phase::Changed && run.exercise != LifecycleExercise::CameraCut {
        run.epoch_before_change = Some(snapshot.epoch);
        match run.exercise {
            LifecycleExercise::Resize => {
                let original = run.initial_size.unwrap();
                run.expected_size = Some(original);
                window
                    .as_mut()
                    .expect("resize requires a window")
                    .1
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
            LifecycleExercise::CreationFailure | LifecycleExercise::CreationSlow => {
                fault.clear();
                context.request_reset();
                run.event(
                    now,
                    frame.0,
                    "creation_fault_released",
                    json!({
                    "generation":fault.snapshot().generation,"reset_pending":reset_pending,
                    "scope":"normal driver creation resumes; old pending generation discarded"}),
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
    window: Option<&mut Window>,
    cameras: &mut Query<(Entity, &mut Camera, &mut Transform, &RenderTarget), With<Camera3d>>,
) {
    if let Some(window) = window {
        if run.exercise == LifecycleExercise::WindowMinimize {
            window.set_minimized(false);
        }
        if matches!(
            run.initial_target.map(|target| target.identity),
            Some(TargetIdentity::Window(_))
        ) {
            if let Some(size) = run.initial_size {
                window.resolution.set_physical_resolution(size[0], size[1]);
            }
        }
    }
    if let Some(extra) = run.secondary.take() {
        commands.entity(extra).despawn();
    }
    if let Some(primary) = run.primary {
        if let Ok((_, mut camera, _, _)) = cameras.get_mut(primary) {
            camera.is_active = true;
        }
    }
}

fn creation_fallback(view: &ViewEvidence, output: [u32; 2], scale: f32, since: u64) -> bool {
    view.active
        && view.fresh
        && view.frame.is_some_and(|f| f >= since)
        && view.mode == MetalFxMode::Disabled
        && view.output == output
        && view.content == output.map(|d| (d as f32 * scale).round() as u32)
        && (view.scale - scale).abs() <= 1e-4
        && matches!(
            (view.state, view.reason),
            (
                MetalFxEffectState::Failed,
                Some(MetalFxEffectReason::ScalerCreationFailed)
            ) | (
                MetalFxEffectState::Pending,
                Some(MetalFxEffectReason::ScalerPending | MetalFxEffectReason::ScalerCreationSlow)
            )
        )
}

fn capture_is_valid(proof: &Value, actual: [u32; 2], expected: [u32; 2]) -> bool {
    proof["nonuniform"] == true && proof["opaque_fraction"] == 1.0 && actual == expected
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn long_changed_phase_preserves_each_phases_recent_evidence_and_honest_counts() {
        let mut ledger = ObservationLedger::default();
        let changed_count = OBSERVATIONS_PER_PHASE * 5;
        let mut frame = 0;
        for (phase, count) in [
            (Phase::Initial, 30),
            (Phase::Changed, changed_count),
            (Phase::Restored, 30),
        ] {
            for _ in 0..count {
                frame += 1;
                ledger.record(phase, json!({"phase":phase.name(),"app_frame":frame}));
            }
        }
        let rows = ledger.observations();
        assert_eq!(rows.len(), OBSERVATIONS_PER_PHASE + 60);
        for pair in rows.windows(2) {
            assert!(
                pair[0]["app_frame"].as_u64().unwrap() < pair[1]["app_frame"].as_u64().unwrap()
            );
        }
        let summary = ledger.report();
        assert_eq!(summary["evicted"], changed_count - OBSERVATIONS_PER_PHASE);
        for (phase, total, retained) in [
            ("initial", 30, 30),
            ("changed", changed_count, OBSERVATIONS_PER_PHASE),
            ("restored", 30, 30),
        ] {
            let records: Vec<_> = rows.iter().filter(|row| row["phase"] == phase).collect();
            assert_eq!(records.len(), retained);
            assert_eq!(summary["phases"][phase]["total"], total);
            assert_eq!(summary["phases"][phase]["retained"], retained);
            assert_eq!(summary["phases"][phase]["evicted"], total - retained);
            assert_eq!(
                summary["phases"][phase]["first_retained_app_frame"],
                records[0]["app_frame"]
            );
            assert_eq!(
                summary["phases"][phase]["last_retained_app_frame"],
                records.last().unwrap()["app_frame"]
            );
        }
        assert_eq!(
            rows[30]["app_frame"],
            30 + changed_count - OBSERVATIONS_PER_PHASE + 1
        );
        assert_eq!(rows.last().unwrap()["phase"], "restored");
        assert_eq!(rows.last().unwrap()["app_frame"], frame);
    }

    #[test]
    fn offscreen_fault_target_uses_the_real_image_without_a_window() {
        let mut images = Assets::<Image>::default();
        let handle = images.add(crate::offscreen::render_image(320, 180));
        let capture = crate::offscreen::CaptureTarget::Image(handle.clone());
        for exercise in [
            LifecycleExercise::CreationFailure,
            LifecycleExercise::CreationSlow,
        ] {
            for metadata in [None, Some((Entity::from_bits(7), [1280, 720]))] {
                let target =
                    LifecycleTarget::resolve(exercise, &capture, &images, metadata).unwrap();
                assert_eq!(target.identity, TargetIdentity::Image(handle.id()));
                assert_eq!(target.size, [320, 180]);
            }
        }
        for exercise in [
            LifecycleExercise::Resize,
            LifecycleExercise::CameraCut,
            LifecycleExercise::LateCamera,
            LifecycleExercise::MultipleViews,
            LifecycleExercise::InactiveCutResume,
            LifecycleExercise::WindowMinimize,
            LifecycleExercise::OsSleepResume,
        ] {
            assert!(LifecycleTarget::resolve(exercise, &capture, &images, None).is_err());
        }
        images
            .get_mut(&handle)
            .unwrap()
            .texture_descriptor
            .size
            .width = 0;
        assert!(LifecycleTarget::resolve(
            LifecycleExercise::CreationFailure,
            &capture,
            &images,
            None
        )
        .is_err());
        images.remove(handle.id());
        assert!(LifecycleTarget::resolve(
            LifecycleExercise::CreationFailure,
            &capture,
            &images,
            None
        )
        .is_err());
    }

    #[test]
    fn offscreen_fault_camera_must_render_the_selected_image() {
        let mut images = Assets::<Image>::default();
        let handle = images.add(crate::offscreen::render_image(320, 180));
        let other = images.add(crate::offscreen::render_image(320, 180));
        let target = LifecycleTarget {
            identity: TargetIdentity::Image(handle.id()),
            size: [320, 180],
        };
        assert!(target.matches_camera_target(&handle.into()));
        assert!(!target.matches_camera_target(&other.into()));
        assert!(!target.matches_camera_target(&bevy::camera::RenderTarget::default()));
    }

    #[test]
    fn window_capture_rejects_a_camera_on_another_same_sized_window() {
        let primary = Entity::from_bits(7);
        let other = Entity::from_bits(8);
        let target = LifecycleTarget {
            identity: TargetIdentity::Window(primary),
            size: [320, 180],
        };
        assert!(target.matches_camera_target(&RenderTarget::default()));
        assert!(target.matches_camera_target(&RenderTarget::Window(
            bevy::window::WindowRef::Entity(primary)
        )));
        assert!(!target.matches_camera_target(&RenderTarget::Window(
            bevy::window::WindowRef::Entity(other)
        )));
    }

    #[test]
    fn offscreen_fault_target_replacement_or_resize_cannot_reuse_old_evidence() {
        let mut images = Assets::<Image>::default();
        let handle = images.add(crate::offscreen::render_image(320, 180));
        let other = images.add(crate::offscreen::render_image(320, 180));
        let initial = LifecycleTarget {
            identity: TargetIdentity::Image(handle.id()),
            size: [320, 180],
        };
        assert!(initial.validate_continuity(initial).is_ok());
        assert!(LifecycleTarget {
            identity: TargetIdentity::Image(other.id()),
            ..initial
        }
        .validate_continuity(initial)
        .is_err());
        assert!(LifecycleTarget {
            size: [640, 360],
            ..initial
        }
        .validate_continuity(initial)
        .is_err());
        let window = LifecycleTarget {
            identity: TargetIdentity::Window(Entity::from_bits(7)),
            ..initial
        };
        assert!(window.validate_continuity(initial).is_err());
        assert!(LifecycleTarget {
            size: [640, 360],
            ..window
        }
        .validate_continuity(window)
        .is_ok());
    }

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
        assert!(!rejected_views(std::slice::from_ref(&rejected), 50));
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

    #[test]
    fn inactive_phase_accepts_stale_prior_render_but_not_new_render_evidence() {
        let prior = ViewEvidence {
            active: false,
            fresh: false,
            ..view()
        };
        assert!(inactive_view_settled(std::slice::from_ref(&prior), 51, 57));
        assert!(!inactive_view_settled(
            &[ViewEvidence {
                fresh: true,
                ..prior.clone()
            }],
            51,
            57
        ));
        assert!(!inactive_view_settled(
            &[ViewEvidence {
                frame: Some(52),
                ..prior
            }],
            51,
            58
        ));
    }

    #[test]
    fn ignored_resize_cannot_pass_at_the_original_window_dimensions() {
        let mut run = LifecycleRun::new(LifecycleExercise::Resize);
        run.expected_size = Some([960, 540]);
        assert!(!run.ready_view(&view(), MetalFxMode::Temporal, [1280, 720], 0.5));
        let resized = ViewEvidence {
            content: [480, 270],
            output: [960, 540],
            ..view()
        };
        assert!(run.ready_view(&resized, MetalFxMode::Temporal, [960, 540], 0.5));
    }

    #[test]
    fn creation_fault_exercises_are_explicit_lifecycle_modes() {
        assert!(LifecycleExercise::parse("creation-failure").is_ok());
        assert!(LifecycleExercise::parse("creation-slow").is_ok());
        assert!(LifecycleExercise::parse("driver-crash").is_err());
    }

    #[test]
    fn native_lifecycle_modes_are_explicit_and_do_not_offer_power_control() {
        assert!(LifecycleExercise::parse("window-minimize").is_ok());
        assert!(LifecycleExercise::parse("os-sleep-resume").is_ok());
        assert!(LifecycleExercise::parse("force-sleep").is_err());
        assert!(LifecycleExercise::parse("lock-screen").is_err());
    }

    #[test]
    fn partially_transparent_scene_is_not_a_valid_fallback_capture() {
        let mut pixels: Vec<u8> = (0..10_000)
            .flat_map(|i| [(i % 251) as u8, (i % 239) as u8, (i % 233) as u8, 255])
            .collect();
        let opaque = crate::metrics::image_proof(&pixels, 100, 100);
        assert!(capture_is_valid(&opaque, [100, 100], [100, 100]));
        pixels[3] = 254;
        let partial = crate::metrics::image_proof(&pixels, 100, 100);
        assert_eq!(partial["nonuniform"], true);
        assert!(!capture_is_valid(&partial, [100, 100], [100, 100]));
    }

    #[test]
    fn creation_fallback_requires_fresh_failure_or_pending_and_actual_dimensions() {
        let failed = ViewEvidence {
            state: MetalFxEffectState::Failed,
            reason: Some(MetalFxEffectReason::ScalerCreationFailed),
            mode: MetalFxMode::Disabled,
            ..view()
        };
        assert!(creation_fallback(&failed, [1280, 720], 0.5, 50));
        for reason in [
            MetalFxEffectReason::ScalerPending,
            MetalFxEffectReason::ScalerCreationSlow,
        ] {
            assert!(creation_fallback(
                &ViewEvidence {
                    state: MetalFxEffectState::Pending,
                    reason: Some(reason),
                    ..failed.clone()
                },
                [1280, 720],
                0.5,
                50
            ));
        }
        for bad in [
            view(),
            ViewEvidence {
                state: MetalFxEffectState::Encoded,
                ..failed.clone()
            },
            ViewEvidence {
                fresh: false,
                ..failed.clone()
            },
            ViewEvidence {
                frame: Some(49),
                ..failed.clone()
            },
            ViewEvidence {
                content: [1280, 720],
                ..failed.clone()
            },
            ViewEvidence {
                output: [960, 540],
                ..failed.clone()
            },
            ViewEvidence {
                mode: MetalFxMode::Temporal,
                ..failed.clone()
            },
            ViewEvidence {
                reason: Some(MetalFxEffectReason::MissingPrepass),
                ..failed.clone()
            },
            ViewEvidence {
                active: false,
                ..failed.clone()
            },
            ViewEvidence {
                scale: 0.75,
                ..failed
            },
        ] {
            assert!(!creation_fallback(&bad, [1280, 720], 0.5, 50));
        }
    }
}
