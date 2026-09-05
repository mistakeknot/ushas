# Bevy frame GPU scope: source inventory and attribution boundaries

A complete single-view sample needs an inventory of **actual submitted Metal
command buffers**, including wgpu-generated work. Instrumenting tracked render
passes, the MetalFX buffer, or the final application submit alone misses work.
Current source does not carry enough provenance to assign every upload or shared
job to an original extracted frame/view/configuration. Unknown or mixed ownership
must invalidate a complete-frame sample rather than disappear from its cost.

This is a read-only inventory, not a HAL patch or validated producer. It inspected
Ushas `43b650f266c7a2dabce776c852c3d6a674f56aa2`, the smoke dependency lock
(`47160823cc814215372c0a6905bdb08e93cf02589b8801f956ebc416948738eb`), and the
installed Bevy **0.19.0** / wgpu, wgpu-core and wgpu-hal **29.0.4** sources. No
build or GPU workload was run. The separate
[MetalFX proxy plan](../../tools/render-timing-probe/METALFX_PROXY_PLAN.md) remains
a feasibility experiment; this inventory assumes no proxy hardware success.

## Frame and submission boundaries

Bevy's pipelined renderer transfers the render sub-app to its render thread after
extraction. The main app can advance while that render world executes. Freeze
frame ID, epoch/configuration, target, dimensions and scale **during extraction**;
never resolve them from a live main-world reference when a command buffer completes.
The render-world entity is not the public camera ID: preserve the associated
`MainEntity`, as Ushas effect observations already do.

Within the render app, `ExtractCommands` precedes asset/mesh preparation, view
creation, queueing, resource preparation, rendering and cleanup. Some preparation
systems already issue uploads or direct submits. Admission only at the first
camera pass would be too late. A prospective admission seam is after extracted
commands are applied and before `PrepareAssets` / `PrepareMeshes`, with explicit
bootstrap handling for work created before that point. Parallel preparation
tasks need the immutable scope passed to their work; a render-thread-local value
alone does not propagate to worker threads.

The root `RenderGraph` has Begin, Render, Submit and Finish sets. The camera driver
sets `CurrentView` while running each camera or auxiliary-view schedule, then
removes it. Each system's `RenderContextState` finishes its encoders into
`PendingCommandBuffers`; this is an ordered collection, not a single frame buffer.
Other paths submit directly. After the entire root graph returns,
`render_system` creates another encoder for screenshot and GPU-readback commands,
submits it, presents window surfaces, and collects screenshots. Therefore
`RenderGraphSystems::Finish` is not the end of all frame submission.

Relevant source anchors:

- [Pipelined extraction and render-app transfer](/Users/sma/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/bevy_render-0.19.0/src/pipelined_rendering.rs:178).
- [Render set ordering](/Users/sma/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/bevy_render-0.19.0/src/lib.rs:295).
- [RenderContext creation, deferred flush and explicit FlushCommands](/Users/sma/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/bevy_render-0.19.0/src/renderer/render_context.rs:81).
- [Camera and auxiliary-view driver](/Users/sma/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/bevy_core_pipeline-0.19.0/src/schedule.rs:133).
- [Final readback submit and presentation](/Users/sma/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/bevy_render-0.19.0/src/renderer/mod.rs:72).

## GPU work inventory

Paths below are relative to the named, version-pinned crate's `src/`. These are
source paths that can submit work, not assertions that every optional effect ran
in a particular capture. The initial producer profile must declare enabled
features/components and reject unrecognized work. One camera does not imply one
GPU view: lights can create auxiliary shadow views, and UI has its own view.

