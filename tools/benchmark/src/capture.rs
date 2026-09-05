//! Separate image-target replay evidence. Never installed in scored runs.
use bevy::prelude::*;
use bevy::render::view::screenshot::{Screenshot, ScreenshotCaptured};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

#[derive(Component, Clone)]
pub struct CaptureTicket {
    pub scene: String,
    pub epoch: u64,
    pub tick: u32,
    pub requested_frame: u64,
    pub view: u64,
    pub output: [u32; 2],
    pub path: PathBuf,
}

#[derive(Resource, Clone, Default)]
pub struct CaptureResults(pub Arc<Mutex<Vec<Value>>>);

pub fn request(commands: &mut Commands, image: Handle<Image>, ticket: CaptureTicket) -> Entity {
    commands
        .spawn((Screenshot::image(image), ticket))
        .observe(receive)
        .id()
}

pub fn capture_ticks(frames: u32) -> Vec<u32> {
    if frames == 0 {
        return Vec::new();
    }
    let mut ticks = vec![0, frames / 3, frames * 2 / 3, frames - 1];
    // The standard scripted camera cuts at tick 900. Custom short replays still
    // retain their first, intermediate and final original frames.
    for tick in [900, 901, 908, 916] {
        if tick < frames {
            ticks.push(tick);
        }
    }
    ticks.sort_unstable();
    ticks.dedup();
    ticks
}

pub fn joined(capture: &Value, proof: &Value) -> bool {
    let frame = capture["requested_frame"].as_u64();
    capture["pixel_valid"] == true
        && frame.is_some_and(|f| f > 0)
        && frame == proof["frame"].as_u64()
        && frame == proof["extracted_frame"].as_u64()
        && capture["scene"].as_str().is_some_and(|s| !s.is_empty())
        && capture["scene"] == proof["scene"]
        && ["epoch", "view", "tick"]
            .iter()
            .all(|key| capture[key].as_u64().is_some() && capture[key] == proof[key])
        && capture["screenshot_entity"].as_u64().is_some()
        && capture["screenshot_entity"] == proof["extracted_screenshot_entity"]
        && proof["qualified"] == true
        && proof["camera_pose_matches"] == true
        && proof["clock_matches"] == true
        && proof["hdr_format_matches"] == true
        && proof["render_jitter_matches"] == true
        && proof["screenshot_target_valid"] == true
        && proof["output"] == json!([capture["pixels"]["width"], capture["pixels"]["height"]])
}

fn receive(
    event: On<ScreenshotCaptured>,
    tickets: Query<&CaptureTicket>,
    results: Res<CaptureResults>,
) {
    let Ok(ticket) = tickets.get(event.entity) else {
        return;
    };
    let mut record = json!({"scene":ticket.scene,"epoch":ticket.epoch,"tick":ticket.tick,
        "requested_frame":ticket.requested_frame,"view":ticket.view,"screenshot_entity":event.entity.to_bits(),
        "path":ticket.path,"valid":false,"pixel_valid":false});
    let outcome = (|| -> Result<Value, String> {
        let rgba = event
            .image
            .clone()
            .try_into_dynamic()
            .map_err(|e| e.to_string())?
            .to_rgba8();
        if [rgba.width(), rgba.height()] != ticket.output {
            return Err("capture dimensions changed".into());
        }
        let bytes = rgba.as_raw();
        let opaque = bytes.chunks_exact(4).filter(|p| p[3] == 255).count();
        let colors: std::collections::HashSet<_> = bytes
            .chunks_exact(4)
            .step_by(16)
            .map(|p| [p[0], p[1], p[2]])
            .collect();
        if opaque != bytes.len() / 4 || colors.len() <= 32 {
            return Err("capture is transparent or lacks scene variation".into());
        }
        let file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&ticket.path)
            .map_err(|e| e.to_string())?;
        let mut encoder = png::Encoder::new(file, rgba.width(), rgba.height());
        encoder.set_color(png::ColorType::Rgba);
        encoder.set_depth(png::BitDepth::Eight);
        let mut writer = encoder.write_header().map_err(|e| e.to_string())?;
        writer.write_image_data(bytes).map_err(|e| e.to_string())?;
        writer.finish().map_err(|e| e.to_string())?;
        let saved = std::fs::read(&ticket.path).map_err(|e| e.to_string())?;
        Ok(
            json!({"width":rgba.width(),"height":rgba.height(),"opaque_pixels":opaque,"sampled_colors":colors.len(),
            "rgba8_sha256":format!("{:x}",Sha256::digest(bytes)),"png_sha256":format!("{:x}",Sha256::digest(saved))}),
        )
    })();
    match outcome {
        Ok(proof) => {
            record["pixel_valid"] = json!(true);
            record["pixels"] = proof;
        }
        Err(error) => record["error"] = json!(error),
    }
    results
        .0
        .lock()
        .expect("capture results poisoned")
        .push(record);
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn checkpoints_are_unique_bounded_and_include_last_tick() {
        for frames in [1, 2, 120, 900, 1200] {
            let ticks = capture_ticks(frames);
            assert_eq!(ticks.last(), Some(&(frames - 1)));
            assert!(ticks.windows(2).all(|w| w[0] < w[1]));
            assert!(ticks.len() <= 8);
        }
        assert!(capture_ticks(0).is_empty());
        assert!(capture_ticks(1200).contains(&916));
    }
    #[test]
    fn replay_requires_actual_extracted_entity_and_original_identity() {
        let capture = json!({"pixel_valid":true,"requested_frame":10,"epoch":1,"view":4,"tick":0,"scene":"materials","screenshot_entity":8,"pixels":{"width":2560,"height":1440}});
        let proof = json!({"frame":10,"extracted_frame":10,"epoch":1,"view":4,"tick":0,"scene":"materials","extracted_screenshot_entity":8,"qualified":true,"screenshot_target_valid":true,"output":[2560,1440],
            "camera_pose_matches":true,"clock_matches":true,"hdr_format_matches":true,"render_jitter_matches":true,"simulation_tick":0,"simulation_seconds":0.0});
        assert!(joined(&capture, &proof));
        for key in [
            "frame",
            "extracted_frame",
            "epoch",
            "view",
            "tick",
            "extracted_screenshot_entity",
        ] {
            let mut changed = proof.clone();
            changed[key] = json!(999);
            assert!(!joined(&capture, &changed));
        }
        let mut changed = proof.clone();
        changed["screenshot_target_valid"] = json!(false);
        assert!(!joined(&capture, &changed));
        let mut changed = capture.clone();
        changed["pixel_valid"] = json!(false);
        assert!(!joined(&changed, &proof));
        for key in [
            "camera_pose_matches",
            "clock_matches",
            "hdr_format_matches",
            "render_jitter_matches",
        ] {
            let mut changed = proof.clone();
            changed[key] = Value::Null;
            assert!(
                !joined(&capture, &changed),
                "missing {key} cannot qualify a replay"
            );
        }
    }
}
