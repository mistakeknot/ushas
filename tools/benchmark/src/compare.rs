use crate::{
    config::{Action, Mode, RunConfig},
    control, report,
};
use serde_json::{json, Value};
use std::os::unix::process::CommandExt;
use std::{
    io::{BufRead, BufReader, Write},
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::mpsc,
    time::{Duration, Instant},
};

const ARMS: [(&str, Mode, f32); 6] = [
    ("Native MSAA4", Mode::Native, 1.),
    ("Temporal native", Mode::Temporal, 1.),
    ("Temporal two-thirds", Mode::Temporal, 2. / 3.),
    ("Temporal half", Mode::Temporal, 0.5),
    ("Spatial half", Mode::Spatial, 0.5),
    ("Bilinear half", Mode::Bilinear, 0.5),
];

fn order(round: usize) -> [usize; 6] {
    [
        [0, 1, 2, 3, 4, 5],
        [5, 4, 3, 2, 1, 0],
        [1, 2, 3, 4, 5, 0],
        [0, 5, 4, 3, 2, 1],
    ][round % 4]
}

fn paired_summary(baseline: &[f64], candidate: &[f64]) -> Value {
    if baseline.len() != candidate.len()
        || baseline.is_empty()
        || baseline
            .iter()
            .chain(candidate)
            .any(|v| !v.is_finite() || *v <= 0.)
    {
        return json!({"valid":false,"reason":"missing or invalid matched rounds"});
    }
    let logs: Vec<_> = baseline
        .iter()
        .zip(candidate)
        .map(|(b, c)| (b / c).ln())
        .collect();
    let reduction = 1. - (logs.iter().sum::<f64>() / logs.len() as f64).exp();
    if logs.len() != 4 {
        return json!({"valid":true,"qualified":false,"paired_rounds":logs.len(),"time_reduction":reduction,"ci95":null,"performance_gate":"quick comparison; four rounds required"});
    }
    // Exact enumeration of the 4^4 paired bootstrap, with deterministic quantiles.
    let mut samples = Vec::with_capacity(256);
    for i in 0..256 {
        let mut digits = i;
        let mut mean = 0.;
        for _ in 0..4 {
            mean += logs[digits % 4];
            digits /= 4;
        }
        samples.push(1. - (mean / 4.).exp());
    }
    samples.sort_by(f64::total_cmp);
    let lo = samples[6];
    let hi = samples[249];
    json!({"valid":true,"qualified":true,"paired_rounds":4,"time_reduction":reduction,"ci95":[lo,hi],
        "uncertainty_method":"exact 256-resample paired bootstrap over rounds; percentile95 interval",
        "practical_threshold":0.08,"performance_gate":if lo>0.08{"benefit exceeds8% with paired uncertainty"}else if hi<0.{"slower"}else{"no demonstrated practical benefit"},
        "quality_gate":"Separate retained image pairs require visual review; timing alone does not qualify image quality."})
}

