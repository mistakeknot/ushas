//! Procedural toy interpretation of vgel/thebes' happy Claude character.
//! See `../CHARACTER.md`. Geometry and animation remain `claude-toy-v1`.

use bevy::asset::RenderAssetUsages;
use bevy::mesh::Indices;
use bevy::prelude::*;
use bevy::render::render_resource::PrimitiveTopology;

pub const MODEL_VERSION: &str = "claude-toy-v1";

#[derive(Resource, Default)]
pub struct ClaudeAnimationClock {
    pub seconds: f32,
    pub animated: bool,
}

#[derive(SystemSet, Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ClaudeSystems {
    Animate,
}

pub struct ClaudeAnimationPlugin;
impl Plugin for ClaudeAnimationPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<ClaudeAnimationClock>()
            .add_systems(Update, animate.in_set(ClaudeSystems::Animate));
    }
}

#[derive(Component)]
pub struct ClaudeMotion {
    rest: Transform,
    kind: Joint,
    phase: f32,
}

#[derive(Clone, Copy)]
enum Joint {
    Root,
    Head,
    Arm(f32),
    Leg(f32),
    Tail,
}

impl ClaudeMotion {
    pub fn pose(&self, seconds: f32, animated: bool) -> Transform {
        let mut pose = self.rest;
        if animated {
            let t = seconds + self.phase;
            let rotation = match self.kind {
                Joint::Root => Quat::from_rotation_y(0.52 * (t * 0.55).sin()),
                Joint::Head => Quat::from_euler(
                    EulerRot::YXZ,
                    0.14 * (t * 1.05).sin(),
                    0.04 * (t * 0.8).sin(),
                    0.05 * (t * 1.3).sin(),
                ),
                Joint::Arm(side) => Quat::from_euler(
                    EulerRot::XYZ,
                    0.18 * (t * 1.5 + side).sin(),
                    0.04 * (t * 1.1).sin(),
                    side * 0.16 * (t * 1.35).sin(),
                ),
                Joint::Leg(side) => Quat::from_euler(
                    EulerRot::XYZ,
                    side * 0.10 * (t * 1.5).sin(),
                    0.0,
                    side * 0.025 * (t * 1.3).sin(),
                ),
                Joint::Tail => Quat::from_euler(
                    EulerRot::YXZ,
                    0.32 * (t * 1.2).sin(),
                    0.09 * (t * 1.1).sin(),
                    0.11 * (t * 1.6).sin(),
                ),
            };
            pose.rotation *= rotation;
        }
        pose
    }
}

fn animate(clock: Res<ClaudeAnimationClock>, mut parts: Query<(&ClaudeMotion, &mut Transform)>) {
    for (motion, mut transform) in &mut parts {
        let pose = motion.pose(clock.seconds, clock.animated);
        if *transform != pose {
            *transform = pose;
        }
    }
}

struct Part {
    parent: Option<usize>,
    name: &'static str,
    mesh: Option<Handle<Mesh>>,
    material: Option<Handle<StandardMaterial>>,
    transform: Transform,
    joint: Option<Joint>,
}

/// One immutable mesh/material template shared by any number of toy instances.
#[derive(Resource)]
pub struct ClaudeAssets {
    parts: Vec<Part>,
}
impl ClaudeAssets {
    /// Spawn with feet at local y=0 and face along +Z (about 3.3 wide, 4.7 tall).
    pub fn spawn(&self, commands: &mut Commands, transform: Transform) -> Entity {
        self.spawn_with_phase(commands, transform, transform.translation.x * 1.7)
    }

    /// Same geometry and analytic motion, with an explicit deterministic phase.
    pub fn spawn_with_phase(
        &self,
        commands: &mut Commands,
        transform: Transform,
        phase: f32,
    ) -> Entity {
        let mut entities = Vec::with_capacity(self.parts.len());
        for part in &self.parts {
            let rest = if part.parent.is_none() {
                transform
            } else {
                part.transform
            };
            let mut entity = commands.spawn((Name::new(part.name), rest, Visibility::default()));
            if let Some(parent) = part.parent {
                entity.insert(ChildOf(entities[parent]));
            }
            if let (Some(mesh), Some(material)) = (&part.mesh, &part.material) {
                entity.insert((Mesh3d(mesh.clone()), MeshMaterial3d(material.clone())));
            }
            if let Some(kind) = part.joint {
                entity.insert(ClaudeMotion { rest, kind, phase });
            }
            entities.push(entity.id());
        }
        entities[0]
    }

