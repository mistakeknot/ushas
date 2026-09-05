use crate::config::{Action, RunConfig, PROFILE_VERSION};
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
        "profile_version":if config.standard(){PROFILE_VERSION}else{"custom"},
        "source_revision":env!("USHAS_BENCH_SOURCE_REVISION"),
        "source_dirty":env!("USHAS_BENCH_SOURCE_DIRTY")=="true",
        "binary_sha256":sha256(&std::env::current_exe().map_err(|e|e.to_string())?)?,
        "started_utc":started,"metric":"completed-render throughput",
        "metric_scope":"First cohort admission through asynchronous closing render-queue completion; excludes separate native presentation. Not GPU-only time, displayed FPS, or 1% lows.",
        "target_render_fps":120,"valid":false,"stopped":false,"errors":[],"render_fps":null}))
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

pub fn seal(config: &RunConfig, mut result: EngineResult, mut envelope: Value) -> Value {
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
    let rate = value["render_fps"]
        .as_f64()
        .map(|v| format!("{v:.1} completed-render FPS"))
        .unwrap_or_else(|| "No aggregate benchmark score".into());
    let mut images = String::new();
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
    let html=format!("<!doctype html><html lang=en><meta charset=utf-8><meta name=viewport content='width=device-width,initial-scale=1'><title>Ushas Bench result</title><style>body{{background:#171a1d;color:#eee7dc;font:16px system-ui;margin:4vw;max-width:1400px}}h1{{color:#e89980;font-size:48px}}pre{{white-space:pre-wrap;overflow-wrap:anywhere;background:#22272b;padding:20px;border-radius:12px}}img{{width:100%;border-radius:12px}}figure{{margin:2em 0}}figcaption{{padding:.5em 0;color:#b9b9b3}}a{{color:#e89980}}</style><h1>Ushas Bench</h1><p>{state} · {}</p><h2>{rate}</h2><p>Completed-render throughput covers the measured rendering cohort through its closing queue callback. It is not GPU-only time or displayed FPS. Captures come from a separate deterministic replay.</p><p><a href=result.json>Machine-readable report</a></p>{images}<details><summary>Full evidence and configuration</summary><pre>{}</pre></details></html>",esc(value["profile_version"].as_str().unwrap_or("custom")),esc(&serde_json::to_string_pretty(value).map_err(|e|e.to_string())?));
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
}
