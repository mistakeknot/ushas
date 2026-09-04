//! Bridge render decisions to the shared effect registry.

use bevy::camera::MainPassResolutionOverride;
use bevy::core_pipeline::schedule::Core3d;
use bevy::core_pipeline::upscaling::upscaling;
use bevy::prelude::*;
use bevy::render::camera::ExtractedCamera;
use bevy::render::extract_resource::{ExtractResource, ExtractResourcePlugin};
use bevy::render::renderer::ViewQuery;
use bevy::render::sync_world::MainEntity;
use bevy::render::view::ViewTarget;
use bevy::render::RenderApp;

use crate::{MetalFxEffectReason, MetalFxEffectState};
use crate::{MetalFxEffectStatus, MetalFxMode};

/// Monotonic application frame identity carried into the render world.
#[derive(Resource, Clone, Copy, Default, ExtractResource)]
pub struct MetalFxObservationFrame(pub u64);

/// Original application request, retained when build-time fallback changes config.
#[derive(Resource, Clone, Copy, ExtractResource)]
pub(crate) struct MetalFxRequestedEffect {
    pub mode: MetalFxMode,
    pub scale: f32,
    pub available: bool,
}

impl ExtractResource for MetalFxEffectStatus {
    type Source = Self;

    fn extract_resource(source: &Self) -> Self {
        source.clone()
    }
}

pub(crate) fn install(app: &mut App, mode: MetalFxMode, scale: f32) {
    app.init_resource::<MetalFxEffectStatus>();
    app.init_resource::<MetalFxObservationFrame>();
    let request = MetalFxRequestedEffect {
        mode,
        scale,
        available: crate::is_available(),
    };
    app.insert_resource(request);
    app.add_systems(First, advance_observation_frame);
    let status = app.world().resource::<MetalFxEffectStatus>().clone();
    let frame = *app.world().resource::<MetalFxObservationFrame>();
    if let Some(render_app) = app.get_sub_app_mut(RenderApp) {
        render_app.insert_resource(status);
        render_app.insert_resource(frame);
        render_app.insert_resource(request);
        render_app.add_systems(Core3d, observe_inactive_view.after(upscaling));
        app.add_plugins((
            ExtractResourcePlugin::<MetalFxEffectStatus>::default(),
            ExtractResourcePlugin::<MetalFxObservationFrame>::default(),
            ExtractResourcePlugin::<MetalFxRequestedEffect>::default(),
        ));
    }
}

fn advance_observation_frame(mut frame: ResMut<MetalFxObservationFrame>) {
    frame.0 = frame.0.saturating_add(1);
}

pub(crate) const fn compiled_mode(requested: MetalFxMode) -> MetalFxMode {
    match requested {
        MetalFxMode::Temporal if !cfg!(feature = "temporal") => MetalFxMode::Spatial,
        MetalFxMode::FrameInterpolation if !cfg!(feature = "frame-interpolation") => {
            if cfg!(feature = "temporal") {
                MetalFxMode::Temporal
            } else {
                MetalFxMode::Spatial
            }
        }
        mode => mode,
    }
}

#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
pub(crate) fn observed_content_size(
    output: [u32; 2],
    content_override: Option<[u32; 2]>,
    viewport: Option<([u32; 2], [u32; 2])>,
) -> Result<[u32; 2], MetalFxEffectReason> {
    if output.contains(&0) {
        return Err(MetalFxEffectReason::InvalidDimensions);
    }
    if viewport.is_some_and(|(origin, size)| origin != [0, 0] || size != output) {
        return Err(MetalFxEffectReason::UnsupportedViewport);
    }
    let content = content_override.unwrap_or(output);
    if content.contains(&0) || content[0] > output[0] || content[1] > output[1] {
        return Err(MetalFxEffectReason::InvalidDimensions);
    }
    Ok(content)
}

#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
pub(crate) fn view_scope_error(active_views: usize) -> Option<MetalFxEffectReason> {
    match active_views {
        0 => Some(MetalFxEffectReason::NoRenderView),
        1 => None,
        _ => Some(MetalFxEffectReason::MultipleViewsUnsupported),
    }
}