    pub fn new(meshes: &mut Assets<Mesh>, materials: &mut Assets<StandardMaterial>) -> Self {
        let root = 0;
        let phase = 0.0;
        let paint = |materials: &mut Assets<StandardMaterial>, color: Color| {
            materials.add(StandardMaterial {
                base_color: color,
                perceptual_roughness: 0.72,
                reflectance: 0.22,
                ..default()
            })
        };
        let coral = paint(materials, Color::srgb(0.88, 0.43, 0.30));
        let white = paint(materials, Color::srgb(0.99, 0.99, 0.96));
        let ink = paint(materials, Color::srgb(0.018, 0.012, 0.018));
        let purple = paint(materials, Color::srgb(0.70, 0.51, 0.80));
        let blue = paint(materials, Color::srgb(0.43, 0.71, 0.80));
        let brown = paint(materials, Color::srgb(0.31, 0.16, 0.12));
        let sole = paint(materials, Color::srgb(0.14, 0.075, 0.06));
        let sphere = meshes.add(Sphere::new(1.0).mesh().uv(24, 16));
        let mut model = Model {
            meshes,
            sphere,
            parts: vec![Part {
                parent: None,
                name: "Claude / vgel-thebes procedural toy",
                mesh: None,
                material: None,
                transform: Transform::default(),
                joint: Some(Joint::Root),
            }],
        };

        model.ball(
            root,
            "lavender sweatshirt",
            &purple,
            Vec3::new(0.0, 1.96, -0.025),
            Vec3::new(0.34, 0.60, 0.27),
        );
        model.ball(
            root,
            "blue waistband",
            &blue,
            Vec3::new(0.0, 1.37, -0.015),
            Vec3::new(0.35, 0.22, 0.25),
        );

        for side in [-1.0, 1.0] {
            let leg = model.joint(
                root,
                "trouser leg joint",
                Vec3::new(side * 0.19, 1.37, 0.0),
                Joint::Leg(side),
                phase,
            );
            let path = if side < 0.0 {
                vec![
                    Vec3::ZERO,
                    Vec3::new(-0.12, -0.33, -0.01),
                    Vec3::new(-0.10, -0.69, 0.02),
                    Vec3::new(-0.07, -1.08, 0.04),
                ]
            } else {
                vec![
                    Vec3::ZERO,
                    Vec3::new(0.16, -0.30, -0.01),
                    Vec3::new(0.27, -0.65, 0.01),
                    Vec3::new(0.35, -1.08, 0.04),
                ]
            };
            model.tube(
                leg,
                "soft blue trouser leg",
                &blue,
                &smooth_path(&path),
                (0.19, 0.17),
            );
            let ankle = *path.last().unwrap();
            model.ball(
                leg,
                "rounded trouser cuff",
                &blue,
                ankle,
                Vec3::new(0.18, 0.10, 0.20),
            );
            let shoe = ankle + Vec3::new(side * 0.03, -0.15, 0.16);
            model.ball(
                leg,
                "brown rounded shoe",
                &brown,
                shoe,
                Vec3::new(0.23, 0.135, 0.36),
            );
            model.ball(
                leg,
                "dark shoe sole",
                &sole,
                shoe - Vec3::Y * 0.065,
                Vec3::new(0.235, 0.06, 0.37),
            );
        }

        for side in [-1.0, 1.0] {
            let arm = model.joint(
                root,
                "sleeve joint",
                Vec3::new(side * 0.25, 2.27, 0.015),
                Joint::Arm(side),
                phase,
            );
            let path = if side < 0.0 {
                vec![
                    Vec3::ZERO,
                    Vec3::new(-0.13, -0.30, 0.015),
                    Vec3::new(-0.40, -0.49, 0.07),
                    Vec3::new(-0.58, -0.48, 0.13),
                ]
            } else {
                vec![
                    Vec3::ZERO,
                    Vec3::new(0.15, -0.19, 0.01),
                    Vec3::new(0.26, -0.47, 0.07),
                    Vec3::new(0.28, -0.65, 0.12),
                ]
            };
            model.tube(
                arm,
                "curved lavender sleeve",
                &purple,
                &smooth_path(&path),
                (0.18, 0.125),
            );
            let hand = *path.last().unwrap() + Vec3::new(side * 0.025, -0.075, 0.02);
            model.ball(
                arm,
                "orange mitten hand",
                &coral,
                hand,
                Vec3::new(0.18, 0.155, 0.145),
            );
            model.ball(
                arm,
                "orange thumb",
                &coral,
                hand + Vec3::new(-side * 0.10, -0.10, 0.055),
                Vec3::new(0.065, 0.10, 0.08),
            );
        }

        let tail = model.joint(
            root,
            "tail joint",
            Vec3::new(-0.22, 1.43, -0.21),
            Joint::Tail,
            phase,
        );
        let tail_path = smooth_path(&[
            Vec3::ZERO,
            Vec3::new(-0.33, -0.11, -0.06),
            Vec3::new(-0.51, -0.55, -0.08),
            Vec3::new(-0.72, -0.80, -0.05),
            Vec3::new(-1.01, -0.82, 0.015),
            Vec3::new(-1.28, -0.64, 0.09),
        ]);
        model.tube(tail, "curved tail", &coral, &tail_path, (0.10, 0.045));
        model.ball(
            tail,
            "rounded tail tip",
            &coral,
            *tail_path.last().unwrap(),
            Vec3::splat(0.045),
        );

        let head = model.joint(
            root,
            "sunburst head joint",
            Vec3::new(0.0, 3.10, 0.015),
            Joint::Head,
            phase,
        );
        model.ball(
            head,
            "coral head volume",
            &coral,
            Vec3::ZERO,
            Vec3::new(0.61, 0.69, 0.31),
        );
        // Authored irregular directions and lengths retain the reference's sunburst
        // silhouette. Long narrow rays, not a ring of equal round flower petals.
        for (angle, reach, radius, depth) in [
            (90.0_f32, 1.57, 0.125, -0.015),
            (116.0, 1.74, 0.16, 0.04),
            (144.0, 1.42, 0.115, -0.04),
            (179.0, 1.57, 0.15, 0.015),
            (210.0, 1.57, 0.16, -0.02),
            (234.0, 1.55, 0.15, 0.035),
            (263.0, 1.60, 0.16, 0.08),
            (293.0, 1.45, 0.16, 0.0),
            (322.0, 1.53, 0.145, -0.055),
            (348.0, 1.61, 0.17, 0.025),
            (14.0, 1.61, 0.12, -0.025),
            (48.0, 1.75, 0.17, 0.04),
        ] {
            let angle = angle.to_radians();
            let direction = Vec3::new(angle.cos(), angle.sin(), 0.0);
            let start = direction * 0.27;
            let end = direction * (reach - radius) + Vec3::Z * depth;
            let axis = end - start;
            let mesh = model.meshes.add(
                Capsule3d::new(radius, axis.length())
                    .mesh()
                    .longitudes(20)
                    .latitudes(10),
            );
            let transform = Transform::from_translation((start + end) * 0.5)
                .with_rotation(Quat::from_rotation_arc(Vec3::Y, axis.normalize()))
                .with_scale(Vec3::new(1.0, 1.0, 0.78));
            model.part(head, "long irregular coral ray", mesh, &coral, transform);
        }

        model.ball(
            head,
            "white oval face",
            &white,
            Vec3::new(0.0, 0.0, 0.35),
            Vec3::new(0.50, 0.60, 0.155),
        );
        for (name, points) in [
            (
                "left happy caret eye",
                vec![
                    Vec3::new(-0.29, 0.025, 0.0),
                    Vec3::new(-0.245, 0.18, 0.0),
                    Vec3::new(-0.20, 0.245, 0.0),
                    Vec3::new(-0.15, 0.13, 0.0),
                    Vec3::new(-0.105, 0.035, 0.0),
                ],
            ),
            (
                "right happy caret eye",
                vec![
                    Vec3::new(0.085, 0.15, 0.0),
                    Vec3::new(0.115, 0.30, 0.0),
                    Vec3::new(0.16, 0.345, 0.0),
                    Vec3::new(0.215, 0.27, 0.0),
                    Vec3::new(0.265, 0.13, 0.0),
                ],
            ),
            (
                "happy W mouth",
                vec![
                    Vec3::new(-0.17, -0.13, 0.0),
                    Vec3::new(-0.13, -0.25, 0.0),
                    Vec3::new(-0.065, -0.29, 0.0),
                    Vec3::new(0.0, -0.235, 0.0),
                    Vec3::new(0.02, -0.155, 0.0),
                    Vec3::new(0.055, -0.24, 0.0),
                    Vec3::new(0.13, -0.255, 0.0),
                    Vec3::new(0.19, -0.21, 0.0),
                    Vec3::new(0.185, -0.075, 0.0),
                ],
            ),
        ] {
            let mut path = smooth_path(&points);
            // Follow the convex face instead of burying a planar black curve in it.
            for point in &mut path {
                let r2 = (point.x / 0.50).powi(2) + (point.y / 0.60).powi(2);
                point.z = 0.35 + 0.155 * (1.0 - r2).max(0.0).sqrt() + 0.021;
            }
            model.tube(head, name, &ink, &path, (0.029, 0.029));
            for endpoint in [path[0], *path.last().unwrap()] {
                model.ball(
                    head,
                    "rounded facial stroke end",
                    &ink,
                    endpoint,
                    Vec3::splat(0.029),
                );
            }
        }
        Self { parts: model.parts }
    }
}

