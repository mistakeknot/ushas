//! Strict CLI configuration for reproducible smoke and performance runs.

#[derive(Debug, PartialEq)]
pub struct Config {
    pub mode: String,
    pub subject: String,
    pub offscreen: bool,
    pub completion: bool,
    pub quality_sequence: bool,
    pub quality_moving_reset: bool,
    pub hdr: bool,
    pub native_aa: bool,
    pub lifecycle: Option<String>,
    pub scale: f32,
    pub width: u32,
    pub height: u32,
    pub seconds: f64,
    pub warmup: f64,
    pub pixel_iterations: u32,
    pub cpu_ms: u64,
    pub output: String,
    pub screenshot: Option<String>,
    pub adaptive: bool,
    pub target_fps: Option<f64>,
    pub minimum_scale: f32,
    pub moving: bool,
    pub experimental_timing: bool,
    pub presentation: String,
    pub refresh_hz: f64,
}

impl Config {
    pub fn parse(args: impl IntoIterator<Item = String>) -> Result<Self, String> {
        let mut c = Self {
            mode: "disabled".into(),
            subject: "claude".into(),
            offscreen: false,
            completion: false,
            quality_sequence: false,
            quality_moving_reset: false,
            hdr: false,
            native_aa: false,
            lifecycle: None,
            scale: 1.0,
            width: 1280,
            height: 720,
            seconds: 6.0,
            warmup: 4.0,
            pixel_iterations: 0,
            cpu_ms: 0,
            output: "ushas-smoke.json".into(),
            screenshot: None,
            adaptive: false,
            target_fps: None,
            minimum_scale: 0.5,
            moving: false,
            experimental_timing: false,
            presentation: "default".into(),
            refresh_hz: 120.0,
        };
        let mut args = args.into_iter();
        while let Some(flag) = args.next() {
            match flag.as_str() {
                "--offscreen" => {
                    c.offscreen = true;
                    continue;
                }
                "--quality-sequence" => {
                    c.quality_sequence = true;
                    continue;
                }
                "--quality-moving-reset" => {
                    c.quality_sequence = true;
                    c.quality_moving_reset = true;
                    continue;
                }
                "--completion" => {
                    c.completion = true;
                    continue;
                }
                "--hdr" => {
                    c.hdr = true;
                    continue;
                }
                "--native-aa" => {
                    c.native_aa = true;
                    continue;
                }
                "--experimental-timing" => {
                    c.experimental_timing = true;
                    continue;
                }
                "--adaptive" => {
                    c.adaptive = true;
                    continue;
                }
                "--moving" => {
                    c.moving = true;
                    continue;
                }
                _ => {}
            }
            let value = args.next().ok_or_else(|| format!("{flag} needs a value"))?;
            let number = || {
                value
                    .parse::<f64>()
                    .map_err(|_| format!("invalid value for {flag}"))
            };
            let integer = || {
                value
                    .parse::<u32>()
                    .map_err(|_| format!("{flag} requires an unsigned integer"))
            };
            match flag.as_str() {
                "--mode" => c.mode = value,
                "--subject" => c.subject = value,
                "--lifecycle" => c.lifecycle = Some(value),
                "--presentation" => c.presentation = value,
                "--refresh-hz" => c.refresh_hz = number()?,
                "--scale" => c.scale = number()? as f32,
                "--width" => c.width = integer()?,
                "--height" => c.height = integer()?,
                "--seconds" => c.seconds = number()?,
                "--warmup" => c.warmup = number()?,
                "--pixel-iterations" => c.pixel_iterations = integer()?,
                "--cpu-ms" => c.cpu_ms = u64::from(integer()?),
                "--out" => c.output = value,
                "--screenshot" => c.screenshot = Some(value),
                "--target-fps" => c.target_fps = Some(number()?),
                "--minimum-scale" => c.minimum_scale = number()? as f32,
                _ => return Err(format!("unknown argument {flag}")),
            }
        }
        if !["disabled", "spatial", "temporal", "interpolate"].contains(&c.mode.as_str()) {
            return Err("mode must be disabled, spatial, temporal, or interpolate".into());
        }
        if !["claude", "shapes"].contains(&c.subject.as_str()) {
            return Err("subject must be claude or shapes".into());
        }
        if matches!(
            c.lifecycle.as_deref(),
            Some("creation-failure" | "creation-slow" | "window-minimize" | "os-sleep-resume")
        ) && c.mode != "temporal"
        {
            return Err(
                "creation faults and native lifecycle exercises require temporal mode".into(),
            );
        }
        if c.offscreen && (c.lifecycle.is_some() || c.adaptive || c.mode == "interpolate") {
            return Err("offscreen supports fixed-scale disabled/spatial/temporal rendering only; lifecycle, adaptive and interpolation require the window fixture".into());
        }
        if c.completion && (!c.offscreen || c.experimental_timing) {
            return Err("completion requires offscreen rendering without experimental timestamp instrumentation".into());
        }
        if c.quality_sequence
            && (!c.offscreen
                || c.subject != "claude"
                || c.completion
                || c.moving
                || c.experimental_timing
                || c.screenshot.is_some()
                || c.pixel_iterations != 0
                || c.cpu_ms != 0
                || (c.mode == "disabled" && c.scale == 1.0 && !c.native_aa))
        {
            return Err("quality-sequence requires offscreen Claude, owns its clock/captures/completion, rejects artificial load, and requires native-aa for the native control".into());
        }
        if c.native_aa && (c.mode != "disabled" || c.scale != 1.0) {
            return Err("native-aa is the disabled native-scale MSAA4 control".into());
        }
        if !["default", "single", "dual"].contains(&c.presentation.as_str())
            || (c.presentation != "default" && c.mode != "interpolate")
            || !c.refresh_hz.is_finite()
            || c.refresh_hz <= 0.0
        {
            return Err("presentation must be default, or single/dual in interpolate mode; refresh-hz must be positive".into());
        }
        if !(0.1..=1.0).contains(&c.scale) || !(0.1..=1.0).contains(&c.minimum_scale) {
            return Err("scale and minimum-scale must be finite and in 0.1..=1".into());
        }
        if c.width == 0 || c.height == 0 || c.width > 8192 || c.height > 8192 {
            return Err("dimensions must be in 1..=8192".into());
        }
        if !c.seconds.is_finite() || c.seconds <= 0.0 || !c.warmup.is_finite() || c.warmup < 0.0 {
            return Err("seconds must be positive and warmup nonnegative; both finite".into());
        }
        if c.target_fps.is_some_and(|v| !v.is_finite() || v <= 0.0) {
            return Err("target-fps must be positive and finite".into());
        }
        if c.output.is_empty() || c.screenshot.as_ref().is_some_and(String::is_empty) {
            return Err("output paths must not be empty".into());
        }
        Ok(c)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(args: &[&str]) -> Result<Config, String> {
        Config::parse(args.iter().map(|s| s.to_string()))
    }

    #[test]
    fn deterministic_quality_has_its_own_offscreen_capture_contract() {
        assert!(parse(&["--quality-sequence", "--offscreen", "--native-aa"]).is_ok());
        assert!(parse(&[
            "--quality-sequence",
            "--offscreen",
            "--mode",
            "temporal",
            "--scale",
            "0.33333334",
            "--hdr"
        ])
        .is_ok());
        for extra in [
            vec![],
            vec!["--offscreen"],
            vec!["--offscreen", "--native-aa", "--moving"],
            vec!["--offscreen", "--native-aa", "--completion"],
            vec!["--offscreen", "--native-aa", "--screenshot", "x.png"],
            vec!["--offscreen", "--native-aa", "--subject", "shapes"],
            vec!["--offscreen", "--native-aa", "--pixel-iterations", "1"],
        ] {
            let mut args = vec!["--quality-sequence"];
            args.extend(extra);
            assert!(parse(&args).is_err(), "accepted {args:?}");
        }
    }

    #[test]
    fn default_is_short_native_control() {
        let c = parse(&[]).unwrap();
        assert_eq!((c.mode.as_str(), c.scale), ("disabled", 1.0));
        assert_eq!(c.subject, "claude");
        assert_eq!((c.width, c.height), (1280, 720));
        assert!(c.seconds > 0.0 && c.seconds <= 10.0);
        assert!(!c.adaptive);
        assert!(!c.offscreen);
        assert!(!c.completion);
    }

    #[test]
    fn moving_reset_selects_quality_sequence_and_keeps_its_guards() {
        let c = parse(&["--quality-moving-reset", "--offscreen", "--native-aa"]).unwrap();
        assert!(c.quality_sequence);
        assert!(c.quality_moving_reset);
        assert!(parse(&["--quality-moving-reset"]).is_err());
        assert!(parse(&[
            "--quality-moving-reset",
            "--offscreen",
            "--native-aa",
            "--moving"
        ])
        .is_err());
        assert!(parse(&[
            "--quality-moving-reset",
            "--offscreen",
            "--native-aa",
            "--completion"
        ])
        .is_err());
    }

    #[test]
    fn creation_fault_lifecycle_requires_temporal() {
        for exercise in ["creation-failure", "creation-slow"] {
            assert!(parse(&["--lifecycle", exercise]).is_err());
            assert!(parse(&["--mode", "spatial", "--lifecycle", exercise]).is_err());
            assert!(parse(&["--mode", "temporal", "--lifecycle", exercise]).is_ok());
        }
    }

    #[test]
    fn native_lifecycle_requires_temporal() {
        for exercise in ["window-minimize", "os-sleep-resume"] {
            assert!(parse(&["--lifecycle", exercise]).is_err());
            assert!(parse(&["--mode", "spatial", "--lifecycle", exercise]).is_err());
            assert!(parse(&["--mode", "temporal", "--lifecycle", exercise]).is_ok());
            assert!(
                parse(&["--offscreen", "--mode", "temporal", "--lifecycle", exercise]).is_err()
            );
        }
    }

    #[test]
    fn accepts_explicit_temporal_run() {
        let c = parse(&[
            "--mode",
            "temporal",
            "--scale",
            "0.33333334",
            "--width",
            "1920",
            "--height",
            "1080",
            "--pixel-iterations",
            "1000",
            "--adaptive",
            "--target-fps",
            "120",
            "--minimum-scale",
            "0.5",
            "--moving",
        ])
        .unwrap();
        assert_eq!(c.mode, "temporal");
        assert_eq!(c.pixel_iterations, 1000);
        assert_eq!(c.target_fps, Some(120.0));
        assert_eq!(c.minimum_scale, 0.5);
        assert!(c.adaptive && c.moving);
    }

    #[test]
    fn offscreen_accepts_fixed_render_and_timing_checks() {
        assert!(parse(&[
            "--offscreen",
            "--mode",
            "temporal",
            "--scale",
            "0.5",
            "--moving",
            "--experimental-timing",
        ])
        .is_ok());
        assert!(parse(&["--offscreen", "--native-aa"]).is_ok());
    }

    #[test]
    fn offscreen_rejects_window_lifecycle_and_presentation_modes() {
        for args in [
            vec!["--offscreen", "--lifecycle", "resize"],
            vec!["--offscreen", "--adaptive"],
            vec!["--offscreen", "--mode", "interpolate"],
        ] {
            assert!(parse(&args).is_err(), "accepted {args:?}");
        }
    }

    #[test]
    fn completion_requires_an_offscreen_arm_without_timestamp_instrumentation() {
        assert!(parse(&["--offscreen", "--completion"]).is_ok());
        assert!(parse(&[
            "--offscreen",
            "--completion",
            "--mode",
            "temporal",
            "--scale",
            "0.5"
        ])
        .is_ok());
        assert!(parse(&["--completion"]).is_err());
        assert!(parse(&["--offscreen", "--completion", "--experimental-timing"]).is_err());
    }

    #[test]
    fn refuses_invalid_measurement_arguments() {
        for args in [
            vec!["--mode", "typo"],
            vec!["--scale", "NaN"],
            vec!["--seconds", "0"],
            vec!["--width", "0"],
            vec!["--pixel-iterations", "2.5"],
            vec!["--out"],
            vec!["--target-fps", "inf"],
            vec!["--minimum-scale", "1.1"],
            vec!["--unknown"],
        ] {
            assert!(parse(&args).is_err(), "accepted {args:?}");
        }
    }
}
