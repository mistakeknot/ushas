//! Experimental per-view GPU elapsed intervals. Never a validated governor input.
//!
//! The marker passes preserve destination color, but add work and may perturb
//! tile scheduling. Scope excludes work preceding Core3d, presentation, and
//! unrelated cameras. Compare output and overhead, then verify coverage in a
//! Metal System Trace before assigning stronger semantics.

use crate::MetalFxMode;
use bevy::prelude::Resource;
use std::collections::VecDeque;
use std::sync::{Arc, Mutex};
use std::time::Instant;

const HISTORY_CAPACITY: usize = 240;

/// State of the explicitly enabled timing experiment.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum ExperimentalTimingStatus {
    #[default]
    Pending,
    Unavailable,
    Failed,
    /// Query values exist; render coverage and presentation safety remain unvalidated.
    ObservedUnvalidated,
}

/// Original encode-time identity, retained across asynchronous completion.
#[derive(Debug, Clone, PartialEq)]
pub struct ExperimentalTimingIdentity {
    pub frame_id: u64,
    /// Main-world camera entity bits, including its generation.
    pub view_id: u64,
    /// Increases when this view's mode, scale, or dimensions change.
    pub configuration_generation: u64,
    /// The frozen adaptive context epoch, only when it matches this actual view/configuration.
    pub adaptive_epoch: Option<u64>,
    pub mode: MetalFxMode,
    pub render_scale: f32,
    /// Actual main-pass content viewport, before upscaling.
    pub input_size: [u32; 2],
    pub output_size: [u32; 2],
    pub encoded_at: Instant,
}

/// Experimental data only. There is deliberately no conversion to AdaptiveObservation.
#[derive(Debug, Clone)]
pub struct ExperimentalTimingObservation {
    pub identity: ExperimentalTimingIdentity,
    pub observed_at: Instant,
    /// Begin-marker start/end followed by end-marker start/end.
    pub raw_ticks: [u64; 4],
    pub marker_ms: [Option<f64>; 2],
    pub gpu_elapsed_ms: Option<f64>,
    pub status: ExperimentalTimingStatus,
}

/// Snapshot of a bounded shared history; observers never wait on the GPU.
#[derive(Debug, Clone, Default)]
pub struct ExperimentalTimingSnapshot {
    pub status: ExperimentalTimingStatus,
    pub reason: Option<&'static str>,
    pub observations: Vec<ExperimentalTimingObservation>,
    pub dropped_samples: u64,
}

#[derive(Default)]
struct History {
    status: ExperimentalTimingStatus,
    reason: Option<&'static str>,
    observations: VecDeque<ExperimentalTimingObservation>,
    dropped_samples: u64,
}

/// Clone this resource into main and render worlds to inspect the opt-in experiment.
#[derive(Resource, Clone, Default)]
pub struct ExperimentalFrameTiming(Arc<Mutex<History>>);

impl ExperimentalFrameTiming {
    pub fn snapshot(&self) -> ExperimentalTimingSnapshot {
        self.0
            .lock()
            .map(|history| ExperimentalTimingSnapshot {
                status: history.status,
                reason: history.reason,
                observations: history.observations.iter().cloned().collect(),
                dropped_samples: history.dropped_samples,
            })
            .unwrap_or_default()
    }

    fn publish(&self, observation: ExperimentalTimingObservation) {
        if let Ok(mut history) = self.0.lock() {
            history.status = observation.status;
            history.reason = if observation.gpu_elapsed_ms.is_some() {
                Some("experimental marker envelope; rendered coverage unvalidated")
            } else {
                Some("invalid, missing, or stale GPU queries")
            };
            if history.observations.len() == HISTORY_CAPACITY {
                history.observations.pop_front();
            }
            history.observations.push_back(observation);
        }
    }

    fn status(&self, status: ExperimentalTimingStatus, reason: &'static str, dropped: bool) {
        if let Ok(mut history) = self.0.lock() {
            history.status = status;
            history.reason = Some(reason);
            if dropped {
                history.dropped_samples = history.dropped_samples.saturating_add(1);
            }
        }
    }
}

