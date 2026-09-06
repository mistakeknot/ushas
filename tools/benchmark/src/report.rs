use crate::config::{Action, RunConfig};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::{
    io::{BufReader, Read, Write},
    path::{Component, Path, PathBuf},
};

pub fn reserve_output(path: &Path) -> Result<PathBuf, String> {
    let parent = path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    std::fs::create_dir_all(parent).map_err(|e| format!("create output parent: {e}"))?;
    std::fs::create_dir(path).map_err(|e| format!("output must be a new directory: {e}"))?;
    path.canonicalize().map_err(|e| e.to_string())
}

pub fn sha256(path: &Path) -> Result<String, String> {
    let mut file = std::fs::File::open(path).map_err(|e| e.to_string())?;
    let mut hash = Sha256::new();
    let mut bytes = [0u8; 65536];
    loop {
        let n = file.read(&mut bytes).map_err(|e| e.to_string())?;
        if n == 0 {
            break;
        }
        hash.update(&bytes[..n]);
    }
    Ok(format!("{:x}", hash.finalize()))
}

pub fn utc_now() -> String {
    let seconds = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as libc::time_t;
    // gmtime_r writes into caller-owned storage and is safe alongside render threads.
    unsafe {
        let mut tm: libc::tm = std::mem::zeroed();
        libc::gmtime_r(&seconds, &mut tm);
        format!(
            "{:04}-{:02}-{:02}T{:02}:{:02}:{:02}Z",
            tm.tm_year + 1900,
            tm.tm_mon + 1,
            tm.tm_mday,
            tm.tm_hour,
            tm.tm_min,
            tm.tm_sec
        )
    }
}

pub fn metadata(config: &RunConfig, kind: &str, started: &str) -> Result<Value, String> {
    let mut portable = config.clone();
    portable.out = ".".into();
    Ok(json!({"schema_version":1,"kind":kind,"config":portable,
        "profile_version":config.profile_version(),
        "source_revision":env!("USHAS_BENCH_SOURCE_REVISION"),
        "source_dirty":env!("USHAS_BENCH_SOURCE_DIRTY")=="true",
        "binary_sha256":sha256(&std::env::current_exe().map_err(|e|e.to_string())?)?,
        "started_utc":started,"metric":match config.action { Action::Capture => "deterministic image replay", Action::Video => "deterministic video replay", _ => "completed-render throughput" },
        "metric_scope":metric_scope(config),
        "target_render_fps":if config.action == Action::Video {None}else{Some(120)},"valid":false,"stopped":false,"errors":[],"render_fps":null}))
}

fn metric_scope(config: &RunConfig) -> &'static str {
    if config.action == Action::Video {
        "Separate offscreen video replay with readbacks and encoder backpressure; no benchmark score. Simulation remains 120 Hz and the silent SDR Rec.709 H.264 video has fixed 60 fps timestamps."
    } else if config.action == Action::Capture {
        "Separate offscreen image replay with readbacks; no benchmark score, surface acquisition or presentation. Capture replay does not measure GPU busy time."
    } else if config.background {
        "Offscreen completed-render throughput from first cohort admission through asynchronous closing render-queue completion; includes CPU/render scheduling and queue callback dispatch, with no surface acquisition or presentation. This is not GPU busy time, displayed FPS, frame pacing or 1% lows."
    } else {
        "Window completed-render throughput from first cohort admission through asynchronous closing render-queue completion; includes CPU/render scheduling, surface acquisition and queue callback dispatch; excludes separate native presentation. This is not GPU busy time, displayed FPS, frame pacing or 1% lows."
    }
}

pub fn contained_file(root: &Path, name: &str) -> Result<PathBuf, String> {
    let path = Path::new(name);
    if path.components().any(|c| matches!(c, Component::ParentDir)) {
        return Err("artifact contains parent traversal".into());
    }
    let root = root.canonicalize().map_err(|e| e.to_string())?;
    let resolved = root.join(path).canonicalize().map_err(|e| e.to_string())?;
    if !resolved.starts_with(&root) || !resolved.is_file() {
        return Err("artifact is outside its run directory or not a file".into());
    }
    Ok(resolved)
}

