//! Normal-pipelined native render lab. Completion fences delimit whole cohorts;
//! they cover prior wgpu render submissions, not the native Present buffer.
use crate::capture::{self, CaptureResults, CaptureTicket};
use crate::config::{Action, Mode, RunConfig, StressLoad};
use crate::measurement::{Cohort, FrameToken, Proof};
use crate::report::{emit, EngineResult, SceneResult};
use crate::scene::{LabCamera, LabScenePlugin, SceneState};
use bevy::app::ScheduleRunnerPlugin;
use bevy::camera::{NormalizedRenderTarget, RenderTarget};
use bevy::prelude::*;
use bevy::render::camera::{ExtractedCamera, TemporalJitter};
use bevy::render::extract_resource::{ExtractResource, ExtractResourcePlugin};
use bevy::render::render_resource::{PollType, TextureFormat, TextureUsages};
use bevy::render::renderer::{RenderAdapterInfo, RenderDevice, RenderQueue};
use bevy::render::sync_world::MainEntity;
use bevy::render::view::screenshot::Screenshot;
use bevy::render::view::{ExtractedView, ViewTarget};
use bevy::render::{Extract, ExtractSchedule, Render, RenderApp, RenderSystems};
use bevy::window::{PresentMode, WindowOccluded, WindowResolution};
use bevy::winit::{WinitPlugin, WinitSettings};
use bevy_metalfx::{
    MetalFxEffectState, MetalFxEffectStatus, MetalFxHistoryReset, MetalFxObservationFrame,
    MetalFxPlugin,
};
use serde_json::{json, Value};
use std::collections::{BTreeMap, HashSet};
use std::io::{BufRead, Read};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{mpsc, Arc, Mutex};
use std::time::{Duration, Instant};

const WARMUP_OUTPUTS: u32 = 20;
const BOUNDARY_TIMEOUT: Duration = Duration::from_secs(10);
const WARMUP_TIMEOUT: Duration = Duration::from_secs(90);
const STRESS_CHECKPOINT_FRAMES: u32 = 120;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Phase {
    Warmup,
    Opening,
    Measure,
    Closing,
    Finished,
}

#[derive(Resource, Clone, ExtractResource)]
struct Request {
    epoch: u64,
    phase: Phase,
    tick: u32,
    frame: u64,
    view: u64,
    started_ns: u64,
    scene: String,
    target_valid: bool,
    screenshot: Option<Entity>,
    last: bool,
    reset_expected: bool,
    configuration_generation: u64,
    load: StressLoad,
    window_visible: Option<bool>,
    window_focused: Option<bool>,
    window_occluded: Option<bool>,
}
impl Default for Request {
    fn default() -> Self {
        Self {
            epoch: 1,
            phase: Phase::Warmup,
            tick: 0,
            frame: 0,
            view: 0,
            started_ns: 0,
            scene: String::new(),
            target_valid: false,
            screenshot: None,
            last: false,
            reset_expected: false,
            configuration_generation: 1,
            load: StressLoad::default(),
            window_visible: None,
            window_focused: None,
            window_occluded: None,
        }
    }
}

#[derive(Resource, Clone)]
struct Settings {
    config: RunConfig,
    image: Option<Handle<Image>>,
    origin: Instant,
}
impl Settings {
    fn now(&self) -> u64 {
        self.origin.elapsed().as_nanos().min(u64::MAX as u128) as u64
    }
}

#[derive(Default)]
struct Fence {
    registered: AtomicBool,
    registered_ns: AtomicU64,
    observed_ns: AtomicU64,
}
impl Fence {
    fn observed(&self) -> Option<u64> {
        match self.observed_ns.load(Ordering::Acquire) {
            0 => None,
            n => Some(n),
        }
    }
}

#[derive(Default)]
struct SharedData {
    cohorts: BTreeMap<u64, Cohort>,
    opening: BTreeMap<u64, Arc<Fence>>,
    closing: BTreeMap<u64, Arc<Fence>>,
    latest_ready: Option<(u64, u64, bool)>,
    captured_proofs: BTreeMap<u64, Value>,
    errors: Vec<String>,
    adapter: Value,
    metadata: BTreeMap<u64, Value>,
    terminal: bool,
    readiness: BTreeMap<u64, Value>,
}
#[derive(Resource, Clone, Default)]
struct Shared(Arc<Mutex<SharedData>>);
impl Shared {
    fn error(&self, error: impl Into<String>) {
        let mut data = self.0.lock().expect("benchmark ledger poisoned");
        if data.errors.len() < 32 {
            data.errors.push(error.into());
        }
    }
}

#[derive(Resource)]
struct Controller {
    phase: Phase,
    epoch: u64,
    scene_index: usize,
    next_tick: u32,
    warmup_count: u32,
    last_ready_frame: u64,
    phase_started: Instant,
    result: EngineResult,
    completed_epochs: HashSet<u64>,
    pending_load: Option<StressLoad>,
    stopping: bool,
    manual_stop: bool,
    stress_tick: u64,
    previous_checkpoint_ns: Option<u64>,
    controls: Mutex<mpsc::Receiver<Value>>,
    stress_started: Option<Instant>,
    evicted_stress_samples: u64,
    outcome: Arc<Mutex<Option<MainOutcome>>>,
    shared: Shared,
    window_occluded: Option<bool>,
}

#[derive(Clone)]
struct MainOutcome {
    result: EngineResult,
    stopped: bool,
    evicted_stress_samples: u64,
}

/// Prevent only user-idle sleep while a renderer is running. This local guard
/// survives App::run's ownership transfer and releases the exact activity token
/// on a normal return or Rust unwind; it changes no persistent power settings.
#[cfg(target_os = "macos")]
struct IdleSleepActivity {
    process: objc2::rc::Retained<objc2_foundation::NSProcessInfo>,
    token:
        objc2::rc::Retained<objc2::runtime::ProtocolObject<dyn objc2::runtime::NSObjectProtocol>>,
}

#[cfg(target_os = "macos")]
impl IdleSleepActivity {
    fn begin() -> Self {
        use objc2_foundation::{NSActivityOptions, NSProcessInfo, NSString};
        let process = NSProcessInfo::processInfo();
        let token = process.beginActivityWithOptions_reason(
            NSActivityOptions::IdleDisplaySleepDisabled
                | NSActivityOptions::IdleSystemSleepDisabled,
            &NSString::from_str("Ushas Bench active render workload"),
        );
        Self { process, token }
    }
}

#[cfg(target_os = "macos")]
impl Drop for IdleSleepActivity {
    fn drop(&mut self) {
        // SAFETY: token is the retained object returned by this process's
        // beginActivity call. The non-Clone guard ends it exactly once.
        unsafe { self.process.endActivity(&self.token) };
    }
}

