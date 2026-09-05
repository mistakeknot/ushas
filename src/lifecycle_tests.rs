//! Headless integration tests for plugin setup and lifecycle synchronization.
//! These check ECS/extraction inputs; they do not establish GPU execution.
#![cfg(target_os = "macos")]

use super::*;
use bevy::window::PrimaryWindow;

fn app(mode: MetalFxMode, render_scale: f32, adaptive: bool, minimum_scale: f32) -> (App, Entity) {
    let mut app = App::new();
    app.insert_resource(MetalFxAdaptiveConfig {
        policy: adaptive::AdaptiveConfig {
            minimum_scale,
            ..default()
        },
        ..default()
    });
    let window = app
        .world_mut()
        .spawn((
            Window {
                resolution: bevy::window::WindowResolution::new(1200, 900)
                    .with_scale_factor_override(1.0),
                ..default()
            },
            PrimaryWindow,
        ))
        .id();
    app.add_plugins(MetalFxPlugin {
        mode,
        render_scale,
        adaptive,
        ..default()
    });
    (app, window)
}

fn assert_scale_is_applied(app: &App, camera: Entity, output: UVec2) {
    let scale = app.world().resource::<MetalFxRenderScale>().0;
    assert_eq!(
        app.world().resource::<MetalFxConfig>().render_scale,
        scale,
        "the extracted config must match the selected scale in the same frame"
    );
    let expected = (output.as_vec2() * scale).round().as_uvec2();
    assert_eq!(
        app.world()
            .get::<MainPassResolutionOverride>(camera)
            .map(|s| s.0),
        Some(expected),
        "the real main-pass component must match the selected scale in the same frame"
    );
}

#[test]
fn initial_floor_and_preset_snapping_apply_before_first_render_extraction() {
    for (requested, floor, expected) in [(0.4, 0.5, 0.5), (0.5, 0.6, 2.0 / 3.0)] {
        let (mut app, _) = app(MetalFxMode::Spatial, requested, true, floor);
        let camera = app.world_mut().spawn(Camera3d::default()).id();
        app.update();
        assert_eq!(app.world().resource::<MetalFxRenderScale>().0, expected);
        assert_scale_is_applied(&app, camera, UVec2::new(1200, 900));
        app.update();
        assert_scale_is_applied(&app, camera, UVec2::new(1200, 900));
    }
}

#[test]
fn device_ladder_change_before_first_frame_keeps_configuration_in_sync() {
    let (mut app, _) = app(MetalFxMode::Spatial, 0.4, true, 0.25);
    let camera = app.world_mut().spawn(Camera3d::default()).id();
    // This is the same hook Plugin::finish uses after reading a device band.
    // The hardware query itself is intentionally outside this headless test.
    adaptive_runtime::configure_ladder(&mut app, vec![1.0 / 3.0, 0.5, 2.0 / 3.0, 1.0], 0.4);
    app.update();
    assert_eq!(app.world().resource::<MetalFxRenderScale>().0, 1.0 / 3.0);
    assert_scale_is_applied(&app, camera, UVec2::new(1200, 900));
}

#[test]
fn raising_quality_floor_updates_real_render_inputs_without_gpu_samples() {
    let (mut app, _) = app(MetalFxMode::Spatial, 0.5, true, 0.5);
    let camera = app.world_mut().spawn(Camera3d::default()).id();
    app.update();
    app.world_mut()
        .resource_mut::<MetalFxAdaptiveConfig>()
        .policy
        .minimum_scale = 0.6;
    app.update();
    assert_eq!(app.world().resource::<MetalFxRenderScale>().0, 2.0 / 3.0);
    assert_scale_is_applied(&app, camera, UVec2::new(1200, 900));
}

#[test]
fn camera_spawned_after_startup_gets_the_resolution_override() {
    let (mut app, _) = app(MetalFxMode::Spatial, 0.5, false, 0.5);
    app.update();
    let camera = app.world_mut().spawn(Camera3d::default()).id();
    app.update();
    assert_scale_is_applied(&app, camera, UVec2::new(1200, 900));
}

#[cfg(feature = "temporal")]
#[test]
fn late_temporal_camera_gets_complete_prepass_and_jitter_inputs() {
    let (mut app, _) = app(MetalFxMode::Temporal, 0.5, false, 0.5);
    app.update();
    let camera = app.world_mut().spawn(Camera3d::default()).id();
    app.update();
    assert!(app.world().get::<DepthPrepass>(camera).is_some());
    assert!(app.world().get::<MotionVectorPrepass>(camera).is_some());
    assert!(app.world().get::<TemporalJitter>(camera).is_some());
    assert_eq!(
        app.world().get::<bevy::render::view::Msaa>(camera),
        Some(&bevy::render::view::Msaa::Off)
    );
    assert_scale_is_applied(&app, camera, UVec2::new(1200, 900));
}

#[cfg(feature = "temporal")]
#[test]
fn preexisting_motion_prepass_does_not_hide_missing_temporal_inputs() {
    let (mut app, _) = app(MetalFxMode::Temporal, 0.5, false, 0.5);
    let camera = app
        .world_mut()
        .spawn((Camera3d::default(), MotionVectorPrepass))
        .id();
    app.update();
    assert!(app.world().get::<DepthPrepass>(camera).is_some());
    assert!(app.world().get::<TemporalJitter>(camera).is_some());
    assert_eq!(
        app.world().get::<bevy::render::view::Msaa>(camera),
        Some(&bevy::render::view::Msaa::Off)
    );
}

#[test]
fn resize_updates_existing_render_inputs() {
    let (mut app, window) = app(MetalFxMode::Spatial, 0.5, false, 0.5);
    let camera = app.world_mut().spawn(Camera3d::default()).id();
    app.update();
    app.world_mut()
        .get_mut::<Window>(window)
        .unwrap()
        .resolution
        .set_physical_resolution(1600, 1000);
    app.update();
    assert_scale_is_applied(&app, camera, UVec2::new(1600, 1000));
}

#[test]
fn camera_cut_requested_during_update_persists_without_render_ack() {
    #[derive(Resource, Default)]
    struct ResetAtExtraction(Vec<bool>);
    fn request_once(mut reset: ResMut<MetalFxHistoryReset>, mut requested: Local<bool>) {
        if !*requested {
            reset.request();
            *requested = true;
        }
    }
    fn capture_before_extraction(
        reset: Res<MetalFxHistoryReset>,
        mut observations: ResMut<ResetAtExtraction>,
    ) {
        observations.0.push(reset.is_requested());
    }
    let (mut app, _) = app(MetalFxMode::Spatial, 0.5, false, 0.5);
    app.init_resource::<ResetAtExtraction>();
    app.add_systems(Update, request_once);
    app.add_systems(Last, capture_before_extraction);
    app.update();
    app.update();
    assert_eq!(
        app.world().resource::<ResetAtExtraction>().0,
        vec![true, true]
    );
}