fn validate_capture(config: &RunConfig, capture: &mut Value) -> Result<(), String> {
    if capture["valid"] != true || capture["pixel_valid"] != true {
        return Err("capture lacks extracted-frame and pixel proof".into());
    }
    let path = contained_file(
        &config.out,
        capture["path"].as_str().ok_or("capture path missing")?,
    )?;
    let size = std::fs::metadata(&path).map_err(|e| e.to_string())?.len();
    if size > 512 * 1024 * 1024 {
        return Err("capture exceeds size limit".into());
    }
    let decoder = png::Decoder::new(BufReader::new(
        std::fs::File::open(&path).map_err(|e| e.to_string())?,
    ));
    let mut reader = decoder.read_info().map_err(|e| e.to_string())?;
    if [reader.info().width, reader.info().height] != [config.width, config.height] {
        return Err("capture dimensions incompatible".into());
    }
    let mut bytes = vec![
        0;
        reader
            .output_buffer_size()
            .ok_or("capture has no bounded pixel buffer")?
    ];
    let info = reader.next_frame(&mut bytes).map_err(|e| e.to_string())?;
    if [info.width, info.height] != [config.width, config.height]
        || info.color_type != png::ColorType::Rgba
        || info.bit_depth != png::BitDepth::Eight
    {
        return Err("capture dimensions or format incompatible".into());
    }
    if bytes[..info.buffer_size()]
        .chunks_exact(4)
        .any(|p| p[3] != 255)
    {
        return Err("capture is not opaque".into());
    }
    let hash = sha256(&path)?;
    if capture["pixels"]["png_sha256"].as_str() != Some(&hash) {
        return Err("capture file differs from retained hash".into());
    }
    capture["path"] = json!(path.strip_prefix(&config.out).map_err(|e| e.to_string())?);
    Ok(())
}

fn validate_video(config: &RunConfig, video: Option<&Value>) -> Result<(), String> {
    let video = video.ok_or("encoded artifact missing")?;
    let frames = config.scenes().len() as u64 * 600;
    for (key, value) in [
        ("width", 2560),
        ("height", 1440),
        ("fps", 60),
        ("simulation_hz", 120),
        ("frame_count", frames),
        ("bitrate", 30_000_000),
    ] {
        if video[key].as_u64() != Some(value) {
            return Err(format!("{key} does not match the video contract"));
        }
    }
    for (key, value) in [
        ("path", "video.mp4"),
        ("codec", "h264"),
        ("color_space", "rec709"),
    ] {
        if video[key].as_str() != Some(value) {
            return Err(format!("{key} does not match the video contract"));
        }
    }
    if video["duration_seconds"].as_f64() != Some(frames as f64 / 60.) {
        return Err("duration does not match fixed frame cadence".into());
    }
    let path = contained_file(&config.out, "video.mp4")?;
    if std::fs::metadata(&path).map_err(|e| e.to_string())?.len() == 0
        || config.out.join("video.partial.mp4").exists()
    {
        return Err("movie is empty or unfinished".into());
    }
    if video["sha256"].as_str() != Some(&sha256(&path)?) {
        return Err("movie differs from retained hash".into());
    }
    Ok(())
}

