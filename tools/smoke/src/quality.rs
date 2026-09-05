//! Matched, serial 60 Hz simulation samples. Not video, FPS, or presentation.

// BEGIN PURE CONTRACT
const LAST_TICK: u32 = 144;
const CUT_TICK: u32 = 128;
const CAPTURES: [(u32, &str); 12] = [
    (31, "settled"),
    (63, "motion32"),
    (93, "motion62"),
    (94, "motion63"),
    (95, "motion64"),
    (127, "before-cut"),
    (128, "cut0"),
    (129, "cut1"),
    (130, "cut2"),
    (132, "cut4"),
    (136, "cut8"),
    (144, "cut16"),
];

fn pose_seconds(tick: u32) -> f32 {
    tick.clamp(32, 127).saturating_sub(32) as f32 / 60.0
}
fn jitter_offset(tick: u32) -> [f32; 2] {
    fn radical(mut n: u32, base: u32) -> f32 {
        let (mut sum, mut denominator) = (0.0, 1.0);
        while n > 0 {
            denominator *= base as f32;
            sum += (n % base) as f32 / denominator;
            n /= base;
        }
        sum - 0.5
    }
    [radical(tick % 32 + 1, 2), radical(tick % 32 + 1, 3)]
}

#[derive(Clone, Debug)]
struct Identity {
    tick: u32,
    request_frame: u64,
    render_frame: u64,
    effect_frame: u64,
    view: u64,
    requested_view: u64,
    shot: Option<u64>,
    extracted_shot: Option<u64>,
    ready: bool,
    temporal: bool,
    reset_before: bool,
    reset_after: bool,
}
fn validate_identity(p: &Identity, previous: Option<(u32, u64)>) -> Result<(), &'static str> {
    if p.tick > LAST_TICK
        || p.request_frame != p.render_frame
        || p.effect_frame != p.render_frame
        || p.view != p.requested_view
        || p.shot != p.extracted_shot
        || !p.ready
    {
        return Err("request, extracted capture, render view, or effect identity mismatch");
    }
    if previous.map_or(p.tick != 0, |(tick, frame)| {
        p.tick != tick + 1 || p.render_frame != frame + 1
    }) {
        return Err("logical tick or rendered frame was skipped or repeated");
    }
    let expected_reset = p.temporal && (p.tick == 0 || p.tick == CUT_TICK);
    if p.reset_before != expected_reset || p.reset_after {
        return Err(
            "history reset request was missing, unexpected, or not acknowledged in this render frame",
        );
    }
    Ok(())
}

#[cfg(test)]
mod contract_tests {
    use super::*;
    fn proof(tick: u32) -> Identity {
        Identity {
            tick,
            request_frame: 1000 + u64::from(tick),
            render_frame: 1000 + u64::from(tick),
            effect_frame: 1000 + u64::from(tick),
            view: 9,
            requested_view: 9,
            shot: Some(55),
            extracted_shot: Some(55),
            ready: true,
            temporal: true,
            reset_before: tick == 0 || tick == CUT_TICK,
            reset_after: false,
        }
    }
    #[test]
    fn poses_are_settled_then_move_at_sixty_hz_then_hold_for_a_pure_camera_cut() {
        assert_eq!(pose_seconds(0), pose_seconds(31));
        assert_eq!(pose_seconds(32), 0.0);
        assert!((pose_seconds(63) - 31.0 / 60.0).abs() < 1e-6);
        assert_eq!(pose_seconds(127), pose_seconds(CUT_TICK));
        assert_eq!(pose_seconds(CUT_TICK), pose_seconds(LAST_TICK));
        assert_eq!(CAPTURES.len(), 12);
        assert!(CAPTURES.windows(2).all(|w| w[0].0 < w[1].0));
    }
    #[test]
    fn phase_is_independent_of_warmup_length_and_preserves_halton_order() {
        assert_eq!(jitter_offset(0)[0], 0.0);
        assert!((jitter_offset(0)[1] + 1.0 / 6.0).abs() < 1e-6);
        assert_eq!(jitter_offset(1)[0], -0.25);
        assert_eq!(jitter_offset(32), jitter_offset(0));
    }
    #[test]
    fn rejects_stale_effect_wrong_view_or_capture_and_skipped_render_ticks() {
        let base = proof(2);
        assert!(validate_identity(&base, Some((1, 1001))).is_ok());
        for kind in 0..7 {
            let mut p = base.clone();
            match kind {
                0 => p.effect_frame -= 1,
                1 => p.render_frame += 1,
                2 => p.view += 1,
                3 => p.extracted_shot = None,
                4 => p.ready = false,
                5 => p.tick += 1,
                _ => p.request_frame += 1,
            }
            assert!(
                validate_identity(&p, Some((1, 1001))).is_err(),
                "accepted {kind}"
            );
        }
        assert!(validate_identity(&proof(1), None).is_err());
        assert!(validate_identity(&proof(0), None).is_ok());
    }
    #[test]
    fn cut_requires_same_frame_reset_ack_and_no_reset_on_later_frames() {
        assert!(validate_identity(&proof(128), Some((127, 1127))).is_ok());
        let mut p = proof(128);
        p.reset_after = true;
        assert!(validate_identity(&p, Some((127, 1127))).is_err());
        p.reset_after = false;
        p.reset_before = false;
        assert!(validate_identity(&p, Some((127, 1127))).is_err());
        p = proof(129);
        p.reset_before = true;
        assert!(validate_identity(&p, Some((128, 1128))).is_err());
    }
}
// END PURE CONTRACT

