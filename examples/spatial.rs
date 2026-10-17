//! Spatial MetalFX upscaling on a minimal 3D scene.
//!
//! The scene is rendered at half the window's width and height and MetalFX
//! reconstructs the full-resolution image. On platforms without MetalFX the
//! plugin disables itself and the scene renders natively.
//!
//! Run: `cargo run --example spatial`

use bevy::prelude::*;
use bevy_metalfx::{MetalFxMode, MetalFxPlugin};

fn main() {
    App::new()
        .add_plugins(DefaultPlugins)
        .add_plugins(MetalFxPlugin {
            render_scale: 0.5,
            mode: MetalFxMode::Spatial,
            ..default()
        })
        .add_systems(Startup, setup)
        .add_systems(Update, spin)
        .run();
}

#[derive(Component)]
struct Spin;

fn setup(
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
) {
    let colors = [
        Color::srgb(0.95, 0.26, 0.15),
        Color::srgb(0.15, 0.68, 0.92),
        Color::srgb(0.92, 0.76, 0.22),
    ];
    for (i, color) in colors.into_iter().enumerate() {
        commands.spawn((
            Mesh3d(meshes.add(Cuboid::new(1.25, 1.25, 1.25))),
            MeshMaterial3d(materials.add(color)),
            Transform::from_xyz((i as f32 - 1.0) * 1.9, 0.0, 0.0),
            Spin,
        ));
    }
    commands.spawn((
        DirectionalLight {
            illuminance: 12_000.0,
            ..default()
        },
        Transform::from_rotation(Quat::from_euler(EulerRot::XYZ, -0.8, -0.5, 0.0)),
    ));
    commands.spawn((
        Camera3d::default(),
        Transform::from_xyz(0.0, 2.0, 7.0).looking_at(Vec3::ZERO, Vec3::Y),
    ));
}

fn spin(time: Res<Time>, mut cubes: Query<&mut Transform, With<Spin>>) {
    for mut transform in &mut cubes {
        transform.rotate_y(0.6 * time.delta_secs());
    }
}