pub fn seal(config: &RunConfig, mut result: EngineResult, mut envelope: Value) -> Value {
    if config.background || config.action == Action::Video {
        for (key, expected) in [
            ("render_target", "offscreen_image"),
            ("runner", "schedule_loop"),
        ] {
            if result.environment[key].as_str() != Some(expected) {
                result
                    .errors
                    .push(format!("background {key} must be {expected}"));
            }
        }
        if result.environment["live_preview"].as_bool() != Some(false) {
            result
                .errors
                .push("background live_preview must be false".into());
        }
        if !matches!(config.action, Action::Capture | Action::Video) {
            if result.environment["measured_readbacks"].as_bool() != Some(false) {
                result.errors.push(
                    "background measured_readbacks must be false outside capture replay".into(),
                );
            }
            if !result.captures.is_empty() {
                result
                    .errors
                    .push("background benchmark/stress cannot contain capture readbacks".into());
            }
        }
    }
    let expected: Vec<_> = config
        .scenes()
        .iter()
        .map(|s| s.as_str().to_string())
        .collect();
    if config.action != Action::Stress {
        let actual: Vec<_> = result.scenes.iter().map(|s| s.scene.clone()).collect();
        if expected != actual {
            result
                .errors
                .push("missing, reordered or duplicate scene result".into());
        }
        if result
            .scenes
            .iter()
            .any(|s| !s.valid || !s.errors.is_empty() || s.frames != config.frames)
        {
            result
                .errors
                .push("one or more scenes failed the requested complete cohort".into());
        }
    }
    for capture in &mut result.captures {
        if let Err(e) = validate_capture(config, capture) {
            result.errors.push(format!("capture: {e}"));
        }
    }
    if config.action == Action::Capture {
        for scene in &expected {
            let ticks: std::collections::BTreeSet<_> = result
                .captures
                .iter()
                .filter(|c| c["scene"] == *scene)
                .filter_map(|c| c["tick"].as_u64())
                .collect();
            let required: std::collections::BTreeSet<_> =
                crate::capture::capture_ticks(config.frames)
                    .into_iter()
                    .map(u64::from)
                    .collect();
            let count = result
                .captures
                .iter()
                .filter(|c| c["scene"] == *scene)
                .count();
            if ticks != required || count != required.len() {
                result
                    .errors
                    .push(format!("{scene}: missing or duplicate capture checkpoint"));
            }
        }
    }
    if config.action == Action::Video {
        for scene in &mut result.scenes {
            scene.render_fps = None;
        }
        if let Err(error) = validate_video(config, result.video.as_ref()) {
            result.errors.push(format!("video: {error}"));
        }
        if result.environment["measured_readbacks"] != true {
            result.errors.push("video readback evidence missing".into());
        }
    }
    let fps = if config.action == Action::Benchmark {
        geometric_fps(&result.scenes)
    } else {
        None
    };
    if config.action == Action::Benchmark && fps.is_none() {
        result
            .errors
            .push("completed-render score unavailable".into());
    }
    let valid = result.valid && result.errors.is_empty() && !result.stopped;
    envelope["valid"] = json!(valid);
    envelope["stopped"] = json!(result.stopped);
    envelope["errors"] = json!(result.errors);
    envelope["scenes"] = json!(result.scenes);
    envelope["captures"] = json!(result.captures);
    envelope["video"] = json!(result.video);
    envelope["stress_samples"] = json!(result.stress_samples);
    envelope["environment"] = result.environment;
    envelope["render_fps"] = json!(if valid { fps } else { None });
    envelope["finished_utc"] = json!(utc_now());
    envelope
}

pub fn write_bundle(root: &Path, value: &Value) -> Result<PathBuf, String> {
    let path = root.join("result.json");
    let temporary = root.join("result.json.pending");
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&temporary)
        .map_err(|e| e.to_string())?;
    serde_json::to_writer_pretty(&mut file, value).map_err(|e| e.to_string())?;
    file.write_all(b"\n")
        .and_then(|_| file.sync_all())
        .map_err(|e| e.to_string())?;
    if path.exists() {
        return Err("refusing to replace an existing result".into());
    }
    std::fs::rename(&temporary, &path).map_err(|e| e.to_string())?;
    let esc = |s: &str| {
        s.replace('&', "&amp;")
            .replace('<', "&lt;")
            .replace('>', "&gt;")
            .replace('"', "&quot;")
            .replace('\'', "&#39;")
    };
    let state = if value["valid"] == true {
        "Completed"
    } else {
        "Not qualified"
    };
    let rate = if value["kind"] == "video" {
        value["video"]["duration_seconds"]
            .as_f64()
            .map(|seconds| format!("{seconds:.0}-second video replay"))
            .unwrap_or_else(|| "No completed video".into())
    } else {
        value["render_fps"]
            .as_f64()
            .map(|v| format!("{v:.1} completed-render FPS"))
            .unwrap_or_else(|| "No aggregate benchmark score".into())
    };
    let mut images = String::new();
    if value["valid"] == true
        && value["kind"] == "video"
        && contained_file(root, "video.mp4").is_ok()
    {
        images.push_str("<p><a href=video.mp4>Open video</a></p><video controls style='width:100%' src=video.mp4></video>");
    }
    let captures = value["captures"].as_array().into_iter().flatten().chain(
        value["arms"]
            .as_array()
            .into_iter()
            .flatten()
            .flat_map(|a| a["captures"].as_array().into_iter().flatten()),
    );
    let mut shown = std::collections::BTreeSet::new();
    for c in captures {
        if let Some(p) = c["path"].as_str() {
            if shown.insert(p.to_string()) && contained_file(root, p).is_ok() {
                images.push_str(&format!("<figure><img loading=lazy src=\"{}\"><figcaption>{} · tick {}</figcaption></figure>",esc(p),esc(c["scene"].as_str().unwrap_or("scene")),c["tick"]));
            }
        }
    }
    let html=format!("<!doctype html><html lang=en><meta charset=utf-8><meta name=viewport content='width=device-width,initial-scale=1'><title>Ushas Bench result</title><style>body{{background:#171a1d;color:#eee7dc;font:16px system-ui;margin:4vw;max-width:1400px}}h1{{color:#e89980;font-size:48px}}pre{{white-space:pre-wrap;overflow-wrap:anywhere;background:#22272b;padding:20px;border-radius:12px}}img{{width:100%;border-radius:12px}}figure{{margin:2em 0}}figcaption{{padding:.5em 0;color:#b9b9b3}}a{{color:#e89980}}</style><h1>Ushas Bench</h1><p>{state} · {}</p><h2>{rate}</h2><p>{}</p><p>Captures come from a separate deterministic replay.</p><p><a href=result.json>Machine-readable report</a></p>{images}<details><summary>Full evidence and configuration</summary><pre>{}</pre></details></html>",esc(value["profile_version"].as_str().unwrap_or("custom")),esc(value["metric_scope"].as_str().unwrap_or("Completed rendering is not GPU-only time or displayed FPS.")),esc(&serde_json::to_string_pretty(value).map_err(|e|e.to_string())?));
    std::fs::write(root.join("index.html"), html).map_err(|e| e.to_string())?;
    Ok(path)
}