use bevy::camera::NormalizedRenderTarget;
use bevy::prelude::*;
use bevy::render::camera::{ExtractedCamera, TemporalJitter};
use bevy::render::sync_world::MainEntity;
use bevy::render::view::screenshot::{Screenshot, ScreenshotCaptured};
use bevy::render::view::ExtractedView;
use bevy::render::{Extract, ExtractSchedule, Render, RenderApp, RenderSystems};
use bevy_metalfx::{
    MetalFxEffectStatus, MetalFxHistoryReset, MetalFxMode, MetalFxObservationFrame,
};
use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::fs::{File, OpenOptions};
use std::io::Write;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// Only this optional resource enables deterministic Claude animation.
#[derive(Resource, Default)]
pub struct PoseClock(pub f32);

#[derive(Resource, Clone)]
struct Settings {
    mode: MetalFxMode,
    scale: f32,
    size: [u32; 2],
    hdr: bool,
    msaa: u32,
    warmup: Duration,
    target: Handle<Image>,
}
#[derive(Clone)]
struct TickRequest {
    tick: u32,
    main_frame: u64,
    view: u64,
    shot: Option<Entity>,
}
#[derive(Resource, Default)]
struct Request(Option<TickRequest>);
#[derive(Resource, Default)]
struct ExtractedRequest {
    request: Option<TickRequest>,
    extraction_frame: u64,
    extracted_shot: Option<Entity>,
    reset_before: bool,
    msaa: Option<u32>,
}
#[derive(Resource, Clone, Default)]
struct RenderProofs(Arc<Mutex<Vec<Value>>>);
#[derive(Component)]
struct Shot {
    tick: u32,
    name: &'static str,
    request_frame: u64,
}

#[derive(Resource)]
pub struct QualityRun {
    started: Instant,
    next_tick: Option<u32>,
    readiness: crate::gate::Readiness,
    directory: PathBuf,
    output: File,
    images: BTreeMap<u64, Value>,
    errors: Vec<String>,
    finished: bool,
}
impl QualityRun {
    /// Reserve the report and a new private capture directory before GPU setup.
    pub fn prepare(config: &crate::config::Config) -> std::io::Result<Self> {
        let path = std::path::absolute(&config.output)?;
        let output = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)?;
        let directory = PathBuf::from(format!("{}.quality", path.display()));
        let mut builder = std::fs::DirBuilder::new();
        #[cfg(unix)]
        {
            use std::os::unix::fs::DirBuilderExt;
            builder.mode(0o700);
        }
        builder.create(&directory)?;
        Ok(Self {
            started: Instant::now(),
            next_tick: None,
            readiness: Default::default(),
            directory,
            output,
            images: BTreeMap::new(),
            errors: vec![],
            finished: false,
        })
    }
}

