# Metal frame-cost timing feasibility

Date: 2026-09-04. Roadmap gate: `shadow-work-vzox.3`.

**Verdict: do not enable an autonomous GPU-budget governor from the existing
timer or Bevy diagnostics.** This Mac exposes timestamp capabilities, but the
straightforward query path fails on hardware. A narrower path works in a
headless dependent-compute probe: pass-descriptor timestamps, with query
resolution submitted only after the workload completes. This is a candidate
for a rendered-view experiment, not a validated frame-cost signal.

Until that experiment passes, accept an explicitly supplied, validated frame
signal with its provenance and expose automatic GPU timing as unavailable.
CPU frame duration and the dedicated MetalFX command-buffer duration must not
silently substitute for a render-plus-upscale budget measurement.

## Observed hardware evidence

The [standalone probe](../../tools/timing-probe/) pins wgpu 29.0.4 and owns its
lockfile; it does not depend on Ushas or the concurrently changing integration.
It ran on Apple M5 Max, macOS 26.5.2 / 25F84. The sandbox could not enumerate a
Metal adapter; the same binary outside the sandbox selected the physical GPU.

Both adapter support and enabled device features were read at runtime:

| Feature | Supported and enabled |
|---|---|
| `TIMESTAMP_QUERY` | Yes |
| `TIMESTAMP_QUERY_INSIDE_ENCODERS` | Yes |
| `TIMESTAMP_QUERY_INSIDE_PASSES` | No |
| Queue timestamp period | 1 ns per tick |

This corrects the earlier assumption that the Apple GPU simply lacks
encoder-level timestamp capability. Cached wgpu-hal 29.0.4 enables
`TIMESTAMP_QUERY` and `TIMESTAMP_QUERY_INSIDE_ENCODERS` together when Metal
stage-boundary sampling is supported. Capability flags still do not prove
that queries work.

The probe uses three compute command buffers with a read/write dependency on
the same storage buffer. Each frame owns separate queries and readback
storage. It verifies a deterministic integer result, records raw timestamps,
and carries the sequence ID from encoding into asynchronous readback.

| Experiment | Result |
|---|---|
| Same submission for work and query resolve; pass descriptors and encoder writes | 1/16 valid intervals; all 16 workload outputs correct. All 8 encoder-write samples had zero timestamps. Most descriptor samples lacked the final pair. |
| Resolve after workload completion; both timestamp modes | First 8 descriptor samples valid. Encoder samples 9–10 were zero. Run stopped at sample 11 on a workload-output mismatch; cause not established. |
| Resolve after workload completion; descriptors only | 8/8 valid intervals and correct outputs; exit 0. |

In the final control, light work measured 0.129–0.131 ms and heavy work
1.474–1.483 ms. Adding a deliberate 20 ms CPU delay before submission did not
add that delay to the GPU interval. These values establish discrimination in
this synthetic workload; they are not Ushas performance numbers. Samples
encoded as IDs 1–4 were observed after ID 4 had been submitted, preserving
their original identities despite delayed collection.

The [evidence manifest](../../tools/timing-probe/evidence.json) contains exact
commands, outcomes, and links to every JSONL result, including failures. No
window, presentation, MetalFX workload, or Metal System Trace was involved.
The measurements therefore do not establish presentation safety, GPU busy
time, or rendered-frame scope. The passing control drains work in the probe;
shipping that wait in a render loop would be unacceptable.

## Why the existing routes are insufficient

Ushas `src/node/encode.rs` intentionally puts raw MetalFX encoding on a
dedicated command buffer. `GpuTimingSink` reads its `GPUStartTime` and
`GPUEndTime`; scene rendering can be on other command buffers and dependencies
can extend this interval. Disabled mode's empty timed buffer is not a
whole-frame control. Keep this metric explicitly scoped to that command
buffer. It cannot supply the governor's frame budget on its own.

