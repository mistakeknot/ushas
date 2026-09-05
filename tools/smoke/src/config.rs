//! Strict CLI configuration for reproducible smoke and performance runs.

#[derive(Debug, PartialEq)]
pub struct Config {
    pub mode: String,
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
    fn default_is_short_native_control() {
        let c = parse(&[]).unwrap();
        assert_eq!((c.mode.as_str(), c.scale), ("disabled", 1.0));
        assert_eq!((c.width, c.height), (1280, 720));
        assert!(c.seconds > 0.0 && c.seconds <= 10.0);
        assert!(!c.adaptive);
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