pub struct QualityPlugin;
impl Plugin for QualityPlugin {
    fn build(&self, app: &mut App) {
        let c = &app.world().resource::<crate::RunConfig>().0;
        let crate::offscreen::CaptureTarget::Image(target) =
            app.world().resource::<crate::offscreen::CaptureTarget>()
        else {
            panic!("quality needs an image target");
        };
        let settings = Settings {
            mode: crate::gate::mode(&c.mode),
            scale: c.scale,
            size: [c.width, c.height],
            hdr: c.hdr,
            msaa: if c.native_aa { 4 } else { 1 },
            warmup: Duration::from_secs_f64(c.warmup.max(3.0)),
            target: target.clone(),
        };
        let proofs = RenderProofs::default();
        app.insert_resource(settings.clone())
            .insert_resource(proofs.clone())
            .init_resource::<PoseClock>()
            .init_resource::<Request>()
            .insert_resource(bevy::time::TimeUpdateStrategy::ManualDuration(
                Duration::from_secs_f64(1.0 / 60.0),
            ))
            .add_systems(PreUpdate, drive)
            .add_systems(PostUpdate, set_jitter);
        app.get_sub_app_mut(RenderApp)
            .expect("quality needs RenderApp")
            .insert_resource(settings)
            .insert_resource(proofs)
            .init_resource::<ExtractedRequest>()
            .add_systems(ExtractSchedule, extract_request)
            .add_systems(
                Render,
                record_render
                    .after(RenderSystems::Render)
                    .before(RenderSystems::Cleanup),
            );
    }
}

fn camera_pose(tick: u32) -> Transform {
    if tick >= CUT_TICK {
        Transform::from_xyz(-1.4, 2.1, 7.5).looking_at(Vec3::new(0.2, 0.5, 0.0), Vec3::Y)
    } else {
        let x = 0.75 * (pose_seconds(tick) * 0.8).sin();
        Transform::from_xyz(x, 2.4, 8.0).looking_at(Vec3::new(0.0, 0.4, 0.0), Vec3::Y)
    }
}

#[allow(clippy::too_many_arguments)]
fn drive(
    mut commands: Commands,
    settings: Res<Settings>,
    mut run: ResMut<QualityRun>,
    mut request: ResMut<Request>,
    mut clock: ResMut<PoseClock>,
    mut cameras: Query<(Entity, &Camera, &mut Transform), With<Camera3d>>,
    frame: Res<MetalFxObservationFrame>,
    status: Res<MetalFxEffectStatus>,
    mut reset: Option<ResMut<MetalFxHistoryReset>>,
    proofs: Res<RenderProofs>,
    completion: Res<crate::completion::CompletionReport>,
    mut exit: MessageWriter<AppExit>,
) {
    request.0 = None;
    if run.finished {
        return;
    }
    if run.started.elapsed() > Duration::from_secs(75) {
        run.errors.push("quality deadline exceeded".into());
    }
    let records = proofs.0.lock().expect("quality proofs poisoned");
    if records.iter().any(|p| p["valid"] != true) && run.errors.is_empty() {
        run.errors.push(
            "one or more scripted render frames failed identity/readiness/reset checks".into(),
        );
    }
    let done = run.next_tick == Some(LAST_TICK + 1)
        && records.len() == LAST_TICK as usize + 1
        && run.images.len() == CAPTURES.len();
    if done || !run.errors.is_empty() {
        finish(&mut run, &settings, &records, &completion, &mut exit);
        return;
    }
    drop(records);
    let Ok((entity, camera, mut transform)) = cameras.single_mut() else {
        return;
    };
    if !camera.is_active {
        return;
    }
    if run.next_tick.is_none() {
        let snapshot = status.snapshot(entity.to_bits(), frame.0);
        let observation = snapshot.last_observation.as_ref();
        let ready = snapshot.is_fresh(2, Duration::from_millis(500))
            && observation.is_some_and(|o| {
                crate::gate::arm_matches(o, settings.mode, settings.scale, settings.size)
            });
        run.readiness
            .observe(ready, observation.map(|o| o.frame_id));
        if run.readiness.count < 20 || run.started.elapsed() < settings.warmup {
            *transform = camera_pose(0);
            return;
        }
        run.next_tick = Some(0);
    }
    let tick = run.next_tick.unwrap();
    if tick > LAST_TICK {
        return;
    }
    clock.0 = pose_seconds(tick);
    *transform = camera_pose(tick);
    if settings.mode == MetalFxMode::Temporal && (tick == 0 || tick == CUT_TICK) {
        if let Some(reset) = &mut reset {
            reset.request();
        }
    }
    let shot = CAPTURES
        .iter()
        .find(|(at, _)| *at == tick)
        .map(|(_, name)| {
            commands
                .spawn((
                    Screenshot::image(settings.target.clone()),
                    Shot {
                        tick,
                        name,
                        request_frame: frame.0,
                    },
                ))
                .observe(capture)
                .id()
        });
    request.0 = Some(TickRequest {
        tick,
        main_frame: frame.0,
        view: entity.to_bits(),
        shot,
    });
    run.next_tick = Some(tick + 1);
}

