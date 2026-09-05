# Next bounded experiment: observe MetalFX encoder creation

Status: the instance-local observer is implemented. Root's strict CPU build
and fake-delegate tests passed after a recorded failing checkpoint (51 checks,
30 expected failures, then zero failures). Expanded descriptor and ownership
regressions now pass 67 checks, including actual ledger JSON Boolean types.
Root also compiled the optimized executable strictly and passed its three
CPU-only CLI checks plus actual header/identity/completion JSON Boolean checks.
Executable source review is
complete; the artifact validator passes ten CPU tests and independent source
review. The corrected external watchdog passed a CPU helper-process cleanup
regression after the reviewer reproduced the original cleanup gap.
The first real-buffer Spatial OFF run (`proxy-spatial-off-02`, source
`75577e1`) completed 16 frames with intact raw pixels and PNGs, but its strict
protocol analysis failed on 49 numeric values in Boolean fields. The original
run remains a schema failure and cannot serve as a reference. CPU regressions
reproduced the actual serialization defect; explicit Boolean boxing fixed it
and both strict CPU suites now pass. A fresh OFF reference is required. No
calls/counters proxy hardware arm has run yet.
Root schedules all hardware. The successful
native stage probe establishes a useful counter path, not complete Ushas scope
or exclusive GPU occupancy.

## Smallest discriminating step

Test an **instance-local `NSProxy<MTLCommandBuffer>` passed only to MetalFX's
`encodeToCommandBuffer:`**. Keep the real command buffer and its queue under the
probe's control. Do not replace the device, swizzle Metal classes, change an
object's class, or mix raw encoding into a wgpu encoder.

