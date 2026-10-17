//! Temporal MetalFX upscaling with a UI overlay that stays at output resolution.
//!
//! The 3D scene is rendered at half resolution and reconstructed by MetalFX
//! before the UI is drawn, so the text stays sharp. Each frame the example reads
//! `MetalFxEffectStatus` and shows the effective mode, content size and output
//! size. A snapshot is used only while it is fresh, which tolerates the
//! render-world lag.
//!
//! Run: `cargo run --example temporal_with_ui --features temporal`

use bevy::prelude::*;
use bevy_metalfx::{MetalFxEffectStatus, MetalFxMode, MetalFxObservationFrame, MetalFxPlugin};
use std::time::Duration;

fn main() {
    App::new()
        .add_plugins(DefaultPlugins)
        .add_plugins(MetalFxPlugin {
            render_scale: 0.5,
            mode: MetalFxMode::Temporal,
            ..default()
        })
        .add_systems(Startup, setup)
        .add_systems(Update, (spin, update_overlay))
        .run();
}

#[derive(Component)]
struct Spin;

#[derive(Component)]
struct Overlay;

fn setup(
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
) {
    commands.spawn((
        Mesh3d(meshes.add(Torus::new(0.6, 1.2))),
        MeshMaterial3d(materials.add(Color::srgb(0.15, 0.68, 0.92))),
        Spin,
    ));
    commands.spawn((
        DirectionalLight {
            illuminance: 12_000.0,
            ..default()
        },
        Transform::from_rotation(Quat::from_euler(EulerRot::XYZ, -0.8, -0.5, 0.0)),
    ));
    commands.spawn((
        Camera3d::default(),
        Transform::from_xyz(0.0, 1.5, 5.0).looking_at(Vec3::ZERO, Vec3::Y),
    ));
    commands.spawn((
        Text::new("waiting for MetalFX status"),
        TextFont {
            font_size: bevy::text::FontSize::Px(20.0),
            ..default()
        },
        Node {
            position_type: PositionType::Absolute,
            left: px(16),
            top: px(16),
            ..default()
        },
        Overlay,
    ));
}

fn spin(time: Res<Time>, mut shapes: Query<&mut Transform, With<Spin>>) {
    for mut transform in &mut shapes {
        transform.rotate_y(0.5 * time.delta_secs());
        transform.rotate_x(0.3 * time.delta_secs());
    }
}

fn update_overlay(
    frame: Res<MetalFxObservationFrame>,
    status: Res<MetalFxEffectStatus>,
    cameras: Query<Entity, With<Camera3d>>,
    mut overlay: Single<&mut Text, With<Overlay>>,
) {
    for camera in &cameras {
        let snapshot = status.snapshot(camera.to_bits(), frame.0);
        if snapshot.is_fresh(2, Duration::from_millis(250)) {
            if let Some(seen) = snapshot.last_observation {
                overlay.0 = format!(
                    "{:?} | content {:?} -> output {:?} | {:?}",
                    seen.effective_mode, seen.content_size, seen.output_size, seen.state
                );
            }
        }
    }
}