pub fn run(config: RunConfig) -> EngineResult {
    #[cfg(target_os = "macos")]
    let _idle_sleep_activity = IdleSleepActivity::begin();
    let shared = Shared::default();
    let captures = CaptureResults::default();
    let origin = Instant::now();
    // App::run transfers ownership to the runner and leaves an empty App behind.
    // Keep the terminal main-world snapshot outside that consumed App.
    let outcome = Arc::new(Mutex::new(None));
    let (tx, rx) = mpsc::sync_channel(16);
    std::thread::spawn(move || {
        let mut input = std::io::stdin().lock();
        let mut line = Vec::with_capacity(4097);
        loop {
            line.clear();
            // Cap allocation before decoding even when a caller sends an unbounded line.
            let Ok(count) = input.by_ref().take(4097).read_until(b'\n', &mut line) else {
                break;
            };
            if count == 0 {
                break;
            }
            if line.len() > 4096 {
                if !line.ends_with(b"\n") {
                    loop {
                        line.clear();
                        match input.by_ref().take(4097).read_until(b'\n', &mut line) {
                            Ok(0) | Err(_) => return,
                            Ok(_) if line.ends_with(b"\n") => break,
                            _ => {}
                        }
                    }
                }
                continue;
            }
            if let Ok(value) = serde_json::from_slice::<Value>(&line) {
                let _ = tx.try_send(value);
            }
        }
    });
    let mut app = App::new();
    let window = Window {
        title: "Ushas Bench — Claude render lab".into(),
        resolution: WindowResolution::new(config.width, config.height)
            .with_scale_factor_override(1.0),
        present_mode: PresentMode::Immediate,
        resizable: false,
        focused: true,
        visible: config.action != Action::Capture,
        ..default()
    };
    let mut defaults = DefaultPlugins.set(WindowPlugin {
        primary_window: Some(window),
        exit_condition: bevy::window::ExitCondition::DontExit,
        ..default()
    });
    if config.action == Action::Capture {
        defaults = defaults.disable::<WinitPlugin>();
    }
    app.add_plugins(defaults);
    if config.action == Action::Capture {
        app.add_plugins(ScheduleRunnerPlugin::run_loop(Duration::ZERO));
    } else {
        app.insert_resource(WinitSettings::continuous());
    }
    let image = if config.action == Action::Capture {
        let mut image = Image::new_target_texture(
            config.width,
            config.height,
            TextureFormat::Rgba8UnormSrgb,
            None,
        );
        image.texture_descriptor.usage |= TextureUsages::COPY_SRC;
        Some(app.world_mut().resource_mut::<Assets<Image>>().add(image))
    } else {
        None
    };
    let target = image
        .as_ref()
        .map_or_else(RenderTarget::default, |i| i.clone().into());
    app.world_mut().spawn((
        Camera3d::default(),
        Camera::default(),
        target,
        bevy::camera::Hdr,
        if config.mode == Mode::Native {
            Msaa::Sample4
        } else {
            Msaa::Off
        },
        LabCamera,
        IsDefaultUiCamera,
        bevy::camera::ShadowLodOrigin,
        bevy::core_pipeline::tonemapping::Tonemapping::AcesFitted,
    ));
    let settings = Settings {
        config: config.clone(),
        image,
        origin,
    };
    let initial_scene = config.scenes()[0];
    app.insert_resource(SceneState {
        kind: initial_scene,
        seed: config.seed,
        generation: 1,
        load: config.load.clone(),
        ..default()
    });
    app.insert_resource(settings.clone())
        .insert_resource(shared.clone())
        .insert_resource(captures.clone())
        .insert_resource(Request::default())
        .insert_resource(Controller {
            phase: Phase::Warmup,
            epoch: 1,
            scene_index: 0,
            next_tick: 0,
            warmup_count: 0,
            last_ready_frame: 0,
            phase_started: Instant::now(),
            result: EngineResult::default(),
            completed_epochs: HashSet::new(),
            pending_load: None,
            stopping: false,
            manual_stop: false,
            stress_tick: 0,
            previous_checkpoint_ns: None,
            controls: Mutex::new(rx),
            stress_started: None,
            evicted_stress_samples: 0,
            outcome: outcome.clone(),
            shared: shared.clone(),
            window_occluded: None,
        });
    app.insert_resource(bevy_metalfx::MetalFxGpuTimingDisabled);
    app.add_plugins((
        MetalFxPlugin {
            mode: config.mode.metalfx(),
            render_scale: config.scale,
            adaptive: false,
            ..default()
        },
        LabScenePlugin,
    ));
    app.add_plugins(ExtractResourcePlugin::<Request>::default());
    app.add_systems(PreUpdate, drive)
        .add_systems(PostUpdate, set_jitter)
        .add_systems(Last, clear_terminal_request);
    if let Some(render) = app.get_sub_app_mut(RenderApp) {
        render
            .insert_resource(settings)
            .insert_resource(shared.clone())
            .insert_resource(Request::default())
            .insert_resource(ExtractedShot::default())
            .add_systems(ExtractSchedule, extract_shot)
            .add_systems(
                Render,
                (
                    initialize_device
                        .after(RenderSystems::ExtractCommands)
                        .before(RenderSystems::PrepareAssets),
                    record_render
                        .after(RenderSystems::Render)
                        .before(RenderSystems::Cleanup),
                    seal_and_poll.after(RenderSystems::PostCleanup),
                ),
            );
    }
    emit(
        "started",
        json!({"message":"Warming the Claude render lab","scene":initial_scene.as_str(),"progress":0.0}),
    );
    app.run();
    let completed = outcome.lock().expect("benchmark outcome poisoned").take();
    let (mut result, evicted_stress_samples) = match completed {
        Some(completed) => {
            let mut result = completed.result;
            result.stopped = completed.stopped;
            (result, completed.evicted_stress_samples)
        }
        None => {
            let mut result = EngineResult::default();
            result
                .errors
                .push("renderer exited before closing the requested workload".into());
            (result, 0)
        }
    };
    let data = shared.0.lock().expect("benchmark ledger poisoned");
    result.errors.extend(data.errors.iter().cloned());
    let cohort_records: Vec<_> = data
        .cohorts
        .values()
        .map(|cohort| {
            let mut value = cohort_json(cohort);
            value["configuration"] = data
                .metadata
                .get(&cohort.epoch)
                .cloned()
                .unwrap_or(Value::Null);
            value
        })
        .collect();
    result.environment = json!({"scope":"normal-pipelined completed-render throughput; includes CPU/render scheduling, surface acquisition and callback dispatch; not GPU busy time, frame pacing or panel delivery",
        "pipelined_rendering":true,"measurement_per_frame_gpu_callbacks":false,"per_frame_gpu_waits":false,"legacy_metalfx_gpu_timing_disabled":true,
        "completion_boundary":"after RenderSystems::PostCleanup; prior wgpu submissions, not final native Present buffer",
        "present_mode_requested":"Immediate","present_mode_resolved":null,
        "present_mode_resolved_unavailable_reason":"Bevy 0.19 surface configuration is private; the window field retains the request rather than a resolved runtime policy.",
        "presentation_note":"Completed-render throughput of this native window path may include drawable acquisition and presentation backpressure; it is not uncapped hardware capacity.",
        "output_physical_pixels":[config.width,config.height],
        "stress_retention":{"maximum_samples":8192,"evicted_completed_samples":evicted_stress_samples,"retained_completed_cohort_details":8},
        "adapter":data.adapter,"platform":platform(),"cohorts":cohort_records,"readiness":data.readiness,
        "idle_sleep_prevention":{"requested":cfg!(target_os="macos"),"mechanism":"NSProcessInfo scoped activity","options":["IdleDisplaySleepDisabled","IdleSystemSleepDisabled"],"scope":"engine lifetime; no persistent OS setting changes; does not prevent explicit user sleep or lock"},
        "capture_scope":"separate deterministic image-target replay; no capture in scored intervals"});
    result.captures = captures.0.lock().expect("capture results poisoned").clone();
    for capture in &mut result.captures {
        let proof = capture["requested_frame"]
            .as_u64()
            .and_then(|frame| data.captured_proofs.get(&frame));
        if let Some(proof) = proof {
            let matching = capture::joined(capture, proof);
            capture["render_proof"] = proof.clone();
            capture["valid"] = json!(matching && capture["pixel_valid"] == true);
        }
    }
    if config.action == Action::Capture
        && (result.captures.is_empty() || result.captures.iter().any(|v| v["valid"] != true))
    {
        result
            .errors
            .push("missing, failed or unjoined quality capture".into());
    }
    if config.action == Action::Capture {
        let expected: HashSet<_> = config
            .scenes()
            .iter()
            .flat_map(|scene| {
                capture::capture_ticks(config.frames)
                    .into_iter()
                    .map(move |tick| (scene.as_str().to_string(), u64::from(tick)))
            })
            .collect();
        let actual: HashSet<_> = result
            .captures
            .iter()
            .filter_map(|c| Some((c["scene"].as_str()?.to_string(), c["tick"].as_u64()?)))
            .collect();
        if actual != expected || actual.len() != result.captures.len() {
            result
                .errors
                .push("quality checkpoint set has missing or duplicate captures".into());
        }
    }
    result.valid = result.errors.is_empty()
        && (config.action == Action::Stress || result.scenes.len() == config.scenes().len())
        && result.scenes.iter().all(|s| s.valid)
        && !(result.stopped && config.action != Action::Stress);
    if config.action == Action::Stress && result.stress_samples.is_empty() {
        result.valid = false;
        result
            .errors
            .push("stress ended before a completed checkpoint".into());
    }
    result
}