// PostUpdate runs after the library's Update jitter generator. The logical phase,
// not varying warmup/render FrameCount, supplies the extracted temporal offset.
fn set_jitter(request: Res<Request>, mut cameras: Query<&mut TemporalJitter>) {
    let tick = request.0.as_ref().map_or(LAST_TICK, |r| r.tick);
    for mut jitter in &mut cameras {
        jitter.offset = Vec2::from_array(jitter_offset(tick));
    }
}

fn extract_request(
    mut commands: Commands,
    request: Extract<Res<Request>>,
    frame: Extract<Res<MetalFxObservationFrame>>,
    reset: Extract<Option<Res<MetalFxHistoryReset>>>,
    shots: Extract<Query<&Screenshot>>,
    cameras: Extract<Query<(Entity, &Msaa), With<Camera3d>>>,
    settings: Res<Settings>,
) {
    let extracted_shot = request.0.as_ref().and_then(|r| r.shot).filter(|entity| {
        shots
            .get(*entity)
            .is_ok_and(|s| s.0.as_image() == Some(&settings.target))
    });
    let msaa = request.0.as_ref().and_then(|r| {
        cameras
            .iter()
            .find(|(e, _)| e.to_bits() == r.view)
            .map(|(_, msaa)| msaa.samples())
    });
    commands.insert_resource(ExtractedRequest {
        request: request.0.clone(),
        extraction_frame: frame.0,
        extracted_shot,
        reset_before: reset.as_ref().is_some_and(|r| r.is_requested()),
        msaa,
    });
}