fn inactive_state(
    mode: MetalFxMode,
    available: bool,
) -> Option<(MetalFxEffectState, MetalFxEffectReason)> {
    if mode == MetalFxMode::Disabled {
        Some((
            MetalFxEffectState::Disabled,
            MetalFxEffectReason::ModeDisabled,
        ))
    } else if !available {
        Some((
            MetalFxEffectState::Unavailable,
            if cfg!(target_os = "macos") {
                MetalFxEffectReason::FrameworkUnavailable
            } else {
                MetalFxEffectReason::UnsupportedPlatform
            },
        ))
    } else {
        None
    }
}

fn publish_inactive_observation(
    status: &MetalFxEffectStatus,
    observation: crate::MetalFxEffectObservation,
) {
    // The reduced-resolution control runs before this observer and publishes
    // its own terminal decision. Preserve failures as well as success; a later
    // bypass observation must not erase a missing control blit.
    if observation.requested_mode == MetalFxMode::Disabled && observation.requested_scale < 1.0 {
        let snapshot = status.snapshot(observation.view_id, observation.frame_id);
        if snapshot.last_observation.is_some_and(|existing| {
            existing.frame_id == observation.frame_id
                && existing.requested_mode == observation.requested_mode
                && existing.requested_scale.to_bits() == observation.requested_scale.to_bits()
                && existing.content_size == observation.content_size
                && existing.output_size == observation.output_size
        }) {
            return;
        }
    }
    status.publish(observation);
}