| Work | Actual encoding/submission path | Ownership implication |
|---|---|---|
| Camera, light, mesh, skin/morph, material, instance and UI data uploads | Bevy render `render_resource/buffer_vec.rs`, `uniform_buffer.rs`, `storage_buffer.rs`; PBR material/mesh/light preparation; UI `lib.rs:1972`; `RenderQueue::write_buffer` and texture upload helpers | Render preparation knows the extracted frame, but shared buffers and queue APIs do not carry original view/config identity. Do not discard routine uniform uploads as unrelated asset loading. |
| Images, fonts/atlases, fallback textures and morph textures | Bevy render `texture/gpu_image.rs:84`, `renderer/render_device.rs:219`, `mesh/morph.rs:87`; texture-with-data eventually queues uploads | Asset IDs are not view IDs. An asset can be prepared later than its original extraction or used by several views. |
| Image/storage growth and allocator migration | Bevy render `texture/gpu_image.rs:115`, `storage.rs:213`, `slab_allocator.rs:840`; each creates its own encoder and directly submits copies | These bypass `RenderContext`. Charge or explicitly classify migration work; a view-pass-only observer cannot see its ownership. |
| Sparse instance/buffer updates | Bevy render `render_resource/sparse_buffer_vec.rs:201`: its own compute encoder and direct submit at 268, scheduled in root graph Begin at 63 | Global buffer work occurs before `CurrentView`; tag as frame-shared work, not an invented camera pass. |
| Mesh preprocessing and indirect draw construction | PBR `render/gpu_preprocess.rs:540,625,856,1026`; early/late preprocessing, reset/build indirect parameters, including shadow cascades | Direct `command_encoder().begin_compute_pass`; tracked-render-pass instrumentation misses it. Preserve camera-to-shadow dependencies and count each physical encoder once. |
| GPU light clustering | PBR `cluster/gpu.rs:972,1022,1087` | Both compute and render encoders can occur. CPU cluster preparation is not a GPU interval, but its uploaded results are GPU work. |
| Shadows and prepasses | PBR `render/light.rs:2834`; core pipeline `prepass/node.rs:190`, deferred prepass/copy-lighting-ID paths | Depth/motion prepasses are part of the Temporal path. Point/spot shadow roots are auxiliary views; cascades can belong to the current camera. Shared shadow work needs explicit dependency ownership. |
| Opaque, alpha-masked, transmissive and transparent scene rendering | Core pipeline `core_3d/main_opaque_pass_3d_node.rs:67`, `main_transparent_pass_3d_node.rs:73,117`; PBR `transmission/node.rs:85,102` | Include draw passes, clear-only passes and attachment resolves. Some direct render-pass calls bypass the tracked helper. |
| Ushas input preparation | [Content crop/copy](../../src/node.rs:660), [motion/depth resolve render passes](../../src/node/resolve.rs:117) | Scale-dependent preprocessing precedes raw MetalFX and belongs to the same frame; it is not just the MetalFX kernel. |
| Raw Spatial/Temporal MetalFX | [Dedicated raw encoder](../../src/node/encode.rs:283), then `RenderContext::add_command_buffer` at 500 | The context flushes preceding wgpu copies/resolves before the raw buffer, then later composition uses a new context encoder. Keep the raw buffer's immutable scope and observe its actual internal encoder factories. |
| Reconstruction or Disabled/failure fallback | [Ushas output passes](../../src/node.rs:915), [bilinear control](../../src/control_upscale.rs:304) | Spatial/Temporal reconstruction runs after MainPass and before EarlyPostProcess; fallback is real output work. Do not label Pending/Failed as an active MetalFX sample. |
| Postprocessing and final view output | Core pipeline `tonemapping/node.rs:127`, `upscaling/node.rs:79,88`; optional PBR SSAO/SSR, volumetric fog, atmosphere, deferred/OIT resolve, mip generation and light-probe generation | Tone mapping uses a direct render pass. Optional effects can add compute/render/copy work and shared resource generation. Additional postprocessing/AA plugin crates outside the locked smoke profile require their own inventory before enablement. |
| UI and text | UI render `render_pass.rs:56`, scheduled after PostProcess and before upscaling (`lib.rs:270`) | Map `UiCameraView` / `UiViewTarget` to the selected scene camera. A UI render entity alone is not the original camera identity. Include native-resolution UI and final composition. |
| Other supported engine extensions | PBR meshlet compute/raster/material passes and persistent-buffer migration; core 2D/custom fullscreen-material systems; arbitrary application render systems | The smoke's one-view 3D profile does not prove these paths. Unknown enabled systems, extra root views or unmanaged direct submissions must invalidate scope. |