fn elapsed_ms(ticks: [u64; 2], period_ns: f32) -> Option<f64> {
    if ticks[0] == 0
        || ticks[1] == u64::MAX
        || ticks[1] <= ticks[0]
        || !period_ns.is_finite()
        || period_ns <= 0.0
    {
        return None;
    }
    Some((ticks[1] - ticks[0]) as f64 * f64::from(period_ns) / 1_000_000.0)
}

use bevy::prelude::{Query, Res, ResMut};
use bevy::render::camera::ExtractedCamera;
use bevy::render::renderer::{RenderContext, RenderDevice, RenderQueue, ViewQuery};
use bevy::render::sync_world::MainEntity;
use bevy::render::view::ViewTarget;
use std::collections::HashMap;
use std::sync::atomic::{AtomicU8, Ordering};
use std::time::Duration;

const MAX_IN_FLIGHT: usize = 8;
const MAX_SAMPLE_AGE: Duration = Duration::from_secs(2);
fn accepted_elapsed(
    ticks: [u64; 4],
    period: f32,
    previous_end: u64,
    age: Duration,
    both_markers_encoded: bool,
    map_succeeded: bool,
) -> Option<f64> {
    if !map_succeeded
        || !both_markers_encoded
        || age > MAX_SAMPLE_AGE
        || ticks[0] <= previous_end
        || ticks[2] < ticks[1]
    {
        return None;
    }
    elapsed_ms([ticks[0], ticks[1]], period)?;
    elapsed_ms([ticks[2], ticks[3]], period)?;
    elapsed_ms([ticks[0], ticks[3]], period)
        .filter(|elapsed| *elapsed <= MAX_SAMPLE_AGE.as_secs_f64() * 1000.0)
}
fn timing_content(
    allocated: [u32; 2],
    content: Option<[u32; 2]>,
    scale: Option<f32>,
) -> ([u32; 2], f32) {
    let content = content.unwrap_or(allocated);
    if content == allocated || allocated.contains(&0) {
        return (content, 1.0);
    }
    let scale = scale
        .filter(|scale| scale.is_finite() && *scale > 0.0 && *scale <= 1.0)
        .filter(|scale| {
            allocated.map(|dimension| (dimension as f32 * scale).round() as u32) == content
        })
        .unwrap_or(content[0] as f32 / allocated[0] as f32);
    (content, scale)
}
const MARKER_SHADER: &str = r#"
@vertex fn vs(@builtin(vertex_index) index: u32) -> @builtin(position) vec4<f32> {
    let p = array<vec2<f32>, 3>(vec2(-1.0, -1.0), vec2(3.0, -1.0), vec2(-1.0, 3.0));
    return vec4(p[index], 0.0, 1.0);
}
@fragment fn fs() -> @location(0) vec4<f32> { return vec4(0.0); }
"#;

/// Explicitly opt in to unvalidated marker observations. This never enables adaptation.
/// Add after `MetalFxPlugin` and configure the renderer's requested timestamp feature.
pub struct ExperimentalFrameTimingPlugin;

impl bevy::app::Plugin for ExperimentalFrameTimingPlugin {
    fn build(&self, app: &mut bevy::app::App) {
        use bevy::core_pipeline::{schedule::Core3d, upscaling::upscaling, Core3dSystems};
        use bevy::prelude::IntoScheduleConfigs;
        use bevy::render::renderer::{RenderGraph, RenderGraphSystems};
        app.init_resource::<ExperimentalFrameTiming>();
        let sink = app.world().resource::<ExperimentalFrameTiming>().clone();
        let Some(render_app) = app.get_sub_app_mut(bevy::render::RenderApp) else {
            sink.status(
                ExperimentalTimingStatus::Unavailable,
                "RenderApp is missing",
                false,
            );
            return;
        };
        render_app
            .insert_resource(sink)
            .init_resource::<ExperimentalFrameTimingState>()
            .add_systems(
                Core3d,
                (
                    begin_view.before(Core3dSystems::Prepass),
                    end_view.after(upscaling).after(crate::MetalFxLabel),
                ),
            )
            .add_systems(
                RenderGraph,
                (
                    harvest.in_set(RenderGraphSystems::Begin),
                    arm_completion.in_set(RenderGraphSystems::Finish),
                ),
            );
    }
}

