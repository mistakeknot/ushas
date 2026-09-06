use crate::config::{Action, Mode, RunConfig, SceneKind};

pub const HELP:&str = "Ushas Bench — Claude render lab\n\nushas-bench benchmark|compare|stress|capture|video --out NEW_DIRECTORY [options]\n\n--mode native|temporal|spatial|bilinear  --scale 1|2/3|1/2\n--width 2560 --height 1440 --frames 1200 --seed 21434\n--scene materials|geometry|lighting (default all)\n--duration 600  --claudes N --lights N --particles N --fill N (stress)\n--rounds 1|4 (compare; four balanced rounds qualify a comparison)\n--background (offscreen rendering without a live preview)\n\nStandard benchmark: three fixed sequences, 1440p, target120FPS.\nBackground runs use the separate claude-lab-offscreen-v1 profile; omit window --preset options.\nResults measure completed rendering, not GPU-only time or displayed FPS.\nCustom dimensions/timelines are labelled custom. Stress and video have no benchmark score.\nVideo: offscreen 1440p, 60 fps H.264 MP4; 30 seconds or one 10-second chapter.\n";

#[derive(Debug)]
pub enum Command {
    Help,
    Version,
    Run(RunConfig),
    Compare { config: RunConfig, rounds: u32 },
}

pub fn parse(args: impl IntoIterator<Item = String>) -> Result<Command, String> {
    let mut args = args.into_iter();
    let Some(kind) = args.next() else {
        return Ok(Command::Help);
    };
    if kind == "--help" || kind == "help" {
        return Ok(Command::Help);
    }
    if kind == "--version" {
        return Ok(Command::Version);
    }
    let mut config = RunConfig::default();
    let compare = kind == "compare";
    config.action = match kind.as_str() {
        "benchmark" | "compare" => Action::Benchmark,
        "stress" => Action::Stress,
        "capture" => Action::Capture,
        "video" => Action::Video,
        _ => return Err(format!("unknown command: {kind}")),
    };
    let mut rounds = 1;
    let mut seen = std::collections::BTreeSet::new();
    while let Some(flag) = args.next() {
        if flag == "--help" {
            return Ok(Command::Help);
        }
        if !seen.insert(flag.clone()) {
            return Err(format!("repeated option: {flag}"));
        }
        if flag == "--background" {
            config.background = true;
            continue;
        }
        let value = args
            .next()
            .ok_or_else(|| format!("missing value for {flag}"))?;
        match flag.as_str() {
            "--out" => config.out = value.into(),
            "--mode" => {
                config.mode = match value.as_str() {
                    "native" => Mode::Native,
                    "temporal" => Mode::Temporal,
                    "spatial" => Mode::Spatial,
                    "bilinear" => Mode::Bilinear,
                    _ => return Err(format!("unknown mode: {value}")),
                }
            }
            "--scale" => {
                config.scale = match value.as_str() {
                    "1" | "1.0" => 1.,
                    "2/3" => 2. / 3.,
                    "1/2" | "0.5" => 0.5,
                    _ => return Err("scale must be1,2/3 or1/2".into()),
                }
            }
            "--width" => config.width = number(&value, &flag)?,
            "--height" => config.height = number(&value, &flag)?,
            "--frames" => config.frames = number(&value, &flag)?,
            "--seed" => config.seed = number(&value, &flag)?,
            "--duration" => config.duration = number(&value, &flag)?,
            "--rounds" => rounds = number(&value, &flag)?,
            "--claudes" => config.load.claudes = Some(number(&value, &flag)?),
            "--lights" => config.load.lights = Some(number(&value, &flag)?),
            "--particles" => config.load.particles = Some(number(&value, &flag)?),
            "--fill" => config.load.fill = number(&value, &flag)?,
            "--scene" => {
                config.scene = Some(match value.as_str() {
                    "materials" => SceneKind::Materials,
                    "geometry" => SceneKind::Geometry,
                    "lighting" => SceneKind::Lighting,
                    _ => return Err(format!("unknown scene: {value}")),
                })
            }
            "--preset" if value == "standard-v1" || value == crate::config::PROFILE_VERSION => {}
            _ => return Err(format!("unknown option or preset: {flag} {value}")),
        }
    }
    if config.action == Action::Video {
        config.background = true;
    }
    if config.background && seen.contains("--preset") {
        return Err("an explicit window preset cannot be combined with --background; omit --preset to select the offscreen profile".into());
    }
    if compare && (seen.contains("--mode") || seen.contains("--scale")) {
        return Err("compare chooses six fixed arms; omit --mode and --scale".into());
    }
    if !compare && seen.contains("--rounds") {
        return Err("--rounds is a compare option".into());
    }
    if config.action != Action::Stress && seen.contains("--duration") {
        return Err("--duration is a stress option".into());
    }
    if ![1, 4].contains(&rounds) {
        return Err("comparison rounds must be1 or4".into());
    }
    config.validate()?;
    Ok(if compare {
        Command::Compare { config, rounds }
    } else {
        Command::Run(config)
    })
}

