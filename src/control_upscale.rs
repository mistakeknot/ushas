//! Crop-aware bilinear control for a reduced main-pass viewport.
//!
//! Runs before EarlyPostProcess: the scene is expanded into the next main texture
//! before tonemapping and native-resolution UI. Bevy's final blit does not
//! understand MainPassResolutionOverride and cannot perform this expansion.

use bevy::camera::MainPassResolutionOverride;
use bevy::core_pipeline::schedule::{Core3d, Core3dSystems};
use bevy::prelude::*;
use bevy::render::camera::ExtractedCamera;
use bevy::render::render_resource::{
    binding_types::{sampler, texture_2d},
    BindGroupLayoutDescriptor, BindGroupLayoutEntries, CachedPipelineState, CachedRenderPipelineId,
    FragmentState, PipelineCache, RenderPipelineDescriptor, VertexState,
};
use bevy::render::renderer::{RenderContext, ViewQuery};
use bevy::render::sync_world::MainEntity;
use bevy::render::view::ViewTarget;
use bevy::render::RenderApp;
use bevy::shader::Shader;

use crate::{
    MetalFxEffectObservation, MetalFxEffectReason as Reason, MetalFxEffectState as State,
    MetalFxEffectStatus, MetalFxMode, MetalFxObservationFrame,
};

const SHADER: &str = r#"
struct VertexOutput {
    @builtin(position) position: vec4<f32>,
    @location(0) uv: vec2<f32>,
};
@vertex fn vs(@builtin(vertex_index) index: u32) -> VertexOutput {
    let uv = vec2<f32>(f32((index << 1u) & 2u), f32(index & 2u));
    var result: VertexOutput;
    result.position = vec4<f32>(uv.x * 2.0 - 1.0, 1.0 - uv.y * 2.0, 0.0, 1.0);
    result.uv = uv;
    return result;
}
@group(0) @binding(0) var content: texture_2d<f32>;
@group(0) @binding(1) var linear_sampler: sampler;
@fragment fn fs(in: VertexOutput) -> @location(0) vec4<f32> {
    return textureSample(content, linear_sampler, in.uv);
}
"#;

#[derive(Resource)]
struct ControlPipeline {
    shader: Handle<Shader>,
    layout: BindGroupLayoutDescriptor,
}

#[derive(Default)]
pub(crate) struct ControlCache {
    pipeline: Option<(wgpu::TextureFormat, CachedRenderPipelineId)>,
    input: Option<ControlInput>,
}

struct ControlInput {
    key: (u64, [u32; 2], wgpu::TextureFormat),
    texture: wgpu::Texture,
    bind_group: wgpu::BindGroup,
}

/// Install for the Disabled control or early Spatial/Temporal reconstruction.
/// Active fallback preserves the original effect observation, including failure.
pub(crate) fn install(app: &mut App) {
    if app.get_sub_app(RenderApp).is_none() {
        return;
    }
    let Some(mut shaders) = app.world_mut().get_resource_mut::<Assets<Shader>>() else {
        return;
    };
    let shader = shaders.add(Shader::from_wgsl(SHADER, "ushas/control_upscale.wgsl"));
    let mode = app
        .world()
        .get_resource::<crate::node::MetalFxConfig>()
        .map_or(MetalFxMode::Disabled, |config| config.mode);
    if let Some(render_app) = app.get_sub_app_mut(RenderApp) {
        render_app.insert_resource(ControlPipeline {
            shader,
            layout: BindGroupLayoutDescriptor::new(
                "ushas bilinear control layout",
                &BindGroupLayoutEntries::sequential(
                    wgpu::ShaderStages::FRAGMENT,
                    (
                        texture_2d(wgpu::TextureSampleType::Float { filterable: true }),
                        sampler(wgpu::SamplerBindingType::Filtering),
                    ),
                ),
            ),
        });
        let system = bilinear_control
            .after(Core3dSystems::MainPass)
            .before(Core3dSystems::EarlyPostProcess);
        if matches!(mode, MetalFxMode::Spatial | MetalFxMode::Temporal) {
            render_app.add_systems(Core3d, system.after(crate::MetalFxLabel));
        } else if mode == MetalFxMode::Disabled {
            render_app.add_systems(Core3d, system);
        }
    }
}

