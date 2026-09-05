//! Bounded, inspectable render test. JSON names each observation's actual scope.
mod claude;
mod completion;
mod config;
mod gate;
mod lifecycle;
mod metrics;
mod offscreen;
mod scene;

use bevy::app::{AppExit, ScheduleRunnerPlugin};
use bevy::prelude::*;
use bevy::render::renderer::RenderAdapterInfo;
use bevy::render::view::screenshot::ScreenshotCaptured;
use bevy::window::{PresentMode, WindowResolution};
use bevy::winit::{WinitPlugin, WinitSettings};
use bevy_metalfx::{
    MetalFxEffectState, MetalFxEffectStatus, MetalFxObservationFrame, MetalFxPlugin,
    MetalFxRenderScale,
};
use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::time::{Duration, Instant};

#[derive(Component, Clone, Copy)]
enum CapturePurpose {
    Warmup,
    Final,
}

#[derive(Resource)]
pub struct RunConfig(pub config::Config);

#[derive(Resource)]
struct RunState {
    started: Instant,
    previous: Instant,
    measurement_started: Option<Instant>,
    readiness: gate::Readiness,
    warmup_capture_requested: bool,
    warmup_screenshot: Option<Value>,
    rendered: BTreeMap<u64, f64>,
    frame_ms: Vec<f64>,
    frames: Vec<Value>,
    counts: BTreeMap<String, usize>,
    screenshot_requested: bool,
    screenshot: Option<Value>,
    finished: bool,
}
impl Default for RunState {
    fn default() -> Self {
        let now = Instant::now();
        Self {
            started: now,
            previous: now,
            measurement_started: None,
            readiness: gate::Readiness::default(),
            warmup_capture_requested: false,
            warmup_screenshot: None,
            rendered: BTreeMap::new(),
            frame_ms: vec![],
            frames: vec![],
            counts: BTreeMap::new(),
            screenshot_requested: false,
            screenshot: None,
            finished: false,
        }
    }
}
fn main() -> AppExit {
    let args: Vec<_> = std::env::args().skip(1).collect();
    if args.iter().any(|s| s == "--help") {
        println!(
            "ushas-smoke --mode disabled|spatial|temporal|interpolate --scale 0.5\n\
            --width 1280 --height 720 --warmup 4 --seconds 6 --out result.json\n\
            [--screenshot result.png] [--pixel-iterations 1000] [--cpu-ms 20] [--moving]\n\
            [--subject claude|shapes (default claude)]\n\
            [--adaptive --target-fps 60 --minimum-scale 0.5]\n\
            [--offscreen: fixed-scale image rendering; no lifecycle/adaptive/interpolation/presentation]\n\
            [--completion: offscreen serial completed-render cadence, one frame in flight]\n\
            Runs unpaced; target-fps defines the analysis/controller budget, not a frame cap."
        );
        return AppExit::Success;
    }
    let config = match config::Config::parse(args) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("{e}");
            return AppExit::error();
        }
    };
    let lifecycle_exercise = match config
        .lifecycle
        .as_deref()
        .map(lifecycle::LifecycleExercise::parse)
        .transpose()
    {
        Ok(value) => value,
        Err(error) => {
            eprintln!("{error}");
            return AppExit::error();
        }
    };
    let mode = gate::mode(&config.mode);
    let plugin = MetalFxPlugin {
        mode,
        render_scale: config.scale,
        adaptive: config.adaptive,
        #[cfg(target_os = "macos")]
        gpu_timing_sink: Some(bevy_metalfx::GpuTimingSink::new()),
        #[cfg(target_os = "macos")]
        dual_present: (!config.offscreen).then(|| {
            bevy_metalfx::present::MetalFxDualPresent::new(
                bevy_metalfx::PresentSink::new(),
                config.presentation != "default",
            )
            .with_single_present(config.presentation == "single")
            .with_refresh_interval(1.0 / config.refresh_hz)
        }),
    };
    let window = Window {
        title: format!("Ushas smoke — {} {:.3}", config.mode, config.scale),
        resolution: WindowResolution::new(config.width, config.height)
            .with_scale_factor_override(1.0),
        present_mode: PresentMode::AutoNoVsync,
        // An occluded Metal surface can stop view rendering entirely. This
        // bounded fixture must remain visible while it warms and measures.
        window_level: bevy::window::WindowLevel::AlwaysOnTop,
        visible: !config.offscreen,
        ..default()
    };
    let experimental_timing = config.experimental_timing;
    let offscreen = config.offscreen;
    let completion_enabled = config.completion;
    let completion_scale = config.scale;
    let size = (config.width, config.height);
    let mut renderer = bevy::render::RenderPlugin::default();
    #[cfg(target_os = "macos")]
    if experimental_timing {
        let mut settings = bevy::render::settings::WgpuSettings::default();
        settings.features |= bevy_metalfx::frame_timing::requested_features();
        renderer.render_creation = settings.into();
    }
    let mut app = App::new();
    let policy = bevy_metalfx::MetalFxAdaptiveConfig {
        target: config.target_fps.map_or(
            bevy_metalfx::MetalFxAdaptiveTarget::Monitor,
            bevy_metalfx::MetalFxAdaptiveTarget::Explicit,
        ),
        minimum_scale: config.minimum_scale,
        ..default()
    };
    app.insert_resource(policy);
    if let Some(exercise) = lifecycle_exercise {
        app.add_plugins(lifecycle::LifecyclePlugin::new(exercise));
    }
    app.insert_resource(RunConfig(config))
        .insert_resource(RunState::default())
        .init_resource::<offscreen::CaptureTarget>();
    let mut defaults = DefaultPlugins.set(renderer).set(WindowPlugin {
        // With Winit disabled this is metadata only: the scale-override
        // systems still use its physical dimensions, but no native window or
        // swapchain is created. The scene camera targets the image below.
        primary_window: Some(window),
        exit_condition: if offscreen {
            bevy::window::ExitCondition::DontExit
        } else {
            bevy::window::ExitCondition::OnAllClosed
        },
        ..default()
    });
    if offscreen {
        defaults = defaults.disable::<WinitPlugin>();
    }
    if completion_enabled {
        defaults =
            defaults.disable::<bevy::render::pipelined_rendering::PipelinedRenderingPlugin>();
    }
    app.add_plugins(defaults);
    if offscreen {
        let image = app
            .world_mut()
            .resource_mut::<Assets<Image>>()
            .add(offscreen::render_image(size.0, size.1));
        app.insert_resource(offscreen::CaptureTarget::Image(image))
            .add_plugins(ScheduleRunnerPlugin::run_loop(Duration::ZERO));
    } else {
        app.insert_resource(WinitSettings::continuous());
    }
    app.add_plugins((plugin, scene::ScenePlugin))
        .add_systems(Last, observe_run);
    if completion_enabled {
        let offscreen::CaptureTarget::Image(target) =
            app.world().resource::<offscreen::CaptureTarget>()
        else {
            unreachable!("completion CLI requires an image target");
        };
        app.add_plugins(completion::CompletionPlugin {
            target: target.clone(),
            mode,
            scale: completion_scale,
            output_size: [size.0, size.1],
            timeout: Duration::from_secs(5),
        });
    }
    #[cfg(target_os = "macos")]
    if experimental_timing {
        app.add_plugins(bevy_metalfx::frame_timing::ExperimentalFrameTimingPlugin);
    }
    app.run()
}
fn capture_image(
    event: On<ScreenshotCaptured>,
    config: Res<RunConfig>,
    mut state: ResMut<RunState>,
    purposes: Query<&CapturePurpose>,
) {
    let mut path = config
        .0
        .screenshot
        .clone()
        .unwrap_or_else(|| format!("{}.png", config.0.output));
    let warmup = matches!(purposes.get(event.entity), Ok(CapturePurpose::Warmup));
    if warmup {
        path = format!("{path}.warmup.png");
    }
    let result = (|| -> Result<Value, String> {
        if let Some(parent) = std::path::Path::new(&path)
            .parent()
            .filter(|p| !p.as_os_str().is_empty())
        {
            std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
        }
        let dynamic = event
            .image
            .clone()
            .try_into_dynamic()
            .map_err(|e| e.to_string())?;
        let rgba = dynamic.to_rgba8();
        let mut proof = metrics::image_proof(rgba.as_raw(), rgba.width(), rgba.height());
        dynamic.save(&path).map_err(|e| e.to_string())?;
        proof["path"] = json!(path);
        proof["width"] = json!(rgba.width());
        proof["height"] = json!(rgba.height());
        Ok(proof)
    })();
    let proof = Some(match result {
        Ok(v) => v,
        Err(e) => json!({"error":e,"path":path}),
    });
    if warmup {
        state.warmup_screenshot = proof;
    } else {
        state.screenshot = proof;
    }
}
#[allow(clippy::too_many_arguments)]
fn observe_run(
    mut commands: Commands,
    config: Res<RunConfig>,
    mut run: ResMut<RunState>,
    capture_target: Res<offscreen::CaptureTarget>,
    frame: Res<MetalFxObservationFrame>,
    status: Res<MetalFxEffectStatus>,
    scale: Res<MetalFxRenderScale>,
    adapter: Option<Res<RenderAdapterInfo>>,
    cameras: Query<(Entity, &Camera), With<Camera3d>>,
    adaptive_status: Res<bevy_metalfx::MetalFxAdaptiveStatus>,
    lifecycle: Option<Res<lifecycle::LifecycleRun>>,
    mut exit: MessageWriter<AppExit>,
    #[cfg(target_os = "macos")] gpu: Option<Res<bevy_metalfx::GpuTimingDiag>>,
    #[cfg(target_os = "macos")] present: Option<Res<bevy_metalfx::present::MetalFxDualPresent>>,
    #[cfg(target_os = "macos")] timing: Option<
        Res<bevy_metalfx::frame_timing::ExperimentalFrameTiming>,
    >,
    (mut completion_request, completion_report): (
        Option<ResMut<completion::CompletionRequest>>,
        Option<Res<completion::CompletionReport>>,
    ),
) {
    if run.finished {
        return;
    }
    let now = Instant::now();
    let dt = now.duration_since(run.previous).as_secs_f64() * 1000.0;
    run.previous = now;
    let elapsed = now.duration_since(run.started).as_secs_f64();
    let camera = cameras.single().ok();
    let snapshot = camera.map(|(entity, _)| status.snapshot(entity.to_bits(), frame.0));
    let fresh = snapshot
        .as_ref()
        .is_some_and(|s| s.is_fresh(2, Duration::from_millis(500)));
    let observation = snapshot.as_ref().and_then(|s| s.last_observation.as_ref());
    let observed = observation
        .map(|o| o.state)
        .unwrap_or(MetalFxEffectState::NoRender);
    let ready = fresh
        && observation.is_some_and(|o| {
            gate::arm_matches(
                o,
                gate::mode(&config.0.mode),
                scale.0,
                [config.0.width, config.0.height],
            )
        });
    run.readiness
        .observe(ready, observation.map(|o| o.frame_id));
    if run.measurement_started.is_none()
        && lifecycle.as_ref().is_none_or(|l| l.finished())
        && elapsed >= config.0.warmup
        && run.readiness.count >= 20
    {
        if !run.warmup_capture_requested {
            run.warmup_capture_requested = true;
            commands
                .spawn((capture_target.screenshot(), CapturePurpose::Warmup))
                .observe(capture_image);
        } else if run.warmup_screenshot.as_ref().is_some_and(|s| {
            s["nonuniform"] == true
                && s["width"] == config.0.width
                && s["height"] == config.0.height
        }) {
            if let Some(request) = &mut completion_request {
                request.0 = completion::Epoch {
                    id: 2,
                    phase: completion::Phase::Measure,
                };
            }
            run.measurement_started = Some(now);
            #[cfg(target_os = "macos")]
            if let Some(present) = &present {
                present.sink.reset();
            }
        }
    }
    let measured = run
        .measurement_started
        .map(|t| now.duration_since(t).as_secs_f64());
    if measured.is_some_and(|v| v < config.0.seconds) {
        run.frame_ms.push(dt);
        if let Some(o) = observation.filter(|_| ready) {
            let timestamp = o
                .observed_at
                .checked_duration_since(run.started)
                .map(|d| d.as_secs_f64());
            if let Some(timestamp) = timestamp {
                run.rendered.entry(o.frame_id).or_insert(timestamp);
            }
        }
        let label = if fresh {
            format!("{observed:?}")
        } else {
            "NoFreshObservation".into()
        };
        *run.counts.entry(label).or_default() += 1;
        run.frames.push(json!({"frame":frame.0,"elapsed_s":elapsed,"loop_ms":dt,
            "requested_scale":scale.0,"ready":ready,"fresh":fresh,"state":format!("{observed:?}"),
            "effect_frame":observation.map(|o|o.frame_id),"effect_age_ms":snapshot.as_ref().and_then(|s|s.wall_age()).map(|d|d.as_secs_f64()*1000.0),"reason":observation.and_then(|o|o.reason).map(|r|format!("{r:?}")),
            "content_size":observation.map(|o|o.content_size),"output_size":observation.map(|o|o.output_size),
            "effective_mode":observation.map(|o|format!("{:?}",o.effective_mode))}));
    }
    let measured_done = measured.is_some_and(|v| v >= config.0.seconds);
    let timed_out = elapsed > config.0.warmup + config.0.seconds + 30.0;
    if (measured_done || timed_out) && !run.screenshot_requested {
        if let Some(request) = &mut completion_request {
            request.0 = completion::Epoch {
                id: 3,
                phase: completion::Phase::Drain,
            };
        }
        run.screenshot_requested = true;
        commands
            .spawn((capture_target.screenshot(), CapturePurpose::Final))
            .observe(capture_image);
    }
    if (measured_done && run.screenshot.is_some())
        || elapsed > config.0.warmup + config.0.seconds + 35.0
    {
        let valid_image = run.screenshot.as_ref().is_some_and(|s| {
            s["nonuniform"] == true
                && s["width"] == config.0.width
                && s["height"] == config.0.height
        });
        let ready_count = run.frames.iter().filter(|v| v["ready"] == true).count();
        let completion_snapshot = completion_report.as_ref().map(|report| report.snapshot());
        let completion_valid = !config.0.completion
            || (completion_report.as_ref().and_then(|r| r.current_epoch())
                == Some(completion::Epoch {
                    id: 3,
                    phase: completion::Phase::Drain,
                })
                && completion_snapshot
                    .as_ref()
                    .and_then(|r| r["epochs"].as_array())
                    .is_some_and(|epochs| {
                        epochs.iter().any(|e| {
                            e["epoch"] == 2
                                && e["phase"] == "Measure"
                                && e["valid"] == true
                                && e["qualified_render_frames"]
                                    .as_u64()
                                    .is_some_and(|n| n >= 20)
                        })
                    }));
        let valid = lifecycle.as_ref().is_none_or(|l| l.passed())
            && completion_valid
            && !timed_out
            && measured_done
            && run.frames.len() >= 20
            && valid_image
            && ready_count == run.frames.len();
        let (target_fps, target_source) = analysis_budget(&config.0, &adaptive_status);
        let mut report = json!({"schema":1,"source_revision":env!("USHAS_SOURCE_REV"),
            "subject":config.0.subject,"scene_version":if config.0.subject == "claude" {claude::MODEL_VERSION} else {"shapes-v1"},
            "source_dirty_at_build":env!("USHAS_SOURCE_DIRTY"),"valid":valid,
            "timed_out":timed_out,"mode":config.0.mode,"lifecycle":lifecycle.as_ref().map(|l|l.report()),"initial_scale":config.0.scale,
            "final_scale":scale.0,"width":config.0.width,"height":config.0.height,
            "pixel_iterations":config.0.pixel_iterations,"cpu_delay_ms":config.0.cpu_ms,
            "moving":config.0.moving,"hdr":config.0.hdr,"native_aa":config.0.native_aa,"adaptive_requested":config.0.adaptive,
            "offscreen":config.0.offscreen,"render_target":if config.0.offscreen {"image"} else {"window"},
            "target_fps":target_fps,"requested_target_fps":config.0.target_fps,"target_source":target_source,
            "minimum_scale":config.0.minimum_scale,
            "warmup_s":config.0.warmup,"measurement_s":config.0.seconds,"wall_elapsed_s":elapsed,
            "adapter":adapter.as_ref().map(|a|json!({"name":a.name,"backend":format!("{:?}",a.backend),"driver":a.driver,"driver_info":a.driver_info})),
            "validity_scope":"render smoke only; experimental timing and panel delivery have separate gates",
            "render_proof":"MetalFX OutputWritten is command encoding; screenshot checks nonuniform output; neither proves panel delivery",
            "frame_loop":metrics::summarize(&run.frame_ms,target_fps),
            "adaptive_status":format!("{:?}", *adaptive_status),"camera":camera.map(|(e,c)|json!({"entity":e.to_bits(),"active":c.is_active,"target_size":c.physical_target_size().map(|s|s.to_array())})),
            "rendered_observations":{"unique_frames":run.rendered.len(),"first_frame":run.rendered.keys().next(),"last_frame":run.rendered.keys().next_back(),"timestamps_s":run.rendered},
            "warmup_screenshot":run.warmup_screenshot,"environment":runtime_environment(config.0.offscreen),
            "retained_effects":status.snapshots(frame.0).iter().map(|s|format!("{s:?}")).collect::<Vec<_>>(),
            "effect_counts":run.counts,"screenshot":run.screenshot,"frames":run.frames});
        report["completion_requested"] = json!(config.0.completion);
        report["serial_completion"] = json!(completion_snapshot);
        if config.0.offscreen {
            report["presentation"] = json!({"available":false,
                "scope":"offscreen image rendering only; no swapchain, drawable, or panel delivery"});
        }
        #[cfg(target_os = "macos")]
        if let Some(stats) = gpu.and_then(|g| g.0.stats()) {
            report["metalfx_command_buffer_diagnostic"] = json!({"scope":"dedicated command-buffer elapsed INCLUDING upstream waits; NOT frame GPU time or isolated pass cost",
                "window":"most recent up to240 completion callbacks; may include warmup","count":stats.count,
                "mean_ms":stats.mean_ms,"p50_ms":stats.p50_ms,"p99_ms":stats.p99_ms});
        }
        #[cfg(target_os = "macos")]
        {
            report["display_awake_at_finish"] = json!(bevy_metalfx::display_awake());
            report["presentation_requested"] = if config.0.offscreen {
                json!("unavailable_offscreen")
            } else {
                json!(config.0.presentation)
            };
            report["presentation_assumed_refresh_hz"] = if config.0.offscreen {
                Value::Null
            } else {
                json!(config.0.refresh_hz)
            };
            if let Some(present) = &present {
                let (encoded, dropped, displayed, callbacks, committed) = present.sink.counts();
                let stats = present.sink.stats();
                report["presentation"] = json!({"encoded":encoded,"dropped":dropped,
                    "presented_time_count":displayed,"callbacks":callbacks,"committed":committed,
                    "timestamp_fps":stats.as_ref().map(|s|s.presented_fps),
                    "judder_ms":stats.as_ref().map(|s|s.judder_ms),
                    "inversions":stats.as_ref().map(|s|s.inversions),
                    "scope":"positive MTLDrawable.presentedTime callbacks from owned layer; ordering/content/latency require independent corroboration"});
            }
            if let Some(timing) = &timing {
                let snapshot = timing.snapshot();
                let mut retained_status_counts = std::collections::BTreeMap::<String, usize>::new();
                for observation in &snapshot.observations {
                    *retained_status_counts
                        .entry(format!("{:?}", observation.status))
                        .or_default() += 1;
                }
                let mut in_flight_counts = std::collections::BTreeMap::<String, usize>::new();
                for slot in &snapshot.in_flight {
                    *in_flight_counts
                        .entry(format!("{:?}", slot.phase))
                        .or_default() += 1;
                }
                let measured_count = snapshot
                    .observations
                    .iter()
                    .filter(|o| run.rendered.contains_key(&o.identity.frame_id))
                    .count();
                let now = Instant::now();
                report["experimental_timing"] = json!({"status":format!("{:?}",snapshot.status),
                    "reason":snapshot.reason,"dropped":snapshot.dropped_samples,"validated_for_governor":false,
                    "completed_total":snapshot.completed_samples,
                    "retained_unfiltered_count":snapshot.observations.len(),
                    "retained_unfiltered_status_counts":retained_status_counts,
                    "retained_unfiltered_frame_range":[snapshot.observations.iter().map(|o|o.identity.frame_id).min(),snapshot.observations.iter().map(|o|o.identity.frame_id).max()],
                    "measurement_observation_count":measured_count,
                    "retained_outside_measurement_count":snapshot.observations.len()-measured_count,
                    "latest_completion":snapshot.observations.last().map(|o|json!({
                        "frame":o.identity.frame_id,"view":o.identity.view_id,"generation":o.identity.configuration_generation,
                        "status":format!("{:?}",o.status),"gpu_elapsed_ms":o.gpu_elapsed_ms,
                        "in_measurement_window":run.rendered.contains_key(&o.identity.frame_id),
                        "cpu_stages":cpu_timing_stages(o.stages,Some(o.observed_at.saturating_duration_since(o.identity.encoded_at))),
                    })),
                    "cpu_stage_scope":"CPU Instant offsets from encode; callback timestamps include wgpu polling delay; GPU completion versus polling delay is unknown without independent trace",
                    "in_flight_snapshot_age_ms":snapshot.in_flight_observed_at.map(|at|now.saturating_duration_since(at).as_secs_f64()*1000.0),
                    "in_flight_counts":in_flight_counts,
                    "in_flight":snapshot.in_flight.iter().map(|slot|json!({
                        "frame":slot.identity.frame_id,"view":slot.identity.view_id,"generation":slot.identity.configuration_generation,
                        "configured_mode":format!("{:?}",slot.identity.mode),"scale":slot.identity.render_scale,
                        "input_size":slot.identity.input_size,"output_size":slot.identity.output_size,
                        "phase":format!("{:?}",slot.phase),"callback_ready":slot.callback_ready,
                        "age_ms":now.saturating_duration_since(slot.identity.encoded_at).as_secs_f64()*1000.0,
                        "cpu_stages":cpu_timing_stages(slot.stages,None),
                    })).collect::<Vec<_>>(),
                    "observations":snapshot.observations.iter().filter(|o|run.rendered.contains_key(&o.identity.frame_id)).map(|o|json!({
                        "frame":o.identity.frame_id,"view":o.identity.view_id,"mode":format!("{:?}",o.identity.mode),"adaptive_epoch":o.identity.adaptive_epoch,
                        "readback_latency_ms":o.observed_at.saturating_duration_since(o.identity.encoded_at).as_secs_f64()*1000.0,
                        "generation":o.identity.configuration_generation,"scale":o.identity.render_scale,
                        "input_size":o.identity.input_size,"output_size":o.identity.output_size,
                        "raw_ticks":o.raw_ticks,"marker_ms":o.marker_ms,"gpu_elapsed_ms":o.gpu_elapsed_ms,
                        "cpu_stages":cpu_timing_stages(o.stages,Some(o.observed_at.saturating_duration_since(o.identity.encoded_at))),
                        "status":format!("{:?}",o.status)})).collect::<Vec<_>>()});
            }
        }
        let written = (|| -> Result<(), Box<dyn std::error::Error>> {
            if let Some(parent) = std::path::Path::new(&config.0.output)
                .parent()
                .filter(|p| !p.as_os_str().is_empty())
            {
                std::fs::create_dir_all(parent)?;
            }
            std::fs::write(&config.0.output, serde_json::to_vec_pretty(&report)?)?;
            Ok(())
        })();
        if let Err(error) = &written {
            eprintln!("report write failed: {error}");
        }
        println!(
            "smoke valid={valid} frames={} report={}",
            run.frames.len(),
            config.0.output
        );
        run.finished = true;
        exit.write(if valid && written.is_ok() {
            AppExit::Success
        } else {
            AppExit::error()
        });
    }
}