Apple documents [NSProxy's forwarding hooks](https://developer.apple.com/documentation/Foundation/NSProxy)
and [MetalFX encoding into a command buffer](https://developer.apple.com/videos/play/wwdc2022/10103/).
Those contracts make a small compatibility experiment possible; they do not
promise that MetalFX accepts a proxy. The current SDK also declares
`encodeToCommandBuffer:(id<MTLCommandBuffer>)` for both spatial and temporal
scalers. Ushas already has a dedicated raw MetalFX command buffer in
`src/node/encode.rs`, so a successful proxy can be scoped to that boundary
without changing wgpu's encoder-mode rules.

Use a new isolated executable, with these explicit controls:

```text
--mode spatial|temporal --observe off|calls|counters --out NEW_DIRECTORY
```

All runs use the same deterministic 160×90 input and 320×180 output, one logical
view and 16 frames. Spatial is the first compatibility check; Temporal is
required before proposing an Ushas integration. A two-slot admission limit,
asynchronous command-buffer callbacks and a finite deadline preserve the prior
no-wait contract. Keep all 16 tiny frame inputs alive through the run so this
experiment cannot accidentally change temporal history by recycling textures.
Temporal uses one scaler per run, fixed zero motion/jitter, a valid depth
input, a first-frame reset and persistent sequential history. No interpolation
or dynamic resolution is included yet.

The `off` arm sends the real command buffer directly. `calls` forwards without
adding counter attachments and records which encoder-creation selectors the
framework actually invokes. `counters` adds samples only through the observed,
supported paths. This separates proxy compatibility from counter attachment
compatibility. Preserve opaque output PNGs and per-frame output hashes in all
three arms. Corresponding deterministic frame outputs must match exactly
between the real-buffer and proxy arms; a mismatch remains a transparency
failure, not something to waive as performance noise. A composition sentinel
may prove final frame identity but must not be claimed to survive MetalFX's
filtering unchanged.

Every frame owns its exact MetalFX output and a dedicated readback buffer.
Encode the GPU copy of that output before its final completion callback;
retain both through hashing and PNG saving. Never hash a shared output after a
later frame may have overwritten it. Known setup, MetalFX and final composition/
readback command buffers are labeled with their original source identity.

## Proxy boundary and preservation rules

Intercept the documented command-buffer encoder factories:

| Selector | Instrumentation |
|---|---|
| `renderCommandEncoderWithDescriptor:` | Copy descriptor; attach four vertex/fragment samples |
| `computeCommandEncoderWithDescriptor:` | Copy descriptor; attach two encoder samples |
| `computeCommandEncoder` | Construct its equivalent default serial descriptor with samples |
| `computeCommandEncoderWithDispatchType:` | Preserve requested dispatch type in the sampled descriptor |
| `blitCommandEncoderWithDescriptor:` | Copy descriptor; attach two encoder samples |
| `blitCommandEncoder` | Construct its equivalent default descriptor with samples |

Render/compute/blit descriptors declare `NSCopying` in the inspected SDK. Never
mutate the framework's original descriptor. Preserve dispatch type, attachments,
load/store actions, resource references and all other copied fields. For this
first experiment use sample-attachment slot 0 only when it is empty; an existing
sample buffer makes the frame unavailable rather than overwriting diagnostics
or guessing how many attachment slots this device accepts.

`parallelRenderCommandEncoderWithDescriptor:`, resource-state and acceleration-
structure encoder factories are initially unsupported. Forward them to preserve
execution, but mark the complete observation unavailable. Record all forwarded
selectors. Any unknown/private command-buffer operation also makes coverage
unavailable until reviewed; successful pixels cannot hide an unobserved path.
The proxy must not commit, enqueue, block, replace the queue, or suppress a
framework operation. Normal properties and debug groups are forwarded with
their exact signatures. Keep the original raw command buffer strongly alive.

Return each real encoder directly; do not introduce an encoder proxy merely to
rewrite labels. Preserve the framework's own labels. Record a source ordinal,
family, counter-buffer ownership and original frame/view/epoch/generation when
the factory is called. After MetalFX encoding finishes, retain each encoder's
actual label for the trace ledger. Give the dedicated *command buffer* a unique
frame/view/epoch/generation label before encoding. The first trace join requires
unique framework encoder labels within that owned command buffer; duplicate or
missing labels are an explicit ambiguous-join failure. Do not infer frame
ownership from CPU creation order or fit timestamps to find a likely encoder.

Completion handlers on the real command buffer resolve the owned sample
buffers directly and deliver immutable records asynchronously. Per-frame
limits are 32 observed encoders, 128 samples and 256 retained selector events;
there are at most two live synthetic frames. This fixed-work probe postpones
new admission when its ring is full; it has no independent application render
loop. A future producer must skip observation while preserving normal rendering.
Any lost/unsupported/overflowed member invalidates the whole sampled
frame. A counter callback cannot publish before the frame's encoder inventory
has been sealed after `encodeToCommandBuffer:` returns.

At seal, check the real buffer's status and actual label. Any unexpected
enqueue/commit or rewritten/missing label invalidates the observation. Register
the callback before invoking MetalFX to detect completion-before-seal. The
owner uses a status-aware commit helper: commit only NotEnqueued/Enqueued
buffers, and never double-commit an already committed buffer. Unexpected
framework submission remains unavailable even if the owner can finish safely.

## Required proof and stopping limits

Before hardware, CPU tests with a fake command-buffer delegate must exercise
the real forwarding implementation: default and explicit dispatch types,
copied descriptor preservation, occupied sample slots, unsupported/unknown
selectors, original identity, callback ownership and seal/completion ordering.
Include fake framework enqueue/commit, rewritten labels and owner no-double-
commit cases.
No fake test may call `MTLCreateSystemDefaultDevice`. Source review is separate
from compile success.

Root then runs the real-buffer/calls/counters arms in fresh processes. Retain
every selector, scaler support result, allocation/encoding error, output hash,
command-buffer status and original identity. A traced counters run must match
every MetalFX CPU encoder and GPU stage within each uniquely owned command
buffer. Inventory **all target-process command buffers and encoders**, including
the declared setup/composition/readback families: MetalFX receives the real
device/queue and could create work away from the supplied proxy. Any additional
unassigned work makes scope incomplete. Inspect Idle, driver waits,
cross-frame overlap, delivery age and other-process overlap as in the native
probe. Compare output and CPU overhead across arms without turning CPU duration
into GPU cost. A later paired run still needs to quantify total instrumentation
overhead against the same uninstrumented rendering work.

The executable's internal 15-second deadline is cooperative: it starts after
synchronous preparation, and an encoding call can block its serial control
queue. Every hardware invocation therefore requires an external process-group
watchdog, including preparation and encoding. See `METALFX_PROXY_RUN.md` for the
bounded invocation and fresh-artifact protocol.

Limit this route to two implementation/compatibility attempts: transparent
forwarding first, descriptor attachment second. If MetalFX rejects the proxy,
uses an unobservable path, loses labels needed for an unambiguous join, fails
output parity, or produces incomplete counters, retain that negative result.
Do not escalate to private method hooks or an unbounded backend rewrite. A
HAL-only stage observer can remain a partial diagnostic, but full autonomous
adaptation must remain unavailable until another complete producer is validated.

## If the MetalFX experiment succeeds: isolated Bevy/wgpu patch

The smallest next integration is a local, non-published Bevy-render/wgpu-HAL
instrumentation overlay under this probe directory. Instrument actual Metal
render/compute/blit descriptor creation in wgpu-hal 29.0.4, plus the raw MetalFX
proxy above. `RenderContext::begin_tracked_render_pass` alone is insufficient:
direct `command_encoder()` users and compute work must be covered too.

Do **not** set scope by calling `CommandEncoder::as_hal_mut` on an encoder that
will use wgpu commands; that repeats the known Raw/Wgpu mixing failure. Instead
propagate an explicit immutable diagnostic scope through a small patched
Bevy-render encoder-construction seam into the local HAL observer. The scope
must come from the extracted render world's original frame/configuration, not
a live main-world reference. Source-created structured encoder labels can carry
the same scope for trace reconciliation; they are not a substitute for an owned
in-memory ledger.

Start with one supported full view. Its scope includes that frame's shadow and
mesh preprocessing, prepasses, main rendering, depth/motion resolves, raw
MetalFX, reconstruction, postprocessing, UI and final composition exactly once.
Multiple views, unsupported encoder kinds and unassigned submitted work with
possible frame relevance must fail closed. Asset-upload/readback/presentation
exclusions need explicit inventory and review; they cannot silently disappear
from a claim of full-frame work.

Each encoder carries the scope captured at creation. Each actual Metal command
buffer completion resolves its own stages into that frame's bounded ledger.
After the render schedule seals the expected command-buffer inventory, publish
only when all registered submitted work has completed with the same immutable
identity. Missing callbacks, dropped records, failed buffers, late members or
expired samples invalidate the frame asynchronously. Preserve existing Bevy
submission order and pipelining; no drain or frame-loop wait is permitted.

This is a concrete integration direction, not yet a patch-size or complete-
coverage claim. A source inventory and a full fixture trace must establish that
the single-view scope actually reaches every relevant encoder before the
producer can be considered for the governor. Even then, stage durations can
reflect contention and must not be described as exclusive GPU occupancy.