fn phase_request(
    request: &mut Request,
    controller: &Controller,
    frame: u64,
    view: u64,
    settings: &Settings,
    scene: &SceneState,
    target_valid: bool,
) {
    *request = Request {
        epoch: controller.epoch,
        phase: controller.phase,
        tick: controller.next_tick,
        frame,
        view,
        started_ns: settings.now(),
        scene: scene.kind.as_str().into(),
        target_valid,
        screenshot: None,
        last: false,
        reset_expected: false,
        configuration_generation: scene.generation,
        load: scene.load.clone(),
        window_visible: None,
        window_focused: None,
        window_occluded: controller.window_occluded,
    };
}

#[allow(clippy::too_many_arguments)]
fn drive(
    mut commands: Commands,
    settings: Res<Settings>,
    shared: Res<Shared>,
    captures: Res<CaptureResults>,
    mut controller: ResMut<Controller>,
    mut request: ResMut<Request>,
    mut scene: ResMut<SceneState>,
    frame: Res<MetalFxObservationFrame>,
    mut cameras: Query<(Entity, &mut Camera), With<LabCamera>>,
    windows: Query<&Window>,
    mut occlusions: MessageReader<WindowOccluded>,
    keys: Res<ButtonInput<KeyCode>>,
    mut reset: Option<ResMut<MetalFxHistoryReset>>,
    mut exit: MessageWriter<AppExit>,
) {
    if controller.phase == Phase::Finished {
        return;
    }
    let Ok((camera_entity, mut camera)) = cameras.single_mut() else {
        return;
    };
    let view = camera_entity.to_bits();
    if let Some(event) = occlusions.read().last() {
        controller.window_occluded = Some(event.occluded);
    }
    let target_valid = settings.image.is_some()
        || windows.single().is_ok_and(|w| {
            [w.physical_width(), w.physical_height()]
                == [settings.config.width, settings.config.height]
        });
    let controls: Vec<_> = controller
        .controls
        .lock()
        .expect("control pipe poisoned")
        .try_iter()
        .collect();
    for command in controls {
        match command["event"].as_str() {
            Some("stop") => {
                controller.stopping = true;
                controller.manual_stop = true;
            }
            Some("configure") if settings.config.action == Action::Stress => {
                let mut load = scene.load.clone();
                let mut invalid = false;
                for (key, maximum, destination) in [
                    ("claudes", 256, &mut load.claudes),
                    ("lights", 16, &mut load.lights),
                    ("particles", 16384, &mut load.particles),
                ] {
                    if let Some(value) = command.get(key) {
                        if let Some(n) = value.as_u64().filter(|n| *n <= maximum) {
                            *destination = Some(n as u32);
                        } else {
                            invalid = true;
                        }
                    }
                }
                if let Some(value) = command.get("fill") {
                    if let Some(n) = value.as_u64().filter(|n| *n <= 8000) {
                        load.fill = n as u32;
                    } else {
                        invalid = true;
                    }
                }
                let mut candidate = settings.config.clone();
                candidate.load = load.clone();
                if invalid || candidate.validate().is_err() {
                    emit(
                        "error",
                        json!({"message":"stress configuration exceeds bounded controls"}),
                    );
                } else if load != scene.load {
                    controller.pending_load = Some(load);
                }
            }
            _ => {}
        }
    }
    if keys.just_pressed(KeyCode::Escape) || crate::control::stop_requested() {
        controller.stopping = true;
        controller.manual_stop = true;
    }
    if settings.config.action == Action::Stress
        && controller
            .stress_started
            .is_some_and(|start| start.elapsed() >= Duration::from_secs(settings.config.duration))
    {
        controller.stopping = true;
    }
    if !shared
        .0
        .lock()
        .expect("benchmark ledger poisoned")
        .errors
        .is_empty()
    {
        controller.stopping = true;
    }
    // Complete earlier stress checkpoints without stalling the current renderer.
    if settings.config.action == Action::Stress {
        collect_stress(&shared, &mut controller);
    }
    if controller.stopping || controller.pending_load.is_some() {
        if controller.phase == Phase::Measure && controller.next_tick > 0 {
            if let Some(cohort) = shared
                .0
                .lock()
                .expect("benchmark ledger poisoned")
                .cohorts
                .get_mut(&controller.epoch)
            {
                cohort.expected = controller.next_tick;
            }
            controller.phase = Phase::Closing;
            controller.phase_started = Instant::now();
        } else if matches!(controller.phase, Phase::Warmup | Phase::Opening) {
            if controller.stopping {
                finish(&mut controller, &settings, &mut exit);
                return;
            }
            apply_load(&mut controller, &mut scene);
        }
    }
    match controller.phase {
        Phase::Warmup => {
            if let Some((epoch, observed, ready)) = shared
                .0
                .lock()
                .expect("benchmark ledger poisoned")
                .latest_ready
            {
                if epoch == controller.epoch && observed > controller.last_ready_frame {
                    controller.last_ready_frame = observed;
                    controller.warmup_count = if ready {
                        controller.warmup_count + 1
                    } else {
                        0
                    };
                }
            }
            if controller.warmup_count >= WARMUP_OUTPUTS {
                controller.phase = Phase::Opening;
                controller.phase_started = Instant::now();
            } else if controller.phase_started.elapsed() > WARMUP_TIMEOUT {
                let epoch = controller.epoch;
                controller
                    .result
                    .errors
                    .push(format!("fresh output warmup timed out for {}; see environment.readiness epoch {epoch} for original frame, target, and window state", scene.kind.as_str()));
                finish(&mut controller, &settings, &mut exit);
                return;
            }
        }
        Phase::Opening => {
            let observed = shared
                .0
                .lock()
                .expect("benchmark ledger poisoned")
                .opening
                .get(&controller.epoch)
                .and_then(|f| f.observed());
            if let Some(at) = observed {
                let count = if settings.config.action == Action::Stress {
                    STRESS_CHECKPOINT_FRAMES
                } else {
                    settings.config.frames
                };
                shared
                    .0
                    .lock()
                    .expect("benchmark ledger poisoned")
                    .cohorts
                    .insert(controller.epoch, Cohort::new(controller.epoch, count, at));
                controller.phase = Phase::Measure;
                controller.next_tick = 0;
                controller.phase_started = Instant::now();
                if settings.config.action == Action::Stress && controller.stress_started.is_none() {
                    controller.stress_started = Some(Instant::now());
                }
            } else if controller.phase_started.elapsed() > BOUNDARY_TIMEOUT {
                controller
                    .result
                    .errors
                    .push("opening completion timed out".into());
                finish(&mut controller, &settings, &mut exit);
                return;
            }
        }
        Phase::Closing => {
            let observed = shared
                .0
                .lock()
                .expect("benchmark ledger poisoned")
                .closing
                .get(&controller.epoch)
                .and_then(|f| f.observed());
            if let Some(at) = observed {
                if settings.config.action == Action::Stress {
                    collect_stress(&shared, &mut controller);
                    if controller.stopping {
                        finish(&mut controller, &settings, &mut exit);
                        return;
                    }
                    apply_load(&mut controller, &mut scene);
                } else {
                    let expected = if settings.config.action == Action::Capture {
                        capture::capture_ticks(settings.config.frames).len()
                    } else {
                        0
                    };
                    let received = captures
                        .0
                        .lock()
                        .expect("capture results poisoned")
                        .iter()
                        .filter(|v| v["epoch"] == controller.epoch)
                        .count();
                    if received < expected {
                        if controller.phase_started.elapsed() > BOUNDARY_TIMEOUT {
                            controller
                                .result
                                .errors
                                .push("quality readback timed out".into());
                            finish(&mut controller, &settings, &mut exit);
                            return;
                        }
                    } else {
                        let mut data = shared.0.lock().expect("benchmark ledger poisoned");
                        if let Some(cohort) = data.cohorts.get_mut(&controller.epoch) {
                            if cohort.closing_ns.is_none() {
                                cohort.close(at);
                            }
                            let fps = cohort.fps();
                            let valid = fps.is_some() && !controller.stopping;
                            let result = SceneResult {
                                scene: scene.kind.as_str().into(),
                                valid,
                                frames: cohort.frames.len() as u32,
                                elapsed_seconds: cohort.seconds().unwrap_or(0.),
                                render_fps: if settings.config.action == Action::Capture {
                                    None
                                } else {
                                    fps
                                },
                                errors: cohort.errors.clone(),
                            };
                            emit(
                                "scene_complete",
                                json!({"scene":result.scene,"valid":valid,"render_fps":result.render_fps}),
                            );
                            controller.result.scenes.push(result);
                        }
                        drop(data);
                        controller.scene_index += 1;
                        if controller.stopping
                            || controller.scene_index >= settings.config.scenes().len()
                        {
                            finish(&mut controller, &settings, &mut exit);
                            return;
                        }
                        scene.kind = settings.config.scenes()[controller.scene_index];
                        scene.generation += 1;
                        restart_warmup(&mut controller);
                    }
                }
            } else if controller.phase_started.elapsed() > BOUNDARY_TIMEOUT {
                controller
                    .result
                    .errors
                    .push("closing completion timed out".into());
                finish(&mut controller, &settings, &mut exit);
                return;
            }
        }
        _ => {}
    }
    if controller.phase == Phase::Measure {
        let count = if settings.config.action == Action::Stress {
            STRESS_CHECKPOINT_FRAMES
        } else {
            settings.config.frames
        };
        if controller.next_tick >= count {
            if settings.config.action == Action::Stress {
                controller.epoch += 1;
                controller.next_tick = 0;
                shared
                    .0
                    .lock()
                    .expect("benchmark ledger poisoned")
                    .cohorts
                    .insert(controller.epoch, Cohort::new(controller.epoch, count, 0));
            } else {
                controller.phase = Phase::Closing;
                controller.phase_started = Instant::now();
            }
        }
    }
    camera.is_active = matches!(controller.phase, Phase::Warmup | Phase::Measure);
    phase_request(
        &mut request,
        &controller,
        frame.0,
        view,
        &settings,
        &scene,
        target_valid,
    );
    if settings.image.is_none() {
        if let Ok(window) = windows.single() {
            request.window_visible = Some(window.visible);
            request.window_focused = Some(window.focused);
        }
    }
    if controller.phase == Phase::Measure {
        scene.tick = if settings.config.action == Action::Stress {
            controller.stress_tick.min(u32::MAX as u64) as u32
        } else {
            controller.next_tick
        };
        let previous_seconds = scene.time_seconds;
        scene.time_seconds = if settings.config.action == Action::Stress {
            controller
                .stress_started
                .map_or(0., |start| start.elapsed().as_secs_f32())
        } else {
            scene.tick as f32 / 120.
        };
        scene.caption = format!(
            "{} · {} · {} / {}",
            scene.kind.as_str(),
            settings.config.mode.as_str(),
            controller.next_tick + 1,
            settings.config.frames
        );
        request.reset_expected = settings.config.mode == Mode::Temporal
            && if settings.config.action == Action::Stress {
                let first_of_segment = controller.next_tick == 0
                    && shared
                        .0
                        .lock()
                        .expect("benchmark ledger poisoned")
                        .opening
                        .contains_key(&controller.epoch);
                stress_reset(first_of_segment, previous_seconds, scene.time_seconds)
            } else {
                controller.next_tick == 0 || scene.tick == crate::scene::CAMERA_CUT_TICK
            };
        if request.reset_expected {
            if let Some(reset) = &mut reset {
                reset.request();
            }
        }
        if settings.config.action == Action::Capture
            && capture::capture_ticks(settings.config.frames).contains(&controller.next_tick)
        {
            let ticket = CaptureTicket {
                scene: scene.kind.as_str().into(),
                epoch: controller.epoch,
                tick: controller.next_tick,
                requested_frame: frame.0,
                view,
                output: [settings.config.width, settings.config.height],
                path: settings.config.out.join(format!(
                    "capture-{}-{:04}.png",
                    scene.kind.as_str(),
                    controller.next_tick
                )),
            };
            request.screenshot = Some(capture::request(
                &mut commands,
                settings.image.clone().expect("capture target"),
                ticket,
            ));
        }
        request.last = controller.next_tick + 1
            == if settings.config.action == Action::Stress {
                STRESS_CHECKPOINT_FRAMES
            } else {
                settings.config.frames
            };
        controller.next_tick += 1;
        controller.stress_tick += 1;
        if controller.next_tick.is_multiple_of(120) {
            let progress = if settings.config.action == Action::Stress {
                controller
                    .stress_started
                    .map_or(0., |start| start.elapsed().as_secs_f64())
                    / settings.config.duration as f64
            } else {
                (controller.scene_index as f64
                    + controller.next_tick as f64 / settings.config.frames as f64)
                    / settings.config.scenes().len() as f64
            };
            emit(
                "progress",
                json!({"scene":scene.kind.as_str(),"progress":progress.clamp(0.,1.)}),
            );
        }
    } else {
        scene.tick = 0;
        scene.time_seconds = 0.;
        scene.caption = format!("{} · warming", scene.kind.as_str());
    }
    // This timer is a safety limit, never the benchmark's score denominator.
    if settings.config.action != Action::Stress
        && controller.phase == Phase::Measure
        && controller.phase_started.elapsed() > Duration::from_secs(600)
    {
        controller
            .result
            .errors
            .push("measured scene exceeded finite deadline".into());
        finish(&mut controller, &settings, &mut exit);
    }
}