/// Request explicitly through `RenderPlugin` settings before device creation.
pub const fn requested_features() -> wgpu::Features {
    wgpu::Features::TIMESTAMP_QUERY
}

enum Phase {
    Idle,
    Begun,
    Encoded,
    WorkloadPending(Arc<AtomicU8>),
    Mapping(Arc<AtomicU8>),
}

struct Slot {
    queries: wgpu::QuerySet,
    resolve: wgpu::Buffer,
    readback: wgpu::Buffer,
    identity: Option<ExperimentalTimingIdentity>,
    phase: Phase,
    last_end_tick: u64,
    both_markers_encoded: bool,
}

#[derive(Clone, PartialEq)]
struct Configuration {
    view_id: u64,
    mode: MetalFxMode,
    scale_bits: u32,
    input_size: [u32; 2],
    output_size: [u32; 2],
    input_format: wgpu::TextureFormat,
    output_format: wgpu::TextureFormat,
}

/// Render-world scratch state. Initialize only when this experiment is requested.
#[derive(Resource, Default)]
pub struct ExperimentalFrameTimingState {
    slots: Vec<Slot>,
    pipelines: HashMap<wgpu::TextureFormat, wgpu::RenderPipeline>,
    configuration: Option<Configuration>,
    generation: u64,
}

fn allowed_format(format: wgpu::TextureFormat) -> bool {
    matches!(
        format,
        wgpu::TextureFormat::Rgba16Float
            | wgpu::TextureFormat::Rgba8Unorm
            | wgpu::TextureFormat::Rgba8UnormSrgb
            | wgpu::TextureFormat::Bgra8Unorm
            | wgpu::TextureFormat::Bgra8UnormSrgb
            | wgpu::TextureFormat::Rgb10a2Unorm
    )
}

fn pipeline(device: &wgpu::Device, format: wgpu::TextureFormat) -> wgpu::RenderPipeline {
    let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("ushas experimental destination-preserving marker"),
        source: wgpu::ShaderSource::Wgsl(MARKER_SHADER.into()),
    });
    let preserve = wgpu::BlendComponent {
        src_factor: wgpu::BlendFactor::Zero,
        dst_factor: wgpu::BlendFactor::One,
        operation: wgpu::BlendOperation::Add,
    };
    device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some("ushas experimental marker"),
        layout: None,
        vertex: wgpu::VertexState {
            module: &shader,
            entry_point: Some("vs"),
            compilation_options: Default::default(),
            buffers: &[],
        },
        primitive: Default::default(),
        depth_stencil: None,
        multisample: Default::default(),
        fragment: Some(wgpu::FragmentState {
            module: &shader,
            entry_point: Some("fs"),
            compilation_options: Default::default(),
            targets: &[Some(wgpu::ColorTargetState {
                format,
                blend: Some(wgpu::BlendState {
                    color: preserve,
                    alpha: preserve,
                }),
                write_mask: wgpu::ColorWrites::ALL,
            })],
        }),
        multiview_mask: None,
        cache: None,
    })
}

fn new_slot(device: &wgpu::Device) -> Slot {
    Slot {
        queries: device.create_query_set(&wgpu::QuerySetDescriptor {
            label: Some("ushas view frame queries"),
            ty: wgpu::QueryType::Timestamp,
            count: 4,
        }),
        resolve: device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("ushas view query resolve"),
            size: 256,
            usage: wgpu::BufferUsages::QUERY_RESOLVE | wgpu::BufferUsages::COPY_SRC,
            mapped_at_creation: false,
        }),
        readback: device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("ushas view query readback"),
            size: 32,
            usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        }),
        identity: None,
        phase: Phase::Idle,
        last_end_tick: 0,
        both_markers_encoded: false,
    }
}