The inventory found explicit submit sites beyond the central graph submit:
sparse buffers, image/storage resize, slab growth, meshlet persistent-buffer
growth, `FlushCommands`, uncovered-swapchain clears, final screenshot/readback,
and optional Tracy GPU diagnostics. A `RenderDevice::create_command_encoder`
hook covers many application-created encoders, but not wgpu's internal encoders
or raw application access to the underlying device/queue.

## wgpu work added below Bevy

`wgpu-core::PendingWrites` owns a separate HAL encoder. `queue.write_buffer`,
`queue.write_texture` and staged mapped-buffer unmap append work to it; the next
queue submission prepends that physical command buffer ahead of submitted user
buffers. Texture writes can also initialize untouched regions. Its current
metadata tracks destination resources, not original frame/view/configuration.
Bootstrap zero-buffer initialization also begins through this path.

The submit path can further open internal `Transit` work, initialize buffer or
texture memory, insert barriers, and add surface transitions. One public wgpu
command buffer can therefore expand into several physical HAL buffers. Preserve
the owning logical-buffer scope through that expansion; discover the physical
set at submit, not only at public `finish()`.

- [PendingWrites ownership and activation](/Users/sma/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/wgpu-core-29.0.4/src/device/queue.rs:300),
  [prepended at submit](/Users/sma/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/wgpu-core-29.0.4/src/device/queue.rs:1419).
- [Mapped-buffer upload path](/Users/sma/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/wgpu-core-29.0.4/src/resource.rs:890).
- [Submit-time initialization and Transit buffers](/Users/sma/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/wgpu-core-29.0.4/src/device/queue.rs:1295).
- [Asset retry/deferred preparation](/Users/sma/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/bevy_render-0.19.0/src/render_asset.rs:380).

At Metal level, instrument actual factory calls, not just descriptor-bearing
variants. Render uses `renderCommandEncoderWithDescriptor`; compute has both
bare and descriptor branches; ordinary copies use bare `blitCommandEncoder`.
Timestamp fallback can create an additional descriptor-based dummy blit that
writes a byte. Acceleration-structure encoders are another family and must be
explicitly unsupported until covered. Existing sample attachments and finite
hardware attachment slots cannot be overwritten silently.

Metal `Queue::submit` attaches its fence callback to the last physical buffer,
or creates an extra empty buffer for an empty submit. It also relabels that last
buffer **`(wgpu internal) Signal`**. Therefore labels are corroborating trace
metadata, not durable identity. `Queue::present` creates and commits another
native buffer labelled Present; it is separate from the user-buffer inventory.

See HAL Metal [blit factories and timestamp fallback](/Users/sma/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/wgpu-hal-29.0.4/src/metal/command.rs:157),
[render factory](/Users/sma/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/wgpu-hal-29.0.4/src/metal/command.rs:1019),
[compute factories](/Users/sma/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/wgpu-hal-29.0.4/src/metal/command.rs:1663),
and [submit/present](/Users/sma/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/wgpu-hal-29.0.4/src/metal/mod.rs:543).

## Work that has no unique original-view assignment today

The largest unresolved case is a pending-write buffer containing contributions
from different frames or worlds. The queue can be cloned and used outside the
render schedule; uploads can remain pending until a later unrelated submit.
Tagging that buffer with the current frame at submission misattributes its
earlier writes. Asset retries also retain `(asset ID, extracted asset)` rather
than the original extraction frame. A single native blit encoder containing
mixed contributions cannot be divided into per-frame stage durations afterward.

Capture provenance at upload/unmap enqueue time and retain all contributing
scopes. If the set is homogeneous, it may be assigned under a declared policy.
If mixed, classify it as mixed/unassigned and withhold a complete-frame sample.
Splitting pending encoders or flushing between owners would change submission
topology; that is outside this observation-only proposal. Freezing the main loop
or draining the GPU is not an attribution solution compatible with normal
pipelining.

