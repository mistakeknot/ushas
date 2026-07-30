//! Prepass resolve passes.
//!
//! Bevy renders depth and motion vectors at full physical resolution, but
//! MetalFX wants them at the content size the frame was actually rendered at.
//! Both are resolved with a fullscreen triangle rather than
//! `copy_texture_to_texture`, for different reasons: the motion prepass texture
//! lacks `COPY_SRC`, and the depth target needs a format change on the way.
//!
//! Each pass keeps its pipeline and bind group in a `Mutex` on the node. This
//! module is a child of `node`, so it reaches those private fields directly
//! without widening their visibility.

use bevy::render::render_resource::{RenderPassDescriptor, TextureView};
use bevy::render::renderer::{RenderContext, RenderDevice};

use super::{MetalFxUpscaleNode, ResolvePipeline};

impl MetalFxUpscaleNode {
    /// Resolve full-res motion vectors into the content-sized RG16Float texture.
    pub(super) fn resolve_motion(
        &self,
        device: &RenderDevice,
        render_context: &mut RenderContext,
        src_texture: &bevy::render::render_resource::Texture,
        content_motion_view: &TextureView,
        content_w: u32,
        content_h: u32,
    ) {
        let motion_attachment_texture = src_texture;
        let mut mr = self.motion_resolve.lock().unwrap();
        if mr.is_none() {
            let wgpu_dev = device.wgpu_device();
            let shader = wgpu_dev.create_shader_module(wgpu::ShaderModuleDescriptor {
                label: Some("motion_resolve_shader"),
                source: wgpu::ShaderSource::Wgsl(include_str!("motion_resolve.wgsl").into()),
            });
            let bgl = wgpu_dev.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                label: Some("motion_resolve_bgl"),
                entries: &[wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: false },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                }],
            });
            let pipeline_layout =
                wgpu_dev.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                    label: Some("motion_resolve_layout"),
                    bind_group_layouts: &[Some(&bgl)],
                    // wgpu 29 replaced `push_constant_ranges` with a single
                    // `immediate_size`. Neither resolve pipeline uses push
                    // constants, so this is zero rather than a translation.
                    immediate_size: 0,
                });
            let pipeline = wgpu_dev.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some("motion_resolve_pipeline"),
                layout: Some(&pipeline_layout),
                vertex: wgpu::VertexState {
                    module: &shader,
                    entry_point: Some("vs_main"),
                    buffers: &[],
                    compilation_options: Default::default(),
                },
                fragment: Some(wgpu::FragmentState {
                    module: &shader,
                    entry_point: Some("fs_main"),
                    targets: &[Some(wgpu::ColorTargetState {
                        format: wgpu::TextureFormat::Rg16Float,
                        blend: None,
                        write_mask: wgpu::ColorWrites::ALL,
                    })],
                    compilation_options: Default::default(),
                }),
                primitive: wgpu::PrimitiveState {
                    topology: wgpu::PrimitiveTopology::TriangleList,
                    ..Default::default()
                },
                depth_stencil: None,
                multisample: Default::default(),
                multiview_mask: None,
                cache: None,
            });
            *mr = Some(ResolvePipeline {
                pipeline,
                bind_group_layout: bgl,
            });
        }
        let mr_ref = mr.as_ref().unwrap();

        let src_motion_view = motion_attachment_texture
            .create_view(&bevy::render::render_resource::TextureViewDescriptor::default());

        // Get or create cached bind group for motion resolve.
        let mut mr_bg = self.motion_resolve_bind_group.lock().unwrap();
        let need_new = match &*mr_bg {
            Some((src_id, _)) if *src_id == src_motion_view.id() => false,
            _ => true,
        };
        if need_new {
            let wgpu_dev = device.wgpu_device();
            let src_view_wgpu: &wgpu::TextureView = &src_motion_view;
            let bg = wgpu_dev.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("motion_resolve_bg"),
                layout: &mr_ref.bind_group_layout,
                entries: &[wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(src_view_wgpu),
                }],
            });
            *mr_bg = Some((src_motion_view.id(), bg));
        }
        let bind_group = &mr_bg.as_ref().unwrap().1;

        let mut pass = render_context
            .command_encoder()
            .begin_render_pass(&RenderPassDescriptor {
                label: Some("metalfx_motion_resolve"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: content_motion_view,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT),
                        store: wgpu::StoreOp::Store,
                    },
                    depth_slice: None,
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
        pass.set_pipeline(&mr_ref.pipeline);
        pass.set_bind_group(0, bind_group, &[]);
        pass.set_viewport(0.0, 0.0, content_w as f32, content_h as f32, 0.0, 1.0);
        pass.draw(0..3, 0..1);
    }

    /// Resolve full-res depth into the content-sized Depth32Float texture.
    ///
    /// Kept in its own scope by the caller: the render pass guard must drop
    /// before `as_hal_mut` runs for the MetalFX encode, or wgpu's snatch lock
    /// panics.
    pub(super) fn resolve_depth(
        &self,
        device: &RenderDevice,
        render_context: &mut RenderContext,
        src_texture: &bevy::render::render_resource::Texture,
        content_depth_view: &TextureView,
        content_w: u32,
        content_h: u32,
    ) {
        let depth_attachment_texture = src_texture;
        // Lazy-init depth resolve render pipeline.
        let mut dr = self.depth_resolve.lock().unwrap();
        if dr.is_none() {
            let wgpu_dev = device.wgpu_device();
            let shader = wgpu_dev.create_shader_module(wgpu::ShaderModuleDescriptor {
                label: Some("depth_resolve_shader"),
                source: wgpu::ShaderSource::Wgsl(include_str!("depth_resolve.wgsl").into()),
            });
            let bgl = wgpu_dev.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                label: Some("depth_resolve_bgl"),
                entries: &[wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Depth,
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                }],
            });
            let pipeline_layout =
                wgpu_dev.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                    label: Some("depth_resolve_layout"),
                    bind_group_layouts: &[Some(&bgl)],
                    // wgpu 29 replaced `push_constant_ranges` with a single
                    // `immediate_size`. Neither resolve pipeline uses push
                    // constants, so this is zero rather than a translation.
                    immediate_size: 0,
                });
            let pipeline = wgpu_dev.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some("depth_resolve_pipeline"),
                layout: Some(&pipeline_layout),
                vertex: wgpu::VertexState {
                    module: &shader,
                    entry_point: Some("vs_main"),
                    buffers: &[],
                    compilation_options: Default::default(),
                },
                fragment: Some(wgpu::FragmentState {
                    module: &shader,
                    entry_point: Some("fs_main"),
                    targets: &[],
                    compilation_options: Default::default(),
                }),
                primitive: wgpu::PrimitiveState {
                    topology: wgpu::PrimitiveTopology::TriangleList,
                    ..Default::default()
                },
                depth_stencil: Some(wgpu::DepthStencilState {
                    format: wgpu::TextureFormat::Depth32Float,
                    depth_write_enabled: Some(true),
                    depth_compare: Some(wgpu::CompareFunction::Always),
                    stencil: Default::default(),
                    bias: Default::default(),
                }),
                multisample: Default::default(),
                multiview_mask: None,
                cache: None,
            });
            *dr = Some(ResolvePipeline {
                pipeline,
                bind_group_layout: bgl,
            });
        }
        let dr_ref = dr.as_ref().unwrap();

        // Create source depth view (prepass texture — changes if prepass is recreated).
        // Destination view is stored in CachedState (stable across frames).
        let src_depth_view = depth_attachment_texture
            .create_view(&bevy::render::render_resource::TextureViewDescriptor::default());

        // Get or create cached bind group (keyed on src + dst TextureViewId).
        // dst_id is stable (stored in CachedState), src_id changes on prepass recreation.
        let mut dr_bg = self.depth_resolve_bind_group.lock().unwrap();
        let need_new_bg = match &*dr_bg {
            Some((src_id, dst_id, _))
                if *src_id == src_depth_view.id() && *dst_id == content_depth_view.id() =>
            {
                false
            }
            _ => true,
        };
        if need_new_bg {
            let wgpu_dev = device.wgpu_device();
            // Extract the raw wgpu TextureView from Bevy's wrapped type.
            let src_view_wgpu: &wgpu::TextureView = &src_depth_view;
            let bg = wgpu_dev.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("depth_resolve_bg"),
                layout: &dr_ref.bind_group_layout,
                entries: &[wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(src_view_wgpu),
                }],
            });
            *dr_bg = Some((src_depth_view.id(), content_depth_view.id(), bg));
        }
        let bind_group = &dr_bg.as_ref().unwrap().2;

        // Dispatch depth resolve render pass.
        let mut pass = render_context
            .command_encoder()
            .begin_render_pass(&RenderPassDescriptor {
                label: Some("metalfx_depth_resolve"),
                color_attachments: &[],
                depth_stencil_attachment: Some(
                    bevy::render::render_resource::RenderPassDepthStencilAttachment {
                        view: content_depth_view,
                        depth_ops: Some(bevy::render::render_resource::Operations {
                            // Clear to 1.0 (near plane in Bevy's reversed-Z).
                            // Safe default: out-of-viewport fragments read as near-plane
                            // rather than far-plane (infinity), preventing edge ghosting.
                            load: bevy::render::render_resource::LoadOp::Clear(1.0),
                            store: bevy::render::render_resource::StoreOp::Store,
                        }),
                        stencil_ops: None,
                    },
                ),
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
        pass.set_pipeline(&dr_ref.pipeline);
        pass.set_bind_group(0, bind_group, &[]);
        pass.set_viewport(0.0, 0.0, content_w as f32, content_h as f32, 0.0, 1.0);
        pass.draw(0..3, 0..1);
        // pass drops here → render encoder ends
    }
}
