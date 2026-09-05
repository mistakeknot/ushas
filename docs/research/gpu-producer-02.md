# GPU-cost producer, second investigation

Status: **design/prototype, not validated**. Base Ushas revision
`9e254c9c397e955cac15cd531ffa4696e5880b33`; Bevy 0.19.0 and
wgpu/wgpu-hal 29.0.4 are the inspected dependency versions. Root alone schedules
compilation and GPU experiments. Nothing in this investigation supplies the
governor until the scope and hardware gates below pass independent review.

## What changes from the failed producer

The [first trace](marker-scope-01.md) established accurate outer marker clocks,
but also idle gaps, overlap with other frames and incomplete preprocessing
ownership. Serial CPU completion cadence is a different metric and is not a
replacement GPU-cost producer.

In pinned `wgpu-hal/src/metal/command.rs`, render-pass timestamp setup explicitly
sets `endOfVertexSampleIndex` and `startOfFragmentSampleIndex` to
`MTLCounterDontSample`. Its normal two timestamps therefore span from the
start of vertex processing to the end of fragment processing. They cannot
separate a gap between the stages. Bevy's `RenderContext` also exposes its
encoder directly; instrumenting only `begin_tracked_render_pass` would miss
compute and untracked passes. Raw MetalFX encoding creates additional encoders
outside that wgpu HAL path.

Apple's [live GPU profiling example](https://developer.apple.com/videos/play/tech-talks/10001/)
uses all four render-stage timestamps, per-encoder sample buffers, and
`resolveCounterRange` from command-buffer completion handlers. This provides a
concrete alternative to both outer-frame markers and later wgpu query-resolve
submissions. It also requires missing/error timestamps to remain failures.

The current [Bevy diagnostics documentation](https://docs.rs/bevy_render/0.19.0/bevy_render/diagnostic/struct.RenderDiagnosticsPlugin.html)
still does not offer Metal GPU timing. Its macOS encoder timestamp no-op remains
in the cached source; [Bevy #22257](https://github.com/bevyengine/bevy/issues/22257)
was closed by disabling the problematic path, not by validating a replacement.
[wgpu #9414](https://github.com/gfx-rs/wgpu/issues/9414) remains open as inspected
on September 5, 2026. Its empty-pass reproduction and asserted cause do not
establish the behavior of the nonempty render passes proposed here.

## Alternatives and boundary of the first probe

| Route | Useful property | Remaining problem |
|---|---|---|
| Existing Bevy diagnostics or encoder timestamp writes | Small integration | Disabled/problematic on Metal; no complete provenance or coverage |
| Real render-pass descriptor pairs through public wgpu | No raw/wgpu encoder mixing | Two timestamps include vertex-to-fragment gaps; deferred resolve previously went stale |
| Four Metal render-stage samples, compute/blit bounds, direct completion-handler resolve | Actual-work ownership, finer intervals, no later resolve/map submission | Requires local instrumentation hook; hardware reliability and MetalFX coverage unproved |
| Whole command-buffer `GPUStartTime`/`GPUEndTime` | Includes raw MetalFX | Includes dependency waits and overlaps; the earlier negative gate still applies |
| Metal 4 counter-heap/backend migration | New timing API | Separate backend/queue integration, far larger than this bounded experiment |

The selected first experiment is an isolated native Metal offscreen
**render → dependent compute → composition render** chain. All three perform
real work on per-slot textures. A diagnostic readback blit is sampled and
reported separately; it is excluded from the declared rendering-chain cost.
No dummy marker pass is inserted. There is no MetalFX in this first probe, so
even a successful result cannot yet claim Ushas frame scope.

Four small arms cross a low/high fragment-iteration count with a zero/20 ms CPU
gap between the first and later command-buffer submissions. The CPU gap is
scheduled asynchronously, not implemented as a render-loop wait. A bounded
four-slot ring skips admissions while full. Command-buffer completion callbacks
resolve shared counter buffers directly and preserve each original frame,
view, configuration epoch, slot generation, dimensions and load. No query set
is reused while its callback owns it. All failures and skipped admissions are
retained; there is no indefinite polling or completion wait.

Every frame carries an integer pixel sentinel through the actual texture
dependency chain. Readback checks the sentinel, alpha and nonuniform output;
these checks establish this synthetic chain's output, not consumer image
quality. GPU-stage intervals are united within that frame's declared scope;
overlapping stages are not added twice. Other frames' intervals are never
assigned by CPU creation order. Cross-frame overlap is reported rather than
used to infer globally exclusive GPU occupancy.

The first discriminating outcomes are: complete nonzero four-stage records;
correct original identity and pixels; prompt callback delivery without wgpu
polling; fragment work responding to GPU load; and the stage union staying
stable when a CPU gap expands the outer envelope. These are necessary but not
sufficient. A Metal System Trace must subsequently match the named stage
families and interval union, inspect recorded idle within stages, and assess
instrumentation overhead against the same uninstrumented work.

## Integration gate after that experiment

If the first probe works, the smallest candidate dependency experiment is an
isolated patch to wgpu-hal's actual encoder construction, attaching private
counter buffers to real render/compute/blit descriptors and resolving them in
Metal completion callbacks. Frame/view/epoch context must be propagated from
the extracted render world into each owned encoder; labels alone cannot create
ownership after the fact. The patch must cover direct encoder use, not just
Bevy's tracked-render helper. It must remain opt-in and preserve normal
submission/pipelining, with bounded callback storage and no frame-loop waits.

MetalFX internal encoder coverage is an unresolved integration gate. A HAL-only
observer misses it. Passing an observational command-buffer proxy to MetalFX
could intercept its descriptor creation, but compatibility with the framework
is unproved and must be a separate, bounded experiment. It is not acceptable to
silently add the existing dedicated-buffer duration to the stage union: its
upstream waits would reintroduce the old failure. If complete raw MetalFX
coverage cannot be established safely, this route remains unavailable for the
full adaptation contract.

Before any live governor input: independently review the prototype; validate
native and active Temporal output; trace all declared preprocessing, prepass,
main render, reconstruction, postprocess and composition work for one supported
view; reject incomplete/mixed/stale epochs; test CPU-delay and GPU-load controls;
measure sample age and dropped observations under normal pipelining; and
quantify added instrumentation cost. A bounded scope may exclude explicitly
unsupported features, but cannot call a partial stage sum a full-frame cost.

## Prototype and current evidence

The isolated source is [StageProbe.m](../../tools/render-timing-probe/StageProbe.m),
with [exact root-run commands](../../tools/render-timing-probe/README.md).
The [analyzer](../../tools/render-timing-probe/analyze.py) has eleven passing CPU
tests after observed failures for missing interval/protocol checks. It rejects
live-slot reuse, changed or stale generations, laundered arm identities,
missing/error/inverted stages, incorrect dependency ordering, missing or failed
PNG-save outcomes,
stale delivery, incomplete counters and malformed final summaries. Actual CPU
submission timestamps verify that the requested negative-control gap occurred.
Cross-frame intersections and the global interval union are retained separately
from each frame's union. Structural validity does not certify control response.

Root compiled the native source successfully with ARC, blocks and strict
warnings (`-Wall -Wextra -Werror`), with no diagnostics. The optimized build and
GPU execution are still pending. There is no current hardware result for this
second producer investigation.
