//! Optional serial, offscreen completed-render measurement. Not GPU busy time.
//!
//! Current Spatial/Temporal MetalFX work is a raw-encoded wgpu command buffer
//! added to the same RenderContext; later main reconstruction reads its output.
//! The fence below follows both that submitted dependency chain and Bevy's final
//! screenshot/readback submission. It does not measure an unrelated Metal queue
//! or the experimental frame-generation presentation path (which is rejected).
//! See wgpu 29.0.4 Queue::on_submitted_work_done and Bevy 0.19 render_system.

// BEGIN PURE CONTRACT
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Phase {
    Warmup,
    Measure,
    Drain,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Epoch {
    pub id: u64,
    pub phase: Phase,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Scope {
    pub view_id: u64,
    pub target: String,
    pub mode: String,
    pub scale_bits: u32,
    pub content_size: [u32; 2],
    pub output_size: [u32; 2],
}

#[derive(Clone, Debug)]
pub struct Proof {
    pub frame_id: u64,
    pub scope: Scope,
    pub ready: bool,
    pub state: String,
    pub reason: Option<String>,
}

#[derive(Clone, Debug)]
struct FrameRecord {
    epoch: Epoch,
    frame_id: u64,
    scope: Option<Scope>,
    admitted_ns: u64,
    callback_ns: Option<u64>,
    proof: Option<Proof>,
    qualified: bool,
    failure: Option<String>,
}

#[derive(Clone, Debug)]
struct Boundary {
    epoch: Epoch,
    drain_started_ns: u64,
    drain_completed_ns: u64,
}

#[derive(Default, Debug)]
struct Ledger {
    epoch: Option<Epoch>,
    in_flight: Option<FrameRecord>,
    records: Vec<FrameRecord>,
    boundaries: Vec<Boundary>,
    errors: Vec<String>,
    last_frame: Option<u64>,
}

#[derive(Debug, PartialEq)]
struct Summary {
    frames: usize,
    qualified: usize,
    seconds: Option<f64>,
    frames_per_second: Option<f64>,
    valid: bool,
}

fn accept_drain(poll: Result<(), String>, ready: bool, callback_ns: u64) -> Result<u64, String> {
    poll?;
    if !ready {
        return Err("completion callback missing after bounded queue drain".into());
    }
    Ok(callback_ns)
}

impl Ledger {
    fn begin_epoch(&mut self, epoch: Epoch, start: u64, completed: u64) -> Result<(), String> {
        if self.in_flight.is_some() {
            return Err("epoch boundary has an unfinished frame".into());
        }
        if epoch.id == 0 || self.epoch.is_some_and(|old| epoch.id <= old.id) {
            return Err("epoch identity did not advance".into());
        }
        if completed < start
            || self
                .records
                .last()
                .and_then(|r| r.callback_ns)
                .is_some_and(|at| start < at)
        {
            return Err("epoch drain timestamps did not advance".into());
        }
        if self.boundaries.len() >= 64 {
            return Err("epoch retention limit reached".into());
        }
        self.epoch = Some(epoch);
        self.boundaries.push(Boundary {
            epoch,
            drain_started_ns: start,
            drain_completed_ns: completed,
        });
        Ok(())
    }

    fn admit(&mut self, frame_id: u64, scope: Option<Scope>, at: u64) -> Result<(), String> {
        let epoch = self.epoch.ok_or("no drained epoch")?;
        if !self.errors.is_empty() || self.in_flight.is_some() {
            return Err("new frame admission while failed or in flight".into());
        }
        if self.last_frame.is_some_and(|previous| frame_id <= previous) {
            return Err("render frame identity did not advance".into());
        }
        if self
            .boundaries
            .last()
            .is_some_and(|b| at < b.drain_completed_ns)
            || self
                .records
                .last()
                .and_then(|r| r.callback_ns)
                .is_some_and(|done| at < done)
        {
            return Err("frame admitted before prior work drained".into());
        }
        if self.records.len() >= 65_536 {
            return Err("frame retention limit reached".into());
        }
        self.last_frame = Some(frame_id);
        self.in_flight = Some(FrameRecord {
            epoch,
            frame_id,
            scope,
            admitted_ns: at,
            callback_ns: None,
            proof: None,
            qualified: false,
            failure: None,
        });
        Ok(())
    }

    fn finish(
        &mut self,
        epoch: u64,
        frame: u64,
        at: u64,
        proof: Option<Proof>,
    ) -> Result<(), String> {
        let current = self
            .in_flight
            .as_ref()
            .ok_or("callback without an admitted frame")?;
        if current.epoch.id != epoch || current.frame_id != frame {
            return Err("callback identity does not match the admitted frame".into());
        }
        if at < current.admitted_ns {
            return Err("callback predates frame admission".into());
        }
        let mut current = self.in_flight.take().expect("checked above");
        current.callback_ns = Some(at);
        current.qualified = current
            .scope
            .as_ref()
            .zip(proof.as_ref())
            .is_some_and(|(scope, p)| p.frame_id == frame && p.ready && scope == &p.scope);
        if !current.qualified {
            current.failure = Some(
                if current.scope.is_none() || proof.is_none() {
                    "NoRender"
                } else {
                    "EffectProofMismatch"
                }
                .into(),
            );
        }
        current.proof = proof;
        self.records.push(current);
        Ok(())
    }

    fn fail(&mut self, message: String) {
        if self.errors.len() < 16 {
            self.errors.push(message);
        }
    }

    fn summary(&self, epoch: u64) -> Summary {
        let frames: Vec<_> = self
            .records
            .iter()
            .filter(|r| r.epoch.id == epoch)
            .collect();
        let qualified = frames.iter().filter(|r| r.qualified).count();
        let seconds = frames
            .first()
            .zip(frames.last())
            .and_then(|(first, last)| {
                last.callback_ns?
                    .checked_sub(first.admitted_ns)
                    .filter(|ns| *ns > 0)
            })
            .map(|ns| ns as f64 / 1_000_000_000.0);
        let valid = !frames.is_empty()
            && qualified == frames.len()
            && seconds.is_some()
            && self.errors.is_empty()
            && self.epoch.is_some_and(|current| current.id > epoch)
            && self.in_flight.as_ref().is_none_or(|r| r.epoch.id != epoch);
        Summary {
            frames: frames.len(),
            qualified,
            seconds,
            frames_per_second: if valid {
                seconds.map(|s| qualified as f64 / s)
            } else {
                None
            },
            valid,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scope() -> Scope {
        Scope {
            view_id: 7,
            target: "image:1".into(),
            mode: "Temporal".into(),
            scale_bits: 0.5_f32.to_bits(),
            content_size: [640, 360],
            output_size: [1280, 720],
        }
    }
    fn proof(frame_id: u64) -> Proof {
        Proof {
            frame_id,
            scope: scope(),
            ready: true,
            state: "OutputWritten".into(),
            reason: None,
        }
    }
    fn measuring() -> Ledger {
        let mut ledger = Ledger::default();
        ledger
            .begin_epoch(
                Epoch {
                    id: 2,
                    phase: Phase::Measure,
                },
                10,
                20,
            )
            .unwrap();
        ledger
    }

    #[test]
    fn measurement_starts_after_a_drained_boundary_and_includes_interframe_gaps() {
        let mut ledger = Ledger::default();
        ledger
            .begin_epoch(
                Epoch {
                    id: 1,
                    phase: Phase::Warmup,
                },
                0,
                5,
            )
            .unwrap();
        ledger.admit(1, Some(scope()), 10).unwrap();
        assert!(ledger
            .begin_epoch(
                Epoch {
                    id: 2,
                    phase: Phase::Measure
                },
                11,
                12
            )
            .is_err());
        ledger.finish(1, 1, 100, Some(proof(1))).unwrap();
        ledger
            .begin_epoch(
                Epoch {
                    id: 2,
                    phase: Phase::Measure,
                },
                110,
                120,
            )
            .unwrap();
        ledger.admit(2, Some(scope()), 200_000_000).unwrap();
        ledger.finish(2, 2, 300_000_000, Some(proof(2))).unwrap();
        ledger.admit(3, Some(scope()), 800_000_000).unwrap();
        ledger.finish(2, 3, 1_200_000_000, Some(proof(3))).unwrap();
        ledger
            .begin_epoch(
                Epoch {
                    id: 3,
                    phase: Phase::Drain,
                },
                1_200_000_001,
                1_200_000_002,
            )
            .unwrap();
        assert_eq!(
            ledger.summary(2),
            Summary {
                frames: 2,
                qualified: 2,
                seconds: Some(1.0),
                frames_per_second: Some(2.0),
                valid: true
            }
        );
    }

    #[test]
    fn missing_callback_never_becomes_a_zero_cost_or_completed_frame() {
        let mut ledger = measuring();
        ledger.admit(10, Some(scope()), 30).unwrap();
        assert!(ledger.admit(11, Some(scope()), 40).is_err());
        ledger.fail("completion timeout".into());
        let summary = ledger.summary(2);
        assert!(!summary.valid);
        assert_eq!(summary.frames, 0);
        assert_eq!(summary.frames_per_second, None);
        assert!(ledger.in_flight.is_some());
    }

    #[test]
    fn duplicate_and_wrong_epoch_callbacks_cannot_complete_another_frame() {
        let mut ledger = measuring();
        ledger.admit(10, Some(scope()), 30).unwrap();
        assert!(ledger.finish(1, 10, 40, Some(proof(10))).is_err());
        assert!(ledger.finish(2, 11, 40, Some(proof(11))).is_err());
        ledger.finish(2, 10, 40, Some(proof(10))).unwrap();
        assert!(ledger.finish(2, 10, 41, Some(proof(10))).is_err());
        assert!(ledger.admit(10, Some(scope()), 42).is_err());
        assert_eq!(ledger.records.len(), 1);
    }

    #[test]
    fn no_render_is_an_unqualified_completion_not_rendered_zero_time() {
        let mut ledger = measuring();
        ledger.admit(1, None, 30).unwrap();
        ledger.finish(2, 1, 40, None).unwrap();
        assert_eq!(ledger.records[0].failure.as_deref(), Some("NoRender"));
        assert_eq!(ledger.summary(2).frames_per_second, None);
        assert_eq!(ledger.summary(2).qualified, 0);
    }

    #[test]
    fn stale_or_mismatched_effect_proof_invalidates_the_measurement() {
        for mismatch in 0..4 {
            let mut ledger = measuring();
            ledger.admit(1, Some(scope()), 30).unwrap();
            let mut p = proof(1);
            match mismatch {
                0 => p.frame_id = 0,
                1 => p.scope.view_id += 1,
                2 => p.scope.content_size = [1280, 720],
                _ => p.ready = false,
            }
            ledger.finish(2, 1, 40, Some(p)).unwrap();
            assert!(!ledger.summary(2).valid);
            assert_eq!(ledger.summary(2).frames_per_second, None);
        }
    }

    #[test]
    fn boundaries_and_callback_times_must_advance() {
        let mut ledger = measuring();
        assert!(ledger
            .begin_epoch(
                Epoch {
                    id: 2,
                    phase: Phase::Drain
                },
                21,
                22
            )
            .is_err());
        assert!(ledger
            .begin_epoch(
                Epoch {
                    id: 3,
                    phase: Phase::Drain
                },
                25,
                24
            )
            .is_err());
        assert!(ledger.admit(1, Some(scope()), 19).is_err());
        ledger.admit(1, Some(scope()), 30).unwrap();
        assert!(ledger.finish(2, 1, 29, Some(proof(1))).is_err());
    }

    #[test]
    fn measured_rate_is_withheld_until_the_final_boundary_is_drained() {
        let mut ledger = measuring();
        ledger.admit(1, Some(scope()), 30).unwrap();
        ledger.finish(2, 1, 40, Some(proof(1))).unwrap();
        assert_eq!(ledger.summary(2).frames_per_second, None);
        ledger
            .begin_epoch(
                Epoch {
                    id: 3,
                    phase: Phase::Drain,
                },
                41,
                42,
            )
            .unwrap();
        assert!(ledger.summary(2).valid);
        assert!(ledger.summary(2).frames_per_second.is_some());
    }

    #[test]
    fn poll_success_requires_callback_and_timeout_cannot_be_overridden_by_a_callback() {
        assert!(accept_drain(Ok(()), false, 0).is_err());
        assert!(accept_drain(Err("timeout".into()), true, 20).is_err());
        assert_eq!(accept_drain(Ok(()), true, 20), Ok(20));
    }
}
// END PURE CONTRACT

use bevy::camera::NormalizedRenderTarget;
use bevy::prelude::*;
use bevy::render::camera::ExtractedCamera;
use bevy::render::extract_resource::{ExtractResource, ExtractResourcePlugin};
use bevy::render::pipelined_rendering::PipelinedRenderingPlugin;
use bevy::render::render_resource::PollType;
use bevy::render::renderer::{RenderDevice, RenderQueue};
use bevy::render::sync_world::MainEntity;
use bevy::render::{Render, RenderApp, RenderSystems};
use bevy_metalfx::{MetalFxEffectState, MetalFxEffectStatus, MetalFxMode, MetalFxObservationFrame};
use serde_json::{json, Value};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// Extracted with the frame. Advance the epoch when entering measurement and
/// again before requesting a final screenshot; never mutate an existing epoch.
#[derive(Resource, Clone, Copy, ExtractResource)]
pub struct CompletionRequest(pub Epoch);

impl Default for CompletionRequest {
    fn default() -> Self {
        Self(Epoch {
            id: 1,
            phase: Phase::Warmup,
        })
    }
}

/// One fixed image target and arm for the entire run. The fixture must disable
/// `PipelinedRenderingPlugin` and any other producer using this render queue.
pub struct CompletionPlugin {
    pub target: Handle<Image>,
    pub mode: MetalFxMode,
    pub scale: f32,
    pub output_size: [u32; 2],
    pub timeout: Duration,
}

#[derive(Resource)]
struct Settings {
    target: Handle<Image>,
    mode: MetalFxMode,
    scale: f32,
    output_size: [u32; 2],
    timeout: Duration,
}

/// Shared immutable snapshots; callbacks never lock or mutate this ledger.
#[derive(Resource, Clone)]
pub struct CompletionReport {
    origin: Instant,
    timeout: Duration,
    ledger: Arc<Mutex<Ledger>>,
}

impl CompletionReport {
    /// Last epoch adopted only after its boundary queue drain succeeded.
    pub fn current_epoch(&self) -> Option<Epoch> {
        self.ledger
            .lock()
            .expect("completion ledger poisoned")
            .epoch
    }

    fn now_ns(&self) -> u64 {
        self.origin.elapsed().as_nanos().min(u64::MAX as u128) as u64
    }

    /// Includes all retained epochs and records. Invalid or empty epochs have
    /// a null throughput; queue completion alone is not a rendered-view proof.
    pub fn snapshot(&self) -> Value {
        let ledger = self.ledger.lock().expect("completion ledger poisoned");
        json!({
            "scope":"serial completed-render cadence; includes CPU scheduling, render preparation, callback delivery and polling; NOT normal pipelined app FPS, GPU busy cost, GPU hardware latency, or presentation",
            "max_render_frames_in_flight":1,
            "timeout_seconds":self.timeout.as_secs_f64(),
            "clock":"CPU Instant offsets in milliseconds; callback-observed GPU submission completion",
            "boundary":"after full Bevy render_system, including its final screenshot/readback submission",
            "errors":ledger.errors,
            "in_flight":ledger.in_flight.as_ref().map(record_json),
            "epochs":ledger.boundaries.iter().map(|b| {
                let summary = ledger.summary(b.epoch.id);
                json!({"epoch":b.epoch.id,"phase":format!("{:?}",b.epoch.phase),
                    "drain_started_ms":b.drain_started_ns as f64 / 1e6,
                    "drain_completed_ms":b.drain_completed_ns as f64 / 1e6,
                    "completed_frame_fences":summary.frames,"qualified_render_frames":summary.qualified,
                    "elapsed_seconds":summary.seconds,"completed_render_fps":summary.frames_per_second,
                    "valid":summary.valid})
            }).collect::<Vec<_>>(),
            "frames":ledger.records.iter().map(record_json).collect::<Vec<_>>()
        })
    }
}

fn scope_json(scope: &Scope) -> Value {
    json!({"view_id":scope.view_id,"image_target":scope.target,"mode":scope.mode,
        "scale":f32::from_bits(scope.scale_bits),"content_size":scope.content_size,
        "output_size":scope.output_size})
}

fn record_json(record: &FrameRecord) -> Value {
    json!({"epoch":record.epoch.id,"phase":format!("{:?}",record.epoch.phase),
        "frame_id":record.frame_id,"scope":record.scope.as_ref().map(scope_json),
        "admitted_ms":record.admitted_ns as f64 / 1e6,
        "callback_observed_ms":record.callback_ns.map(|ns| ns as f64 / 1e6),
        "qualified":record.qualified,"failure":record.failure,
        "effect":record.proof.as_ref().map(|p| json!({"frame_id":p.frame_id,
            "scope":scope_json(&p.scope),"ready":p.ready,"state":p.state,"reason":p.reason}))})
}

impl Plugin for CompletionPlugin {
    fn build(&self, app: &mut App) {
        assert!(
            !app.is_plugin_added::<PipelinedRenderingPlugin>(),
            "completion mode requires PipelinedRenderingPlugin disabled"
        );
        assert!(
            self.mode != MetalFxMode::FrameInterpolation,
            "completion mode is offscreen reconstruction only"
        );
        assert!(self.scale.is_finite() && self.scale > 0.0 && self.scale <= 1.0);
        assert!(!self.output_size.contains(&0));
        assert!(
            self.timeout > Duration::ZERO && self.timeout <= Duration::from_secs(10),
            "completion timeout must be finite and at most ten seconds"
        );
        let report = CompletionReport {
            origin: Instant::now(),
            timeout: self.timeout,
            ledger: Arc::default(),
        };
        let request = CompletionRequest::default();
        app.insert_resource(report.clone()).insert_resource(request);
        let render = app
            .get_sub_app_mut(RenderApp)
            .expect("completion mode needs RenderApp");
        render
            .insert_resource(report)
            .insert_resource(request)
            .insert_resource(Settings {
                target: self.target.clone(),
                mode: self.mode,
                scale: self.scale,
                output_size: self.output_size,
                timeout: self.timeout,
            });
        render.add_systems(
            Render,
            (
                admit_frame
                    .after(RenderSystems::ExtractCommands)
                    .before(RenderSystems::PrepareAssets),
                // RenderGraphSystems::Finish is too early: render_system then
                // submits screenshot/readback commands. Wait after the entire set.
                complete_frame
                    .after(RenderSystems::Render)
                    .before(RenderSystems::Cleanup),
            ),
        );
        app.add_plugins(ExtractResourcePlugin::<CompletionRequest>::default());
    }
}

#[derive(Default)]
struct Callback {
    at_ns: AtomicU64,
    ready: AtomicBool,
}

/// A finite wait after the latest submission. No new GPU command buffer is
/// created. Callback receipt includes the time until this poll dispatches it.
fn drain(
    device: &RenderDevice,
    queue: &RenderQueue,
    report: &CompletionReport,
    timeout: Duration,
) -> Result<u64, String> {
    let callback = Arc::new(Callback::default());
    let result = callback.clone();
    let clock = report.clone();
    queue.on_submitted_work_done(move || {
        result.at_ns.store(clock.now_ns(), Ordering::Relaxed);
        result.ready.store(true, Ordering::Release);
    });
    let poll = device
        .poll(PollType::Wait {
            submission_index: None,
            timeout: Some(timeout),
        })
        .map(|_| ())
        .map_err(|error| format!("GPU completion wait failed: {error}"));
    accept_drain(
        poll,
        callback.ready.load(Ordering::Acquire),
        callback.at_ns.load(Ordering::Relaxed),
    )
}

/// Fail closed rather than return to another render submission with work still
/// in flight. The wrapper retains this JSON in stderr and the nonzero exit.
/// This does not bypass normal shutdown with process::exit or extend timeouts.
fn fatal(report: &CompletionReport, message: String) -> ! {
    report
        .ledger
        .lock()
        .expect("completion ledger poisoned")
        .fail(message.clone());
    eprintln!("USHAS_COMPLETION_FAILURE {}", report.snapshot());
    panic!("serial completion measurement failed: {message}");
}

fn admit_frame(
    request: Res<CompletionRequest>,
    report: Res<CompletionReport>,
    settings: Res<Settings>,
    frame: Res<MetalFxObservationFrame>,
    device: Res<RenderDevice>,
    queue: Res<RenderQueue>,
    cameras: Query<(&MainEntity, &ExtractedCamera)>,
) {
    let needs_boundary = report
        .ledger
        .lock()
        .expect("completion ledger poisoned")
        .epoch
        != Some(request.0);
    if needs_boundary {
        let start = report.now_ns();
        let completed = drain(&device, &queue, &report, settings.timeout)
            .unwrap_or_else(|error| fatal(&report, error));
        let result = report
            .ledger
            .lock()
            .expect("completion ledger poisoned")
            .begin_epoch(request.0, start, completed);
        if let Err(error) = result {
            fatal(&report, error);
        }
    }
    let scope = cameras.single().ok().and_then(|(entity, camera)| {
        let Some(NormalizedRenderTarget::Image(target)) = &camera.target else {
            return None;
        };
        (target.handle.id() == settings.target.id()
            && camera.physical_target_size.map(|s| s.to_array()) == Some(settings.output_size)
            && camera.viewport.is_none())
        .then(|| Scope {
            view_id: entity.id().to_bits(),
            target: format!("{:?}", target.handle.id()),
            mode: format!("{:?}", settings.mode),
            scale_bits: settings.scale.to_bits(),
            content_size: settings
                .output_size
                .map(|n| (n as f32 * settings.scale).round().max(1.0) as u32),
            output_size: settings.output_size,
        })
    });
    let result = report
        .ledger
        .lock()
        .expect("completion ledger poisoned")
        .admit(frame.0, scope, report.now_ns());
    if let Err(error) = result {
        fatal(&report, error);
    }
}

fn complete_frame(
    report: Res<CompletionReport>,
    settings: Res<Settings>,
    device: Res<RenderDevice>,
    queue: Res<RenderQueue>,
    status: Res<MetalFxEffectStatus>,
) {
    let current = report
        .ledger
        .lock()
        .expect("completion ledger poisoned")
        .in_flight
        .clone();
    let current =
        current.unwrap_or_else(|| fatal(&report, "render completion without admission".into()));
    let proof = current.scope.as_ref().and_then(|scope| {
        let observation = status
            .snapshot(scope.view_id, current.frame_id)
            .last_observation?;
        Some(Proof {
            frame_id: observation.frame_id,
            scope: Scope {
                view_id: observation.view_id,
                target: scope.target.clone(),
                mode: format!("{:?}", observation.requested_mode),
                scale_bits: observation.requested_scale.to_bits(),
                content_size: observation.content_size,
                output_size: observation.output_size,
            },
            ready: observation.effective_mode == settings.mode
                && observation.state
                    == if settings.mode == MetalFxMode::Disabled {
                        MetalFxEffectState::Disabled
                    } else {
                        MetalFxEffectState::OutputWritten
                    },
            state: format!("{:?}", observation.state),
            reason: observation.reason.map(|r| format!("{r:?}")),
        })
    });
    let completed = drain(&device, &queue, &report, settings.timeout)
        .unwrap_or_else(|error| fatal(&report, error));
    let result = report
        .ledger
        .lock()
        .expect("completion ledger poisoned")
        .finish(current.epoch.id, current.frame_id, completed, proof);
    if let Err(error) = result {
        fatal(&report, error);
    }
}
