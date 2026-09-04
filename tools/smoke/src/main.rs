//! Bounded, inspectable render test. JSON names each observation's actual scope.
mod config;
mod metrics;
mod scene;

use bevy::app::AppExit;
use bevy::prelude::*;
use bevy::render::renderer::RenderAdapterInfo;
use bevy::render::view::screenshot::{Screenshot, ScreenshotCaptured};
use bevy::window::{PresentMode, WindowResolution};
use bevy::winit::WinitSettings;
use bevy_metalfx::{
    MetalFxEffectState, MetalFxEffectStatus, MetalFxMode, MetalFxObservationFrame, MetalFxPlugin,
    MetalFxRenderScale,
};
use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::time::{Duration, Instant};

#[derive(Resource)]
pub struct RunConfig(pub config::Config);

#[derive(Resource)]
struct RunState {
    started: Instant,
    previous: Instant,
    measurement_started: Option<Instant>,
    stable_ready_frames: usize,
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
            stable_ready_frames: 0,
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
            [--adaptive --target-fps 60 --minimum-scale 0.5]\n\
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
    let mode = match config.mode.as_str() {
        "spatial" => MetalFxMode::Spatial,
        "temporal" => MetalFxMode::Temporal,
        "interpolate" => MetalFxMode::FrameInterpolation,
        _ => MetalFxMode::Disabled,
    };
    let plugin = MetalFxPlugin {
        mode,
        render_scale: config.scale,
        adaptive: config.adaptive,
        #[cfg(target_os = "macos")]
        gpu_timing_sink: Some(bevy_metalfx::GpuTimingSink::new()),
        #[cfg(target_os = "macos")]
        dual_present: Some(
            bevy_metalfx::present::MetalFxDualPresent::new(
                bevy_metalfx::PresentSink::new(),
                config.presentation != "default",
            )
            .with_single_present(config.presentation == "single")
            .with_refresh_interval(1.0 / config.refresh_hz),
        ),
        ..default()
    };
    let window = Window {
        title: format!("Ushas smoke — {} {:.3}", config.mode, config.scale),
        resolution: WindowResolution::new(config.width, config.height)
            .with_scale_factor_override(1.0),
        present_mode: PresentMode::AutoNoVsync,
        // An occluded Metal surface can stop view rendering entirely. This
        // bounded fixture must remain visible while it warms and measures.
        window_level: bevy::window::WindowLevel::AlwaysOnTop,
        ..default()
    };
    let experimental_timing = config.experimental_timing;
    let mut renderer = bevy::render::RenderPlugin::default();
    #[cfg(target_os = "macos")]
    if experimental_timing {
        let mut settings = bevy::render::settings::WgpuSettings::default();
        settings.features |= bevy_metalfx::frame_timing::requested_features();
        renderer.render_creation = settings.into();
    }
    let mut app = App::new();
    let policy = bevy_metalfx::MetalFxAdaptiveConfig {
        policy: bevy_metalfx::adaptive::AdaptiveConfig {
            target_fps: config.target_fps.unwrap_or(60.0),
            minimum_scale: config.minimum_scale,
            ..default()
        },
        ..default()
    };
    app.insert_resource(policy);
    app.insert_resource(RunConfig(config))
        .insert_resource(RunState::default())
        .insert_resource(WinitSettings::continuous())
        .add_plugins(DefaultPlugins.set(renderer).set(WindowPlugin {
            primary_window: Some(window),
            ..default()
        }))
        .add_plugins((plugin, scene::ScenePlugin))
        .add_systems(Last, observe_run);
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
) {
    let path = config
        .0
        .screenshot
        .clone()
        .unwrap_or_else(|| format!("{}.png", config.0.output));
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
    state.screenshot = Some(match result {
        Ok(v) => v,
        Err(e) => json!({"error":e,"path":path}),
    });
}
#[allow(clippy::too_many_arguments)]
fn observe_run(
    mut commands: Commands,
    config: Res<RunConfig>,
    mut run: ResMut<RunState>,
    frame: Res<MetalFxObservationFrame>,
    status: Res<MetalFxEffectStatus>,
    scale: Res<MetalFxRenderScale>,
    adapter: Option<Res<RenderAdapterInfo>>,
    cameras: Query<(Entity, &Camera), With<Camera3d>>,
    adaptive_status: Res<bevy_metalfx::MetalFxAdaptiveStatus>,
    mut exit: MessageWriter<AppExit>,
    #[cfg(target_os = "macos")] gpu: Option<Res<bevy_metalfx::GpuTimingDiag>>,
    #[cfg(target_os = "macos")] present: Option<Res<bevy_metalfx::present::MetalFxDualPresent>>,
    #[cfg(target_os = "macos")] timing: Option<
        Res<bevy_metalfx::frame_timing::ExperimentalFrameTiming>,
    >,
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
    let active_ready = fresh && observed == MetalFxEffectState::OutputWritten;
    // Disabled is a control, not an active-effect assertion. Its render proof
    // comes from the captured image, including native where no node runs.
    let ready = if config.0.mode == "disabled" {
        fresh && observed == MetalFxEffectState::Disabled
    } else {
        active_ready
    };
    run.stable_ready_frames = if ready {
        run.stable_ready_frames + 1
    } else {
        0
    };
    if run.measurement_started.is_none()
        && elapsed >= config.0.warmup
        && run.stable_ready_frames >= 20
    {
        run.measurement_started = Some(now);
        #[cfg(target_os = "macos")]
        if let Some(present) = &present {
            present.sink.reset();
        }
    }
    let measured = run
        .measurement_started
        .map(|t| now.duration_since(t).as_secs_f64());
    if measured.is_some_and(|v| v < config.0.seconds) {
        run.frame_ms.push(dt);
        let label = if fresh {
            format!("{observed:?}")
        } else {
            "NoFreshObservation".into()
        };
        *run.counts.entry(label).or_default() += 1;
        run.frames.push(json!({"frame":frame.0,"elapsed_s":elapsed,"loop_ms":dt,
            "requested_scale":scale.0,"ready":ready,"fresh":fresh,"state":format!("{observed:?}"),
            "effect_frame":observation.map(|o|o.frame_id),"reason":observation.and_then(|o|o.reason).map(|r|format!("{r:?}")),
            "content_size":observation.map(|o|o.content_size),"output_size":observation.map(|o|o.output_size),
            "effective_mode":observation.map(|o|format!("{:?}",o.effective_mode))}));
    }
    let measured_done = measured.is_some_and(|v| v >= config.0.seconds);
    let timed_out = elapsed > config.0.warmup + config.0.seconds + 30.0;
    if (measured_done || timed_out) && !run.screenshot_requested {
        run.screenshot_requested = true;
        commands
            .spawn(Screenshot::primary_window())
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
        let valid = measured_done
            && run.frames.len() >= 20
            && valid_image
            && ready_count == run.frames.len();
        let mut report = json!({"schema":1,"source_revision":env!("USHAS_SOURCE_REV"),
            "source_dirty_at_build":env!("USHAS_SOURCE_DIRTY"),"valid":valid,
            "timed_out":timed_out,"mode":config.0.mode,"initial_scale":config.0.scale,
            "final_scale":scale.0,"width":config.0.width,"height":config.0.height,
            "pixel_iterations":config.0.pixel_iterations,"cpu_delay_ms":config.0.cpu_ms,
            "moving":config.0.moving,"adaptive_requested":config.0.adaptive,
            "target_fps":config.0.target_fps.unwrap_or(60.0),"minimum_scale":config.0.minimum_scale,
            "warmup_s":config.0.warmup,"measurement_s":config.0.seconds,"wall_elapsed_s":elapsed,
            "adapter":adapter.as_ref().map(|a|json!({"name":a.name,"backend":format!("{:?}",a.backend),"driver":a.driver,"driver_info":a.driver_info})),
            "render_proof":"MetalFX OutputWritten is command encoding; screenshot checks nonuniform output; neither proves panel delivery",
            "frame_loop":metrics::summarize(&run.frame_ms,config.0.target_fps.unwrap_or(60.0)),
            "adaptive_status":format!("{:?}", *adaptive_status),"camera":camera.map(|(e,c)|json!({"entity":e.to_bits(),"active":c.is_active,"target_size":c.physical_target_size().map(|s|s.to_array())})),
            "retained_effects":status.snapshots(frame.0).iter().map(|s|format!("{s:?}")).collect::<Vec<_>>(),
            "effect_counts":run.counts,"screenshot":run.screenshot,"frames":run.frames});
        #[cfg(target_os = "macos")]
        if let Some(stats) = gpu.and_then(|g| g.0.stats()) {
            report["metalfx_command_buffer_diagnostic"] = json!({"scope":"dedicated command-buffer elapsed INCLUDING upstream waits; NOT frame GPU time or isolated pass cost",
                "window":"most recent up to240 completion callbacks; may include warmup","count":stats.count,
                "mean_ms":stats.mean_ms,"p50_ms":stats.p50_ms,"p99_ms":stats.p99_ms});
        }
        #[cfg(target_os = "macos")]
        {
            report["display_awake_at_finish"] = json!(bevy_metalfx::display_awake());
            report["presentation_requested"] = json!(config.0.presentation);
            report["presentation_assumed_refresh_hz"] = json!(config.0.refresh_hz);
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
                report["experimental_timing"] = json!({"status":format!("{:?}",snapshot.status),
                    "reason":snapshot.reason,"dropped":snapshot.dropped_samples,"validated_for_governor":false,
                    "observations":snapshot.observations.iter().map(|o|json!({
                        "frame":o.identity.frame_id,"view":o.identity.view_id,
                        "generation":o.identity.configuration_generation,"scale":o.identity.render_scale,
                        "input_size":o.identity.input_size,"output_size":o.identity.output_size,
                        "raw_ticks":o.raw_ticks,"marker_ms":o.marker_ms,"gpu_elapsed_ms":o.gpu_elapsed_ms,
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