/// Compatibility constructor: preserves the smoke fixture's per-spawn asset allocation.
/// Applications with repeated instances should build `ClaudeAssets` once instead.
pub fn spawn(
    commands: &mut Commands,
    meshes: &mut Assets<Mesh>,
    materials: &mut Assets<StandardMaterial>,
    transform: Transform,
) -> Entity {
    ClaudeAssets::new(meshes, materials).spawn(commands, transform)
}

struct Model<'a> {
    meshes: &'a mut Assets<Mesh>,
    sphere: Handle<Mesh>,
    parts: Vec<Part>,
}
impl Model<'_> {
    fn part(
        &mut self,
        parent: usize,
        name: &'static str,
        mesh: Handle<Mesh>,
        material: &Handle<StandardMaterial>,
        transform: Transform,
    ) -> usize {
        let index = self.parts.len();
        self.parts.push(Part {
            parent: Some(parent),
            name,
            mesh: Some(mesh),
            material: Some(material.clone()),
            transform,
            joint: None,
        });
        index
    }
    fn ball(
        &mut self,
        parent: usize,
        name: &'static str,
        material: &Handle<StandardMaterial>,
        center: Vec3,
        size: Vec3,
    ) {
        self.part(
            parent,
            name,
            self.sphere.clone(),
            material,
            Transform::from_translation(center).with_scale(size),
        );
    }
    fn joint(
        &mut self,
        parent: usize,
        name: &'static str,
        position: Vec3,
        kind: Joint,
        _phase: f32,
    ) -> usize {
        let index = self.parts.len();
        self.parts.push(Part {
            parent: Some(parent),
            name,
            mesh: None,
            material: None,
            transform: Transform::from_translation(position),
            joint: Some(kind),
        });
        index
    }
    fn tube(
        &mut self,
        parent: usize,
        name: &'static str,
        material: &Handle<StandardMaterial>,
        path: &[Vec3],
        radii: (f32, f32),
    ) {
        let mesh = self.meshes.add(tube_mesh(path, radii));
        self.part(parent, name, mesh, material, Transform::default());
    }
}