fn marker(
    context: &mut RenderContext,
    view: &wgpu::TextureView,
    pipeline: &wgpu::RenderPipeline,
    queries: &wgpu::QuerySet,
    index: u32,
    identity: &ExperimentalTimingIdentity,
) {
    let label = format!(
        "ushas_view_timing_{} frame={} view={} generation={}",
        if index == 0 { "begin" } else { "end" },
        identity.frame_id,
        identity.view_id,
        identity.configuration_generation
    );
    let attachments = [Some(wgpu::RenderPassColorAttachment {
        view,
        depth_slice: None,
        resolve_target: None,
        ops: wgpu::Operations {
            load: wgpu::LoadOp::Load,
            store: wgpu::StoreOp::Store,
        },
    })];
    let mut pass = context
        .command_encoder()
        .begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some(&label),
            color_attachments: &attachments,
            timestamp_writes: Some(wgpu::RenderPassTimestampWrites {
                query_set: queries,
                beginning_of_pass_write_index: Some(index),
                end_of_pass_write_index: Some(index + 1),
            }),
            ..Default::default()
        });
    pass.set_pipeline(pipeline);
    pass.draw(0..3, 0..1);
}

/// Run before `Core3dSystems::Prepass`. Only one active camera is supported.
#[allow(clippy::too_many_arguments)]
pub(crate) fn begin_view(
    mut context: RenderContext,
    view: ViewQuery<(
        &ViewTarget,
        &MainEntity,
        &ExtractedCamera,
        Option<&bevy::camera::MainPassResolutionOverride>,
    )>,
    cameras: Query<&ExtractedCamera>,
    frame: Res<crate::MetalFxObservationFrame>,
    config: Option<Res<crate::node::MetalFxConfig>>,
    request: Option<Res<crate::effect_runtime::MetalFxRequestedEffect>>,
    adaptive_context: Option<Res<crate::MetalFxAdaptiveContext>>,
    sink: Res<ExperimentalFrameTiming>,
    mut state: ResMut<ExperimentalFrameTimingState>,
) {
    if !context
        .render_device()
        .features()
        .contains(wgpu::Features::TIMESTAMP_QUERY)
    {
        sink.status(
            ExperimentalTimingStatus::Unavailable,
            "TIMESTAMP_QUERY not enabled on RenderDevice",
            false,
        );
        return;
    }
    if cameras
        .iter()
        .filter(|c| c.physical_target_size.is_some_and(|s| s.x > 0 && s.y > 0))
        .count()
        != 1
    {
        sink.status(
            ExperimentalTimingStatus::Unavailable,
            "experimental timing supports exactly one active camera",
            false,
        );
        return;
    }
    let (target, main_entity, camera, resolution) = view.into_inner();
    let Some(output_format) = target.out_texture_view_format() else {
        sink.status(
            ExperimentalTimingStatus::Pending,
            "view output not prepared",
            false,
        );
        return;
    };
    if target.out_texture().is_none()
        || !allowed_format(target.main_texture_format())
        || !allowed_format(output_format)
    {
        sink.status(
            ExperimentalTimingStatus::Unavailable,
            "unsupported marker attachment format or missing output",
            false,
        );
        return;
    }
    let size = target.main_texture().size();
    let (input_size, render_scale) = timing_content(
        [size.width, size.height],
        resolution.map(|r| r.0.to_array()),
        config
            .as_ref()
            .map(|c| c.render_scale)
            .or_else(|| request.as_ref().map(|r| r.scale)),
    );
    let output_size = camera
        .physical_target_size
        .map(|v| [v.x, v.y])
        .unwrap_or([0, 0]);
    let configuration = Configuration {
        view_id: main_entity.id().to_bits(),
        mode: config
            .as_ref()
            .map(|c| c.mode)
            .unwrap_or(MetalFxMode::Disabled),
        scale_bits: render_scale.to_bits(),
        input_size,
        output_size,
        input_format: target.main_texture_format(),
        output_format,
    };
    if state.configuration.as_ref() != Some(&configuration) {
        state.generation = state.generation.saturating_add(1);
        state.configuration = Some(configuration.clone());
    }
    let adaptive_epoch = adaptive_context
        .as_ref()
        .map(|context| context.snapshot())
        .filter(|snapshot| {
            snapshot.view_id == Some(configuration.view_id)
                && snapshot.mode == configuration.mode
                && snapshot.render_scale.to_bits() == configuration.scale_bits
                && snapshot.output_size == configuration.output_size
        })
        .map(|snapshot| snapshot.epoch);
    let identity = ExperimentalTimingIdentity {
        frame_id: frame.0,
        view_id: configuration.view_id,
        configuration_generation: state.generation,
        adaptive_epoch,
        mode: configuration.mode,
        render_scale: f32::from_bits(configuration.scale_bits),
        input_size: configuration.input_size,
        output_size,
        encoded_at: Instant::now(),
    };
    if state.slots.iter().any(|slot| {
        slot.identity
            .as_ref()
            .is_some_and(|id| id.frame_id == frame.0 && id.view_id == identity.view_id)
    }) {
        sink.status(
            ExperimentalTimingStatus::Failed,
            "duplicate view/frame timing request",
            true,
        );
        return;
    }
    let device = context.render_device().wgpu_device();
    for format in [target.main_texture_format(), output_format] {
        state
            .pipelines
            .entry(format)
            .or_insert_with(|| pipeline(device, format));
    }
    let available = state
        .slots
        .iter()
        .position(|slot| matches!(slot.phase, Phase::Idle));
    let index = match available {
        Some(index) => index,
        None if state.slots.len() < MAX_IN_FLIGHT => {
            state.slots.push(new_slot(device));
            state.slots.len() - 1
        }
        None => {
            sink.status(
                ExperimentalTimingStatus::Pending,
                "query ring is full; sample skipped without waiting",
                true,
            );
            return;
        }
    };
    let pipeline = state.pipelines.get(&target.main_texture_format()).unwrap();
    // Use the raw view, not get_color_attachment: the latter consumes Bevy's clear bookkeeping.
    marker(
        &mut context,
        target.main_texture_view(),
        pipeline,
        &state.slots[index].queries,
        0,
        &identity,
    );
    state.slots[index].identity = Some(identity);
    state.slots[index].phase = Phase::Begun;
    state.slots[index].both_markers_encoded = false;
}