#[derive(Debug, Default, Clone, Serialize, Deserialize)]
pub struct SceneResult {
    pub scene: String,
    pub valid: bool,
    pub frames: u32,
    pub elapsed_seconds: f64,
    pub render_fps: Option<f64>,
    pub errors: Vec<String>,
}
#[derive(Debug, Default, Clone, Serialize, Deserialize)]
pub struct EngineResult {
    pub valid: bool,
    pub stopped: bool,
    pub errors: Vec<String>,
    pub scenes: Vec<SceneResult>,
    pub captures: Vec<Value>,
    pub stress_samples: Vec<Value>,
    pub environment: Value,
    pub video: Option<Value>,
}

pub fn emit(event: &str, data: Value) {
    let mut object = data.as_object().cloned().unwrap_or_default();
    object.insert("schema_version".into(), json!(1));
    object.insert("event".into(), json!(event));
    println!("{}", Value::Object(object));
}

pub fn geometric_fps(scenes: &[SceneResult]) -> Option<f64> {
    if scenes.is_empty() {
        return None;
    }
    let mut sum = 0.;
    for scene in scenes {
        let fps = scene.render_fps?;
        if !scene.valid
            || !scene.errors.is_empty()
            || scene.frames == 0
            || !scene.elapsed_seconds.is_finite()
            || scene.elapsed_seconds <= 0.
            || !fps.is_finite()
            || fps <= 0.
        {
            return None;
        }
        let measured = scene.frames as f64 / scene.elapsed_seconds;
        if (fps - measured).abs() > 1e-6 * measured.max(1.) {
            return None;
        }
        sum += fps.ln();
    }
    Some((sum / scenes.len() as f64).exp())
}

