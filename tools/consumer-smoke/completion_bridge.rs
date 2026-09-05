//! Consumer-only adapter for the byte-identical archived smoke completion module.
use bevy::prelude::*;
use bevy::render::view::screenshot::Screenshot;
use bevy_metalfx::MetalFxMode;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use crate::completion::{CompletionPlugin, CompletionReport, CompletionRequest, Epoch, Phase};
use crate::consumer_probe::{ConsumerProbe, capture};
use crate::consumer_readiness::{CompletionAction, CompletionGate};

#[derive(Resource)]
pub struct CompletionTarget(pub Handle<Image>);

#[derive(Resource)]
pub struct ConsumerCompletion {
    origin: Instant,
    gate: CompletionGate,
    target: Handle<Image>,
    warmup: PathBuf,
    final_image: PathBuf,
}

pub fn enabled() -> bool {
    std::env::var("SW_CONSUMER_COMPLETED").as_deref() == Ok("1")
}

pub fn install(app: &mut App, mode: MetalFxMode, scale: f32, directory: &str, arm: &str) {
    // PostStartup retarget_offscreen fills this reserved handle with its existing
    // 1600x900 image. The completion observer and camera then share one identity.
    let target = app
        .world_mut()
        .resource_mut::<Assets<Image>>()
        .add(Image::default());
    app.insert_resource(CompletionTarget(target.clone()));
    app.insert_resource(ConsumerCompletion {
        origin: Instant::now(),
        gate: CompletionGate::default(),
        target: target.clone(),
        warmup: PathBuf::from(directory).join(format!("{arm}_warmup.png")),
        final_image: PathBuf::from(directory).join(format!("{arm}_final.png")),
    });
    app.add_plugins(CompletionPlugin {
        target,
        mode,
        scale,
        output_size: [1600, 900],
        timeout: Duration::from_secs(5),
    });
}

impl ConsumerCompletion {
    pub fn tick(
        &mut self,
        commands: &mut Commands,
        probe: &mut ConsumerProbe,
        request: &mut CompletionRequest,
        report: &CompletionReport,
        ready: bool,
        main_frame: u64,
    ) -> Option<bool> {
        let adopted = report.current_epoch().map(|epoch| epoch.id);
        let action = self.gate.advance(
            self.origin.elapsed(),
            adopted,
            ready,
            probe.capture_valid(&self.warmup),
            probe.capture_valid(&self.final_image),
        );
        match action {
            CompletionAction::Wait => {}
            CompletionAction::CaptureWarmup | CompletionAction::CaptureFinal => {
                probe.completion = Some(report.snapshot());
                let path = if action == CompletionAction::CaptureWarmup {
                    &self.warmup
                } else {
                    &self.final_image
                };
                commands
                    .spawn(Screenshot::image(self.target.clone()))
                    .observe(capture(path.clone(), 0, main_frame, adopted));
            }
            CompletionAction::BeginMeasure => {
                request.0 = Epoch {
                    id: 2,
                    phase: Phase::Measure,
                }
            }
            CompletionAction::BeginDrain => {
                request.0 = Epoch {
                    id: 3,
                    phase: Phase::Drain,
                }
            }
            CompletionAction::Finish => {
                probe.completion = Some(report.snapshot());
                return Some(probe.finish(true));
            }
        }
        None
    }
}
