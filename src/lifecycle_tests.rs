//! Headless integration tests for plugin setup and lifecycle synchronization.
//! These check ECS/extraction inputs; they do not establish GPU execution.
#![cfg(target_os = "macos")]

use super::*;
use bevy::camera::{CameraUpdateSystems, RenderTarget, RenderTargetInfo};
use bevy::window::{PrimaryWindow, WindowRef};

/// What Bevy's `camera_system` does for a primary-window target, without the
/// render plugins: fill in each camera's computed target size from the window.
/// The plugin reads only that computed size, so it is the one input these
/// headless tests must supply themselves. Registered in `CameraUpdateSystems`
/// so the plugin's ordering against that set is exercised, not bypassed.
fn size_cameras_from_the_primary_window(
    windows: Query<&Window, With<PrimaryWindow>>,
    mut cameras: Query<(&mut Camera, &RenderTarget)>,
) {
    let Ok(window) = windows.single() else {
        return;
    };
    for (mut camera, target) in &mut cameras {
        if !matches!(target, RenderTarget::Window(WindowRef::Primary)) {
            continue;
        }
        camera.computed.target_info = Some(RenderTargetInfo {
            physical_size: window.physical_size(),
            scale_factor: window.scale_factor(),
        });
    }
}

fn sized(physical_size: UVec2) -> Camera {
    let mut camera = Camera::default();
    camera.computed.target_info = Some(RenderTargetInfo {
        physical_size,
        scale_factor: 1.0,
    });
    camera
}

fn app(mode: MetalFxMode, render_scale: f32, adaptive: bool, minimum_scale: f32) -> (App, Entity) {
    let mut app = App::new();
    app.insert_resource(MetalFxAdaptiveConfig {
        minimum_scale,
        ..default()
    });
    app.add_systems(
        PostStartup,
        size_cameras_from_the_primary_window.in_set(CameraUpdateSystems),
    );
    app.add_systems(
        PostUpdate,
        size_cameras_from_the_primary_window.in_set(CameraUpdateSystems),
    );
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

#[cfg(feature = "temporal")]
#[test]
fn temporal_camera_enforces_msaa_even_with_all_inputs_and_after_settings_change() {
    use bevy::render::view::Msaa;
    let (mut app, _) = app(MetalFxMode::Temporal, 0.5, false, 0.5);
    let camera = app
        .world_mut()
        .spawn((
            Camera3d::default(),
            MotionVectorPrepass,
            DepthPrepass,
            TemporalJitter::default(),
            Msaa::Sample4,
        ))
        .id();
    app.update();
    assert_eq!(app.world().get::<Msaa>(camera), Some(&Msaa::Off));
    app.world_mut().entity_mut(camera).insert(Msaa::Sample8);
    app.update();
    assert_eq!(app.world().get::<Msaa>(camera), Some(&Msaa::Off));
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

/// A camera that renders to an image in an app with no window at all: the
/// headless case the window-sized override never reached. The target is set
/// directly, as Bevy's `camera_system` would from the image's size.
#[test]
fn image_target_camera_without_any_window_gets_the_override() {
    let mut app = App::new();
    app.add_plugins(MetalFxPlugin {
        mode: MetalFxMode::Spatial,
        render_scale: 0.5,
        ..default()
    });
    let camera = app
        .world_mut()
        .spawn((
            Camera3d::default(),
            RenderTarget::Image(Handle::<Image>::default().into()),
            sized(UVec2::new(512, 512)),
        ))
        .id();
    app.update();
    assert_scale_is_applied(&app, camera, UVec2::new(512, 512));
    assert_eq!(
        app.world().get::<MipBias>(camera).map(|b| b.0),
        Some(mip_bias_for_scale(0.5))
    );

    // The image is replaced by a larger one: the override follows the camera.
    app.world_mut()
        .get_mut::<Camera>(camera)
        .unwrap()
        .computed
        .target_info = Some(RenderTargetInfo {
        physical_size: UVec2::new(1024, 768),
        scale_factor: 1.0,
    });
    app.update();
    assert_scale_is_applied(&app, camera, UVec2::new(1024, 768));
}

/// A camera that arrives with an override of its own but no `MipBias`, as a
/// consumer that set the override by hand would: the bias is the override's
/// pair, and both follow the scale from then on. Before the plugin owned the
/// bias this camera kept sampling full-size mips indefinitely.
#[test]
fn an_existing_override_without_a_mip_bias_gets_one() {
    let (mut app, _) = app(MetalFxMode::Spatial, 0.5, false, 0.5);
    let camera = app
        .world_mut()
        .spawn((
            Camera3d::default(),
            MainPassResolutionOverride(UVec2::new(600, 450)),
        ))
        .id();
    app.update();
    assert_eq!(
        app.world().get::<MipBias>(camera).map(|b| b.0),
        Some(mip_bias_for_scale(0.5))
    );
    assert_scale_is_applied(&app, camera, UVec2::new(1200, 900));
}

/// A camera Bevy has not sized yet is left alone rather than guessed at, and
/// picked up as soon as its target size is known.
#[test]
fn unsized_camera_gets_no_override_until_its_target_is_known() {
    let mut app = App::new();
    app.add_plugins(MetalFxPlugin {
        mode: MetalFxMode::Spatial,
        render_scale: 0.5,
        ..default()
    });
    let camera = app.world_mut().spawn(Camera3d::default()).id();
    app.update();
    assert!(
        app.world()
            .get::<MainPassResolutionOverride>(camera)
            .is_none(),
        "no target size, no override"
    );
    app.world_mut()
        .get_mut::<Camera>(camera)
        .unwrap()
        .computed
        .target_info = Some(RenderTargetInfo {
        physical_size: UVec2::new(640, 480),
        scale_factor: 2.0,
    });
    app.update();
    assert_scale_is_applied(&app, camera, UVec2::new(640, 480));
}

/// Two cameras, two targets, one app: the window camera is sized from the
/// window and the image camera from its image. The window-sized override
/// gave both the window's size.
#[test]
fn each_camera_is_sized_from_its_own_target() {
    let (mut app, _) = app(MetalFxMode::Spatial, 0.5, false, 0.5);
    let window_camera = app.world_mut().spawn(Camera3d::default()).id();
    let image_camera = app
        .world_mut()
        .spawn((
            Camera3d::default(),
            RenderTarget::Image(Handle::<Image>::default().into()),
            sized(UVec2::new(256, 256)),
        ))
        .id();
    app.update();
    assert_scale_is_applied(&app, window_camera, UVec2::new(1200, 900));
    assert_scale_is_applied(&app, image_camera, UVec2::new(256, 256));
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