#[allow(clippy::too_many_arguments)]
fn record_render(
    request: Res<ExtractedRequest>,
    settings: Res<Settings>,
    proofs: Res<RenderProofs>,
    frame: Res<MetalFxObservationFrame>,
    status: Res<MetalFxEffectStatus>,
    reset: Option<Res<MetalFxHistoryReset>>,
    cameras: Query<(
        &MainEntity,
        &ExtractedCamera,
        &ExtractedView,
        Option<&TemporalJitter>,
    )>,
) {
    let Some(r) = &request.request else {
        return;
    };
    let mut records = proofs.0.lock().expect("quality proofs poisoned");
    if records.len() > LAST_TICK as usize {
        return;
    }
    let previous = records
        .last()
        .and_then(|p| Some((p["tick"].as_u64()? as u32, p["render_frame"].as_u64()?)));
    let mut record = json!({"tick":r.tick,"request_frame":r.main_frame,"extraction_frame":request.extraction_frame,
        "render_frame":frame.0,"shot_entity":r.shot.map(Entity::to_bits), "extracted_shot_entity":request.extracted_shot.map(Entity::to_bits),
        "simulation_seconds":pose_seconds(r.tick),"jitter_index":r.tick % 32,"msaa_samples":request.msaa,
        "reset_ordinal":if settings.mode == MetalFxMode::Temporal { if r.tick >= CUT_TICK { 2 } else { 1 } } else { 0 },
        "reset_before_encode":request.reset_before,"reset_after_encode":reset.as_ref().is_some_and(|r| r.is_requested()),
        "valid":false,"error":"missing or ambiguous extracted view"});
    if let Ok((entity, camera, view, jitter)) = cameras.single() {
        let target_matches = matches!(&camera.target, Some(NormalizedRenderTarget::Image(image)) if image.handle == settings.target);
        let snapshot = status.snapshot(entity.id().to_bits(), frame.0);
        let observation = snapshot.last_observation.as_ref();
        let expected_jitter = jitter_offset(r.tick);
        let jitter_valid = if settings.mode == MetalFxMode::Temporal {
            jitter.is_some_and(|j| (j.offset - Vec2::from_array(expected_jitter)).length() < 1e-6)
        } else {
            jitter.is_none()
        };
        let expected_camera = camera_pose(r.tick).to_matrix();
        let camera_pose_matches = view
            .world_from_view
            .to_matrix()
            .abs_diff_eq(expected_camera, 0.00001);
        let format_matches = (view.target_format
            == bevy::render::render_resource::TextureFormat::Rgba16Float)
            == settings.hdr;
        record["expected_world_from_view"] = json!(expected_camera.to_cols_array());
        record["camera_pose_matches"] = json!(camera_pose_matches);
        record["format_matches"] = json!(format_matches);
        let ready = camera_pose_matches
            && format_matches
            && target_matches
            && request.extraction_frame == frame.0
            && request.msaa == Some(settings.msaa)
            && camera.physical_target_size == Some(UVec2::from_array(settings.size))
            && jitter_valid
            && snapshot.is_fresh(0, Duration::from_secs(5))
            && observation.is_some_and(|o| {
                crate::gate::arm_matches(o, settings.mode, settings.scale, settings.size)
            });
        let identity = Identity {
            tick: r.tick,
            request_frame: r.main_frame,
            render_frame: frame.0,
            effect_frame: observation.map_or(0, |o| o.frame_id),
            view: entity.id().to_bits(),
            requested_view: r.view,
            shot: r.shot.map(Entity::to_bits),
            extracted_shot: request.extracted_shot.map(Entity::to_bits),
            ready,
            temporal: settings.mode == MetalFxMode::Temporal,
            reset_before: request.reset_before,
            reset_after: reset.as_ref().is_some_and(|r| r.is_requested()),
        };
        let verdict = validate_identity(&identity, previous);
        record["valid"] = json!(verdict.is_ok());
        record["error"] = json!(verdict.err());
        record["view_id"] = json!(entity.id().to_bits());
        record["image_target"] = json!(format!("{:?}", settings.target.id()));
        record["target_matches"] = json!(target_matches);
        record["main_texture_format"] = json!(format!("{:?}", view.target_format));
        record["world_from_view"] = json!(view.world_from_view.to_matrix().to_cols_array());
        record["jitter"] = json!(jitter.map(|j| j.offset.to_array()));
        record["effect"] = observation.map_or(Value::Null, |o| json!({"frame_id":o.frame_id,"view_id":o.view_id,
            "requested_mode":format!("{:?}",o.requested_mode),"effective_mode":format!("{:?}",o.effective_mode),
            "scale":o.requested_scale,"content_size":o.content_size,"output_size":o.output_size,
            "state":format!("{:?}",o.state),"reason":format!("{:?}",o.reason)}));
    }
    records.push(record);
}