/// A single-view, full-viewport control. All failure exits precede the main
/// texture swap, so a pending pipeline leaves Bevy's existing output intact.
#[allow(clippy::too_many_arguments, clippy::type_complexity)]
fn bilinear_control(
    view: ViewQuery<(
        &MainEntity,
        &ExtractedCamera,
        &ViewTarget,
        Option<&MainPassResolutionOverride>,
    )>,
    cameras: Query<(&ExtractedCamera, &ViewTarget), With<Camera3d>>,
    request: Res<crate::effect_runtime::MetalFxRequestedEffect>,
    effect_config: Option<Res<crate::node::MetalFxConfig>>,
    frame: Res<MetalFxObservationFrame>,
    status: Res<MetalFxEffectStatus>,
    config: Res<ControlPipeline>,
    pipeline_cache: Res<PipelineCache>,
    mut cache: Local<ControlCache>,
    mut context: RenderContext,
) {
    let (main_entity, camera, target, resolution) = view.into_inner();
    let source = target.main_texture();
    let size = source.size();
    let output = [size.width, size.height];
    let content = resolution.map(|r| r.0.to_array()).unwrap_or(output);
    let view_id = main_entity.id().to_bits();
    let snapshot = status.snapshot(view_id, frame.0);
    let prior = snapshot.last_observation.as_ref();
    let action = control_action(
        effect_config
            .as_ref()
            .map_or(MetalFxMode::Disabled, |config| config.mode),
        frame.0,
        prior.map(|o| o.frame_id),
        prior.is_some_and(|o| o.state == State::OutputWritten),
    );
    if action == ControlAction::Bypass {
        return;
    }
    let publish = |state, reason| {
        if action != ControlAction::DisabledControl {
            return;
        }
        status.publish(MetalFxEffectObservation::new(
            frame.0,
            view_id,
            MetalFxMode::Disabled,
            MetalFxMode::Disabled,
            request.scale,
            content,
            output,
            state,
            Some(reason),
        ));
    };
    if let Some(reason) = crate::effect_runtime::view_scope_error(cameras.iter().take(2).count()) {
        publish(State::Unavailable, reason);
        return;
    }
    if let Err(reason) = crate::effect_runtime::observed_content_size(
        output,
        Some(content),
        camera
            .viewport
            .as_ref()
            .map(|v| (v.physical_position.to_array(), v.physical_size.to_array())),
    ) {
        publish(State::Unavailable, reason);
        return;
    }
    let content = match crop_extent(output, Some(content)) {
        Ok(Some(content)) => content,
        Ok(None) => {
            publish(State::Disabled, Reason::ModeDisabled);
            return;
        }
        Err(()) => {
            publish(State::Unavailable, Reason::InvalidDimensions);
            return;
        }
    };
    let format = target.main_texture_format();
    if !matches!(
        format,
        wgpu::TextureFormat::Rgba16Float
            | wgpu::TextureFormat::Rgba8Unorm
            | wgpu::TextureFormat::Rgba8UnormSrgb
            | wgpu::TextureFormat::Bgra8Unorm
            | wgpu::TextureFormat::Bgra8UnormSrgb
            | wgpu::TextureFormat::Rgb10a2Unorm
    ) || !source.usage().contains(wgpu::TextureUsages::COPY_SRC)
    {
        publish(State::Unavailable, Reason::UnsupportedFormat);
        return;
    }
    let pipeline_id = match cache.pipeline {
        Some((cached_format, id)) if cached_format == format => id,
        _ => {
            let id = pipeline_cache.queue_render_pipeline(RenderPipelineDescriptor {
                label: Some("ushas bilinear control".into()),
                layout: vec![config.layout.clone()],
                vertex: VertexState {
                    shader: config.shader.clone(),
                    entry_point: Some("vs".into()),
                    ..default()
                },
                fragment: Some(FragmentState {
                    shader: config.shader.clone(),
                    entry_point: Some("fs".into()),
                    targets: vec![Some(wgpu::ColorTargetState {
                        format,
                        blend: None,
                        write_mask: wgpu::ColorWrites::ALL,
                    })],
                    ..default()
                }),
                ..default()
            });
            cache.pipeline = Some((format, id));
            id
        }
    };
    let Some(pipeline) = pipeline_cache.get_render_pipeline(pipeline_id) else {
        if matches!(
            pipeline_cache.get_render_pipeline_state(pipeline_id),
            CachedPipelineState::Err(_)
        ) {
            publish(State::Failed, Reason::BlitPipelineFailed);
        } else {
            publish(State::Pending, Reason::BlitPipelinePending);
        }
        return;
    };
    let key = (view_id, content, format);
    if cache.input.as_ref().is_none_or(|input| input.key != key) {
        let device = context.render_device().wgpu_device();
        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("ushas bilinear content crop"),
            size: wgpu::Extent3d {
                width: content[0],
                height: content[1],
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format,
            usage: wgpu::TextureUsages::COPY_DST | wgpu::TextureUsages::TEXTURE_BINDING,
            view_formats: &[],
        });
        let texture_view = texture.create_view(&default());
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("ushas bilinear linear sampler"),
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..default()
        });
        let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("ushas bilinear content"),
            layout: &pipeline_cache.get_bind_group_layout(&config.layout),
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(&texture_view),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::Sampler(&sampler),
                },
            ],
        });
        cache.input = Some(ControlInput {
            key,
            texture,
            bind_group,
        });
    }
    let input = cache.input.as_ref().unwrap();
    context.command_encoder().copy_texture_to_texture(
        source.as_image_copy(),
        input.texture.as_image_copy(),
        wgpu::Extent3d {
            width: content[0],
            height: content[1],
            depth_or_array_layers: 1,
        },
    );
    // Everything that can skip has completed. The full destination is now
    // written before any later stage observes the newly selected main texture.
    let post_process = target.post_process_write();
    let attachments = [Some(wgpu::RenderPassColorAttachment {
        view: post_process.destination,
        depth_slice: None,
        resolve_target: None,
        ops: wgpu::Operations {
            load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT),
            store: wgpu::StoreOp::Store,
        },
    })];
    let mut pass = context
        .command_encoder()
        .begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("ushas_bilinear_control"),
            color_attachments: &attachments,
            ..default()
        });
    pass.set_pipeline(pipeline);
    pass.set_bind_group(0, &input.bind_group, &[]);
    pass.draw(0..3, 0..1);
    drop(pass);
    publish(State::Disabled, Reason::ModeDisabled);
}