#[cfg(test)]
mod tests {
    use super::*;
    fn scene(fps: f64) -> SceneResult {
        SceneResult {
            scene: "materials".into(),
            valid: true,
            frames: 1200,
            elapsed_seconds: 1200. / fps,
            render_fps: Some(fps),
            errors: vec![],
        }
    }
    #[test]
    fn geomean_does_not_hide_individual_scenes() {
        let s = vec![scene(60.), scene(120.), scene(240.)];
        assert!((geometric_fps(&s).unwrap() - 120.).abs() < 1e-8);
        assert_eq!(s[0].render_fps, Some(60.));
    }
    #[test]
    fn invalid_or_missing_scene_cannot_be_scored() {
        assert_eq!(geometric_fps(&[]), None);
        let mut s = scene(120.);
        s.valid = false;
        assert_eq!(geometric_fps(&[s]), None);
        let mut s = scene(120.);
        s.render_fps = None;
        assert_eq!(geometric_fps(&[s]), None);
    }
    #[test]
    fn stored_rate_must_match_completed_count_and_elapsed() {
        let mut s = scene(120.);
        s.render_fps = Some(999.);
        assert_eq!(geometric_fps(&[s]), None);
    }
    #[test]
    fn envelope_rejects_missing_duplicate_or_cancelled_chapters() {
        let config = RunConfig::default();
        let scenes = vec![scene(120.)];
        let result = EngineResult {
            valid: true,
            scenes,
            ..Default::default()
        };
        let value = seal(&config, result, json!({}));
        assert_eq!(value["valid"], false);
        assert!(value["render_fps"].is_null());
        let scenes = crate::config::SceneKind::ALL
            .iter()
            .map(|kind| {
                let mut s = scene(120.);
                s.scene = kind.as_str().into();
                s
            })
            .collect();
        let result = EngineResult {
            valid: true,
            scenes,
            stopped: true,
            ..Default::default()
        };
        assert!(seal(&config, result, json!({}))["render_fps"].is_null());
    }
    #[test]
    fn output_reservation_and_artifact_containment_do_not_overwrite_or_escape() {
        let base = std::env::temp_dir().join(format!(
            "ushas-report-test-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let root = reserve_output(&base).unwrap();
        std::fs::write(root.join("kept.txt"), "retained").unwrap();
        assert!(reserve_output(&base).is_err());
        assert!(contained_file(&root, "../outside.txt").is_err());
        assert!(contained_file(&root, "kept.txt").is_ok());
        std::os::unix::fs::symlink("/etc/hosts", root.join("link.txt")).unwrap();
        assert!(contained_file(&root, "link.txt").is_err());
        assert_eq!(
            std::fs::read_to_string(root.join("kept.txt")).unwrap(),
            "retained"
        );
        std::fs::remove_dir_all(base).unwrap();
    }
    fn complete_result() -> EngineResult {
        EngineResult {
            valid: true,
            scenes: crate::config::SceneKind::ALL
                .iter()
                .map(|kind| {
                    let mut result = scene(120.);
                    result.scene = kind.as_str().into();
                    result
                })
                .collect(),
            environment: json!({"render_target":"offscreen_image", "runner":"schedule_loop",
                "live_preview":false, "measured_readbacks":false}),
            ..Default::default()
        }
    }
    #[test]
    fn video_reports_require_a_verified_movie_and_never_retain_scores() {
        let mut value = serde_json::to_value(RunConfig::default()).unwrap();
        value["action"] = json!("video");
        value["background"] = json!(true);
        let config: RunConfig = serde_json::from_value(value).expect("video configuration");
        let value = seal(&config, complete_result(), json!({}));
        assert_eq!(
            value["valid"], false,
            "a video cannot qualify without its encoded artifact"
        );
        assert!(value["render_fps"].is_null());
        assert!(value["scenes"]
            .as_array()
            .unwrap()
            .iter()
            .all(|s| s["render_fps"].is_null()));
        assert!(metric_scope(&config).contains("no benchmark score"));
    }
    #[test]
    fn background_metadata_declares_a_separate_profile_and_metric_scope() {
        let config = RunConfig {
            background: true,
            ..Default::default()
        };
        let value = metadata(&config, "benchmark", "2026-09-05T00:00:00Z").unwrap();
        assert_eq!(value["config"]["background"], true);
        assert_eq!(value["profile_version"], "claude-lab-offscreen-v1");
        let scope = value["metric_scope"].as_str().unwrap();
        for required in [
            "offscreen",
            "CPU",
            "callback",
            "no surface",
            "presentation",
            "not GPU busy",
        ] {
            assert!(
                scope.to_lowercase().contains(&required.to_lowercase()),
                "missing scope: {required}"
            );
        }
    }
    #[test]
    fn background_sealing_rejects_false_or_missing_execution_target_evidence() {
        for action in [Action::Benchmark, Action::Stress] {
            let config = RunConfig {
                background: true,
                action,
                ..Default::default()
            };
            let good = seal(&config, complete_result(), json!({}));
            assert_eq!(good["valid"], true);
            if action == Action::Stress {
                assert!(good["render_fps"].is_null());
            }
            for (key, wrong) in [
                ("render_target", json!("window_surface")),
                ("runner", json!("winit")),
                ("live_preview", json!(true)),
                ("live_preview", json!("false")),
                ("measured_readbacks", json!(true)),
                ("measured_readbacks", json!("false")),
            ] {
                let mut result = complete_result();
                result.environment[key] = wrong;
                let value = seal(&config, result, json!({}));
                assert_eq!(value["valid"], false, "{action:?}/{key}");
                assert!(value["render_fps"].is_null());
                assert!(value["errors"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .any(|error| error.as_str().unwrap().contains(key)));
            }
            for key in [
                "render_target",
                "runner",
                "live_preview",
                "measured_readbacks",
            ] {
                let mut result = complete_result();
                result.environment.as_object_mut().unwrap().remove(key);
                assert_eq!(
                    seal(&config, result, json!({}))["valid"],
                    false,
                    "missing {key}"
                );
            }
        }
    }
    #[test]
    fn background_failure_evidence_is_preserved_and_legacy_window_reports_still_seal() {
        let config = RunConfig {
            background: true,
            ..Default::default()
        };
        let mut result = complete_result();
        result.valid = false;
        result.errors.push("retained engine failure".into());
        result.environment = json!({"diagnostic":"retained"});
        let value = seal(
            &config,
            result,
            json!({"source_revision":"retained source"}),
        );
        assert_eq!(value["valid"], false);
        assert!(value["render_fps"].is_null());
        assert_eq!(value["environment"]["diagnostic"], "retained");
        assert_eq!(value["source_revision"], "retained source");
        assert!(value["errors"]
            .as_array()
            .unwrap()
            .contains(&json!("retained engine failure")));
        let mut legacy = complete_result();
        legacy.environment = Value::Null;
        assert_eq!(
            seal(&RunConfig::default(), legacy, json!({}))["valid"],
            true
        );
    }
    #[test]
    fn background_capture_replay_accepts_readback_but_never_scores() {
        let base = std::env::temp_dir().join(format!(
            "ushas-background-capture-test-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let root = reserve_output(&base).unwrap();
        let path = root.join("capture.png");
        let mut encoder = png::Encoder::new(std::fs::File::create(&path).unwrap(), 128, 128);
        encoder.set_color(png::ColorType::Rgba);
        encoder.set_depth(png::BitDepth::Eight);
        let mut writer = encoder.write_header().unwrap();
        writer
            .write_image_data(&[10, 20, 30, 255].repeat(128 * 128))
            .unwrap();
        writer.finish().unwrap();
        let mut config = RunConfig {
            action: Action::Capture,
            background: true,
            width: 128,
            height: 128,
            frames: 1,
            scene: Some(crate::config::SceneKind::Materials),
            out: root,
            ..Default::default()
        };
        let mut result = complete_result();
        result.scenes = vec![SceneResult {
            frames: 1,
            elapsed_seconds: 1. / 120.,
            ..scene(120.)
        }];
        result.environment["measured_readbacks"] = json!(true);
        result.captures = vec![
            json!({"scene":"materials", "tick":0, "valid":true, "pixel_valid":true,
            "path":path, "pixels":{"png_sha256":sha256(&path).unwrap()}}),
        ];
        let good = seal(&config, result.clone(), json!({}));
        assert_eq!(good["valid"], true);
        assert!(good["render_fps"].is_null());
        assert!(metric_scope(&config)
            .contains("Separate offscreen image replay with readbacks; no benchmark score"));
        for key in ["render_target", "runner", "live_preview"] {
            let mut wrong = result.clone();
            wrong.environment.as_object_mut().unwrap().remove(key);
            assert_eq!(
                seal(&config, wrong, json!({}))["valid"],
                false,
                "capture missing {key}"
            );
        }
        config.background = false;
        let legacy = seal(&config, result, json!({}));
        assert_eq!(legacy["valid"], true);
        assert!(legacy["render_fps"].is_null());
        std::fs::remove_dir_all(base).unwrap();
    }
    #[test]
    fn background_measurement_cannot_hide_capture_readbacks_behind_a_false_flag() {
        for action in [Action::Benchmark, Action::Stress] {
            let config = RunConfig {
                background: true,
                action,
                ..Default::default()
            };
            let mut result = complete_result();
            result.captures.push(json!({"valid":false}));
            let value = seal(&config, result, json!({}));
            assert_eq!(value["valid"], false);
            assert!(value["errors"].as_array().unwrap().contains(&json!(
                "background benchmark/stress cannot contain capture readbacks"
            )));
        }
    }
}