Shared asset uploads, root compute, light-probe generation and auxiliary shadows
also lack one intrinsic camera owner. For one selected camera, explicitly declared
frame-shared dependencies can contribute once to that frame's cost, but that is a
defined accounting policy, not recovered original-view provenance. Additional
cameras/consumers or untracked background queue work invalidate that policy.
Resource IDs, thread identity and proximity in a trace are insufficient evidence.

## Readback, diagnostics and presentation policy

Keep render-to-target work separate from diagnostic readback and panel delivery,
while retaining every excluded command buffer in the inventory. Screenshots are
not merely a CPU image save: Bevy's final encoder copies its prepared texture to
a buffer and can render a `screenshot_to_screen_pass` back to the target. Its
capture preparation also redirects output. A capture frame is consequently
perturbed; do not treat excluding the final copy's duration as proving ordinary
unperturbed rendering. Generic GPU readback copies and map/CPU delivery need their
own class. See [screenshot commands](/Users/sma/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/bevy_render-0.19.0/src/view/window/screenshot.rs:498)
and [generic readback commands](/Users/sma/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/bevy_render-0.19.0/src/gpu_readback.rs:326).

Ushas's experimental timestamp resolver submits additional diagnostic buffers
from root graph Finish ([frame_timing.rs](../../src/frame_timing.rs:691)); optional
Tracy diagnostics do likewise. Counter-resolution/readback buffers belong to the
measurement protocol, not the scene-cost numerator, and must not recursively
create measured workload. Their interference remains an overhead-validation gate.

Normal swapchain presentation, uncovered-window clears and transaction scheduling
are explicit presentation/surface classes. Frame interpolation and the custom
presentation path add history copies and an owned second Metal queue
([present/resource.rs](../../src/present/resource.rs:101)); reject that mode from
the initial single-queue Spatial/Temporal producer. An offscreen profile excludes
these by construction but proves no panel delivery or window pacing.

## Smallest defensible attribution and seal seams

1. **Admit the extracted scope before preparation.** Freeze a unique original
   camera/target/configuration scope and separately declared frame-shared work.
   Reject unsupported views/layouts and tag bootstrap work explicitly. Propagate
   scope to worker tasks and retained asynchronous jobs; do not use live global
   metadata or mutate an encoder through `as_hal_mut` before wgpu encoding.
2. **Own logical encoder and upload provenance at creation/enqueue.** Cover both
   RenderContext encoder factories, standalone RenderDevice callers, raw MetalFX,
   and queue-write/unmap contributors. Record a side ledger; labels may aid trace
   joins but are mutable. Unknown underlying-device/queue access fails closed.
3. **Expand and register at wgpu-core/HAL submit.** Inherit scope for Transit and
   initialization work; inventory PendingWrites separately; observe actual Metal
   render/compute/blit families and every committed buffer, including empty-signal
   and presentation buffers. Preserve existing order and pipelining.
4. **Seal schedule membership, then complete asynchronously.** A system after
   `render_system` can account for its final readback and normal present calls;
   the safer full render-app boundary is after cleanup/PostCleanup and deferred
   work, with an explicit ban or classification for later GPU submissions. Mark
   the expected members sealed, but publish only after every registered included
   buffer completes successfully. Late members, missing callbacks, failed buffers,
   unknown/mixed ownership or bounded-ledger overflow invalidate that frame.

These are necessary seams, not a proven small patch or sufficient validation.
Before connecting a producer to the governor, reconcile the complete physical
inventory against a Metal trace for the declared feature profile, prove all
included stages and explicit exclusions, test delayed/out-of-order completions
and missing members, and measure instrumentation overhead with a declared control.
Stage unions must avoid double-counting overlapping channels/buffers; even a
validated stage interval must not be renamed exclusive GPU occupancy. Until
those gates pass, the existing validated-input boundary remains closed.