#[derive(Debug, PartialEq)]
enum ControlAction {
    Bypass,
    DisabledControl,
    ActiveFallback,
}

fn control_action(
    mode: MetalFxMode,
    frame: u64,
    observed_frame: Option<u64>,
    output_written: bool,
) -> ControlAction {
    match mode {
        MetalFxMode::Disabled => ControlAction::DisabledControl,
        MetalFxMode::Spatial | MetalFxMode::Temporal
            if observed_frame != Some(frame) || !output_written =>
        {
            ControlAction::ActiveFallback
        }
        _ => ControlAction::Bypass,
    }
}

fn crop_extent(output: [u32; 2], content: Option<[u32; 2]>) -> Result<Option<[u32; 2]>, ()> {
    let content = content.unwrap_or(output);
    if output.contains(&0)
        || content.contains(&0)
        || content[0] > output[0]
        || content[1] > output[1]
    {
        return Err(());
    }
    Ok((content != output).then_some(content))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fallback_requires_this_frames_success_before_skipping_reconstruction() {
        assert_eq!(
            control_action(MetalFxMode::Disabled, 12, Some(12), false),
            ControlAction::DisabledControl
        );
        for mode in [MetalFxMode::Spatial, MetalFxMode::Temporal] {
            assert_eq!(
                control_action(mode, 12, Some(12), true),
                ControlAction::Bypass
            );
            assert_eq!(
                control_action(mode, 12, Some(11), true),
                ControlAction::ActiveFallback
            );
            assert_eq!(
                control_action(mode, 12, Some(12), false),
                ControlAction::ActiveFallback
            );
            assert_eq!(
                control_action(mode, 12, None, false),
                ControlAction::ActiveFallback
            );
        }
        assert_eq!(
            control_action(MetalFxMode::FrameInterpolation, 12, None, false),
            ControlAction::Bypass
        );
    }

    #[test]
    fn installation_tolerates_headless_and_minimal_render_apps() {
        let mut app = App::new();
        install(&mut app);
        app.insert_sub_app(RenderApp, bevy::app::SubApp::new());
        install(&mut app);
        assert!(!app
            .get_sub_app(RenderApp)
            .unwrap()
            .world()
            .contains_resource::<ControlPipeline>());
    }

    #[test]
    fn crop_uses_actual_content_extent_without_applying_scale_twice() {
        assert_eq!(
            crop_extent([1280, 720], Some([640, 360])),
            Ok(Some([640, 360]))
        );
        assert_eq!(
            crop_extent([1281, 721], Some([854, 481])),
            Ok(Some([854, 481]))
        );
    }

    #[test]
    fn native_extent_needs_no_extra_pass() {
        assert_eq!(crop_extent([1280, 720], None), Ok(None));
        assert_eq!(crop_extent([1280, 720], Some([1280, 720])), Ok(None));
    }

    #[test]
    fn invalid_content_cannot_be_copied_or_reported_as_a_control() {
        assert_eq!(crop_extent([0, 720], None), Err(()));
        assert_eq!(crop_extent([1280, 720], Some([0, 360])), Err(()));
        assert_eq!(crop_extent([1280, 720], Some([1281, 360])), Err(()));
        assert_eq!(crop_extent([1280, 720], Some([640, 721])), Err(()));
    }
}