fn analysis_budget(
    config: &config::Config,
    status: &bevy_metalfx::MetalFxAdaptiveStatus,
) -> (f64, String) {
    if config.adaptive {
        return (status.target_fps, format!("{:?}", status.target_source));
    }
    (
        config.target_fps.unwrap_or(60.0),
        if config.target_fps.is_some() {
            "Explicit"
        } else {
            "FixedAnalysisFallback60"
        }
        .into(),
    )
}

#[cfg(test)]
mod target_budget_tests {
    use super::*;

    #[test]
    fn adaptive_analysis_uses_the_resolved_controller_budget() {
        let config = config::Config::parse(["--adaptive".into()]).unwrap();
        let status = bevy_metalfx::MetalFxAdaptiveStatus {
            target_fps: 119.88,
            target_source: bevy_metalfx::MetalFxAdaptiveTargetSource::MonitorReportedRefresh {
                window_id: 1,
                monitor_id: 2,
            },
            ..default()
        };
        let (fps, source) = analysis_budget(&config, &status);
        assert_eq!(fps, 119.88);
        assert!(source.starts_with("MonitorReportedRefresh"));
        assert_eq!(
            metrics::summarize(&[10.0], fps)["budget_ms"],
            1000.0 / 119.88
        );
    }

    #[test]
    fn fixed_analysis_preserves_explicit_and_default_budgets() {
        let status = bevy_metalfx::MetalFxAdaptiveStatus {
            target_fps: 144.0,
            ..default()
        };
        let config = config::Config::parse([]).unwrap();
        assert_eq!(
            analysis_budget(&config, &status),
            (60.0, "FixedAnalysisFallback60".into())
        );
        let config = config::Config::parse(["--target-fps".into(), "90".into()]).unwrap();
        assert_eq!(analysis_budget(&config, &status), (90.0, "Explicit".into()));
    }
}

