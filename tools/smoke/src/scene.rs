use bevy::asset::{load_internal_asset, uuid_handle};
use bevy::pbr::{Material, MaterialPlugin};
use bevy::prelude::*;
use bevy::render::render_resource::{AsBindGroup, ShaderType};
use bevy::shader::{Shader, ShaderRef};

const LOAD_SHADER: Handle<Shader> = uuid_handle!("39a4fc76-ed86-4dd1-9290-ad23f16a4d4d");

#[derive(Clone, Copy, ShaderType)]
pub struct LoadParams {
    pub iterations: u32,
    pub padding: UVec3,
}

#[derive(Asset, TypePath, AsBindGroup, Clone)]
pub struct LoadMaterial {
    #[uniform(0)]
    pub params: LoadParams,
}

impl Material for LoadMaterial {
    fn fragment_shader() -> ShaderRef {
        LOAD_SHADER.into()
    }
    fn enable_shadows() -> bool {
        false
    }
}

pub struct ScenePlugin;

impl Plugin for ScenePlugin {
    fn build(&self, app: &mut App) {
        load_internal_asset!(app, LOAD_SHADER, "load.wgsl", Shader::from_wgsl);
        app.add_plugins((
            MaterialPlugin::<LoadMaterial>::default(),
            crate::claude::ClaudePlugin,
        ))
        .add_systems(Startup, setup)
        .add_systems(Update, (configure_camera, animate));
    }
}

#[derive(Component)]
struct Moving;

fn setup(
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    mut load_materials: ResMut<Assets<LoadMaterial>>,
    config: Res<crate::RunConfig>,
    capture_target: Res<crate::offscreen::CaptureTarget>,
    lifecycle: Option<Res<crate::lifecycle::LifecycleRun>>,
) {
    commands.spawn((
        Text::new(if config.0.subject == "claude" {
            "USHAS  |  Claude by vgel  |  motion / fine detail"
        } else {
            "USHAS  |  thin geometry / motion / disocclusion"
        }),
        TextFont {
            font_size: bevy::text::FontSize::Px(20.0),
            ..default()
        },
        TextColor(Color::WHITE),
        Node {
            position_type: PositionType::Absolute,
            left: px(18),
            top: px(16),
            ..default()
        },
        BackgroundColor(Color::srgba(0.015, 0.02, 0.025, 0.9)),
    ));
    if !lifecycle
        .as_ref()
        .is_some_and(|l| l.exercise() == crate::lifecycle::LifecycleExercise::LateCamera)
    {
        let camera = spawn_camera(&mut commands);
        if let Some(target) = capture_target.image_render_target() {
            // Bevy's default UI-camera fallback only considers windows.
            commands.entity(camera).insert((target, IsDefaultUiCamera));
        }
    }
    commands.spawn((
        DirectionalLight {
            illuminance: 12_000.0,
            ..default()
        },
        Transform::from_rotation(Quat::from_euler(EulerRot::XYZ, -0.8, -0.5, 0.0)),
    ));
    if config.0.subject == "claude" {
        for (x, scale, turn) in [(-2.65, 0.55, 0.16), (0.0, 0.72, 0.0), (2.65, 0.55, -0.16)] {
            crate::claude::spawn(
                &mut commands,
                &mut meshes,
                &mut materials,
                Transform::from_xyz(x, -1.15, -0.4)
                    .with_scale(Vec3::splat(scale))
                    .with_rotation(Quat::from_rotation_y(turn)),
            );
        }
        commands.spawn((
            DirectionalLight {
                illuminance: 2_500.0,
                ..default()
            },
            Transform::from_rotation(Quat::from_euler(EulerRot::XYZ, -0.5, 2.3, 0.0)),
        ));
    } else {
        let colors = [
            Color::srgb(0.95, 0.26, 0.15),
            Color::srgb(0.15, 0.68, 0.92),
            Color::srgb(0.92, 0.76, 0.22),
        ];
        for (i, color) in colors.into_iter().enumerate() {
            commands.spawn((
                Mesh3d(meshes.add(Cuboid::new(1.25, 1.25, 1.25))),
                MeshMaterial3d(materials.add(StandardMaterial {
                    base_color: color,
                    perceptual_roughness: 0.35,
                    ..default()
                })),
                Transform::from_xyz((i as f32 - 1.0) * 1.9, 0.25, 0.0),
                Moving,
            ));
        }
    }
    // Thin foreground rails and a contrasting background reveal temporal edge
    // reconstruction and disocclusion; the custom material retains its prepass.
    let white = materials.add(StandardMaterial {
        base_color: Color::srgb(0.85, 0.9, 0.88),
        ..default()
    });
    let rails: Vec<f32> = if config.0.subject == "claude" {
        vec![-4.25, -3.55, -1.6, 1.6, 3.55, 4.25]
    } else {
        (0..18).map(|i| -4.25 + i as f32 * 0.5).collect()
    };
    for x in rails {
        commands.spawn((
            Mesh3d(meshes.add(Cuboid::new(0.025, 3.2, 0.025))),
            MeshMaterial3d(white.clone()),
            Transform::from_xyz(x, 0.3, 1.0),
        ));
    }
    commands.spawn((
        Mesh3d(meshes.add(Rectangle::new(24.0, 16.0))),
        MeshMaterial3d(load_materials.add(LoadMaterial {
            params: LoadParams {
                iterations: config.0.pixel_iterations,
                padding: UVec3::ZERO,
            },
        })),
        Transform::from_xyz(0.0, 0.0, -3.0),
    ));
}

fn animate(
    time: Res<Time<Real>>,
    config: Res<crate::RunConfig>,
    mut objects: Query<&mut Transform, With<Moving>>,
) {
    if config.0.moving {
        for mut t in &mut objects {
            t.rotation = Quat::from_euler(
                EulerRot::XYZ,
                time.elapsed_secs() * 0.4,
                time.elapsed_secs() * 0.6,
                0.0,
            );
        }
    }
    if config.0.cpu_ms > 0 {
        std::thread::sleep(std::time::Duration::from_millis(config.0.cpu_ms));
    }
}

pub(crate) fn spawn_camera(commands: &mut Commands) -> Entity {
    commands
        .spawn((
            Camera3d::default(),
            Msaa::Off,
            bevy::core_pipeline::tonemapping::Tonemapping::AcesFitted,
            Transform::from_xyz(0.0, 2.4, 8.0).looking_at(Vec3::new(0.0, 0.4, 0.0), Vec3::Y),
        ))
        .id()
}

#[derive(Component)]
struct ConfiguredCamera;
fn configure_camera(
    mut commands: Commands,
    config: Res<crate::RunConfig>,
    cameras: Query<Entity, (With<Camera3d>, Without<ConfiguredCamera>)>,
) {
    for camera in &cameras {
        let mut entity = commands.entity(camera);
        entity.insert(ConfiguredCamera);
        if config.0.hdr {
            entity.insert(bevy::camera::Hdr);
        }
        if config.0.native_aa {
            entity.insert(Msaa::Sample4);
        }
    }
}