fn restart_warmup(controller: &mut Controller) {
    controller.epoch += 1;
    controller.phase = Phase::Warmup;
    controller.next_tick = 0;
    controller.warmup_count = 0;
    controller.last_ready_frame = 0;
    controller.phase_started = Instant::now();
}
fn stress_reset(first_of_segment: bool, previous_seconds: f32, seconds: f32) -> bool {
    let cut_seconds = crate::scene::CAMERA_CUT_TICK as f32 / 120.;
    first_of_segment || previous_seconds < cut_seconds && seconds >= cut_seconds
}
fn apply_load(controller: &mut Controller, scene: &mut SceneState) {
    if let Some(load) = controller.pending_load.take() {
        scene.load = load;
        scene.generation += 1;
    }
    controller.previous_checkpoint_ns = None;
    restart_warmup(controller);
}
fn finish(controller: &mut Controller, settings: &Settings, exit: &mut MessageWriter<AppExit>) {
    if controller.manual_stop && settings.config.action != Action::Stress {
        controller.result.errors.push("benchmark cancelled".into());
    }
    controller.phase = Phase::Finished;
    *controller
        .outcome
        .lock()
        .expect("benchmark outcome poisoned") = Some(MainOutcome {
        result: controller.result.clone(),
        stopped: controller.manual_stop,
        evicted_stress_samples: controller.evicted_stress_samples,
    });
    controller
        .shared
        .0
        .lock()
        .expect("benchmark ledger poisoned")
        .terminal = true;
    exit.write(AppExit::Success);
}

