//! Smoke clock adapter for the unchanged attributed `claude-toy-v1` geometry.
use bevy::prelude::*;
pub use ushas_claude_model::{spawn, MODEL_VERSION};
use ushas_claude_model::{ClaudeAnimationClock, ClaudeAnimationPlugin, ClaudeSystems};

pub struct ClaudePlugin;
impl Plugin for ClaudePlugin {
    fn build(&self, app: &mut App) {
        app.add_plugins(ClaudeAnimationPlugin)
            .add_systems(Update, update_clock.before(ClaudeSystems::Animate));
    }
}

fn update_clock(
    time: Res<Time<Real>>,
    config: Res<crate::RunConfig>,
    clock: Option<Res<crate::quality::PoseClock>>,
    mut animation: ResMut<ClaudeAnimationClock>,
) {
    animation.seconds = clock.as_ref().map_or_else(|| time.elapsed_secs(), |c| c.0);
    animation.animated = config.0.moving || clock.is_some();
}

#[cfg(test)]
mod tests {
    use super::*;
    use bevy::ecs::world::CommandQueue;
    use std::time::Duration;
    use ushas_claude_model::ClaudeMotion as Motion;

    fn fixture() -> (App, Entity, Transform) {
        let mut app = App::new();
        app.insert_resource(Time::<Real>::default());
        let mut config = crate::config::Config::parse(Vec::<String>::new()).unwrap();
        config.moving = false;
        app.insert_resource(crate::RunConfig(config));
        app.add_plugins(ClaudePlugin);
        let mut meshes = Assets::<Mesh>::default();
        let mut materials = Assets::<StandardMaterial>::default();
        let mut queue = CommandQueue::default();
        let rest = Transform::from_xyz(2.5, -1.0, -0.5).with_scale(Vec3::splat(0.7));
        let entity = spawn(
            &mut Commands::new(&mut queue, app.world()),
            &mut meshes,
            &mut materials,
            rest,
        );
        queue.apply(app.world_mut());
        app.insert_resource(meshes).insert_resource(materials);
        (app, entity, rest)
    }

    #[test]
    fn subject_is_opaque_lit_geometry_with_a_face_and_articulated_body() {
        let (mut app, _, _) = fixture();
        let world = app.world_mut();
        let mut geometry = world.query::<(&Name, &Mesh3d, &MeshMaterial3d<StandardMaterial>)>();
        let parts: Vec<_> = geometry
            .iter(world)
            .map(|(name, mesh, material)| {
                (name.as_str().to_owned(), mesh.0.clone(), material.0.clone())
            })
            .collect();
        assert!(
            parts.iter().any(|p| p.0 == "white oval face"),
            "a recognizable face must be real mesh geometry"
        );
        assert!(parts.iter().any(|p| p.0 == "happy W mouth"));
        assert!(parts.iter().any(|p| p.0 == "curved tail"));
        assert!(
            world.query::<&Motion>().iter(world).count() >= 7,
            "head, limbs and tail need real transforms for prepass motion"
        );
        let meshes = world.resource::<Assets<Mesh>>();
        let materials = world.resource::<Assets<StandardMaterial>>();
        for (_, mesh, material) in parts {
            let mesh = meshes.get(&mesh).unwrap();
            assert!(mesh.contains_attribute(Mesh::ATTRIBUTE_NORMAL));
            let material = materials.get(&material).unwrap();
            assert!(!material.unlit);
            assert_eq!(material.alpha_mode, AlphaMode::Opaque);
        }
    }

    #[test]
    fn animation_preserves_placement_and_returns_to_the_static_pose() {
        let (mut app, entity, rest) = fixture();
        app.world_mut()
            .resource_mut::<Time<Real>>()
            .advance_by(Duration::from_secs_f32(1.25));
        app.update();
        assert_eq!(*app.world().get::<Transform>(entity).unwrap(), rest);
        app.world_mut().resource_mut::<crate::RunConfig>().0.moving = true;
        app.update();
        let moving = *app.world().get::<Transform>(entity).unwrap();
        assert_eq!(moving.translation, rest.translation);
        assert_eq!(moving.scale, rest.scale);
        assert_ne!(
            moving.rotation, rest.rotation,
            "moving arm must change the rendered subject"
        );
        app.world_mut().resource_mut::<crate::RunConfig>().0.moving = false;
        app.update();
        assert_eq!(*app.world().get::<Transform>(entity).unwrap(), rest);
    }

    #[test]
    fn quality_clock_overrides_real_time_and_animates_without_motion_flag() {
        let (mut app, entity, _) = fixture();
        app.world_mut().insert_resource(crate::quality::PoseClock(0.75));
        app.update();
        let expected = app.world().get::<Motion>(entity).unwrap().pose(0.75, true);
        assert_eq!(*app.world().get::<Transform>(entity).unwrap(), expected);
        app.world_mut().resource_mut::<Time<Real>>()
            .advance_by(Duration::from_secs(50));
        app.update();
        assert_eq!(*app.world().get::<Transform>(entity).unwrap(), expected);
        app.world_mut().resource_mut::<crate::quality::PoseClock>().0 = 1.0;
        app.update();
        assert_ne!(*app.world().get::<Transform>(entity).unwrap(), expected);
    }
}