fn number<T: std::str::FromStr>(value: &str, flag: &str) -> Result<T, String> {
    value
        .parse()
        .map_err(|_| format!("invalid number for {flag}: {value}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    fn p(args: &[&str]) -> Result<Command, String> {
        parse(args.iter().map(|s| s.to_string()))
    }
    #[test]
    fn application_argv_preserves_mode_ratio_and_spaced_path() {
        let Command::Run(c) = p(&[
            "benchmark",
            "--out",
            "/tmp/a run",
            "--mode",
            "temporal",
            "--scale",
            "2/3",
        ])
        .unwrap() else {
            panic!()
        };
        assert_eq!(c.mode, Mode::Temporal);
        assert_eq!(c.scale, 2. / 3.);
        assert_eq!(c.out.to_str(), Some("/tmp/a run"));
        assert!(c.standard());
    }
    #[test]
    fn comparison_round_count_is_explicit_and_bounded() {
        let Command::Compare { rounds, .. } =
            p(&["compare", "--out", "/tmp/compare", "--rounds", "4"]).unwrap()
        else {
            panic!()
        };
        assert_eq!(rounds, 4);
        assert!(p(&["compare", "--out", "/tmp/compare", "--rounds", "100"]).is_err());
    }
    #[test]
    fn bad_or_repeated_options_cannot_override_a_profile_silently() {
        assert!(p(&["benchmark", "--out", "/tmp/a", "--mode", "typo"]).is_err());
        assert!(p(&[
            "benchmark",
            "--out",
            "/tmp/a",
            "--width",
            "1280",
            "--width",
            "2560"
        ])
        .is_err());
        assert!(p(&["benchmark", "--out", "/tmp/a", "--duration", "5"]).is_err());
        assert!(p(&["stress", "--out"]).is_err());
    }
    #[test]
    fn stress_loads_remain_scoreless() {
        let Command::Run(c) = p(&[
            "stress",
            "--out",
            "/tmp/a",
            "--fill",
            "8000",
            "--claudes",
            "256",
        ])
        .unwrap() else {
            panic!()
        };
        assert!(!c.standard());
        assert_eq!(c.action, Action::Stress);
    }
    #[test]
    fn help_and_version_need_no_renderer_or_output() {
        assert!(matches!(p(&["--help"]).unwrap(), Command::Help));
        assert!(matches!(p(&["--version"]).unwrap(), Command::Version));
    }
    #[test]
    fn video_has_fixed_cadence_dimensions_and_no_benchmark_preset() {
        let Command::Run(c) = p(&["video", "--out", "/tmp/replay", "--scene", "geometry"])
            .expect("video is an independently renderable action")
        else {
            panic!()
        };
        assert_eq!(c.action.as_str(), "video");
        assert!(c.background);
        assert_eq!([c.width, c.height, c.frames], [2560, 1440, 1200]);
        assert_eq!(c.profile_version(), "claude-lab-video-v1");
        assert!(!c.standard());
        for args in [
            vec!["--width", "1280"],
            vec!["--height", "720"],
            vec!["--frames", "120"],
            vec!["--preset", "standard-v1"],
            vec!["--duration", "10"],
            vec!["--fill", "5"],
        ] {
            let mut command = vec!["video", "--out", "/tmp/replay"];
            command.extend(args);
            assert!(p(&command).is_err());
        }
    }
    #[test]
    fn background_is_valueless_for_every_action_and_preserves_other_arguments() {
        for command in ["benchmark", "compare", "stress", "capture"] {
            for args in [
                vec![command, "--background", "--out", "/tmp/background run"],
                vec![command, "--out", "/tmp/background run", "--background"],
            ] {
                let config = match p(&args).unwrap() {
                    Command::Run(config) | Command::Compare { config, .. } => config,
                    _ => panic!("expected a run"),
                };
                assert!(config.background);
                assert_eq!(config.out.to_str(), Some("/tmp/background run"));
            }
        }
    }
    #[test]
    fn background_rejects_repetition_values_and_explicit_window_presets() {
        assert!(p(&[
            "benchmark",
            "--out",
            "/tmp/a",
            "--background",
            "--background"
        ])
        .unwrap_err()
        .contains("repeated option: --background"));
        assert!(p(&["benchmark", "--out", "/tmp/a", "--background", "false"]).is_err());
        for preset in ["standard-v1", crate::config::PROFILE_VERSION] {
            for options in [
                vec!["--background", "--preset", preset],
                vec!["--preset", preset, "--background"],
            ] {
                let mut args = vec!["benchmark", "--out", "/tmp/a"];
                args.extend(options);
                assert!(p(&args).unwrap_err().contains("window preset"));
            }
            let Command::Run(window) =
                p(&["benchmark", "--out", "/tmp/a", "--preset", preset]).unwrap()
            else {
                panic!()
            };
            assert!(!window.background);
        }
    }
}
