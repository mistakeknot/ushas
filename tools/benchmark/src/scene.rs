//! Deterministic Claude render chamber; no asset or simulation clock downloads.
use crate::config::{SceneKind, StressLoad};
use bevy::asset::uuid_handle;
use bevy::pbr::{Material, MaterialPlugin};
use bevy::prelude::*;
use bevy::render::render_resource::AsBindGroup;
use bevy::shader::{Shader, ShaderRef};
use ushas_claude_model::{
    ClaudeAnimationClock, ClaudeAnimationPlugin, ClaudeAssets, ClaudeSystems,
};

#[derive(Component)]
pub struct LabCamera;

#[derive(Resource, Clone, Default)]
pub struct SceneState {
    pub kind: SceneKind,
    pub tick: u32,
    pub time_seconds: f32,
    pub seed: u64,
    pub generation: u64,
    pub load: StressLoad,
    pub caption: String,
    pub video: bool,
}

#[derive(SystemSet, Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum LabSceneSystems {
    Apply,
}

pub struct LabScenePlugin;
impl Plugin for LabScenePlugin {
    fn build(&self, app: &mut App) {
        app.world_mut()
            .resource_mut::<Assets<Shader>>()
            .insert(
                FILL_SHADER.id(),
                Shader::from_wgsl(FILL_WGSL, "embedded://ushas-bench/stress-fill.wgsl"),
            )
            .expect("fixed embedded shader handle");
        app.add_plugins((
            ClaudeAnimationPlugin,
            MaterialPlugin::<StressFillMaterial>::default(),
        ))
        .init_resource::<SceneState>()
        .init_resource::<SceneRuntime>()
        .insert_resource(GlobalAmbientLight {
            color: Color::srgb(0.61, 0.72, 0.85),
            brightness: 220.,
            ..default()
        })
        .add_systems(Startup, prepare_assets)
        .add_systems(
            Update,
            (rebuild_scene, apply_pose)
                .chain()
                .in_set(LabSceneSystems::Apply),
        )
        .configure_sets(Update, ClaudeSystems::Animate.after(LabSceneSystems::Apply));
    }
}

#[derive(Clone, PartialEq, Eq)]
struct SceneKey {
    kind: SceneKind,
    generation: u64,
    seed: u64,
    load: StressLoad,
}
impl From<&SceneState> for SceneKey {
    fn from(s: &SceneState) -> Self {
        Self {
            kind: s.kind,
            generation: s.generation,
            seed: s.seed,
            load: s.load.clone(),
        }
    }
}
#[derive(Resource, Default)]
struct SceneRuntime {
    key: Option<SceneKey>,
    root: Option<Entity>,
}
#[derive(Component)]
struct LabInstance;
#[derive(Component)]
struct LabLight;
#[derive(Component)]
struct MaterialDisplay;
#[derive(Component)]
struct Particle(u32);
#[derive(Component)]
struct Turntable {
    rest: Transform,
    phase: f32,
}
#[derive(Component)]
struct Caption;

#[derive(Resource)]
struct LabAssets {
    claude: ClaudeAssets,
    cube: Handle<Mesh>,
    sphere: Handle<Mesh>,
    particle: Handle<Mesh>,
    cylinder: Handle<Mesh>,
    ring: Handle<Mesh>,
    floor: Handle<StandardMaterial>,
    wall: Handle<StandardMaterial>,
    metal: Handle<StandardMaterial>,
    plinth: Handle<StandardMaterial>,
    cyan: Handle<StandardMaterial>,
    coral: Handle<StandardMaterial>,
    white: Handle<StandardMaterial>,
    specimens: Vec<Handle<StandardMaterial>>,
    fill: Handle<StressFillMaterial>,
}