fn collect_stress(shared: &Shared, controller: &mut Controller) {
    let mut data = shared.0.lock().expect("benchmark ledger poisoned");
    let ready: Vec<_> = data
        .closing
        .iter()
        .filter_map(|(epoch, fence)| fence.observed().map(|at| (*epoch, at)))
        .filter(|(epoch, _)| !controller.completed_epochs.contains(epoch))
        .collect();
    for (epoch, at) in ready {
        let metadata = data.metadata.get(&epoch).cloned().unwrap_or(Value::Null);
        let Some(cohort) = data.cohorts.get_mut(&epoch) else {
            continue;
        };
        if cohort.closing_ns.is_none() {
            cohort.close(at);
        }
        let start = controller
            .previous_checkpoint_ns
            .or_else(|| cohort.frames.first().map(|(t, _)| t.started_ns))
            .unwrap_or(at);
        let elapsed = at.saturating_sub(start) as f64 / 1e9;
        let valid = cohort.errors.is_empty();
        let sample = json!({"epoch":epoch,"frames":cohort.frames.len(),"elapsed_seconds":elapsed,
            "render_fps":if valid && elapsed>0.{Some(cohort.frames.len() as f64/elapsed)}else{None},"valid":valid,"errors":cohort.errors,
            "rate_unavailable_reason":if elapsed==0.{Some("callbacks dispatched together")}else{None},"configuration":metadata,
            "first_frame":cohort.frames.first().map(|(t,_)|t.frame),"last_frame":cohort.frames.last().map(|(t,_)|t.frame),
            "scope":"periodic callback-observed completed-render throughput; callback batching is not frame pacing","thermal":thermal()});
        emit("progress", sample.clone());
        if controller.result.stress_samples.len() >= 8192 {
            controller.result.stress_samples.remove(0);
            controller.evicted_stress_samples += 1;
        }
        controller.result.stress_samples.push(sample);
        controller.previous_checkpoint_ns = Some(at);
        controller.completed_epochs.insert(epoch);
        if !valid {
            controller.stopping = true;
            let remaining = 32usize.saturating_sub(controller.result.errors.len());
            controller
                .result
                .errors
                .extend(cohort.errors.iter().take(remaining).cloned());
        }
    }
    // Keep all pending cohorts plus only eight completed detailed frame ledgers.
    // Periodic summaries retain their completion counts and configuration.
    let completed: Vec<_> = data
        .cohorts
        .iter()
        .filter(|(_, c)| c.closing_ns.is_some())
        .map(|(e, _)| *e)
        .collect();
    let drop_count = completed.len().saturating_sub(8);
    for epoch in completed.into_iter().take(drop_count) {
        data.cohorts.remove(&epoch);
        data.opening.remove(&epoch);
        data.closing.remove(&epoch);
        data.metadata.remove(&epoch);
        controller.completed_epochs.remove(&epoch);
    }
    if data
        .cohorts
        .values()
        .filter(|c| c.closing_ns.is_none())
        .count()
        > 16
        && data.errors.len() < 32
    {
        data.errors
            .push("pending stress checkpoint retention exceeded".into());
    }
}

fn jitter_offset(tick: u32) -> [f32; 2] {
    fn radical(mut n: u32, base: u32) -> f32 {
        let (mut sum, mut d) = (0., 1.);
        while n > 0 {
            d *= base as f32;
            sum += (n % base) as f32 / d;
            n /= base;
        }
        sum - 0.5
    }
    let n = tick % 32 + 1;
    [radical(n, 2), radical(n, 3)]
}
fn set_jitter(request: Res<Request>, mut cameras: Query<&mut TemporalJitter, With<LabCamera>>) {
    for mut jitter in &mut cameras {
        jitter.offset = Vec2::from_array(jitter_offset(request.tick));
    }
}

fn clear_terminal_request(
    controller: Res<Controller>,
    mut request: ResMut<Request>,
    mut cameras: Query<&mut Camera, With<LabCamera>>,
) {
    if controller.phase == Phase::Finished {
        request.phase = Phase::Finished;
        request.screenshot = None;
        request.last = false;
        for mut camera in &mut cameras {
            camera.is_active = false;
        }
    }
}