fn smooth_path(control: &[Vec3]) -> Vec<Vec3> {
    let mut path = Vec::with_capacity((control.len() - 1) * 6 + 1);
    for i in 0..control.len() - 1 {
        let a = control[i.saturating_sub(1)];
        let b = control[i];
        let c = control[i + 1];
        let d = control[(i + 2).min(control.len() - 1)];
        for step in 0..6 {
            let t = step as f32 / 6.0;
            path.push(
                0.5 * ((2.0 * b)
                    + (-a + c) * t
                    + (2.0 * a - 5.0 * b + 4.0 * c - d) * t * t
                    + (-a + 3.0 * b - 3.0 * c + d) * t * t * t),
            );
        }
    }
    path.push(*control.last().unwrap());
    path
}

/// Closed, smoothly shaded tube; all curves in the model have nonzero segments.
fn tube_mesh(path: &[Vec3], radii: (f32, f32)) -> Mesh {
    const SIDES: usize = 12;
    let mut positions = Vec::with_capacity(path.len() * SIDES + 2);
    let mut normals = Vec::with_capacity(path.len() * SIDES + 2);
    let mut uv = Vec::with_capacity(path.len() * SIDES + 2);
    let mut indices = Vec::new();
    for (i, point) in path.iter().enumerate() {
        let tangent = (path[(i + 1).min(path.len() - 1)] - path[i.saturating_sub(1)]).normalize();
        let reference = if tangent.z.abs() < 0.95 {
            Vec3::Z
        } else {
            Vec3::Y
        };
        let normal = tangent.cross(reference).normalize();
        let binormal = tangent.cross(normal).normalize();
        let v = i as f32 / (path.len() - 1) as f32;
        let radius = radii.0 + (radii.1 - radii.0) * v;
        for side in 0..SIDES {
            let u = side as f32 / SIDES as f32;
            let angle = u * std::f32::consts::TAU;
            let outward = normal * angle.cos() + binormal * angle.sin();
            positions.push((*point + outward * radius).to_array());
            normals.push(outward.to_array());
            uv.push([u, v]);
            if i + 1 < path.len() {
                let a = (i * SIDES + side) as u32;
                let b = (i * SIDES + (side + 1) % SIDES) as u32;
                let c = a + SIDES as u32;
                let d = b + SIDES as u32;
                indices.extend_from_slice(&[a, b, c, b, d, c]);
            }
        }
    }
    for end in [0, path.len() - 1] {
        let center = positions.len() as u32;
        positions.push(path[end].to_array());
        let normal = if end == 0 {
            path[0] - path[1]
        } else {
            path[end] - path[end - 1]
        };
        normals.push(normal.normalize().to_array());
        uv.push([0.5, 0.5]);
        for side in 0..SIDES {
            let a = (end * SIDES + side) as u32;
            let b = (end * SIDES + (side + 1) % SIDES) as u32;
            indices.extend(if end == 0 {
                [center, b, a]
            } else {
                [center, a, b]
            });
        }
    }
    Mesh::new(
        PrimitiveTopology::TriangleList,
        RenderAssetUsages::default(),
    )
    .with_inserted_attribute(Mesh::ATTRIBUTE_POSITION, positions)
    .with_inserted_attribute(Mesh::ATTRIBUTE_NORMAL, normals)
    .with_inserted_attribute(Mesh::ATTRIBUTE_UV_0, uv)
    .with_inserted_indices(Indices::U32(indices))
}