// These helpers are tested with real process groups and missing-round fixtures.
fn sweep_group(process: &mut std::process::Child) -> Result<(), String> {
    // The leader can exit while a descendant still owns the stdout pipe.
    // Always sweep the original process group before waiting on its reader.
    let result = unsafe { libc::kill(-(process.id() as i32), libc::SIGKILL) };
    if result != 0 {
        let error = std::io::Error::last_os_error();
        if error.raw_os_error() != Some(libc::ESRCH) {
            return Err(format!("process-group cleanup failed: {error}"));
        }
    }
    process
        .wait()
        .map(|_| ())
        .map_err(|e| format!("child reap failed: {e}"))
}
fn comparison_summary(
    baseline: &[f64],
    candidate: &[f64],
    rounds: usize,
    valid: bool,
    standard: bool,
) -> Value {
    if !valid || baseline.len() != rounds || candidate.len() != rounds {
        return json!({"valid":false,"qualified":false,"paired_rounds":0,
            "time_reduction":null,"ci95":null,
            "performance_gate":"Comparison incomplete; paired result unavailable"});
    }
    let mut summary = paired_summary(baseline, candidate);
    if !standard {
        summary["qualified"] = json!(false);
        summary["performance_gate"] =
            json!("Custom workload; standard-profile qualification unavailable");
    }
    summary
}
fn child_details(value: &Value) -> Vec<String> {
    let mut errors = Vec::new();
    for error in value["errors"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .chain(value["comparison_child_error"].as_str())
    {
        let error = error.chars().take(1000).collect::<String>();
        if !errors.contains(&error) {
            errors.push(error);
        }
        if errors.len() == 16 {
            break;
        }
    }
    errors
}

fn child_requested_stop(value: &Value) -> bool {
    value["stopped"] == true
}

fn args(config: &RunConfig) -> Vec<String> {
    let mut a = vec![
        config.action.as_str().into(),
        "--out".into(),
        config.out.display().to_string(),
        "--mode".into(),
        config.mode.as_str().into(),
        "--scale".into(),
        if config.scale == 1. {
            "1"
        } else if config.scale == 0.5 {
            "1/2"
        } else {
            "2/3"
        }
        .into(),
        "--width".into(),
        config.width.to_string(),
        "--height".into(),
        config.height.to_string(),
        "--frames".into(),
        config.frames.to_string(),
        "--seed".into(),
        config.seed.to_string(),
    ];
    if let Some(scene) = config.scene {
        a.extend(["--scene".into(), scene.as_str().into()]);
    }
    if config.background {
        a.push("--background".into());
    }
    a
}

fn child(
    config: &RunConfig,
    root: &Path,
    index: usize,
    total: usize,
    label: &str,
) -> Result<Value, String> {
    let name = config
        .out
        .file_name()
        .ok_or("child has no output name")?
        .to_string_lossy();
    let events = std::fs::File::create(root.join(format!("{name}.events.jsonl")))
        .map_err(|e| e.to_string())?;
    let stderr = std::fs::File::create(root.join(format!("{name}.stderr.log")))
        .map_err(|e| e.to_string())?;
    let mut process = Command::new(std::env::current_exe().map_err(|e| e.to_string())?)
        .args(args(config))
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(stderr)
        .process_group(0)
        .spawn()
        .map_err(|e| e.to_string())?;
    let Some(stdout) = process.stdout.take() else {
        if let Err(error) = sweep_group(&mut process) {
            control::request_stop();
            return Err(error);
        }
        return Err("child stdout missing".into());
    };
    let (tx, rx) = mpsc::channel();
    let reader = std::thread::spawn(move || {
        let mut events = events;
        for line in BufReader::new(stdout).lines() {
            let Ok(line) = line else {
                break;
            };
            let _ = writeln!(events, "{line}");
            if let Ok(value) = serde_json::from_str::<Value>(&line) {
                let _ = tx.send(value);
            }
        }
    });
    let started = Instant::now();
    let mut stop_at = None;
    let mut term = false;
    let mut killed = false;
    let mut failure = None;
    let status = loop {
        for event in rx.try_iter() {
            if event["event"] == "progress" {
                let p = event["progress"]
                    .as_f64()
                    .filter(|v| v.is_finite())
                    .unwrap_or(0.)
                    .clamp(0., 1.);
                let scene = event["scene"].as_str();
                report::emit(
                    "progress",
                    json!({"progress":(index as f64+p)/total as f64,
                        "message":format!("{label} · {}",scene.or_else(||event["message"].as_str()).unwrap_or("rendering")),"scene":event["scene"]}),
                );
            }
        }
        if (control::stop_requested() || started.elapsed() > Duration::from_secs(1200))
            && stop_at.is_none()
        {
            failure = Some(
                if control::stop_requested() {
                    "comparison stopped"
                } else {
                    "child exceeded20minute limit"
                }
                .to_string(),
            );
            if let Some(input) = process.stdin.as_mut() {
                let _ = input.write_all(b"{\"event\":\"stop\"}\n");
                let _ = input.flush();
            }
            stop_at = Some(Instant::now());
        }
        if let Some(t) = stop_at {
            if t.elapsed() > Duration::from_secs(1) && !term {
                unsafe {
                    libc::kill(-(process.id() as i32), libc::SIGTERM);
                }
                term = true;
            }
            if t.elapsed() > Duration::from_secs(3) && !killed {
                unsafe {
                    libc::kill(-(process.id() as i32), libc::SIGKILL);
                }
                killed = true;
            }
        }
        match process.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) => {}
            Err(e) => {
                if let Err(cleanup) = sweep_group(&mut process) {
                    control::request_stop();
                    return Err(format!("child status failed: {e}; {cleanup}"));
                }
                let _ = reader.join();
                return Err(format!("child status failed: {e}"));
            }
        }
        std::thread::sleep(Duration::from_millis(25));
    };
    if let Err(error) = sweep_group(&mut process) {
        // Uncertain cleanup must not launch another renderer in the comparison.
        control::request_stop();
        return Err(error);
    }
    let _ = reader.join();
    let path = config.out.join("result.json");
    let mut value: Value = match std::fs::read(&path)
        .ok()
        .and_then(|bytes| serde_json::from_slice::<Value>(&bytes).ok())
    {
        Some(v) if v.is_object() => v,
        _ => {
            return Err(failure
                .unwrap_or_else(|| format!("child exited {status} without a structured result")))
        }
    };
    // Escape stops the renderer locally. Propagate its final stop state to the
    // coordinator before either phase can launch another arm. Return the report
    // normally so the stopped arm and its artifacts remain in the comparison.
    if child_requested_stop(&value) {
        control::request_stop();
    }
    if !status.success() {
        failure.get_or_insert_with(|| format!("child exited {status}"));
    }
    if let Some(error) = failure {
        value["valid"] = json!(false);
        value["render_fps"] = Value::Null;
        value["comparison_child_error"] = json!(error);
    }
    Ok(value)
}