/// A bypass decision belongs to a rendered view, not just an enabled plugin.
/// With no eligible view this system does not run and snapshots stay `NoRender`.
#[allow(clippy::type_complexity)]
fn observe_inactive_view(
    view: ViewQuery<(
        &MainEntity,
        &ViewTarget,
        &ExtractedCamera,
        Option<&MainPassResolutionOverride>,
    )>,
    request: Res<MetalFxRequestedEffect>,
    frame: Res<MetalFxObservationFrame>,
    status: Res<MetalFxEffectStatus>,
    scale: Option<Res<crate::MetalFxRenderScale>>,
) {
    let Some((state, reason)) = inactive_state(request.mode, request.available) else {
        return;
    };
    let (main_entity, target, camera, resolution_override) = view.into_inner();
    let size = target.main_texture().size();
    let output = [size.width, size.height];
    let content = resolution_override
        .map(|resolution| resolution.0.to_array())
        .or_else(|| camera.physical_viewport_size.map(|size| size.to_array()))
        .unwrap_or(output);
    publish_inactive_observation(
        &status,
        crate::MetalFxEffectObservation::new(
            frame.0,
            main_entity.id().to_bits(),
            request.mode,
            MetalFxMode::Disabled,
            scale.map_or(request.scale, |scale| scale.0),
            content,
            output,
            state,
            Some(reason),
        ),
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{MetalFxEffectStatus, MetalFxMode, MetalFxPlugin};

    #[test]
    fn inactive_observer_preserves_a_reduced_controls_terminal_decision() {
        let status = MetalFxEffectStatus::default();
        for terminal in [MetalFxEffectState::Pending, MetalFxEffectState::Failed] {
            let control = crate::MetalFxEffectObservation::new(
                12,
                42,
                MetalFxMode::Disabled,
                MetalFxMode::Disabled,
                0.5,
                [640, 360],
                [1280, 720],
                terminal,
                Some(MetalFxEffectReason::BlitPipelinePending),
            );
            status.publish(control);
            publish_inactive_observation(
                &status,
                crate::MetalFxEffectObservation::new(
                    12,
                    42,
                    MetalFxMode::Disabled,
                    MetalFxMode::Disabled,
                    0.5,
                    [640, 360],
                    [1280, 720],
                    MetalFxEffectState::Disabled,
                    Some(MetalFxEffectReason::ModeDisabled),
                ),
            );
            assert_eq!(status.snapshot(42, 12).state(), terminal);
        }
        // A decision from a previous frame cannot suppress the current view's
        // bypass observation when the control pass did not run this frame.
        publish_inactive_observation(
            &status,
            crate::MetalFxEffectObservation::new(
                13,
                42,
                MetalFxMode::Disabled,
                MetalFxMode::Disabled,
                0.5,
                [640, 360],
                [1280, 720],
                MetalFxEffectState::Disabled,
                Some(MetalFxEffectReason::ModeDisabled),
            ),
        );
        assert_eq!(
            status.snapshot(42, 13).state(),
            MetalFxEffectState::Disabled
        );
    }

    #[test]
    fn scaler_scope_fails_closed_when_the_view_is_missing_or_ambiguous() {
        assert_eq!(view_scope_error(0), Some(MetalFxEffectReason::NoRenderView));
        assert_eq!(view_scope_error(1), None);
        assert_eq!(
            view_scope_error(2),
            Some(MetalFxEffectReason::MultipleViewsUnsupported)
        );
    }

    #[test]
    fn effect_surface_exists_without_a_renderer_in_every_mode() {
        for mode in [
            MetalFxMode::Disabled,
            MetalFxMode::Spatial,
            MetalFxMode::Temporal,
        ] {
            let mut app = App::new();
            app.add_plugins(MetalFxPlugin { mode, ..default() });
            assert!(app.world().contains_resource::<MetalFxEffectStatus>());
            assert!(app.world().contains_resource::<MetalFxObservationFrame>());
        }
    }

    #[test]
    fn no_render_does_not_become_a_successful_effect() {
        let mut app = App::new();
        app.add_plugins(MetalFxPlugin {
            mode: MetalFxMode::Disabled,
            ..default()
        });
        app.update();
        let first = app.world().resource::<MetalFxObservationFrame>().0;
        app.update();
        let frame = app.world().resource::<MetalFxObservationFrame>().0;
        assert!(frame > first);
        assert_eq!(
            app.world()
                .resource::<MetalFxEffectStatus>()
                .snapshot(42, frame)
                .state(),
            crate::MetalFxEffectState::NoRender
        );
    }

    #[test]
    fn compiled_modes_never_choose_a_missing_feature() {
        assert_eq!(compiled_mode(MetalFxMode::Spatial), MetalFxMode::Spatial);
        assert_eq!(compiled_mode(MetalFxMode::Disabled), MetalFxMode::Disabled);
        assert_eq!(
            compiled_mode(MetalFxMode::Temporal),
            if cfg!(feature = "temporal") {
                MetalFxMode::Temporal
            } else {
                MetalFxMode::Spatial
            }
        );
        assert_eq!(
            compiled_mode(MetalFxMode::FrameInterpolation),
            if cfg!(feature = "frame-interpolation") {
                MetalFxMode::FrameInterpolation
            } else if cfg!(feature = "temporal") {
                MetalFxMode::Temporal
            } else {
                MetalFxMode::Spatial
            }
        );
    }

    #[test]
    fn content_size_reports_the_rendered_override_and_rejects_invalid_layouts() {
        assert_eq!(
            observed_content_size([1920, 1080], None, None),
            Ok([1920, 1080])
        );
        assert_eq!(
            observed_content_size([1920, 1080], Some([960, 540]), None),
            Ok([960, 540])
        );
        assert_eq!(
            observed_content_size([1920, 1080], Some([960, 540]), Some(([0, 0], [1920, 1080]))),
            Ok([960, 540])
        );
        for override_size in [[0, 540], [1921, 1080], [960, 1081]] {
            assert_eq!(
                observed_content_size([1920, 1080], Some(override_size), None),
                Err(MetalFxEffectReason::InvalidDimensions)
            );
        }
        assert_eq!(
            observed_content_size([0, 1080], None, None),
            Err(MetalFxEffectReason::InvalidDimensions)
        );
        for viewport in [([1, 0], [1920, 1080]), ([0, 0], [960, 540])] {
            assert_eq!(
                observed_content_size([1920, 1080], None, Some(viewport)),
                Err(MetalFxEffectReason::UnsupportedViewport)
            );
        }
    }

    #[test]
    fn bypass_and_unavailable_are_explicitly_distinct() {
        for available in [false, true] {
            assert_eq!(
                inactive_state(MetalFxMode::Disabled, available),
                Some((
                    MetalFxEffectState::Disabled,
                    MetalFxEffectReason::ModeDisabled
                ))
            );
        }
        assert_eq!(inactive_state(MetalFxMode::Spatial, true), None);
        assert_eq!(
            inactive_state(MetalFxMode::Spatial, false).unwrap().0,
            MetalFxEffectState::Unavailable
        );
    }
}