Bevy 0.19.0's cached `diagnostic/internal.rs:749–759` is more decisive than a
documentation caveat: its `WriteTimestamp for CommandEncoder` returns
unconditionally on macOS. It names the Tahoe presentation-flicker regression
[#22257](https://github.com/bevyengine/bevy/issues/22257). Bevy's current
[diagnostics documentation](https://docs.rs/bevy_render/latest/bevy_render/diagnostic/struct.RenderDiagnosticsPlugin.html#supported-platforms)
also limits Metal diagnostics to CPU time. Turning on that plugin does not
solve this gate.

The upstream report [wgpu #9414](https://github.com/gfx-rs/wgpu/issues/9414)
describes zero timestamp queries on macOS 26 despite advertised support, and
[wgpu-native #624](https://github.com/gfx-rs/wgpu-native/issues/624) reports
non-completing encoder-timestamp submissions. They are relevant corroborating
reports, not proof that their proposed root cause explains this probe. In
particular, this probe's nonempty descriptor passes can produce valid data.

Borrowing raw Metal command buffers after Bevy finishes them is also not an
available public-API shortcut. wgpu 29.0.4 `CommandBuffer` exposes no HAL
accessor. `CommandEncoder::as_hal_mut` enters `EncodingApi::Raw` even if the
closure only attaches an observational completion handler. wgpu-core forbids
mixing that state with prior or later `EncodingApi::Wgpu` operations. Observing
Bevy's existing encoders through this API repeats the raw/wgpu mixing fault
that Ushas already fixed. An upstream observational accessor would be a
separate dependency change.

## Integration route to test next

An opt-in implementation now exists in `src/frame_timing.rs` as
`ExperimentalFrameTimingPlugin`. It inserts destination-preserving render
passes before `Core3dSystems::Prepass` and after both Bevy upscaling and
`MetalFxLabel`. The first pass uses the raw main texture view; the last uses
the raw output view. Neither consumes Bevy's clear bookkeeping. Both draw a
full-screen triangle using ZERO source / ONE destination blending with
Load/Store attachments, creating real render-attachment accesses rather than
empty command buffers. These accesses still require trace validation of the
intended GPU dependency envelope.

The plugin exposes `ExperimentalFrameTiming` snapshots with Pending,
Unavailable, Failed, and ObservedUnvalidated states. It retains original
application frame ID, main-world camera entity, actual configuration generation,
mode/scale/dimensions, and a matching frozen adaptive epoch when available.
Four raw query values retain each marker's own start/end in addition to the
outer envelope. `marker_ms` reports each marker pass separately; it is not a
substitute for measuring total perturbation against an uninstrumented arm.

Eight reusable slots move through workload completion, deferred resolution,
and asynchronous mapping. Render systems never poll or wait for the GPU.
Storage remains reserved until callbacks complete; a full ring skips samples.
Missing features, multiple active cameras, incomplete boundaries, stale
readbacks, and stale query reuse remain explicit failures or unavailable
states. The plugin never publishes a `ValidatedGpuFrameCost`.

The renderer must request `TIMESTAMP_QUERY` before device creation; the helper
`frame_timing::requested_features()` supplies that flag. Add the experiment
after `MetalFxPlugin`, and run matching instrumented and uninstrumented scenes.
This source and its five CPU tests compile; rendered output, added-pass
overhead, CPU-delay controls, and trace coverage remain hardware validation
gates below.

Bevy 0.19 offers useful schedule boundaries without replacing its renderer:

- `RenderGraph` has `Begin`, `Render`, `Submit`, and `Finish` sets.
- `Core3d` runs per current view, with `Prepass`, `MainPass`, and `PostProcess`
  sets; Bevy's final upscaling system follows postprocessing.
- `RenderContext` queues command buffers through `PendingCommandBuffers` in
  topological system order. `FlushCommands` may submit earlier.

CPU schedule order alone does not guarantee a GPU measurement boundary.
Separate command buffers can overlap. An empty marker pass is especially
unsuitable: a timestamp can remain unwritten, or the marker can execute
without waiting for the work being measured. The next experiment needs
timestamps on actual, dependency-carrying first and final render passes, or
an explicitly validated dependency chain that encloses them.

1. Instrument a pinned smoke scene's first relevant render pass and final
   composition pass through descriptor `timestamp_writes`. Include raw
   MetalFX between those boundaries using the scene-to-upscale-to-composition
   texture dependencies. Start with one camera. Define whether shadows,
   prepasses, preprocessing, and UI are included; a view interval is not
   automatically a whole-frame interval.
2. Resolve each frame's query set only after its workload submission is known
   complete. Use `Queue::on_submitted_work_done` to mark records ready, then
   submit resolution in a later frame and map asynchronously. Keep a bounded
   ring of query/resolve/readback storage; skip samples when full instead of
   waiting. No query-set reuse while any part is in flight.
3. Preserve render-frame ID, view ID, scale/configuration generation, timing
   source, scope, submission identity, encoded time, and observation time.
   Reject zero, non-finite, inverted, missing, duplicate, stale, or superseded
   samples. Carry the scale used by that measured frame, not the current one.
4. Compute a single same-clock boundary interval, `end - start`. Never sum
   overlapping command-buffer durations or subtract unrelated GPU clock and
   host-clock timestamps. Call it a **rendered-view GPU elapsed interval**;
   dependency waits and scheduling interference remain part of the result.
5. Capture a Metal System Trace containing the expected scene/prepass,
   MetalFX, and composition encoder families. Align sampled frame IDs to
   their workload. Check interval coverage, overlap, and missing work against
   the trace; CPU load readings alone cannot establish coverage.
6. Validate native and temporal controls under GPU-heavy, CPU-heavy, and
   presentation-limited conditions. Verify that instrumentation does not
   cause flicker, skips, changed output, or materially alter throughput. Check
   60/120 Hz, resize, history reset, delayed callbacks, and two cameras before
   making multi-view claims. Only then calibrate and enable autonomous use.

This route is technically plausible because the narrow descriptor/deferred
control passed. Render-pass descriptors, raw MetalFX ordering, and presentation
have not passed those gates. A failure there should leave the external-signal
contract available and automatic collection unavailable, with the failure
reason retained for diagnosis.

## Capture validity before measuring a window

The first half-resolution smoke captures exposed a separate Bevy 0.19 capture
failure. `temporal-half`, `spatial-half`, and `bilinear-half` PNGs each contained
921,600 pixels of exactly RGBA `(0, 0, 0, 0)`; `native-current` contained real
scene colors and alpha 255 throughout. These files under
`/private/tmp/ushas-roadmap-evidence/` are invalid captures, not evidence that
MetalFX produced a black image.

The cached source explains a path that produces this result. wgpu-hal 29.0.4
checks the macOS window's occlusion state and returns `SurfaceError::Occluded`
before acquiring a drawable (`metal/surface.rs:123–151`). Bevy silently accepts
that result and removes `ViewTarget` when no output attachment exists. Main
application frames continue, but per-view render observations stop. A screenshot
temporarily supplies its own output attachment, allowing rendering to resume.
However, Bevy's window screenshot submission checks for a swapchain view before
issuing either the readback copy or the screen blit; with no drawable it skips
both (`view/window/screenshot.rs:508–530`). `collect_screenshots` still maps the
prepared buffer (`:638–700`), producing zero data. In the temporal artifact,
the effect observation advancing from frame 402 to 952 on screenshot request
fits this path. A live drawable/occlusion observation remains the confirming
probe; the PNG's successful map callback alone does not prove a rendered copy.

Keep the test window visible and verify drawable availability throughout the
measurement and capture, or render the single test camera to an image target
and capture that image. Report application-update cadence separately from
actual per-view rendered observations. Do not relabel a capture failure as
GPU cost, image quality, or successful output evidence.

The control arm also needs an explicit crop-aware upscale. Bevy 0.19's built-in
blit samples the entire full-size main texture with a default nearest sampler;
it does not read `MainPassResolutionOverride`. The override only shrinks the
rendered viewport. `src/control_upscale.rs` expands the actual content crop
with a linear sampler in `EarlyPostProcess`, before tonemapping and UI. Its
rendered output must pass the same capture gate. Bevy places UI before its
final upscaling system, so running a content crop after that system would also
crop and scale UI.

## Reproduction and verification

```sh
cargo test --locked --manifest-path tools/timing-probe/Cargo.toml
cargo clippy --locked --manifest-path tools/timing-probe/Cargo.toml --all-targets -- -D warnings
cargo run --locked --manifest-path tools/timing-probe/Cargo.toml -- --deferred-resolve --pass-descriptors-only
```

The four CPU tests cover original sample identity, interval conversion,
overlap-safe outer boundaries, invalid queries, and integer subtraction before
floating-point conversion. The valid-interval tests failed against the initial
unavailable implementation; all four passed after implementation. GPU validity
comes from the separately retained runtime output.