#[derive(Resource, Default)]
struct ExtractedShot {
    entity: Option<Entity>,
    target_valid: bool,
    frame: u64,
    msaa_samples: Option<u32>,
    jitter: Option<[f32; 2]>,
    reset_before: bool,
    camera_matrix: Option<[f32; 16]>,
    scene: SceneState,
}
#[allow(clippy::type_complexity)]
#[allow(clippy::too_many_arguments)]
fn extract_shot(
    mut commands: Commands,
    request: Extract<Res<Request>>,
    shots: Extract<Query<&Screenshot>>,
    frame: Extract<Res<MetalFxObservationFrame>>,
    settings: Res<Settings>,
    cameras: Extract<
        Query<(Entity, &Msaa, Option<&TemporalJitter>, &GlobalTransform), With<LabCamera>>,
    >,
    reset: Extract<Option<Res<MetalFxHistoryReset>>>,
    scene: Extract<Res<SceneState>>,
) {
    let valid = request.screenshot.is_some_and(|entity| {
        shots.get(entity).is_ok_and(|shot| {
            settings
                .image
                .as_ref()
                .is_some_and(|target| shot.0.as_image() == Some(target))
        })
    });
    let camera = cameras
        .iter()
        .find(|(entity, _, _, _)| entity.to_bits() == request.view);
    commands.insert_resource(ExtractedShot {
        entity: request.screenshot,
        target_valid: valid,
        frame: frame.0,
        msaa_samples: camera.as_ref().map(|(_, msaa, _, _)| msaa.samples()),
        jitter: camera.and_then(|(_, _, jitter, _)| jitter.map(|j| j.offset.to_array())),
        reset_before: reset.as_ref().is_some_and(|r| r.is_requested()),
        camera_matrix: camera.map(|(_, _, _, transform)| transform.to_matrix().to_cols_array()),
        scene: scene.clone(),
    });
}

fn initialize_device(
    device: Res<RenderDevice>,
    info: Res<RenderAdapterInfo>,
    shared: Res<Shared>,
    mut initialized: Local<bool>,
) {
    if *initialized {
        return;
    }
    *initialized = true;
    let error_sink = shared.clone();
    device
        .wgpu_device()
        .on_uncaptured_error(Arc::new(move |error| {
            error_sink.error(format!("wgpu device error: {error}"))
        }));
    let error_sink = shared.clone();
    device
        .wgpu_device()
        .set_device_lost_callback(move |reason, message| {
            // Runner teardown can destroy the device after the terminal seal.
            // A loss before that seal always invalidates the run.
            let terminal = error_sink
                .0
                .lock()
                .expect("benchmark ledger poisoned")
                .terminal;
            if !terminal {
                error_sink.error(format!("wgpu device lost: {reason:?}: {message}"));
            }
        });
    shared.0.lock().expect("benchmark ledger poisoned").adapter = json!({"name":info.name,"backend":format!("{:?}",info.backend),"device_type":format!("{:?}",info.device_type),"driver":info.driver,"driver_info":info.driver_info});
}

#[allow(clippy::too_many_arguments, clippy::type_complexity)]
fn record_render(
    request: Res<Request>,
    settings: Res<Settings>,
    shared: Res<Shared>,
    status: Res<MetalFxEffectStatus>,
    frame: Res<MetalFxObservationFrame>,
    cameras: Query<
        (
            &MainEntity,
            &ExtractedCamera,
            &ExtractedView,
            Option<&TemporalJitter>,
        ),
        With<Camera3d>,
    >,
    shot: Res<ExtractedShot>,
    reset: Option<Res<MetalFxHistoryReset>>,
    render_targets: Query<&ViewTarget, With<Camera3d>>,
) {
    if !matches!(request.phase, Phase::Warmup | Phase::Measure) {
        return;
    }
    let expected = [settings.config.width, settings.config.height];
    let content = expected.map(|n| (n as f32 * settings.config.scale).round().max(1.) as u32);
    let observation = status
        .snapshot(request.view, request.frame)
        .last_observation;
    let extracted_camera = cameras.single().ok();
    let camera_matrix = extracted_camera.map(|(_, _, view, _)| view.world_from_view.to_matrix());
    let pose_valid = shot
        .camera_matrix
        .zip(camera_matrix)
        .is_some_and(|(main, render)| Mat4::from_cols_array(&main).abs_diff_eq(render, 0.00001));
    let clock_valid = shot.scene.kind.as_str() == request.scene
        && shot.scene.seed == settings.config.seed
        && shot.scene.generation == request.configuration_generation
        && shot.scene.load == request.load
        && shot.scene.time_seconds.is_finite()
        && (settings.config.action == Action::Stress
            || shot.scene.tick == request.tick
                && shot.scene.time_seconds == request.tick as f32 / 120.);
    let format_valid = extracted_camera
        .is_some_and(|(_, _, view, _)| view.target_format == TextureFormat::Rgba16Float);
    let render_jitter_valid = settings.config.mode != Mode::Temporal
        || extracted_camera.is_some_and(|(_, _, _, jitter)| {
            jitter.is_some_and(|j| j.offset.to_array() == jitter_offset(request.tick))
        });
    let view_target_ready = render_targets.single().is_ok();
    let window_ready = settings.image.is_some()
        || request.window_visible == Some(true)
            && request.window_occluded != Some(true)
            && (request.phase != Phase::Warmup || request.window_focused == Some(true));
    let camera_valid = view_target_ready
        && window_ready
        && pose_valid
        && clock_valid
        && format_valid
        && render_jitter_valid
        && extracted_camera.is_some_and(|(entity, camera, _, _)| {
            entity.id().to_bits() == request.view
                && camera.physical_target_size.map(|v| v.to_array()) == Some(expected)
                && camera.viewport.is_none()
                && match (&camera.target, &settings.image) {
                    (Some(NormalizedRenderTarget::Image(image)), Some(target)) => {
                        image.handle.id() == target.id()
                    }
                    (Some(NormalizedRenderTarget::Window(_)), None) => true,
                    _ => false,
                }
        });
    let history_valid = !request.reset_expected
        || shot.reset_before && reset.as_ref().is_some_and(|r| !r.is_requested());
    let aa_valid = shot.msaa_samples
        == Some(if settings.config.mode == Mode::Native {
            4
        } else {
            1
        });
    let jitter_valid =
        settings.config.mode != Mode::Temporal || shot.jitter == Some(jitter_offset(request.tick));
    let output_ready = history_valid
        && aa_valid
        && jitter_valid
        && observation.as_ref().is_some_and(|o| {
            o.frame_id == request.frame
                && o.view_id == request.view
                && o.effective_mode == settings.config.mode.metalfx()
                && o.state
                    == if matches!(settings.config.mode, Mode::Native | Mode::Bilinear) {
                        MetalFxEffectState::Disabled
                    } else {
                        MetalFxEffectState::OutputWritten
                    }
        });
    let token = FrameToken {
        epoch: request.epoch,
        tick: request.tick,
        frame: request.frame,
        view: request.view,
        output: expected,
        content,
        scale_bits: settings.config.scale.to_bits(),
        mode: settings.config.mode as u8,
        started_ns: request.started_ns,
    };
    let proof = Proof {
        frame: observation.as_ref().map_or(0, |o| o.frame_id),
        view: observation.as_ref().map_or(0, |o| o.view_id),
        output: observation.as_ref().map_or([0, 0], |o| o.output_size),
        content: observation.as_ref().map_or([0, 0], |o| o.content_size),
        scale_bits: observation
            .as_ref()
            .map_or(0, |o| o.requested_scale.to_bits()),
        mode: settings.config.mode as u8,
        output_ready,
        target_valid: request.target_valid && camera_valid && frame.0 == request.frame,
        reason: format!("effect={:?}; expected_frame={}; observed_frame={:?}; render_target_ready={view_target_ready}; window_visible={:?}; window_focused={:?}; window_occluded={:?}; camera_valid={camera_valid}; history_valid={history_valid}; aa_valid={aa_valid}; jitter_valid={jitter_valid}", observation.as_ref().map(|o| (o.state, o.reason)), request.frame, observation.as_ref().map(|o|o.frame_id), request.window_visible, request.window_focused, request.window_occluded),
    };
    let qualified = proof.output_ready
        && proof.target_valid
        && proof.output == expected
        && proof.content == content
        && proof.scale_bits == token.scale_bits;
    let mut data = shared.0.lock().expect("benchmark ledger poisoned");
    let diagnostic = json!({"scene":request.scene,"phase":format!("{:?}",request.phase),"frame":request.frame,"observed_frame":proof.frame,"view":request.view,
        "qualified":qualified,"output_ready":proof.output_ready,"target_valid":proof.target_valid,"render_target_ready":view_target_ready,
        "window":{"visible":request.window_visible,"focused":request.window_focused,"occluded":request.window_occluded},
        "expected_output":expected,"observed_output":proof.output,"expected_content":content,"observed_content":proof.content,
        "camera_pose_matches":pose_valid,"clock_matches":clock_valid,"hdr_format_matches":format_valid,"render_jitter_matches":render_jitter_valid,
        "reason":proof.reason});
    note_readiness(&mut data, &request, qualified, diagnostic);
    let metadata = json!({"scene":request.scene,"configuration_generation":request.configuration_generation,"load":request.load,"seed":settings.config.seed,"mode":settings.config.mode.as_str(),"scale":settings.config.scale,"output":expected});
    if let Some(existing) = data.metadata.get(&request.epoch) {
        if existing != &metadata && data.errors.len() < 32 {
            data.errors
                .push("configuration changed within an extracted reporting epoch".into());
        }
    } else {
        data.metadata.insert(request.epoch, metadata);
    }
    data.latest_ready = Some((request.epoch, request.frame, qualified));
    if request.phase == Phase::Measure {
        if let Some(cohort) = data.cohorts.get_mut(&request.epoch) {
            cohort.record(token, proof);
        } else {
            data.errors
                .push("measured frame without admitted cohort".into());
        }
        if let Some(entity) = request.screenshot {
            data.captured_proofs.insert(request.frame,json!({"scene":request.scene,"epoch":request.epoch,"tick":request.tick,
                "frame":request.frame,"view":request.view,"output":expected,"content":content,"qualified":qualified,
                "extracted_frame":shot.frame,"extracted_screenshot_entity":shot.entity.map(Entity::to_bits),
                "msaa_samples":shot.msaa_samples,"jitter":shot.jitter,"reset_before":shot.reset_before,"reset_pending_after":reset.as_ref().map(|r|r.is_requested()),
                "simulation_tick":shot.scene.tick,"simulation_seconds":shot.scene.time_seconds,"seed":shot.scene.seed,
                "configuration_generation":shot.scene.generation,"main_world_from_view":shot.camera_matrix,
                "render_world_from_view":camera_matrix.map(|matrix|matrix.to_cols_array()),"camera_pose_matches":pose_valid,
                "clock_matches":clock_valid,"hdr_format_matches":format_valid,"render_jitter_matches":render_jitter_valid,
                "screenshot_target_valid":shot.target_valid && shot.frame==request.frame && shot.entity==Some(entity)}));
        }
    }
}