/// Run after Bevy `upscaling` and `MetalFxLabel`, including in Disabled mode.
pub fn end_view(
    mut context: RenderContext,
    view: ViewQuery<(&ViewTarget, &MainEntity)>,
    frame: Res<crate::MetalFxObservationFrame>,
    mut state: ResMut<ExperimentalFrameTimingState>,
) {
    let (target, entity) = view.into_inner();
    let Some(index) = state.slots.iter().position(|slot| {
        matches!(slot.phase, Phase::Begun)
            && slot
                .identity
                .as_ref()
                .is_some_and(|id| id.frame_id == frame.0 && id.view_id == entity.id().to_bits())
    }) else {
        return;
    };
    let (Some(output), Some(format)) = (target.out_texture(), target.out_texture_view_format())
    else {
        return;
    };
    let Some(pipeline) = state.pipelines.get(&format) else {
        return;
    };
    marker(
        &mut context,
        output,
        pipeline,
        &state.slots[index].queries,
        2,
        state.slots[index].identity.as_ref().unwrap(),
    );
    state.slots[index].phase = Phase::Encoded;
    state.slots[index].both_markers_encoded = true;
}

/// Run in root `RenderGraphSystems::Finish`, after the frame's workload was submitted.
pub fn arm_completion(
    queue: Res<RenderQueue>,
    mut state: ResMut<ExperimentalFrameTimingState>,
    sink: Res<ExperimentalFrameTiming>,
) {
    for slot in &mut state.slots {
        if matches!(slot.phase, Phase::Begun) {
            slot.phase = Phase::Encoded;
            sink.status(
                ExperimentalTimingStatus::Failed,
                "end marker did not encode; queries will be rejected",
                true,
            );
        }
        if matches!(slot.phase, Phase::Encoded) {
            let ready = Arc::new(AtomicU8::new(0));
            let callback_ready = ready.clone();
            queue.on_submitted_work_done(move || callback_ready.store(1, Ordering::Release));
            slot.phase = Phase::WorkloadPending(ready);
        }
    }
}

