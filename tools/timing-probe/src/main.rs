//! Headless capability and timestamp-boundary probe. No Bevy or MetalFX frame is measured.
use serde_json::json;
use std::sync::mpsc;
use std::time::{Duration, Instant};
use ushas_timing_probe::interval;
use wgpu::util::DeviceExt;

const ELEMENTS: u32 = 262_144;
const PASSES: u32 = 3;
const SHADER: &str = r#"
@group(0) @binding(0) var<storage, read_write> data: array<u32>;
@group(0) @binding(1) var<uniform> iterations: vec4<u32>;
@compute @workgroup_size(64)
fn main(@builtin(global_invocation_id) id: vec3<u32>) {
    var value = data[id.x];
    for (var i = 0u; i < iterations.x; i += 1u) {
        value = (value ^ (value >> 13u)) * 1664525u + 1013904223u;
    }
    data[id.x] = value;
}
"#;

fn expected(mut value: u32, iterations: u32) -> u32 {
    for _ in 0..iterations * PASSES {
        value = (value ^ (value >> 13))
            .wrapping_mul(1664525)
            .wrapping_add(1013904223);
    }
    value
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let deferred_resolve = std::env::args().any(|a| a == "--deferred-resolve");
    let pass_descriptors_only = std::env::args().any(|a| a == "--pass-descriptors-only");
    let instance = wgpu::Instance::new(wgpu::InstanceDescriptor {
        backends: wgpu::Backends::METAL,
        ..wgpu::InstanceDescriptor::new_without_display_handle()
    });
    let adapter =
        pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions::default()))?;
    let info = adapter.get_info();
    let timestamp_features = wgpu::Features::TIMESTAMP_QUERY
        | wgpu::Features::TIMESTAMP_QUERY_INSIDE_ENCODERS
        | wgpu::Features::TIMESTAMP_QUERY_INSIDE_PASSES;
    let supported = adapter.features() & timestamp_features;
    let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
        label: Some("ushas timing feasibility probe"),
        required_features: supported,
        ..Default::default()
    }))?;
    println!(
        "{}",
        json!({
            "kind": "capabilities", "adapter": info.name, "backend": format!("{:?}", info.backend),
            "wgpu": "29.0.4", "supported_timestamp_features": format!("{supported:?}"),
            "enabled_timestamp_features": format!("{:?}", device.features()),
            "timestamp_period_ns": queue.get_timestamp_period(),
            "scope": "headless synthetic dependent compute chain; not a rendered frame",
        })
    );
    if std::env::args().any(|a| a == "--capabilities-only") {
        return Ok(());
    }
    if !supported.contains(wgpu::Features::TIMESTAMP_QUERY) {
        return Err("Metal adapter does not support TIMESTAMP_QUERY".into());
    }
    let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("deterministic integer workload"),
        source: wgpu::ShaderSource::Wgsl(SHADER.into()),
    });
    let pipeline = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
        label: Some("timing_probe_compute"),
        layout: None,
        module: &shader,
        entry_point: Some("main"),
        compilation_options: Default::default(),
        cache: None,
    });
    let mut next_frame = 1u64;
    let mut valid_count = 0;
    let mut invalid_count = 0;
    // Small batches exercise delayed asynchronous map delivery while bounding allocation and work.
    for encoder_timestamps in [false, true] {
        if encoder_timestamps && pass_descriptors_only {
            continue;
        }
        if encoder_timestamps
            && !supported.contains(wgpu::Features::TIMESTAMP_QUERY_INSIDE_ENCODERS)
        {
            continue;
        }
        for batch in 0..2 {
            let (sender, receiver) = mpsc::channel();
            let mut pending = Vec::new();
            for index in 0..4 {
                let frame_id = next_frame;
                next_frame += 1;
                let iterations: u32 = if index % 2 == 0 { 16 } else { 256 };
                let cpu_delay_ms = if batch == 1 { 20 } else { 0 };
                let start_cpu = Instant::now();
                let data = device.create_buffer(&wgpu::BufferDescriptor {
                    label: Some("dependency shared by all three command buffers"),
                    size: u64::from(ELEMENTS) * 4,
                    usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC,
                    mapped_at_creation: false,
                });
                let params = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                    label: Some("iterations"),
                    contents: &[iterations.to_le_bytes(), [0; 4], [0; 4], [0; 4]].concat(),
                    usage: wgpu::BufferUsages::UNIFORM,
                });
                let bind = device.create_bind_group(&wgpu::BindGroupDescriptor {
                    label: Some("workload"),
                    layout: &pipeline.get_bind_group_layout(0),
                    entries: &[
                        wgpu::BindGroupEntry {
                            binding: 0,
                            resource: data.as_entire_binding(),
                        },
                        wgpu::BindGroupEntry {
                            binding: 1,
                            resource: params.as_entire_binding(),
                        },
                    ],
                });
                let queries = device.create_query_set(&wgpu::QuerySetDescriptor {
                    label: Some("per-frame queries, never reused in flight"),
                    ty: wgpu::QueryType::Timestamp,
                    count: PASSES * 2,
                });
                let resolved = device.create_buffer(&wgpu::BufferDescriptor {
                    label: Some("query resolve"),
                    size: 256,
                    usage: wgpu::BufferUsages::QUERY_RESOLVE | wgpu::BufferUsages::COPY_SRC,
                    mapped_at_creation: false,
                });
                let readback = device.create_buffer(&wgpu::BufferDescriptor {
                    label: Some("timestamps and verified workload result"),
                    size: 256,
                    usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
                    mapped_at_creation: false,
                });
                let mut buffers = Vec::new();
                for pass_index in 0..PASSES {
                    let mut encoder =
                        device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
                            label: Some("timing_probe_dependent_compute"),
                        });
                    if encoder_timestamps {
                        encoder.write_timestamp(&queries, pass_index * 2);
                    }
                    {
                        let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
                            label: Some("timing_probe_workload"),
                            timestamp_writes: (!encoder_timestamps).then_some(
                                wgpu::ComputePassTimestampWrites {
                                    query_set: &queries,
                                    beginning_of_pass_write_index: Some(pass_index * 2),
                                    end_of_pass_write_index: Some(pass_index * 2 + 1),
                                },
                            ),
                        });
                        pass.set_pipeline(&pipeline);
                        pass.set_bind_group(0, &bind, &[]);
                        pass.dispatch_workgroups(ELEMENTS / 64, 1, 1);
                    }
                    if encoder_timestamps {
                        encoder.write_timestamp(&queries, pass_index * 2 + 1);
                    }
                    buffers.push(encoder.finish());
                }
                let mut resolve = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
                    label: Some("timing_probe_resolve"),
                });
                resolve.resolve_query_set(&queries, 0..PASSES * 2, &resolved, 0);
                resolve.copy_buffer_to_buffer(
                    &resolved,
                    0,
                    &readback,
                    0,
                    u64::from(PASSES * 2) * 8,
                );
                resolve.copy_buffer_to_buffer(&data, 0, &readback, 64, 4);
                let resolve_buffer = resolve.finish();
                // Deliberate pre-submit CPU stall is a control, never a production polling pattern.
                std::thread::sleep(Duration::from_millis(cpu_delay_ms));
                if deferred_resolve {
                    queue.submit(buffers);
                    let (done_tx, done_rx) = mpsc::channel();
                    queue.on_submitted_work_done(move || {
                        let _ = done_tx.send(());
                    });
                    let drain_deadline = Instant::now() + Duration::from_secs(10);
                    loop {
                        device.poll(wgpu::PollType::Poll)?;
                        if done_rx.try_recv().is_ok() {
                            break;
                        }
                        if Instant::now() >= drain_deadline {
                            return Err("workload completion deadline exceeded".into());
                        }
                        std::thread::sleep(Duration::from_millis(1));
                    }
                    queue.submit([resolve_buffer]);
                } else {
                    buffers.push(resolve_buffer);
                    queue.submit(buffers);
                }
                let tx = sender.clone();
                readback.map_async(wgpu::MapMode::Read, .., move |result| {
                    let _ = tx.send((frame_id, result));
                });
                pending.push((frame_id, iterations, cpu_delay_ms, start_cpu, readback));
            }
            drop(sender);
            let deadline = Instant::now() + Duration::from_secs(10);
            let mut completed = 0;
            while completed < pending.len() {
                device.poll(wgpu::PollType::Poll)?;
                while let Ok((frame_id, result)) = receiver.try_recv() {
                    result?;
                    let (_, iterations, delay, cpu_start, buffer) = pending
                        .iter()
                        .find(|p| p.0 == frame_id)
                        .ok_or("unknown completed frame")?;
                    let bytes = buffer.get_mapped_range(..);
                    let ticks: Vec<u64> = bytes[..(PASSES * 16) as usize]
                        .chunks_exact(8)
                        .map(|v| u64::from_le_bytes(v.try_into().unwrap()))
                        .collect();
                    let output = u32::from_le_bytes(bytes[64..68].try_into().unwrap());
                    let expected_output = expected(0, *iterations);
                    let output_verified = output == expected_output;
                    let elapsed = interval(
                        frame_id,
                        ticks[0],
                        ticks[ticks.len() - 1],
                        queue.get_timestamp_period(),
                    );
                    let pass_ms: Vec<Option<f64>> = ticks
                        .chunks_exact(2)
                        .map(|t| {
                            interval(frame_id, t[0], t[1], queue.get_timestamp_period())
                                .map(|s| s.elapsed_ms)
                        })
                        .collect();
                    let timestamps_valid = elapsed.is_some() && pass_ms.iter().all(Option::is_some);
                    println!(
                        "{}",
                        json!({
                            "kind": "sample", "frame_id": frame_id,
                            "timestamp_mode": if encoder_timestamps { "encoder" } else { "pass_descriptor" },
                            "resolve_strategy": if deferred_resolve { "after_workload_completion" } else { "same_submission" },
                            "scope": "three dependent compute command buffers; excludes resolve and presentation",
                            "iterations_per_pass": iterations, "workload_command_buffers": PASSES,
                            "envelope_ms": elapsed.map(|s| s.elapsed_ms), "pass_ms": pass_ms, "raw_ticks": ticks,
                            "timestamps_valid": timestamps_valid,
                            "output_verified": output_verified, "output": output, "expected_output": expected_output,
                            "cpu_pre_submit_delay_ms": delay,
                            "cpu_encode_to_observed_ms": cpu_start.elapsed().as_secs_f64() * 1000.0,
                            "submitted_frame_at_observation": next_frame - 1,
                        })
                    );
                    drop(bytes);
                    buffer.unmap();
                    if timestamps_valid && output_verified {
                        valid_count += 1;
                    } else {
                        invalid_count += 1;
                    }
                    completed += 1;
                }
                if Instant::now() >= deadline {
                    return Err("GPU readback deadline exceeded".into());
                }
                if completed < pending.len() {
                    std::thread::sleep(Duration::from_millis(1));
                }
            }
        }
    }
    println!(
        "{}",
        json!({ "kind": "complete", "validated_samples": valid_count, "invalid_samples": invalid_count, "rendered_frame_signal_validated": false })
    );
    if invalid_count != 0 {
        return Err("one or more timestamp intervals or workload outputs were invalid; inspect sample records".into());
    }
    Ok(())
}