#[cfg(target_os = "macos")]
fn cpu_timing_stages(
    stages: bevy_metalfx::frame_timing::ExperimentalTimingStages,
    harvested_after: Option<Duration>,
) -> Value {
    let ms = |duration: Duration| duration.as_secs_f64() * 1000.0;
    let difference = |end: Option<Duration>, start: Option<Duration>| {
        end.zip(start)
            .and_then(|(end, start)| end.checked_sub(start))
            .map(ms)
    };
    json!({
        "workload_callback_after_encode_ms":stages.workload_callback_after.map(ms),
        "resolve_submit_after_encode_ms":stages.resolve_submitted_after.map(ms),
        "mapping_callback_after_encode_ms":stages.mapping_callback_after.map(ms),
        "harvest_after_encode_ms":harvested_after.map(ms),
        "workload_callback_to_resolve_submit_ms":difference(stages.resolve_submitted_after,stages.workload_callback_after),
        "resolve_submit_to_mapping_callback_ms":difference(stages.mapping_callback_after,stages.resolve_submitted_after),
        "mapping_callback_to_harvest_ms":difference(harvested_after,stages.mapping_callback_after),
    })
}

#[cfg(all(test, target_os = "macos"))]
mod timing_telemetry_tests {
    use super::*;

    #[test]
    fn cpu_stages_keep_missing_callbacks_and_distinguish_resolve_delay() {
        let stages = bevy_metalfx::frame_timing::ExperimentalTimingStages {
            workload_callback_after: Some(Duration::from_millis(10)),
            resolve_submitted_after: Some(Duration::from_millis(25)),
            mapping_callback_after: None,
        };
        let pending = cpu_timing_stages(stages, None);
        assert_eq!(pending["workload_callback_to_resolve_submit_ms"], 15.0);
        assert!(pending["resolve_submit_to_mapping_callback_ms"].is_null());
        let complete = cpu_timing_stages(
            bevy_metalfx::frame_timing::ExperimentalTimingStages {
                mapping_callback_after: Some(Duration::from_millis(125)),
                ..stages
            },
            Some(Duration::from_millis(129)),
        );
        assert_eq!(complete["resolve_submit_to_mapping_callback_ms"], 100.0);
        assert_eq!(complete["mapping_callback_to_harvest_ms"], 4.0);
    }
}

fn runtime_environment(offscreen: bool) -> Value {
    let command = |program: &str, args: &[&str]| {
        std::process::Command::new(program)
            .args(args)
            .output()
            .ok()
            .filter(|o| o.status.success())
            .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_owned())
    };
    let executable = std::env::current_exe().ok();
    let binary_hash = executable
        .as_ref()
        .and_then(|p| p.to_str())
        .and_then(|p| command("/usr/bin/shasum", &["-a", "256", p]));
    json!({"executable":executable,"binary_sha256":binary_hash,
        "rustc":env!("USHAS_RUSTC"),"os":command("/usr/bin/sw_vers", &[]),
        "metal_debug_layer":std::env::var("MTL_DEBUG_LAYER").ok(),
        "features":"frame-interpolation (includes temporal)",
        "surface_mode":if offscreen {"offscreen image / no Winit or swapchain / unpaced ScheduleRunner"} else {"AutoNoVsync / AlwaysOnTop / continuous event loop"},
        "arguments":std::env::args().skip(1).collect::<Vec<_>>()})
}