const FILL_SHADER: Handle<Shader> = uuid_handle!("e7a9c3e1-531a-48a7-b67c-8b3b80c88b3d");
#[derive(Asset, TypePath, AsBindGroup, Debug, Clone)]
struct StressFillMaterial {
    #[uniform(0)]
    params: Vec4,
}
impl Material for StressFillMaterial {
    fn fragment_shader() -> ShaderRef {
        FILL_SHADER.into()
    }
    fn enable_shadows() -> bool {
        false
    }
}
// Standard scenes never spawn this material. A runtime-dependent visible output
// retains the explicitly requested synthetic fragment work in custom stress.
const FILL_WGSL: &str = r#"
#import bevy_pbr::forward_io::VertexOutput
@group(#{MATERIAL_BIND_GROUP}) @binding(0) var<uniform> params:vec4<f32>;
@fragment fn fragment(in:VertexOutput)->@location(0) vec4<f32> {
    var p=in.uv*vec2<f32>(3.7,2.3)+vec2<f32>(params.y*0.1,params.z);
    for(var i=0u;i<min(u32(params.x),8000u);i=i+1u) {
        p=p*1.00001+vec2<f32>(sin(p.y),cos(p.x))*0.017;
    }
    let wave=0.5+0.5*sin(p.x+p.y);
    let grid=step(0.93,fract(in.uv.x*40.0))+step(0.93,fract(in.uv.y*24.0));
    return vec4<f32>(mix(vec3<f32>(0.015,0.07,0.10),vec3<f32>(0.38,0.13,0.08),wave)+grid*0.04,1.0);
}
"#;

fn prepare_assets(
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    mut fill: ResMut<Assets<StressFillMaterial>>,
    state: Res<SceneState>,
) {
    commands.insert_resource(LabAssets::new(&mut meshes, &mut materials, &mut fill));
    if !state.video {
        commands.spawn((
            Caption,
            Text::new("USHAS / RENDER LAB"),
            TextFont {
                font_size: bevy::text::FontSize::Px(19.),
                ..default()
            },
            TextColor(Color::srgb(0.82, 0.88, 0.92)),
            Node {
                position_type: PositionType::Absolute,
                left: px(28),
                right: px(150),
                bottom: px(25),
                padding: UiRect::axes(px(16), px(11)),
                ..default()
            },
            BackgroundColor(Color::srgba(0.015, 0.022, 0.032, 0.86)),
        ));
    }
    commands.spawn((
        Text::new("USHAS   /   CLAUDE RENDER LAB"),
        TextFont {
            font_size: bevy::text::FontSize::Px(22.),
            ..default()
        },
        TextColor(Color::srgb(0.96, 0.56, 0.40)),
        Node {
            position_type: PositionType::Absolute,
            left: px(28),
            top: px(24),
            ..default()
        },
    ));
    commands.spawn((
        Text::new("CLAUDE / VGEL - PROCEDURAL INTERPRETATION"),
        TextFont {
            font_size: bevy::text::FontSize::Px(13.),
            ..default()
        },
        TextColor(Color::srgb(0.59, 0.66, 0.72)),
        Node {
            position_type: PositionType::Absolute,
            left: px(28),
            right: px(28),
            top: px(57),
            ..default()
        },
    ));
    if !state.video {
        commands.spawn((
            Text::new("ESC / STOP"),
            TextFont {
                font_size: bevy::text::FontSize::Px(13.),
                ..default()
            },
            TextColor(Color::srgb(0.59, 0.66, 0.72)),
            Node {
                position_type: PositionType::Absolute,
                right: px(28),
                bottom: px(28),
                ..default()
            },
        ));
    }
}
impl LabAssets {
    fn new(
        meshes: &mut Assets<Mesh>,
        materials: &mut Assets<StandardMaterial>,
        fill: &mut Assets<StressFillMaterial>,
    ) -> Self {
        let claude = ClaudeAssets::new(meshes, materials);
        let cube = meshes.add(Cuboid::new(1., 1., 1.));
        let sphere = meshes.add(Sphere::new(1.).mesh().uv(48, 32));
        let particle = meshes.add(Sphere::new(1.).mesh().ico(0).expect("base icosahedron"));
        let cylinder = meshes.add(Cylinder::new(1., 1.).mesh().resolution(48));
        let ring = meshes.add(Torus::new(0.94, 1.0));
        let mut paint = |color: Color, roughness: f32, metallic: f32| {
            materials.add(StandardMaterial {
                base_color: color,
                perceptual_roughness: roughness,
                metallic,
                ..default()
            })
        };
        let floor = paint(Color::srgb(0.11, 0.14, 0.17), 0.42, 0.5);
        let wall = paint(Color::srgb(0.065, 0.08, 0.10), 0.67, 0.25);
        let metal = paint(Color::srgb(0.21, 0.29, 0.34), 0.25, 0.82);
        let plinth = paint(Color::srgb(0.105, 0.12, 0.16), 0.27, 0.6);
        let mut glow = |color: LinearRgba| {
            materials.add(StandardMaterial {
                base_color: Color::LinearRgba(color),
                emissive: color * 3.0,
                perceptual_roughness: 0.45,
                ..default()
            })
        };
        let cyan = glow(LinearRgba::new(0.12, 0.63, 0.82, 1.));
        let coral = glow(LinearRgba::new(0.91, 0.28, 0.12, 1.));
        let white = glow(LinearRgba::new(0.75, 0.89, 1., 1.));
        let specimens = (0..12)
            .map(|i| {
                materials.add(StandardMaterial {
                    base_color: Color::hsl(i as f32 * 25. + 8., 0.62, 0.51),
                    metallic: if i < 6 { 0.0 } else { 0.95 },
                    perceptual_roughness: 0.08 + (i % 6) as f32 * 0.16,
                    clearcoat: if i % 3 == 0 { 0.8 } else { 0. },
                    clearcoat_perceptual_roughness: 0.08,
                    ..default()
                })
            })
            .collect();
        let fill = fill.add(StressFillMaterial { params: Vec4::ZERO });
        Self {
            claude,
            cube,
            sphere,
            particle,
            cylinder,
            ring,
            floor,
            wall,
            metal,
            plinth,
            cyan,
            coral,
            white,
            specimens,
            fill,
        }
    }
}

fn object(
    commands: &mut Commands,
    parent: Entity,
    name: &'static str,
    mesh: &Handle<Mesh>,
    material: &Handle<StandardMaterial>,
    transform: Transform,
) -> Entity {
    commands
        .spawn((
            Name::new(name),
            ChildOf(parent),
            Mesh3d(mesh.clone()),
            MeshMaterial3d(material.clone()),
            transform,
        ))
        .id()
}
fn block(
    commands: &mut Commands,
    parent: Entity,
    assets: &LabAssets,
    material: &Handle<StandardMaterial>,
    position: Vec3,
    size: Vec3,
) {
    object(
        commands,
        parent,
        "chamber panel",
        &assets.cube,
        material,
        Transform::from_translation(position).with_scale(size),
    );
}
fn pedestal(commands: &mut Commands, parent: Entity, a: &LabAssets, position: Vec3, radius: f32) {
    object(
        commands,
        parent,
        "machined plinth",
        &a.cylinder,
        &a.plinth,
        Transform::from_translation(position + Vec3::Y * 0.24)
            .with_scale(Vec3::new(radius, 0.48, radius)),
    );
    object(
        commands,
        parent,
        "plinth light rim",
        &a.ring,
        &a.cyan,
        Transform::from_translation(position + Vec3::Y * 0.47).with_scale(Vec3::splat(radius)),
    );
}

#[allow(clippy::too_many_arguments)]
fn rebuild_scene(
    mut commands: Commands,
    state: Res<SceneState>,
    assets: Res<LabAssets>,
    mut runtime: ResMut<SceneRuntime>,
) {
    let key = SceneKey::from(&*state);
    if runtime.key.as_ref() == Some(&key) {
        return;
    }
    if let Some(root) = runtime.root.take() {
        commands.entity(root).despawn();
    }
    let root = commands
        .spawn((
            Name::new("Claude lab / scene generation"),
            Transform::default(),
            Visibility::default(),
        ))
        .id();
    let a = &*assets;
    let layout = scene_layout(state.kind, &state.load);
    let (width, depth) = match state.kind {
        SceneKind::Materials => (20., 18.),
        SceneKind::Geometry => (38., 36.),
        SceneKind::Lighting => (30., 28.),
    };
    block(
        &mut commands,
        root,
        a,
        &a.floor,
        Vec3::new(0., -0.2, 0.),
        Vec3::new(width, 0.4, depth),
    );
    block(
        &mut commands,
        root,
        a,
        &a.wall,
        Vec3::new(0., 4.5, -depth / 2.),
        Vec3::new(width, 9., 0.5),
    );
    // Recessed bays and narrow trims give genuine silhouettes and material detail.
    for i in 0..13 {
        let x = (i as f32 - 6.) * width / 13.;
        block(
            &mut commands,
            root,
            a,
            &a.metal,
            Vec3::new(x, 4.4, -depth / 2. + 0.31),
            Vec3::new(0.045, 7.9, 0.10),
        );
        block(
            &mut commands,
            root,
            a,
            if i % 3 == 0 { &a.coral } else { &a.cyan },
            Vec3::new(x + 0.12, 4.4, -depth / 2. + 0.32),
            Vec3::new(0.018, 6.4, 0.07),
        );
    }
    for side in [-1., 1.] {
        block(
            &mut commands,
            root,
            a,
            &a.metal,
            Vec3::new(side * (width / 2. - 0.2), 0.4, 0.),
            Vec3::new(0.35, 0.8, depth),
        );
        block(
            &mut commands,
            root,
            a,
            &a.cyan,
            Vec3::new(side * (width / 2. - 0.39), 0.8, 0.),
            Vec3::new(0.045, 0.035, depth),
        );
        for i in 0..7 {
            let z = -depth / 2. + 1.5 + i as f32 * (depth - 3.) / 6.;
            block(
                &mut commands,
                root,
                a,
                &a.metal,
                Vec3::new(side * (width / 2. - 0.8), 2.5, z),
                Vec3::new(0.10, 5., 0.10),
            );
            block(
                &mut commands,
                root,
                a,
                &a.white,
                Vec3::new(side * (width / 2. - 0.8), 5., z),
                Vec3::new(1.2, 0.035, 0.10),
            );
        }
    }
    for i in 0..9 {
        let z = (i as f32 - 4.) * depth / 9.;
        block(
            &mut commands,
            root,
            a,
            &a.metal,
            Vec3::new(0., 0.005, z),
            Vec3::new(width - 1., 0.012, 0.024),
        );
    }
    for i in 0..layout.claudes {
        let p = claude_placement(state.kind, i, layout.claudes, state.seed);
        let position = Vec3::from_array(p.position);
        pedestal(
            &mut commands,
            root,
            a,
            Vec3::new(position.x, 0., position.z),
            if layout.claudes == 1 { 1.9 } else { 1.0 },
        );
        let toy = a.claude.spawn_with_phase(
            &mut commands,
            Transform::from_translation(position)
                .with_scale(Vec3::splat(p.scale))
                .with_rotation(Quat::from_rotation_y(p.yaw)),
            p.phase,
        );
        commands.entity(toy).insert((ChildOf(root), LabInstance));
    }
    for i in 0..layout.material_displays {
        let side = if i < 6 { -1. } else { 1. };
        let row = (i % 6) as f32;
        let position = Vec3::new(
            side * (3.6 + 0.36 * (row * 0.8).sin()),
            0.,
            (row - 2.5) * 2.25,
        );
        pedestal(&mut commands, root, a, position, 0.68);
        let rest =
            Transform::from_translation(position + Vec3::Y * 1.28).with_scale(Vec3::splat(0.63));
        let entity = object(
            &mut commands,
            root,
            "material specimen",
            &a.sphere,
            &a.specimens[i as usize],
            rest,
        );
        commands.entity(entity).insert((
            MaterialDisplay,
            Turntable {
                rest,
                phase: i as f32 * 0.7,
            },
        ));
        // Offset fine ring exposes changing specular highlights and disocclusion.
        object(
            &mut commands,
            root,
            "specimen orbital ring",
            &a.ring,
            &a.metal,
            Transform::from_translation(position + Vec3::Y * 1.28)
                .with_scale(Vec3::splat(0.78))
                .with_rotation(Quat::from_euler(EulerRot::XYZ, 0.55, 0., 0.35)),
        );
    }
    if state.kind == SceneKind::Geometry {
        // Foreground louvres, crossbars and alternate smooth/faceted props.
        for i in 0..25 {
            let x = (i as f32 - 12.) * 1.25;
            block(
                &mut commands,
                root,
                a,
                &a.metal,
                Vec3::new(x, 0.9, 13.9),
                Vec3::new(0.034, 1.8, 0.07),
            );
        }
        for y in [0.45, 1.3] {
            block(
                &mut commands,
                root,
                a,
                &a.cyan,
                Vec3::new(0., y, 13.9),
                Vec3::new(31., 0.026, 0.04),
            );
        }
        for i in 0..12 {
            let side = if i % 2 == 0 { -1. } else { 1. };
            let rest = Transform::from_xyz(side * 14.3, 1.0, (i / 2) as f32 * 4. - 10.)
                .with_scale(Vec3::splat(0.6));
            let e = object(
                &mut commands,
                root,
                "geometry calibration prop",
                if i % 3 == 0 { &a.cube } else { &a.ring },
                &a.specimens[i],
                rest,
            );
            commands.entity(e).insert(Turntable {
                rest,
                phase: i as f32,
            });
        }
    }
    for i in 0..layout.lights {
        let angle = std::f32::consts::TAU * (i as f32 / layout.lights as f32) + 0.4;
        let radius = width * 0.32;
        let position = Vec3::new(
            angle.sin() * radius,
            if i < layout.shadowed_lights { 8.0 } else { 5.2 },
            angle.cos() * depth * 0.31,
        );
        let color = if i < layout.shadowed_lights {
            Color::srgb(1., 0.90, 0.80)
        } else if i % 2 == 0 {
            Color::srgb(0.42, 0.74, 1.)
        } else {
            Color::srgb(1., 0.48, 0.30)
        };
        let mut e = commands.spawn((
            LabLight,
            ChildOf(root),
            Transform::from_translation(position).looking_at(Vec3::Y * 1.5, Vec3::Y),
        ));
        if i < layout.shadowed_lights {
            e.insert(SpotLight {
                color,
                intensity: 1_200_000.,
                range: 65.,
                inner_angle: 0.60,
                outer_angle: 0.95,
                shadow_maps_enabled: true,
                ..default()
            });
        } else {
            e.insert(PointLight {
                color,
                intensity: 450_000.,
                range: 55.,
                radius: 0.35,
                ..default()
            });
        }
        object(
            &mut commands,
            root,
            "light fixture",
            &a.cube,
            &a.white,
            Transform::from_translation(position).with_scale(Vec3::new(0.9, 0.06, 0.36)),
        );
    }
    for i in 0..layout.particles {
        let e = object(
            &mut commands,
            root,
            "analytic light mote",
            &a.particle,
            if i % 5 == 0 { &a.coral } else { &a.cyan },
            Transform::from_translation(Vec3::from_array(particle_position(
                i,
                state.time_seconds,
                state.seed,
            )))
            .with_scale(Vec3::splat(0.018 + 0.019 * unit(state.seed ^ 91, i))),
        );
        commands.entity(e).insert(Particle(i));
    }
    if layout.fill > 0 {
        commands.spawn((
            Name::new("CUSTOM STRESS / synthetic fragment panel"),
            ChildOf(root),
            Mesh3d(a.cube.clone()),
            MeshMaterial3d(a.fill.clone()),
            Transform::from_xyz(0., 4.5, -depth / 2. + 0.6).with_scale(Vec3::new(
                width * 0.86,
                7.,
                0.1,
            )),
        ));
    }
    runtime.key = Some(key);
    runtime.root = Some(root);
}

#[allow(clippy::type_complexity, clippy::too_many_arguments)]
fn apply_pose(
    state: Res<SceneState>,
    assets: Res<LabAssets>,
    mut clock: ResMut<ClaudeAnimationClock>,
    mut cameras: Query<&mut Transform, With<LabCamera>>,
    mut particles: Query<(&Particle, &mut Transform), Without<LabCamera>>,
    mut props: Query<(&Turntable, &mut Transform), (Without<Particle>, Without<LabCamera>)>,
    mut captions: Query<&mut Text, With<Caption>>,
    mut fill: ResMut<Assets<StressFillMaterial>>,
) {
    clock.seconds = state.time_seconds;
    clock.animated = true;
    let pose = camera_pose(state.kind, state.time_seconds, state.seed);
    for mut camera in &mut cameras {
        *camera = Transform::from_translation(Vec3::from_array(pose.eye))
            .looking_at(Vec3::from_array(pose.target), Vec3::Y);
    }
    for (particle, mut transform) in &mut particles {
        transform.translation = Vec3::from_array(particle_position(
            particle.0,
            state.time_seconds,
            state.seed,
        ));
    }
    for (prop, mut transform) in &mut props {
        *transform = prop.rest;
        transform.rotation *= Quat::from_euler(
            EulerRot::XYZ,
            0.20 * (state.time_seconds + prop.phase).sin(),
            state.time_seconds * 0.25 + prop.phase,
            0.,
        );
    }
    let layout = scene_layout(state.kind, &state.load);
    let caption = format!(
        "{}  /  {} CLAUDE  /  {} LIGHTS  /  {} PARTICLES{}",
        state.caption.replace('·', "/").replace('—', "-"),
        layout.claudes,
        layout.lights,
        layout.particles,
        if layout.fill > 0 {
            format!("  /  CUSTOM SYNTHETIC FILL {}", layout.fill)
        } else {
            String::new()
        }
    );
    for mut text in &mut captions {
        if text.0 != caption {
            text.0.clone_from(&caption);
        }
    }
    if layout.fill > 0 {
        if let Some(mut material) = fill.get_mut(&assets.fill) {
            material.params = Vec4::new(
                layout.fill as f32,
                state.time_seconds,
                unit(state.seed, 71),
                0.,
            );
        }
    }
}

// BEGIN PURE SCENE CONTRACT
pub const CAMERA_CUT_TICK: u32 = 900;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct SceneLayout {
    claudes: u32,
    lights: u32,
    shadowed_lights: u32,
    particles: u32,
    material_displays: u32,
    fill: u32,
}
fn scene_layout(kind: SceneKind, load: &StressLoad) -> SceneLayout {
    let (claudes, lights, shadows, particles, material_displays) = match kind {
        SceneKind::Materials => (1, 6, 2, 0, 12),
        SceneKind::Geometry => (64, 4, 2, 0, 0),
        SceneKind::Lighting => (16, 8, 4, 4096, 0),
    };
    let lights = load.lights.unwrap_or(lights);
    SceneLayout {
        claudes: load.claudes.unwrap_or(claudes),
        lights,
        shadowed_lights: shadows.min(lights),
        particles: load.particles.unwrap_or(particles),
        material_displays,
        fill: load.fill,
    }
}
#[derive(Clone, Copy, Debug, Default, PartialEq)]
struct Placement {
    position: [f32; 3],
    scale: f32,
    yaw: f32,
    phase: f32,
}

// Integer hashing gives a stable seed stream without RNG version/state dependence.
fn unit(seed: u64, index: u32) -> f32 {
    let mut x = seed.wrapping_add((index as u64).wrapping_mul(0x9e3779b97f4a7c15));
    x = (x ^ (x >> 30)).wrapping_mul(0xbf58476d1ce4e5b9);
    x = (x ^ (x >> 27)).wrapping_mul(0x94d049bb133111eb);
    ((x ^ (x >> 31)) >> 40) as f32 / 16_777_216.0
}
fn claude_placement(kind: SceneKind, index: u32, count: u32, seed: u64) -> Placement {
    let columns = (count as f32).sqrt().ceil().max(1.) as u32;
    let rows = count.div_ceil(columns);
    let (spacing, scale) = match kind {
        SceneKind::Materials if count == 1 => (3.5, 1.0),
        SceneKind::Materials => (3.2, 0.64),
        SceneKind::Geometry => (3.2, 0.72),
        SceneKind::Lighting => (4.2, 0.83),
    };
    let x = (index % columns) as f32 - (columns - 1) as f32 * 0.5;
    let z = (index / columns) as f32 - (rows - 1) as f32 * 0.5;
    Placement {
        position: [x * spacing, 0.55, z * spacing],
        scale,
        yaw: -x * 0.025,
        phase: unit(seed, index) * std::f32::consts::TAU,
    }
}
#[derive(Clone, Copy, Debug, Default, PartialEq)]
struct CameraPose {
    eye: [f32; 3],
    target: [f32; 3],
}
fn camera_pose(kind: SceneKind, time: f32, seed: u64) -> CameraPose {
    let phase = unit(seed, 917) * 0.3;
    let after = time >= CAMERA_CUT_TICK as f32 / 120.;
    let (radius, height, target, amplitude) = match kind {
        SceneKind::Materials => (13.2, 5.1, 2.35, 0.27),
        SceneKind::Geometry => (34.0, 16.5, 1.2, 0.28),
        SceneKind::Lighting => (23.5, 10.0, 2.1, 0.30),
    };
    let angle = if after {
        -0.30 + 0.10 * ((time - 7.5) * 0.7 + phase).sin()
    } else {
        0.12 + amplitude * (time * 0.27 + phase).sin()
    };
    CameraPose {
        eye: [
            radius * angle.sin(),
            height + 0.35 * (time * 0.4 + phase).sin(),
            radius * angle.cos(),
        ],
        target: [0., target, if after { -0.3 } else { 0. }],
    }
}
fn particle_position(index: u32, time: f32, seed: u64) -> [f32; 3] {
    let phase = unit(seed, index) * std::f32::consts::TAU;
    let band = unit(seed ^ 0x7077, index);
    let radius = 8.0 + 4.0 * band;
    let angle = phase + time * (0.06 + 0.06 * band);
    [
        radius * angle.cos(),
        0.9 + 7.8 * unit(seed ^ 0x8bad, index) + 0.25 * (time + phase).sin(),
        radius * angle.sin() - 1.5,
    ]
}
// END PURE SCENE CONTRACT

// BEGIN PURE SCENE TESTS
#[cfg(test)]
mod contract_tests {
    use super::*;
    #[test]
    fn standard_scene_workloads_match_the_declared_profile() {
        let d = StressLoad::default();
        assert_eq!(
            scene_layout(SceneKind::Materials, &d),
            SceneLayout {
                claudes: 1,
                lights: 6,
                shadowed_lights: 2,
                particles: 0,
                material_displays: 12,
                fill: 0
            }
        );
        assert_eq!(
            scene_layout(SceneKind::Geometry, &d),
            SceneLayout {
                claudes: 64,
                lights: 4,
                shadowed_lights: 2,
                particles: 0,
                material_displays: 0,
                fill: 0
            }
        );
        assert_eq!(
            scene_layout(SceneKind::Lighting, &d),
            SceneLayout {
                claudes: 16,
                lights: 8,
                shadowed_lights: 4,
                particles: 4096,
                material_displays: 0,
                fill: 0
            }
        );
    }
    #[test]
    fn custom_counts_replace_defaults_and_reduce_shadow_lights_safely() {
        let c = scene_layout(
            SceneKind::Lighting,
            &StressLoad {
                claudes: Some(7),
                lights: Some(1),
                particles: Some(0),
                fill: 73,
            },
        );
        assert_eq!(
            (c.claudes, c.lights, c.shadowed_lights, c.particles, c.fill),
            (7, 1, 1, 0, 73)
        );
    }
    #[test]
    fn every_grid_instance_has_unique_finite_position_and_a_seeded_motion_phase() {
        for (kind, count) in [
            (SceneKind::Geometry, 64),
            (SceneKind::Lighting, 16),
            (SceneKind::Materials, 1),
        ] {
            let positions: Vec<_> = (0..count)
                .map(|i| claude_placement(kind, i, count, 21434))
                .collect();
            for (i, p) in positions.iter().enumerate() {
                assert!(p.scale > 0. && p.position.iter().all(|n| n.is_finite()));
                assert_eq!(*p, claude_placement(kind, i as u32, count, 21434));
                assert!(!positions[..i].iter().any(|q| q.position == p.position));
            }
            assert_ne!(
                positions[0].phase,
                claude_placement(kind, 0, count, 21435).phase
            );
        }
    }
    #[test]
    fn camera_cut_is_discontinuous_and_motion_continues_after_it() {
        let t = CAMERA_CUT_TICK as f32 / 120.;
        for kind in [
            SceneKind::Materials,
            SceneKind::Geometry,
            SceneKind::Lighting,
        ] {
            let before = camera_pose(kind, t - 1. / 120., 21434);
            let at = camera_pose(kind, t, 21434);
            let after = camera_pose(kind, t + 16. / 120., 21434);
            let distance = before
                .eye
                .iter()
                .zip(at.eye)
                .map(|(a, b)| (a - b) * (a - b))
                .sum::<f32>()
                .sqrt();
            assert!(
                distance > 2.,
                "a hard cut must not be an ordinary orbit step"
            );
            assert_ne!(at, after);
            assert_eq!(at, camera_pose(kind, t, 21434));
        }
    }
    #[test]
    fn particles_are_replayable_moving_and_seed_sensitive() {
        let p = particle_position(4095, 1.25, 21434);
        assert!(p.iter().all(|n| n.is_finite()));
        assert_eq!(p, particle_position(4095, 1.25, 21434));
        assert_ne!(p, particle_position(4095, 1.5, 21434));
        assert_ne!(p, particle_position(4095, 1.25, 21435));
        assert_ne!(p, particle_position(4094, 1.25, 21434));
    }
}
// END PURE SCENE TESTS

#[cfg(test)]
mod world_tests {
    use super::*;

    fn fixture() -> App {
        let mut app = App::new();
        app.insert_resource(Assets::<Mesh>::default())
            .insert_resource(Assets::<StandardMaterial>::default())
            .insert_resource(Assets::<StressFillMaterial>::default())
            .insert_resource(SceneState {
                seed: 21434,
                generation: 1,
                ..default()
            })
            .init_resource::<SceneRuntime>()
            .add_plugins(ClaudeAnimationPlugin)
            .add_systems(Startup, prepare_assets)
            .add_systems(
                Update,
                (rebuild_scene, apply_pose)
                    .chain()
                    .in_set(LabSceneSystems::Apply),
            )
            .configure_sets(Update, ClaudeSystems::Animate.after(LabSceneSystems::Apply));
        app.world_mut().spawn((LabCamera, Transform::default()));
        app
    }

    #[test]
    fn scene_rebuilds_match_actual_rendered_counts_and_reuse_all_assets() {
        let mut app = fixture();
        app.update();
        let mesh_count = app.world().resource::<Assets<Mesh>>().len();
        let material_count = app.world().resource::<Assets<StandardMaterial>>().len();
        let mut old_root = None;
        for (i, kind) in SceneKind::ALL.into_iter().enumerate() {
            {
                let mut state = app.world_mut().resource_mut::<SceneState>();
                state.kind = kind;
                state.generation = i as u64 + 2;
            }
            app.update();
            let world = app.world_mut();
            let root = world.resource::<SceneRuntime>().root.unwrap();
            if let Some(old) = old_root {
                assert!(world.get_entity(old).is_err());
            }
            old_root = Some(root);
            let layout = scene_layout(kind, &StressLoad::default());
            assert_eq!(
                world.query::<&LabInstance>().iter(world).count(),
                layout.claudes as usize
            );
            assert_eq!(
                world.query::<&PointLight>().iter(world).count()
                    + world.query::<&SpotLight>().iter(world).count(),
                layout.lights as usize
            );
            assert_eq!(
                world
                    .query::<&SpotLight>()
                    .iter(world)
                    .filter(|l| l.shadow_maps_enabled)
                    .count(),
                layout.shadowed_lights as usize
            );
            assert_eq!(
                world.query::<&MaterialDisplay>().iter(world).count(),
                layout.material_displays as usize
            );
            assert_eq!(
                world.query::<&Particle>().iter(world).count(),
                layout.particles as usize
            );
            assert_eq!(
                world
                    .query::<&MeshMaterial3d<StressFillMaterial>>()
                    .iter(world)
                    .count(),
                0
            );
            assert_eq!(world.resource::<Assets<Mesh>>().len(), mesh_count);
            assert_eq!(
                world.resource::<Assets<StandardMaterial>>().len(),
                material_count
            );
        }
    }

    #[test]
    fn analytic_ticks_keep_entities_but_generation_or_custom_load_rebuilds() {
        let mut app = fixture();
        app.update();
        let root = app.world().resource::<SceneRuntime>().root.unwrap();
        {
            let mut state = app.world_mut().resource_mut::<SceneState>();
            state.tick = 120;
            state.time_seconds = 1.;
            state.caption = "new caption".into();
        }
        app.update();
        assert_eq!(app.world().resource::<SceneRuntime>().root, Some(root));
        app.world_mut().resource_mut::<SceneState>().load = StressLoad {
            claudes: Some(7),
            lights: Some(1),
            particles: Some(13),
            fill: 73,
        };
        app.update();
        assert!(app.world().get_entity(root).is_err());
        let world = app.world_mut();
        assert_eq!(world.query::<&LabInstance>().iter(world).count(), 7);
        assert_eq!(world.query::<&LabLight>().iter(world).count(), 1);
        assert_eq!(world.query::<&Particle>().iter(world).count(), 13);
        assert_eq!(
            world
                .query::<&MeshMaterial3d<StressFillMaterial>>()
                .iter(world)
                .count(),
            1
        );
        let material = world
            .resource::<Assets<StressFillMaterial>>()
            .get(&world.resource::<LabAssets>().fill)
            .unwrap();
        assert_eq!(material.params.x, 73.);
        assert_eq!(material.params.y, 1.);
        let root = world.resource::<SceneRuntime>().root.unwrap();
        world.resource_mut::<SceneState>().generation += 1;
        app.update();
        assert!(app.world().get_entity(root).is_err());
    }

    #[test]
    fn thousands_of_particles_share_a_low_polygon_mesh() {
        let mut app = fixture();
        app.world_mut().resource_mut::<SceneState>().kind = SceneKind::Lighting;
        app.update();
        let world = app.world_mut();
        let handles: Vec<_> = world
            .query_filtered::<&Mesh3d, With<Particle>>()
            .iter(world)
            .map(|m| m.0.clone())
            .collect();
        assert_eq!(handles.len(), 4096);
        assert!(handles.iter().all(|h| h == &handles[0]));
        let mesh = world.resource::<Assets<Mesh>>().get(&handles[0]).unwrap();
        assert!(
            mesh.count_vertices() <= 32,
            "particles must not reuse the expensive hero/specimen sphere"
        );
    }
}