fn note_readiness(data: &mut SharedData, request: &Request, qualified: bool, diagnostic: Value) {
    // Bounded, original observations survive both startup and later scene failures.
    let entry = data.readiness.entry(request.epoch).or_insert_with(|| json!({"observations":0,"qualified_observations":0,"first_failure":null,"last_failure":null,"latest":null}));
    entry["observations"] = json!(entry["observations"]
        .as_u64()
        .unwrap_or(0)
        .saturating_add(1));
    if qualified {
        entry["qualified_observations"] = json!(entry["qualified_observations"]
            .as_u64()
            .unwrap_or(0)
            .saturating_add(1));
    } else {
        if entry["first_failure"].is_null() {
            entry["first_failure"] = diagnostic.clone();
        }
        entry["last_failure"] = diagnostic.clone();
    }
    entry["latest"] = diagnostic;
    if !qualified && request.phase == Phase::Measure && data.errors.len() < 32 {
        data.errors.push(format!(
            "measured render lost qualification in {} at original frame {}: {}",
            request.scene,
            request.frame,
            entry["latest"]["reason"]
                .as_str()
                .unwrap_or("see readiness evidence")
        ));
    }
    while data.readiness.len() > 32 {
        data.readiness.pop_first();
    }
}

fn seal_and_poll(
    request: Res<Request>,
    settings: Res<Settings>,
    shared: Res<Shared>,
    device: Res<RenderDevice>,
    queue: Res<RenderQueue>,
) {
    let fence = {
        let mut data = shared.0.lock().expect("benchmark ledger poisoned");
        if request.phase == Phase::Opening {
            Some(data.opening.entry(request.epoch).or_default().clone())
        } else if request.phase == Phase::Closing || request.phase == Phase::Measure && request.last
        {
            Some(data.closing.entry(request.epoch).or_default().clone())
        } else {
            None
        }
    };
    if let Some(fence) = fence {
        if !fence.registered.swap(true, Ordering::AcqRel) {
            fence.registered_ns.store(settings.now(), Ordering::Release);
            let origin = settings.origin;
            queue.on_submitted_work_done(move || {
                fence.observed_ns.store(
                    origin.elapsed().as_nanos().min(u64::MAX as u128) as u64,
                    Ordering::Release,
                )
            });
        }
    }
    if let Err(error) = device.poll(PollType::Poll) {
        shared.error(format!("nonblocking device poll failed: {error}"));
    }
}

fn cohort_json(cohort: &Cohort) -> Value {
    json!({"epoch":cohort.epoch,"expected_frames":cohort.expected,"opening_callback_ns":cohort.opening_ns,"closing_callback_ns":cohort.closing_ns,
        "errors":cohort.errors,"frames":cohort.frames.iter().map(|(token,proof)|json!({"tick":token.tick,"frame":token.frame,"view":token.view,
            "output":token.output,"content":token.content,"scale":f32::from_bits(token.scale_bits),"mode":token.mode,"scene_tick_started_ns":token.started_ns,
            "proof":{"frame":proof.frame,"view":proof.view,"output":proof.output,"content":proof.content,"scale":f32::from_bits(proof.scale_bits),
                "output_ready":proof.output_ready,"target_valid":proof.target_valid,"reason":proof.reason}})).collect::<Vec<_>>()})
}

