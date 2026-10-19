//! A quality menu built from `MetalFxQuality` presets.
//!
//! The example lists the presets that `MetalFxDeviceScaleBand` admits and lets
//! the number keys switch `MetalFxRenderScale` at runtime. Spatial mode is used,
//! so the band is the plugin's own range. In Temporal mode the band comes from
//! the device and may exclude some presets. Where MetalFX is unavailable the
//! plugin inserts no band, and the menu says so instead of listing presets.
//!
//! Run: `cargo run --example quality_menu`

use bevy::prelude::*;
use bevy_metalfx::{
    MetalFxDeviceScaleBand, MetalFxMode, MetalFxPlugin, MetalFxQuality, MetalFxRenderScale,
};

fn main() {
    App::new()
        .add_plugins(DefaultPlugins)
        .add_plugins(MetalFxPlugin {
            mode: MetalFxMode::Spatial,
            ..default()
        })
        .add_systems(Startup, setup)
        .add_systems(Update, (switch_quality, update_menu))
        .run();
}

#[derive(Component)]
struct Menu;

fn setup(
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
) {
    commands.spawn((
        Mesh3d(meshes.add(Sphere::new(1.2))),
        MeshMaterial3d(materials.add(Color::srgb(0.92, 0.76, 0.22))),
    ));
    commands.spawn((
        DirectionalLight::default(),
        Transform::from_rotation(Quat::from_euler(EulerRot::XYZ, -0.8, -0.5, 0.0)),
    ));
    commands.spawn((
        Camera3d::default(),
        Transform::from_xyz(0.0, 1.0, 4.0).looking_at(Vec3::ZERO, Vec3::Y),
    ));
    commands.spawn((
        Text::default(),
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
        Menu,
    ));
}

/// Presets the device admits, numbered from 1 in ascending render scale.
fn available(band: &MetalFxDeviceScaleBand) -> Vec<MetalFxQuality> {
    MetalFxQuality::ALL
        .into_iter()
        .filter(|preset| preset.is_available_on(band))
        .collect()
}

fn switch_quality(
    keys: Res<ButtonInput<KeyCode>>,
    band: Option<Res<MetalFxDeviceScaleBand>>,
    scale: Option<ResMut<MetalFxRenderScale>>,
) {
    let (Some(band), Some(mut scale)) = (band, scale) else {
        return;
    };
    const DIGITS: [KeyCode; 5] = [
        KeyCode::Digit1,
        KeyCode::Digit2,
        KeyCode::Digit3,
        KeyCode::Digit4,
        KeyCode::Digit5,
    ];
    for (key, preset) in DIGITS.into_iter().zip(available(&band)) {
        if keys.just_pressed(key) {
            scale.0 = preset.render_scale();
        }
    }
}

fn update_menu(
    band: Option<Res<MetalFxDeviceScaleBand>>,
    scale: Option<Res<MetalFxRenderScale>>,
    mut menu: Single<&mut Text, With<Menu>>,
) {
    let (Some(band), Some(scale)) = (band, scale) else {
        menu.0 = String::from("MetalFX is not available on this system");
        return;
    };
    let mut text = String::from("Press a number key to change quality\n");
    for (i, preset) in available(&band).into_iter().enumerate() {
        let marker = if (scale.0 - preset.render_scale()).abs() < 1e-3 {
            ">"
        } else {
            " "
        };
        text += &format!(
            "{marker} {}: {preset:?} ({:.2})\n",
            i + 1,
            preset.render_scale()
        );
    }
    menu.0 = text;
}