fn capture(
    event: On<ScreenshotCaptured>,
    shots: Query<&Shot>,
    frame: Res<MetalFxObservationFrame>,
    mut run: ResMut<QualityRun>,
) {
    let Ok(shot) = shots.get(event.entity) else {
        run.errors.push("unknown screenshot entity".into());
        return;
    };
    let key = event.entity.to_bits();
    if run.images.contains_key(&key) {
        run.errors.push("duplicate screenshot readback".into());
        return;
    }
    let path = run.directory.join(format!("{}.png", shot.name));
    let mut result = json!({"name":shot.name,"tick":shot.tick,"shot_entity":key,"request_frame":shot.request_frame,
        "readback_arrived_main_frame":frame.0,"path":path,"valid":false});
    let saved = (|| -> Result<Value, String> {
        let dynamic = event
            .image
            .clone()
            .try_into_dynamic()
            .map_err(|e| format!("image conversion: {e}"))?;
        let rgba = dynamic.to_rgba8();
        let proof = crate::metrics::image_proof(rgba.as_raw(), rgba.width(), rgba.height());
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)
            .map_err(|e| e.to_string())?;
        dynamic
            .write_to(
                &mut file,
                bevy::image::ImageFormat::Png
                    .as_image_crate_format()
                    .unwrap(),
            )
            .map_err(|e| e.to_string())?;
        file.sync_all().map_err(|e| e.to_string())?;
        result["width"] = json!(rgba.width());
        result["height"] = json!(rgba.height());
        Ok(proof)
    })();
    match saved {
        Ok(proof) => {
            result["valid"] = json!(proof["nonuniform"] == true && proof["opaque_fraction"] == 1.0);
            result["image_proof"] = proof;
        }
        Err(e) => {
            result["error"] = json!(e);
        }
    }
    if result["valid"] != true {
        run.errors.push(format!("invalid capture {}", shot.name));
    }
    run.images.insert(key, result);
}

fn finish(
    run: &mut QualityRun,
    settings: &Settings,
    records: &[Value],
    completion: &crate::completion::CompletionReport,
    exit: &mut MessageWriter<AppExit>,
) {
    let completed = completion.snapshot();
    let frames = completed["frames"].as_array();
    let mut captures = vec![];
    for (tick, name) in CAPTURES {
        let record = records.iter().find(|p| p["tick"] == tick);
        let image = record
            .and_then(|p| p["shot_entity"].as_u64())
            .and_then(|id| run.images.get(&id));
        let mut capture = image
            .cloned()
            .unwrap_or_else(|| json!({"name":name,"tick":tick,"valid":false}));
        let fence =
            record.and_then(|p| frames?.iter().find(|f| f["frame_id"] == p["render_frame"]));
        let valid = capture["valid"] == true
            && capture["width"] == settings.size[0]
            && capture["height"] == settings.size[1]
            && record.is_some_and(|p| {
                p["valid"] == true && capture["request_frame"] == p["request_frame"]
            })
            && fence
                .is_some_and(|f| f["qualified"] == true && f["callback_observed_ms"].is_number());
        capture["valid"] = json!(valid);
        capture["render_proof"] = json!(record);
        capture["completion_proof"] = json!(fence);
        captures.push(capture);
    }
    let valid = run.errors.is_empty()
        && records.len() == LAST_TICK as usize + 1
        && records.iter().all(|p| p["valid"] == true)
        && captures.iter().all(|c| c["valid"] == true);
    let report = json!({"kind":"quality_sequence","protocol":"claude-60hz-sampled-v1","valid":valid,
        "scope":"145 serial render frames at deterministic 1/60 simulation steps; ticks 0..31 held, 32..127 animated/panning, 128..144 held after a camera hard cut. Twelve sampled readbacks, not continuous video, GPU cost, normal app FPS, or presentation.",
        "mode":format!("{:?}",settings.mode),"scale":settings.scale,"output_size":settings.size,"hdr":settings.hdr,
        "msaa_samples":settings.msaa,"scene_version":crate::claude::MODEL_VERSION,"offscreen":true,
        "expected_capture_count":CAPTURES.len(),"scripted_render_frames":records,"captures":captures,"errors":run.errors,
        "history_proof_scope":"reset_before/after proves the requested reset was acknowledged by CPU command encoding; matching completion fence and screenshot entity separately prove submitted work completion and readback, not visual history quality",
        "wall_seconds":run.started.elapsed().as_secs_f64()});
    let write = serde_json::to_writer_pretty(&mut run.output, &report)
        .map_err(std::io::Error::other)
        .and_then(|_| run.output.write_all(b"\n"))
        .and_then(|_| run.output.sync_all());
    if let Err(e) = &write {
        eprintln!("quality report write failed: {e}");
    }
    run.finished = true;
    exit.write(if valid && write.is_ok() {
        AppExit::Success
    } else {
        AppExit::error()
    });
}