fn compatible(child: &Value, config: &RunConfig, parent: &Value) -> bool {
    child["schema_version"] == 1
        && child["kind"] == config.action.as_str()
        && child["valid"] == true
        && child["stopped"] == false
        && child["errors"].as_array().is_some_and(|e| e.is_empty())
        && child["source_revision"] == parent["source_revision"]
        && child["binary_sha256"] == parent["binary_sha256"]
        && child["profile_version"] == parent["profile_version"]
        && child["config"]["action"] == config.action.as_str()
        && child["config"]["background"].as_bool() == Some(config.background)
        && child["config"]["load"] == json!(config.load)
        && child["config"]["width"] == config.width
        && child["config"]["height"] == config.height
        && child["config"]["frames"] == config.frames
        && child["config"]["seed"] == config.seed
        && child["config"]["scene"] == json!(config.scene)
        && child["config"]["mode"] == config.mode.as_str()
        && child["config"]["scale"]
            .as_f64()
            .is_some_and(|s| (s - config.scale as f64).abs() < 1e-7)
}

pub fn run(config: RunConfig, rounds: u32, mut envelope: Value) -> Value {
    // Only this parent consumes stdin; children get their own pipe.
    std::thread::spawn(|| {
        for line in std::io::stdin().lock().lines() {
            if let Ok(line) = line {
                if serde_json::from_str::<Value>(&line)
                    .ok()
                    .is_some_and(|v| v["event"] == "stop")
                {
                    control::request_stop();
                }
            } else {
                break;
            }
        }
    });
    let total = rounds as usize * 6 + 6;
    let mut arms = Vec::new();
    let mut errors = Vec::new();
    let mut rates = vec![Vec::new(); 6];
    let mut index = 0;
    for round in 0..rounds as usize {
        for arm in order(round) {
            if control::stop_requested() {
                break;
            }
            let (label, mode, scale) = ARMS[arm];
            let mut c = config.clone();
            c.mode = mode;
            c.scale = scale;
            c.out = config.out.join(format!("round-{}-arm-{arm}", round + 1));
            report::emit(
                "progress",
                json!({"progress":index as f64/total as f64,"message":format!("Round {}/{} · {label}",round+1,rounds)}),
            );
            let display_label = format!("Round {}/{} · {label}", round + 1, rounds);
            let value = child(&c, &config.out, index, total, &display_label)
                .unwrap_or_else(|e| json!({"valid":false,"errors":[e]}));
            index += 1;
            let valid = compatible(&value, &c, &envelope)
                && value["render_fps"]
                    .as_f64()
                    .is_some_and(|v| v.is_finite() && v > 0.);
            let mut details = child_details(&value);
            if valid {
                rates[arm].push(value["render_fps"].as_f64().unwrap());
            } else {
                if details.is_empty() {
                    details.push(
                        "Incomplete or incompatible child report; see the retained report and log"
                            .into(),
                    );
                }
                errors.push(format!(
                    "Round {} · {label}: {}",
                    round + 1,
                    details.join("; ")
                ));
            }
            arms.push(json!({"label":label,"arm_index":arm,"mode":mode,"scale":scale,"round":round+1,
                "report":format!("round-{}-arm-{arm}/result.json",round+1),"valid":valid,"errors":details,
                "render_fps":if valid{value["render_fps"].clone()}else{Value::Null},"captures":[]}));
        }
        if control::stop_requested() {
            break;
        }
    }
    for (arm, (label, mode, scale)) in ARMS.iter().enumerate() {
        if control::stop_requested() {
            break;
        }
        let mut c = config.clone();
        c.action = Action::Capture;
        c.mode = *mode;
        c.scale = *scale;
        c.out = config.out.join(format!("capture-arm-{arm}"));
        report::emit(
            "progress",
            json!({"progress":index as f64/total as f64,"message":format!("Quality replay · {label}")}),
        );
        let value = child(
            &c,
            &config.out,
            index,
            total,
            &format!("Quality replay · {label}"),
        )
        .unwrap_or_else(|e| json!({"valid":false,"errors":[e]}));
        index += 1;
        if !compatible(&value, &c, &envelope) {
            let mut details = child_details(&value);
            if details.is_empty() {
                details.push("Incomplete or incompatible quality replay".into());
            }
            errors.push(format!("Quality replay · {label}: {}", details.join("; ")));
            for entry in arms.iter_mut().filter(|v| v["arm_index"] == arm) {
                entry["capture_errors"] = json!(details);
                entry["capture_report"] = json!(format!("capture-arm-{arm}/result.json"));
            }
            continue;
        }
        let mut captures = value["captures"].as_array().cloned().unwrap_or_default();
        for capture in &mut captures {
            if let Some(p) = capture["path"].as_str() {
                capture["path"] = json!(PathBuf::from(format!("capture-arm-{arm}")).join(p));
            }
        }
        for entry in arms.iter_mut().filter(|v| v["arm_index"] == arm) {
            entry["captures"] = json!(captures);
            entry["capture_report"] = json!(format!("capture-arm-{arm}/result.json"));
        }
    }
    let stopped = control::stop_requested();
    if stopped {
        errors.push("comparison stopped; all completed and failed arms retained".into());
    }
    let valid = errors.is_empty() && arms.len() == rounds as usize * 6 && index == total;
    let paired: Vec<_> = (1..6)
        .map(|arm| {
            let mut p = comparison_summary(
                &rates[0],
                &rates[arm],
                rounds as usize,
                valid,
                config.standard(),
            );
            p["label"] = json!(ARMS[arm].0);
            p["baseline"] = json!(ARMS[0].0);
            p
        })
        .collect();
    envelope["valid"] = json!(valid);
    envelope["stopped"] = json!(stopped);
    envelope["errors"] = json!(errors);
    envelope["arms"] = json!(arms);
    envelope["paired_summaries"] = json!(paired);
    envelope["rounds"] = json!(rounds);
    envelope["qualified"] = json!(valid && rounds == 4 && config.standard());
    envelope["finished_utc"] = json!(report::utc_now());
    envelope["quality_review"]=json!("Retained original image pairs; human inspection required before recommending a reconstruction arm.");
    envelope
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn child_escape_stops_both_phases_but_an_ordinary_failure_does_not() {
        for kind in ["benchmark", "capture"] {
            let stopped = json!({"kind":kind,"valid":false,"stopped":true,
                "errors":["run stopped"],"captures":[{"path":"retained.png"}]});
            let retained = stopped.clone();
            assert!(child_requested_stop(&stopped));
            assert_eq!(
                stopped, retained,
                "stop decisions must preserve child evidence"
            );
            assert!(!child_requested_stop(&json!({"kind":kind,"valid":false,
                "stopped":false,"errors":["measurement failed"]})));
            assert!(!child_requested_stop(&json!({"kind":kind,"valid":true,
                "stopped":false})));
        }
        for malformed in [
            json!(null),
            json!({}),
            json!({"stopped":"true"}),
            json!({"stopped":1}),
        ] {
            assert!(!child_requested_stop(&malformed));
        }
    }
    #[test]
    fn four_round_order_balances_each_arm_position() {
        let mut sums = [0; 6];
        for r in 0..4 {
            let mut unique = std::collections::BTreeSet::new();
            for (i, a) in order(r).iter().enumerate() {
                sums[*a] += i;
                unique.insert(*a);
            }
            assert_eq!(unique.len(), 6);
        }
        assert_eq!(sums, [10; 6]);
    }
    #[test]
    fn paired_time_gate_uses_time_reduction_not_fps_increase() {
        let p = paired_summary(&[100.; 4], &[110.; 4]);
        assert!((p["time_reduction"].as_f64().unwrap() - 1. / 11.).abs() < 1e-10);
        assert!(p["ci95"][0].as_f64().unwrap() > 0.08);
        assert_eq!(p["qualified"], true);
    }
    #[test]
    fn uncertainty_and_missing_pairs_withhold_benefit() {
        let p = paired_summary(&[100.; 4], &[130., 90., 140., 85.]);
        assert_eq!(p["performance_gate"], "no demonstrated practical benefit");
        assert_eq!(paired_summary(&[100.], &[130.])["qualified"], false);
        assert_eq!(paired_summary(&[100., 100.], &[130.])["valid"], false);
    }
    #[test]
    fn child_arguments_do_not_split_paths_or_inherit_parent_flags() {
        let c = RunConfig {
            out: "/tmp/a folder".into(),
            mode: Mode::Temporal,
            scale: 2. / 3.,
            ..Default::default()
        };
        let a = args(&c);
        assert_eq!(a[2], "/tmp/a folder");
        assert!(!a.contains(&"--rounds".into()));
        assert!(!a.contains(&"--duration".into()));
    }
    #[test]
    fn benchmark_and_capture_children_keep_the_parent_execution_target() {
        for action in [Action::Benchmark, Action::Capture] {
            for background in [false, true] {
                let c = RunConfig {
                    action,
                    background,
                    out: "/tmp/a background comparison".into(),
                    ..Default::default()
                };
                let crate::cli::Command::Run(parsed) = crate::cli::parse(args(&c)).unwrap() else {
                    panic!("comparison children must be renderer runs");
                };
                assert_eq!(parsed.action, action);
                assert_eq!(parsed.background, background);
                assert_eq!(parsed.out, c.out);
            }
        }
    }
    #[test]
    fn incompatible_binary_or_profile_cannot_join_comparison() {
        let c = RunConfig::default();
        let parent = json!({"source_revision":"abc","binary_sha256":"hash","profile_version":"claude-lab-standard-v1"});
        let mut v = parent.clone();
        v["schema_version"] = json!(1);
        v["kind"] = json!("benchmark");
        v["valid"] = json!(true);
        v["stopped"] = json!(false);
        v["errors"] = json!([]);
        v["config"] = json!(c);
        assert!(compatible(&v, &c, &parent));
        v["binary_sha256"] = json!("different");
        assert!(!compatible(&v, &c, &parent));
    }
    fn compatible_fixture() -> (RunConfig, Value, Value) {
        let c = RunConfig::default();
        let parent = json!({"source_revision":"abc","source_dirty":false,"binary_sha256":"hash","profile_version":"claude-lab-standard-v1"});
        let mut v = parent.clone();
        v["schema_version"] = json!(1);
        v["kind"] = json!("benchmark");
        v["valid"] = json!(true);
        v["stopped"] = json!(false);
        v["errors"] = json!([]);
        v["config"] = json!(c);
        (c, parent, v)
    }
    #[test]
    fn missing_or_mistyped_cancellation_and_changed_load_cannot_join() {
        let (c, parent, v) = compatible_fixture();
        assert!(compatible(&v, &c, &parent));
        for stopped in [Value::Null, json!("false"), json!(0), json!(true)] {
            let mut changed = v.clone();
            changed["stopped"] = stopped;
            assert!(!compatible(&changed, &c, &parent));
        }
        let mut changed = v.clone();
        changed["config"]["load"]["fill"] = json!(12);
        assert!(!compatible(&changed, &c, &parent));
        let mut changed = v;
        changed["config"]["action"] = json!("stress");
        assert!(!compatible(&changed, &c, &parent));
    }
    #[test]
    fn execution_target_cannot_change_inside_a_comparison() {
        let (c, parent, mut child) = compatible_fixture();
        child["config"]["background"] = json!(false);
        assert!(compatible(&child, &c, &parent));
        for background in [json!(true), json!("false"), Value::Null] {
            child["config"]["background"] = background;
            assert!(!compatible(&child, &c, &parent));
        }
    }
    #[test]
    fn incomplete_rounds_have_no_numeric_paired_result_and_custom_is_not_qualified() {
        for (b, c) in [
            (vec![100.; 3], vec![130.; 3]),
            (vec![100.; 4], vec![130.; 3]),
        ] {
            let p = comparison_summary(&b, &c, 4, false, true);
            assert_eq!(p["valid"], false);
            assert!(p["time_reduction"].is_null() && p["ci95"].is_null());
        }
        let p = comparison_summary(&[100.; 4], &[130.; 4], 4, true, false);
        assert_eq!(p["valid"], true);
        assert_eq!(p["qualified"], false);
        assert!(p["time_reduction"].as_f64().unwrap() > 0.);
    }
    #[test]
    fn child_failure_details_preserve_actual_causes() {
        let value = json!({"errors":["fresh output warmup timed out"],"comparison_child_error":"child exited1"});
        assert_eq!(
            child_details(&value),
            vec!["fresh output warmup timed out", "child exited1"]
        );
    }
    #[test]
    fn cancellation_sweeps_a_descendant_that_outlives_the_group_leader() {
        use std::io::Read;
        struct Guard(i32);
        impl Drop for Guard {
            fn drop(&mut self) {
                unsafe {
                    libc::kill(-self.0, libc::SIGKILL);
                }
            }
        }
        let mut process = Command::new("/bin/sh")
            .args([
                "-c",
                r#"trap 'exit 0' TERM
/bin/sh -c 'trap "" TERM; echo ready; while :; do sleep 1; done' &
wait
"#,
            ])
            .process_group(0)
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        let _guard = Guard(process.id() as i32);
        let stdout = process.stdout.take().unwrap();
        let (ready_tx, ready_rx) = mpsc::channel();
        let (done_tx, done_rx) = mpsc::channel();
        let reader = std::thread::spawn(move || {
            let mut reader = BufReader::new(stdout);
            let mut line = String::new();
            reader.read_line(&mut line).unwrap();
            ready_tx.send(line).unwrap();
            let mut tail = String::new();
            reader.read_to_string(&mut tail).unwrap();
            let _ = done_tx.send(());
        });
        assert_eq!(
            ready_rx.recv_timeout(Duration::from_secs(2)).unwrap(),
            "ready\n"
        );
        unsafe {
            libc::kill(-(process.id() as i32), libc::SIGTERM);
        }
        let deadline = Instant::now() + Duration::from_secs(2);
        loop {
            if process.try_wait().unwrap().is_some() {
                break;
            }
            assert!(Instant::now() < deadline);
            std::thread::sleep(Duration::from_millis(5));
        }
        sweep_group(&mut process).unwrap();
        let closed = done_rx.recv_timeout(Duration::from_secs(1)).is_ok();
        // Always clean the intentional RED's surviving helper before asserting.
        unsafe {
            libc::kill(-(process.id() as i32), libc::SIGKILL);
        }
        reader.join().unwrap();
        assert!(
            closed,
            "leader exit must not leave reader.join blocked on a surviving descendant"
        );
    }
}