/// Run in root `RenderGraphSystems::Begin`. This never polls or waits for the GPU.
pub fn harvest(
    device: Res<RenderDevice>,
    queue: Res<RenderQueue>,
    mut state: ResMut<ExperimentalFrameTimingState>,
    sink: Res<ExperimentalFrameTiming>,
) {
    for slot in &mut state.slots {
        match &slot.phase {
            Phase::WorkloadPending(ready) if ready.load(Ordering::Acquire) == 1 => {
                let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
                    label: Some("ushas completed-frame query resolve"),
                });
                encoder.resolve_query_set(&slot.queries, 0..4, &slot.resolve, 0);
                encoder.copy_buffer_to_buffer(&slot.resolve, 0, &slot.readback, 0, 32);
                queue.submit([encoder.finish()]);
                let mapped = Arc::new(AtomicU8::new(0));
                let callback_mapped = mapped.clone();
                slot.readback
                    .map_async(wgpu::MapMode::Read, .., move |result| {
                        callback_mapped.store(if result.is_ok() { 1 } else { 2 }, Ordering::Release)
                    });
                slot.phase = Phase::Mapping(mapped);
            }
            Phase::Mapping(mapped) if mapped.load(Ordering::Acquire) != 0 => {
                let success = mapped.load(Ordering::Acquire) == 1;
                let mut ticks = [0u64; 4];
                if success {
                    let bytes = slot.readback.get_mapped_range(..);
                    for (output, bytes) in ticks.iter_mut().zip(bytes.chunks_exact(8)) {
                        *output = u64::from_le_bytes(bytes.try_into().unwrap());
                    }
                    drop(bytes);
                    slot.readback.unmap();
                }
                if let Some(identity) = slot.identity.take() {
                    let period = queue.get_timestamp_period();
                    let marker_ms = [
                        elapsed_ms([ticks[0], ticks[1]], period),
                        elapsed_ms([ticks[2], ticks[3]], period),
                    ];
                    let gpu_elapsed_ms = accepted_elapsed(
                        ticks,
                        period,
                        slot.last_end_tick,
                        identity.encoded_at.elapsed(),
                        slot.both_markers_encoded,
                        success,
                    );
                    sink.publish(ExperimentalTimingObservation {
                        identity,
                        observed_at: Instant::now(),
                        raw_ticks: ticks,
                        marker_ms,
                        gpu_elapsed_ms,
                        status: if gpu_elapsed_ms.is_some() {
                            ExperimentalTimingStatus::ObservedUnvalidated
                        } else {
                            ExperimentalTimingStatus::Failed
                        },
                    });
                    if gpu_elapsed_ms.is_some() {
                        slot.last_end_tick = slot.last_end_tick.max(ticks[3]);
                    }
                }
                slot.phase = Phase::Idle;
            }
            Phase::WorkloadPending(_) | Phase::Mapping(_)
                if slot
                    .identity
                    .as_ref()
                    .is_some_and(|identity| identity.encoded_at.elapsed() > MAX_SAMPLE_AGE) =>
            {
                sink.status(
                    ExperimentalTimingStatus::Failed,
                    "GPU observation overdue; retaining in-flight storage until completion",
                    false,
                );
            }
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn acceptance_requires_complete_fresh_non_overlapping_queries() {
        let good = [1000, 1100, 1900, 2000];
        let accept = |ticks| accepted_elapsed(ticks, 1.0, 0, Duration::ZERO, true, true);
        assert_eq!(accept(good), Some(0.001));
        for bad in [
            [0, 1100, 1900, 2000],
            [1000, 0, 1900, 2000],
            [1000, 1100, 0, 2000],
            [1000, 1100, 1900, 0],
            [1000, 999, 1900, 2000],
            [1000, 1100, 1900, 1899],
            [1000, 1950, 1900, 2000],
            [1000, 1100, 1900, u64::MAX],
        ] {
            assert_eq!(accept(bad), None, "invalid query tuple {bad:?}");
        }
        assert_eq!(
            accepted_elapsed(good, 1.0, 2000, Duration::ZERO, true, true),
            None
        );
        assert_eq!(
            accepted_elapsed(good, 1.0, 0, Duration::ZERO, false, true),
            None
        );
        assert_eq!(
            accepted_elapsed(good, 1.0, 0, Duration::ZERO, true, false),
            None
        );
        assert_eq!(
            accepted_elapsed(
                good,
                1.0,
                0,
                MAX_SAMPLE_AGE + Duration::from_millis(1),
                true,
                true
            ),
            None
        );
        for period in [0.0, -1.0, f32::NAN, f32::INFINITY] {
            assert_eq!(
                accepted_elapsed(good, period, 0, Duration::ZERO, true, true),
                None
            );
        }
        assert_eq!(accept([1, 2, 3, 2_000_000_002]), None);
    }

    #[test]
    fn disabled_control_timing_uses_the_rendered_content_and_scale() {
        assert_eq!(
            timing_content([1280, 720], Some([640, 360]), Some(0.5)),
            ([640, 360], 0.5)
        );
        assert_eq!(
            timing_content([1280, 720], None, Some(0.5)),
            ([1280, 720], 1.0)
        );
        assert_eq!(
            timing_content([1280, 720], Some([960, 540]), Some(0.5)),
            ([960, 540], 0.75)
        );
    }

    fn sample(frame_id: u64, generation: u64, scale: f32) -> ExperimentalTimingObservation {
        ExperimentalTimingObservation {
            identity: ExperimentalTimingIdentity {
                frame_id,
                view_id: 42,
                configuration_generation: generation,
                adaptive_epoch: Some(generation),
                mode: MetalFxMode::Temporal,
                render_scale: scale,
                input_size: [1280, 720],
                output_size: [2560, 1440],
                encoded_at: Instant::now(),
            },
            observed_at: Instant::now(),
            raw_ticks: [1_000_000, 1_010_000, 1_990_000, 2_000_000],
            marker_ms: [Some(0.01), Some(0.01)],
            gpu_elapsed_ms: Some(1.0),
            status: ExperimentalTimingStatus::ObservedUnvalidated,
        }
    }

    #[test]
    fn delayed_results_keep_the_configuration_and_frame_that_encoded_them() {
        let sink = ExperimentalFrameTiming::default();
        let original = sample(7, 1, 0.5);
        let identity = original.identity.clone();
        sink.publish(sample(9, 2, 0.75));
        sink.publish(original);
        let state = sink.snapshot();
        assert_eq!(state.observations.len(), 2);
        assert_eq!(state.observations[1].identity, identity);
        assert_eq!(
            state.observations[1].status,
            ExperimentalTimingStatus::ObservedUnvalidated
        );
    }

    #[test]
    fn history_is_bounded_under_an_unread_consumer() {
        let sink = ExperimentalFrameTiming::default();
        for frame in 1..=300 {
            sink.publish(sample(frame, 1, 0.5));
        }
        let state = sink.snapshot();
        assert_eq!(state.observations.len(), HISTORY_CAPACITY);
        assert_eq!(state.observations.first().unwrap().identity.frame_id, 61);
    }

    #[test]
    fn invalid_queries_are_missing_not_zero_cost() {
        for ticks in [[0, 0], [0, 10], [10, 9], [10, 10], [10, u64::MAX]] {
            assert_eq!(elapsed_ms(ticks, 1.0), None);
        }
        for period in [0.0, -1.0, f32::INFINITY, f32::NAN] {
            assert_eq!(elapsed_ms([10, 20], period), None);
        }
        assert_eq!(elapsed_ms([1_000_000, 2_000_000], 1.0), Some(1.0));
    }
}
