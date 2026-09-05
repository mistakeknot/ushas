//! Instrumentation added only to an archived consumer's image playtest.
use std::fs::{File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use bevy::prelude::*;
use bevy::render::view::screenshot::ScreenshotCaptured;
use bevy_metalfx::{MetalFxEffectState, MetalFxEffectStatus, MetalFxMode};
use serde_json::{json, Value};

use crate::consumer_readiness::{analyze_image, Readiness};

#[derive(Resource)]
pub struct ConsumerProbe {
    path: PathBuf,
    events: File,
    started: Instant,
    mode: MetalFxMode,
    scale: f32,
    gate: Readiness,
    logged_frame: Option<u64>,
    logged_samples: usize,
    last: Value,
    textures: [Handle<Image>; 2],
    captures: Vec<Value>,
}

impl ConsumerProbe {
    pub fn new(directory: &Path, mode: MetalFxMode, scale: f32, assets: &AssetServer) -> Self {
        Self {
            path: directory.join("effect.json"),
            events: OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(directory.join("effect.jsonl"))
                .expect("unique effect log"),
            started: Instant::now(),
            mode,
            scale,
            gate: Readiness::default(),
            logged_frame: None,
            logged_samples: 0,
            last: Value::Null,
            captures: Vec::new(),
            textures: [
                assets.load("textures/blue_marble.jpg"),
                assets.load("textures/heightmap.jpg"),
            ],
        }
    }

    pub fn advance(
        &mut self,
        status: &MetalFxEffectStatus,
        frame: u64,
        view: Option<Entity>,
        assets: &AssetServer,
        script_frame: u64,
    ) -> bool {
        let elapsed = self.started.elapsed();
        if elapsed > Duration::from_secs(75) {
            self.finish(false);
            std::process::exit(1);
        }
        let textures_ready = self
            .textures
            .iter()
            .all(|handle| assets.is_loaded_with_dependencies(handle.id()));
        let snapshot = view.map(|entity| status.snapshot(entity.to_bits(), frame));
        let observation = snapshot.as_ref().and_then(|s| s.last_observation.as_ref());
        let fresh = snapshot
            .as_ref()
            .is_some_and(|s| s.is_fresh(3, Duration::from_millis(250)));
        let output = [1600, 900];
        let content = [
            (1600.0 * self.scale).round() as u32,
            (900.0 * self.scale).round() as u32,
        ];
        let ready = textures_ready
            && fresh
            && observation.is_some_and(|o| {
                o.requested_mode == self.mode
                    && o.effective_mode == self.mode
                    && (o.requested_scale - self.scale).abs() < 0.0001
                    && o.output_size == output
                    && o.content_size == content
                    && o.state
                        == if self.mode == MetalFxMode::Disabled {
                            MetalFxEffectState::Disabled
                        } else {
                            MetalFxEffectState::OutputWritten
                        }
            });
        let observed_frame = observation.map(|o| o.frame_id);
        self.last = json!({
            "main_frame": frame, "script_frame": script_frame,
            "elapsed_seconds": elapsed.as_secs_f64(), "textures_loaded": textures_ready,
            "fresh": fresh, "ready": ready,
            "observation": observation.map(|o| json!({
                "frame_id": o.frame_id, "view_id": o.view_id,
                "requested_mode": format!("{:?}",o.requested_mode),
                "effective_mode": format!("{:?}",o.effective_mode),
                "requested_scale": o.requested_scale, "state": format!("{:?}",o.state),
                "reason": o.reason.map(|reason| format!("{reason:?}")),
                "content_size": o.content_size, "output_size": o.output_size,
                "age_frames": snapshot.as_ref().and_then(|s|s.age_frames()),
                "wall_age_ms": snapshot.as_ref().and_then(|s|s.wall_age()).map(|d|d.as_secs_f64()*1000.0),
            }))
        });
        // Keep a bounded event history; early absence is recorded once.
        if self.logged_samples < 20_000 && (self.logged_frame != observed_frame || frame == 1) {
            self.logged_frame = observed_frame;
            self.logged_samples += 1;
            writeln!(self.events, "{}", self.last).expect("write effect evidence");
            self.events.flush().expect("flush effect evidence");
        }
        self.gate.advance(elapsed, observed_frame, ready)
    }

    pub fn finish(&mut self, complete: bool) -> bool {
        let valid = complete
            && self.gate.started
            && self.captures.len() == 6
            && self.captures.iter().all(|capture| capture["valid"] == true);
        let report = json!({"schema":1,"valid":valid,"mode":format!("{:?}",self.mode),
            "scale":self.scale,"output_size":[1600,900],"warmup_seconds":3.0,
            "distinct_ready":self.gate.distinct_ready,"last":self.last,"captures":self.captures,
            "scope":"CPU effect encoding decisions plus image-target readback; no panel, GPU-time, or frame-generation claim",
            "frame_semantics":"script counter advances on distinct accepted render observations; capture request main-frame identity is logged, completion is asynchronous"});
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&self.path)
            .expect("unique effect report");
        serde_json::to_writer_pretty(&mut file, &report).expect("write effect report");
        file.flush().expect("flush effect report");
        valid
    }
}

pub fn capture(
    path: PathBuf,
    script_frame: u64,
    request_frame: u64,
) -> impl FnMut(On<ScreenshotCaptured>, ResMut<ConsumerProbe>) {
    move |event, mut probe| {
        let dynamic = match event.image.clone().try_into_dynamic() {
            Ok(image) => image.to_rgba8(),
            Err(error) => {
                probe
                    .captures
                    .push(json!({"path":path,"valid":false,"error":format!("{error:?}")}));
                return;
            }
        };
        let pixels = analyze_image(
            dynamic.width(),
            dynamic.height(),
            dynamic.pixels().map(|p| p.0),
        );
        let saved = (|| -> Result<(), Box<dyn std::error::Error>> {
            let mut file = OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&path)?;
            dynamic.write_to(&mut file, image::ImageFormat::Png)?;
            file.flush()?;
            Ok(())
        })();
        let evidence = json!({"path":path,"valid":pixels.valid && saved.is_ok(),
            "script_frame":script_frame,"request_main_frame":request_frame,
            "size":[dynamic.width(),dynamic.height()],"sampled_colors":pixels.sampled_colors,
            "pixel_count":pixels.pixel_count,"opaque_pixels":pixels.opaque_pixels,
            "visible_pixels":pixels.visible_pixels,"save_error":saved.err().map(|e|e.to_string())});
        writeln!(probe.events, "{}", json!({"capture":evidence})).expect("write capture evidence");
        probe.events.flush().expect("flush capture evidence");
        probe.captures.push(evidence);
    }
}