fn thermal() -> Value {
    #[cfg(target_os = "macos")]
    {
        let info = objc2_foundation::NSProcessInfo::processInfo();
        json!({"state":format!("{:?}",info.thermalState()),"scope":"public coarse OS thermal pressure; not GPU temperature or utilization"})
    }
    #[cfg(not(target_os = "macos"))]
    {
        json!({"state":"unavailable"})
    }
}
fn platform() -> Value {
    #[cfg(target_os = "macos")]
    {
        let info = objc2_foundation::NSProcessInfo::processInfo();
        json!({"os":info.operatingSystemVersionString().to_string(),"architecture":std::env::consts::ARCH,"logical_processors":info.processorCount(),"physical_memory_bytes":info.physicalMemory(),"thermal":thermal()})
    }
    #[cfg(not(target_os = "macos"))]
    {
        json!({"os":std::env::consts::OS,"architecture":std::env::consts::ARCH,"thermal":thermal()})
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn controller(outcome: Arc<Mutex<Option<MainOutcome>>>, shared: Shared) -> Controller {
        Controller {
            phase: Phase::Closing,
            epoch: 1,
            scene_index: 0,
            next_tick: 1,
            warmup_count: 20,
            last_ready_frame: 20,
            phase_started: Instant::now(),
            result: EngineResult::default(),
            completed_epochs: HashSet::new(),
            pending_load: None,
            stopping: true,
            manual_stop: true,
            stress_tick: 0,
            previous_checkpoint_ns: None,
            controls: Mutex::new(mpsc::channel().1),
            stress_started: None,
            evicted_stress_samples: 0,
            outcome,
            shared,
            window_occluded: None,
        }
    }

    #[test]
    fn consumed_app_returns_terminal_snapshot_and_clears_old_render_request() {
        let outcome = Arc::new(Mutex::new(None));
        let shared = Shared::default();
        let mut app = App::new();
        app.insert_resource(controller(outcome.clone(), shared.clone()));
        app.insert_resource(Settings {
            config: RunConfig::default(),
            image: None,
            origin: Instant::now(),
        });
        app.insert_resource(Request {
            phase: Phase::Measure,
            last: true,
            ..default()
        });
        app.add_systems(
            PreUpdate,
            |mut controller: ResMut<Controller>,
             settings: Res<Settings>,
             mut exit: MessageWriter<AppExit>| {
                finish(&mut controller, &settings, &mut exit);
            },
        );
        app.add_systems(Last, clear_terminal_request);
        app.add_systems(
            Last,
            (|request: Res<Request>| {
                assert_eq!(request.phase, Phase::Finished);
                assert!(!request.last);
                assert!(request.screenshot.is_none());
            })
            .after(clear_terminal_request),
        );
        app.run();
        assert!(!app.world().contains_resource::<Controller>());
        let retained = outcome.lock().unwrap();
        let retained = retained
            .as_ref()
            .expect("terminal result survives consumed App");
        assert!(retained.stopped);
        assert_eq!(retained.result.errors, ["benchmark cancelled"]);
        assert!(shared.0.lock().unwrap().terminal);
    }

    #[test]
    fn stress_configuration_restarts_warmup_without_carrying_previous_interval() {
        let mut controller = controller(Arc::new(Mutex::new(None)), Shared::default());
        controller.pending_load = Some(StressLoad {
            claudes: Some(64),
            ..default()
        });
        controller.previous_checkpoint_ns = Some(123);
        let mut scene = SceneState::default();
        let generation = scene.generation;
        apply_load(&mut controller, &mut scene);
        assert_eq!(controller.phase, Phase::Warmup);
        assert_eq!(controller.epoch, 2);
        assert_eq!(controller.warmup_count, 0);
        assert_eq!(controller.previous_checkpoint_ns, None);
        assert_eq!(scene.generation, generation + 1);
        assert_eq!(scene.load.claudes, Some(64));
    }

    #[test]
    fn stress_resets_on_configuration_start_and_cut_but_not_periodic_checkpoints() {
        assert!(stress_reset(true, 0., 5.));
        assert!(stress_reset(true, 0., 12.));
        assert!(stress_reset(false, 7.49, 7.51));
        assert!(!stress_reset(false, 5., 5.01));
        assert!(!stress_reset(false, 8., 8.01));
    }

    #[test]
    fn failed_stress_checkpoint_stops_with_bounded_errors() {
        let shared = Shared::default();
        let mut controller = controller(Arc::new(Mutex::new(None)), shared.clone());
        controller.stopping = false;
        controller.result.errors = vec!["previous diagnostic".into(); 31];
        let mut failed = Cohort::new(1, 1, 0);
        failed.fail("unqualified render");
        let fence = Arc::new(Fence::default());
        fence.observed_ns.store(100, Ordering::Release);
        {
            let mut data = shared.0.lock().unwrap();
            data.cohorts.insert(1, failed);
            data.closing.insert(1, fence);
        }
        collect_stress(&shared, &mut controller);
        assert!(
            controller.stopping,
            "an invalid checkpoint must stop admission"
        );
        assert!(
            controller.result.errors.len() <= 32,
            "fault output stays bounded"
        );
    }

    #[test]
    fn stale_observation_retains_original_failure_and_invalidates_measured_admission() {
        let mut data = SharedData::default();
        let mut request = Request {
            epoch: 1,
            phase: Phase::Warmup,
            frame: 192,
            scene: "materials".into(),
            ..default()
        };
        let stale = json!({"frame":192,"observed_frame":191,"render_target_ready":false,
            "window":{"visible":true,"focused":false,"occluded":true},"reason":"expected_frame=192; observed_frame=191; render_target_ready=false"});
        note_readiness(&mut data, &request, false, stale.clone());
        assert!(data.errors.is_empty(), "startup compilation may be unready");
        request.phase = Phase::Measure;
        note_readiness(&mut data, &request, false, stale.clone());
        assert_eq!(
            data.errors.len(),
            1,
            "the next main update must stop admission"
        );
        assert!(data.errors[0].contains("original frame 192"));
        request.frame = 193;
        note_readiness(
            &mut data,
            &request,
            true,
            json!({"frame":193,"observed_frame":193,"qualified":true}),
        );
        assert_eq!(data.readiness[&1]["first_failure"], stale);
        assert_eq!(data.readiness[&1]["last_failure"], stale);
        assert_eq!(data.readiness[&1]["latest"]["frame"], 193);
        assert_eq!(data.readiness[&1]["observations"], 3);
        assert_eq!(data.readiness[&1]["qualified_observations"], 1);
    }

    #[test]
    fn readiness_history_and_repeated_failure_output_are_bounded() {
        let mut data = SharedData::default();
        for epoch in 1..=100 {
            let request = Request {
                epoch,
                phase: Phase::Measure,
                frame: epoch,
                ..default()
            };
            note_readiness(
                &mut data,
                &request,
                false,
                json!({"reason":"no view target"}),
            );
        }
        assert_eq!(data.readiness.len(), 32);
        assert_eq!(data.errors.len(), 32);
        assert!(data.readiness.contains_key(&100));
        assert!(!data.readiness.contains_key(&1));
    }
}
