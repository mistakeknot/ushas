use bevy::{ecs::world::CommandQueue, prelude::*};
use ushas_claude_model::{
    ClaudeAnimationClock, ClaudeAnimationPlugin, ClaudeAssets, MODEL_VERSION,
};

fn fixture(count: usize) -> (App, Vec<Entity>, Transform) {
    let mut app = App::new();
    app.add_plugins(ClaudeAnimationPlugin);
    let mut meshes = Assets::<Mesh>::default();
    let mut materials = Assets::<StandardMaterial>::default();
    let assets = ClaudeAssets::new(&mut meshes, &mut materials);
    let mut queue = CommandQueue::default();
    let rest = Transform::from_xyz(2.5, -1.0, -0.5).with_scale(Vec3::splat(0.7));
    let roots = (0..count)
        .map(|_| assets.spawn(&mut Commands::new(&mut queue, app.world()), rest))
        .collect();
    queue.apply(app.world_mut());
    app.insert_resource(meshes).insert_resource(materials);
    (app, roots, rest)
}

#[test]
fn cached_assets_preserve_the_old_solid_face_and_sunburst_for_sixty_four_instances() {
    let (mut app, roots, _) = fixture(64);
    assert_eq!(MODEL_VERSION, "claude-toy-v1");
    assert_eq!(roots.len(), 64);
    let world = app.world_mut();
    let names: Vec<_> = world
        .query::<(&Name, &Mesh3d)>()
        .iter(world)
        .map(|(n, _)| n.as_str().to_owned())
        .collect();
    assert_eq!(names.iter().filter(|n| *n == "white oval face").count(), 64);
    assert_eq!(names.iter().filter(|n| *n == "happy W mouth").count(), 64);
    assert_eq!(names.iter().filter(|n| *n == "curved tail").count(), 64);
    assert_eq!(
        names
            .iter()
            .filter(|n| *n == "long irregular coral ray")
            .count(),
        64 * 12
    );
    assert_eq!(names.len(), 64 * 41);
    // One template's assets serve every instance: no per-instance mesh/material allocation.
    assert_eq!(world.resource::<Assets<Mesh>>().len(), 21);
    assert_eq!(world.resource::<Assets<StandardMaterial>>().len(), 7);
    for (_, mesh) in world.resource::<Assets<Mesh>>().iter() {
        assert!(mesh.contains_attribute(Mesh::ATTRIBUTE_NORMAL));
    }
    for (_, material) in world.resource::<Assets<StandardMaterial>>().iter() {
        assert!(!material.unlit);
        assert_eq!(material.alpha_mode, AlphaMode::Opaque);
    }
}

#[test]
fn analytic_clock_preserves_original_motion_and_returns_exactly_to_authored_pose() {
    let (mut app, roots, rest) = fixture(1);
    app.world_mut()
        .resource_mut::<ClaudeAnimationClock>()
        .seconds = 1.25;
    app.update();
    assert_eq!(*app.world().get::<Transform>(roots[0]).unwrap(), rest);
    app.world_mut()
        .resource_mut::<ClaudeAnimationClock>()
        .animated = true;
    app.update();
    let moving = *app.world().get::<Transform>(roots[0]).unwrap();
    let expected = rest.rotation
        * Quat::from_rotation_y(0.52 * ((1.25 + rest.translation.x * 1.7) * 0.55).sin());
    assert_eq!(moving.translation, rest.translation);
    assert_eq!(moving.scale, rest.scale);
    assert!(moving.rotation.abs_diff_eq(expected, 1e-6));
    assert_ne!(moving.rotation, rest.rotation);
    // Replaying the same analytic time never integrates an additional delta.
    app.update();
    assert_eq!(*app.world().get::<Transform>(roots[0]).unwrap(), moving);
    app.world_mut()
        .resource_mut::<ClaudeAnimationClock>()
        .animated = false;
    app.update();
    assert_eq!(*app.world().get::<Transform>(roots[0]).unwrap(), rest);
}
