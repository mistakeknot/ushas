# GPU-cost producer, second investigation

Status: **native stage controls and trace agree; full producer not validated**. Base Ushas revision
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

Root compiled the native source successfully with ARC, blocks, `-O2` and strict
warnings (`-Wall -Wextra -Werror`), with no diagnostics. The recorded source is
`58326f8b4f01d5adfcd8091ecb79ef2ca19469b2`; optimized binary SHA-256 is
`99e0d3d4068a3422bb9bf64f723b8b4b8df3edf2c7fad74b49116bca857f2172`.
The run used Apple M5 Max, macOS 26.5.2 build 25F84 and `MTL_DEBUG_LAYER=1`.
The initial SDK-version query failed and its error is retained in the build
receipt; compiler and OS identification succeeded.

## Observed native controls

The untraced run exited 0, completed all 32 admitted frames, retained no command
errors and never exceeded four live slots. Its maximum sample delivery age was
25.843 ms; 27 full-ring admission ticks were skipped. Every per-frame union was
independently recomputed with a separate sweep-line calculation. Four saved
PNGs independently decoded as 320×180 RGBA with frame sentinels 29–32, no
nonopaque pixels, and the expected nonuniform gradients.

Each cell below contains eight frames. These are descriptive feasibility
observations, not independent benchmark replicates or a calibrated forecast.

| Fragment iterations | Requested CPU gap | Observed submission gap, median ms | Stage union, median ms | Outer envelope, median ms |
|---:|---:|---:|---:|---:|
| 0 | 0 ms | 0.044896 | 0.050396 | 0.052063 |
| 1000 | 0 ms | 0.044750 | 1.297438 | 1.301042 |
| 0 | 20 ms | 21.228854 | 0.061416 | 21.512125 |
| 1000 | 20 ms | 21.199417 | 1.308689 | 21.450980 |

High fragment load raises the zero-gap stage median about 25.7×. The CPU-gap
arms add roughly 11 µs to their stage medians while expanding outer envelopes
to about 21.5 ms. That is useful discrimination from the failed outer markers.
It does not imply zero perturbation: the low-load relative shift is about 22%,
and high-load zero-gap frames range from 0.690 to 2.274 ms. Scene-fragment
medians are 0.013397/1.258500 ms without the gap and 0.013833/1.261271 ms with it.

The untraced run has 15 cross-frame stage-overlap pairs. Its summed per-frame
unions are 21.353501 ms, whereas the global union is 19.572290 ms. The larger
sum must not be presented as exclusive hardware occupancy.

Raw data is under `/private/tmp/ushas-roadmap-evidence/stage-probe-02/`;
build/run/source/binary receipts are under
`/private/tmp/ushas-stage-probe-artifacts-02/`. The original execution receipt's
`valid:false` remains intact because it preceded evidence review. The separate
`native-review.json` combines child exit 0, structural record validation,
independent interval recomputation and decoded images, while retaining
`validated_for_governor:false`.

## Observed Metal System Trace

Root separately traced the same frozen binary. Instruments exited 0; all 32
native records again passed structural checks. Its immediate-recording log
contains a backdated-signpost warning, which remains retained. Coverage here is
established from the exact named encoders and GPU stage rows, not signposts or
CPU creation order.

The [new trace join parser](../../tools/render-timing-probe/analyze_trace.py)
matches all 128 expected encoder identities and 192 stage pairs: scene vertex
and fragment, dependent compute, composition vertex and fragment, and the
separately excluded readback. One clock offset is fixed from frame 1's scene
vertex start; all 383 remaining endpoints match exactly, with zero nanosecond
residual. There are no split-stage gaps in this capture; that check includes
the diagnostic readback pairs, while the rendering-stage unions exclude them.

Counter and trace global rendering-stage unions both equal **17.814666 ms**.
The sum of per-frame unions is 22.188333 ms, with 26 cross-frame overlap pairs.
The global state table covers all owned stages and shows **0 ns Idle** inside
their union. The capture also contains 200 GPU rows from other named processes
and three unattributed rows on the same device; none overlap the owned stage
union. These results support the declared synthetic stage boundaries. Trace
`Active` rows are not independent exclusive-busy hardware counters.

The trace and separate exports are
`/private/tmp/ushas-roadmap-evidence/stage-probe-trace-02.*`; traced records and
images are in the sibling `stage-probe-traced-02/` directory. The current audit
is `stage-probe-trace-02.audit-v2.json`. The parser's seven CPU tests cover XML
references, complete identities, unmatched/extra stages, command-buffer
agreement, a fixed offset, split-stage gaps, device-specific state coverage and
foreign-process overlap. Independent review reproduced the actual audit
exactly and checked interval math against 500 discrete-interval oracle cases;
the review found no blocking issue for this explicitly synthetic scope.

The next discriminating gate is the same observed timing path over complete
Bevy plus raw MetalFX work, followed by a matching uninstrumented overhead arm.
Neither is supplied by this native probe. No library timing source, public API,
adaptive governor or production renderer behavior changed in this slice.