#[cfg(test)]
mod tests {
    use super::*;
    use bevy::mesh::VertexAttributeValues;
    use std::collections::BTreeMap;
    #[test]
    fn curved_meshes_are_closed_finite_and_face_outward() {
        let path = smooth_path(&[
            Vec3::ZERO,
            Vec3::new(0.10, 0.35, 0.03),
            Vec3::new(0.35, 0.55, 0.12),
        ]);
        let mesh = tube_mesh(&path, (0.10, 0.05));
        let Some(VertexAttributeValues::Float32x3(positions)) =
            mesh.attribute(Mesh::ATTRIBUTE_POSITION)
        else {
            panic!("tube needs positions")
        };
        let Some(VertexAttributeValues::Float32x3(normals)) =
            mesh.attribute(Mesh::ATTRIBUTE_NORMAL)
        else {
            panic!("tube needs normals")
        };
        assert_eq!(positions.len(), normals.len());
        assert!(positions.iter().all(|v| Vec3::from_array(*v).is_finite()));
        assert!(normals
            .iter()
            .all(|v| (Vec3::from_array(*v).length() - 1.0).abs() < 1e-5));
        let indices: Vec<_> = mesh.indices().unwrap().iter().collect();
        let mut edges = BTreeMap::<(usize, usize), usize>::new();
        for triangle in indices.chunks_exact(3) {
            let [a, b, c] = [triangle[0], triangle[1], triangle[2]];
            let [pa, pb, pc] = [positions[a], positions[b], positions[c]].map(Vec3::from_array);
            let area = (pb - pa).cross(pc - pa);
            assert!(
                area.length_squared() > 1e-12,
                "zero-area triangles cannot carry a solid prepass surface"
            );
            let outward = [normals[a], normals[b], normals[c]]
                .map(Vec3::from_array)
                .into_iter()
                .sum::<Vec3>();
            assert!(
                area.dot(outward) > 0.0,
                "triangle winding must agree with lit surface normals"
            );
            for (a, b) in [(a, b), (b, c), (c, a)] {
                *edges.entry((a.min(b), a.max(b))).or_default() += 1;
            }
        }
        assert!(
            edges.values().all(|count| *count == 2),
            "every edge belongs to two triangles, including the end caps"
        );
    }
}
