# Changelog

All notable changes to `bevy_metalfx` are documented here.

The format follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/).
This is a `0.x` crate, so a **minor** bump is the carrier for breaking changes
and a patch bump never breaks. Entries before 0.3.0 were reconstructed from the
commit history when this file was added.

## [Unreleased]

The checkout is the unpublished **0.5.0-rc.1** candidate. These APIs are not
published; the README's crates.io quick start remains on the released 0.4 series.
Hardware and release acceptance gates are still in progress.

### Added

- `MetalFxEffectStatus`: bounded per-view observations of requested/effective
  modes, actual content/output dimensions, frame identity, and monotonic age.
  Pending, unavailable, failed, encoded, and output-written states replace stale
  success. Explicit freshness checks support render-world lag. Status resources
  remain available with disabled rendering and without a `RenderApp`.
- A deterministic `adaptive::AdaptiveController` and Bevy adapter configured by
  `MetalFxAdaptiveConfig`: explicit application target or primary-window monitor
  refresh (default, with a labelled 60 FPS fallback), quality floor
  (default 0.5), elapsed-time smoothing and hysteresis, settling, measured
  downshift-benefit checks, and an infeasible-budget reason at the floor.
- `MetalFxFrameCostInput`, `ValidatedGpuFrameCost`, and `MetalFxAdaptiveContext`
  for externally validated GPU frame-cost samples with frame/view/epoch/scale
  identity and provenance. `MetalFxAdaptiveStatus` exposes decisions and hold
  reasons. No validated timing producer is installed by default.
- `MetalFxDeviceScaleBand` queries the temporal scale band during plugin
  `finish`, with selector guards and explicit fallback provenance.
  `MetalFxQuality` provides device-filtered presets from one third to native;
  the adaptive quality floor further restricts the usable ladder.
- Opt-in `frame_timing::ExperimentalFrameTimingPlugin` with asynchronous
  per-view timestamp observations. It reports unvalidated measurements and
  never publishes them to the adaptive controller.
- A standalone render smoke with readiness checks, captures, structured
  artifacts, control arms, and explicit failure exits. See
  [tools/smoke/README.md](tools/smoke/README.md).

### Changed (breaking)

- Replaced app-time P99 adaptation with validated GPU frame-cost input. CPU-only
  slow frames, stale samples, pending scalers, and missing timing do not lower
  quality. Evidence resets on configuration/view changes and explicit workload
  resets; failed downshifts restore quality instead of repeatedly descending.
- Spatial and temporal upscaling now write the full-resolution view before
  tonemapping and later post-processing, preserving subsequent native-resolution
  UI. Custom pass ordering must account for this earlier stage. Frame
  interpolation retains its separate experimental late output path.

- `MetalFxHistoryReset` is now `Clone` rather than `Copy`. Requests persist
  through inactive or unready render frames and clear only when a temporal
  encode acknowledges the captured request; an old acknowledgement cannot
  consume a newer cut.

### Removed

- Public `AdaptiveScaleState`. Use `MetalFxAdaptiveConfig` and
  `MetalFxAdaptiveStatus`; `MetalFxPlugin::adaptive` remains the opt-in switch.

### Fixed

- Temporal jitter now uses the actual pixel displacement from Bevy's perspective
  projection. The previous horizontal sign produced a reproducible sawtooth
  artifact in static geometry; paired captured images confirm the correction.
- Frame interpolation uses `Time<Real>` for its inter-frame interval, so pausing
  or changing simulation speed does not alter interpolation timing.
- The reduced-scale `Disabled` comparison uses bilinear sampling of the actual
  content region. Bevy 0.19's final blit sampled the full allocated texture with
  nearest filtering, so it did not provide a valid reduced-scale control.
- Late-added cameras and initial, device-finalized, or policy-clamped scales
  receive resolution overrides without waiting for a further scale change.
  View/size changes invalidate scaler state; unsupported multiple-view and
  viewport layouts report explicit reasons.
- The instrumented `Disabled` command-buffer diagnostic is available before
  plugin early returns. `GpuTimingSink::snapshot()` returns chronological samples.

### Evidence and Limitations

- Effect status describes CPU-side encoding, not GPU completion or presentation.
  The command-buffer timer is a diagnostic interval that can include dependency
  waits; it cannot establish total GPU frame cost or GPU-boundness. Experimental
  frame timing remains unvalidated. No new GPU performance result is claimed.
- Restored the historical presentation record: Shadow Work commit
  [4253273976](https://github.com/mistakeknot/shadow-work/commit/4253273976ba58a454b6cf00ceaa11bd896db8a1)
  records the frozen `e5h3` gate's awake **13/0/0 strong pass**, and asleep
  **10/0/3 weak pass**. This supersedes the old blanket statement that drawable
  timestamps never populated; it does not validate the current checkout's
  presentation, latency, ordering, or interpolation UI/HDR composition.

## [0.4.2] — 2026-08-06

**Upgrade if you rely on the render scale — which is the entire point of this
crate.** Every release before this one, 0.1.0 through 0.4.1, asked for the
reduced render resolution in the wrong world, so Bevy never applied it. The
plugin logged the resolution it wanted and read the component back correctly,
and the GPU rasterized every pixel at full size anyway: MetalFX's cost with none
of its benefit.

### Fixed

- **`MainPassResolutionOverride` was only ever set in the main world, and Bevy
  reads it in the render world.** `main_opaque_pass_3d`, the prepass node and
  the view-uniform writer all take it as `Option<&MainPassResolutionOverride>`
  on the entity `extract_cameras` builds. Nothing carries it across the world
  boundary for you: it is absent from `extract_cameras`' fixed extract list, and
  it cannot be given an `ExtractComponentPlugin` because it is not `Clone`. So
  the main-world insert was inert, in the most expensive way available — the
  component read back correctly, the logs reported the intended resolution, and
  nothing rendered any smaller.

  Measured with a synthetic fragment load at 8000 serial-ALU iterations: a
  native 640×360 window ran 3.6–3.8× faster than native 1280×720, while the same
  640×360 requested through this override ran at full-resolution speed.

  What hid it for the crate's entire life is that the same call also inserts
  `MipBias`, and `MipBias` *is* on the extract list. Half the pair worked, so
  nothing downstream looked disconnected: the logs, the component read-back and
  the mip selection all agreed with each other, and disagreed only with the
  rasterizer.

  The render-world half now exists, both halves are registered by a single
  function so they cannot be separated again, and a test asserts the extract
  system is present in the render world's `ExtractSchedule`. Registration warns
  rather than silently doing nothing when there is no `RenderApp`, and a stale
  override is now removed instead of left behind.

- **A detached thread dereferenced an unretained `MTLDevice`.** Temporal and
  frame-interpolation scaler creation runs on `std::thread::spawn`, and was
  handed a raw pointer *borrowed* from wgpu's device under a safety note
  claiming the pointer would not outlive the scope. For the synchronous spatial
  path that was true. For the two threaded paths it was false by construction: a
  detached thread outlives the scope that spawned it, and nothing retained the
  object. The process survived only because wgpu happened to keep its own
  reference alive.

  Both spawns now take an owned, retained device handle, and neither function
  needs to be `unsafe` any more. This is **not** a demonstrated fix for a known
  crash: several SIGSEGVs were recorded faulting on exactly this thread inside
  `newTemporalScalerWithDevice:`, but they are unreproduced. The claim here is
  narrower — an unretained Objective-C reference handed to a detached thread is
  a defect on its own terms, and it happens to be the shape that produces that
  fault.

### Added

- **A warning when scaler creation has not returned after ten seconds.** A cold
  MPSGraph compile takes on the order of a second. Against a locked session,
  `newTemporalScalerWithDevice:` was measured not to return *at all* — 121
  seconds and 36,052 rendered frames, no result, no error, no crash. MetalFX
  never engaged and nothing said so; the only evidence was a single line saying
  creation had started. The node still waits rather than giving up, since a
  genuinely slow first compile should be allowed to finish, but it now says once
  that MetalFX is not running, that frames are being presented unscaled, and
  that a locked session or sleeping display is the usual cause.

### Changed

- `repository` points at `github.com/mistakeknot/ushas`, where this crate is
  actually developed. Through 0.4.1 it pointed at a two-commit snapshot cut the
  day after the first publish and never updated again; under `MIT OR Apache-2.0`
  that link is the mechanism by which the source promise is kept. The old URL
  redirects here, so the links in already-published versions still resolve.
- The version links at the bottom of this file now point at crates.io. They
  pointed at GitHub release tags that were never created, so all six 404'd.

## [0.4.1] — 2026-07-29

**Upgrade immediately if you are on 0.4.0.** That release panicked the moment
MetalFX first encoded, in every mode. It was compile-verified and unit-tested,
but nothing had run it on a GPU; the first hardware run found this in seconds.

### Fixed

- **Panic on the first MetalFX encode: "Mixing the wgpu encoding API with the
  raw encoding API is not permitted".** wgpu 29 tracks, per command encoder,
  whether it has been used through the wgpu API or through `as_hal`, and panics
  on the second one to appear. This pass necessarily does both — it copies the
  content region and resolves depth/motion with wgpu, then encodes MetalFX
  against the raw `MTLCommandBuffer` — so the raw half now gets a command
  encoder of its own, queued via `add_command_buffer` so submission order still
  matches encode order.

  wgpu 27 tracked no such state and allowed the mixing. This is the part of the
  wgpu 27 → 29 jump that mattered, and it is invisible to the compiler: every
  `as_hal` call site compiled unchanged, which is exactly what made 0.4.0 look
  safe. The 0.4.0 note claiming the wgpu major "was not the risky part" was
  wrong — it was, just not at compile time.

- **Frame interpolation panicked for a second reason**: the history snapshot
  (`prevColorTexture`) is a wgpu copy that was sharing the raw encoder. It now
  runs on the context encoder after the raw buffer is queued, which preserves
  the "snapshot lands after the interpolation pass" contract through
  command-buffer commit order instead of encode order within one buffer.

- **`MetalFxRenderScale` and `MetalFxModeResource` were missing whenever the
  plugin disabled itself** — on non-macOS builds and in `MetalFxMode::Disabled`
  at full resolution. The README promises the plugin "disables itself
  gracefully — no `#[cfg]` guards needed in your app code", but any app reading
  `Res<MetalFxRenderScale>`, the documented way to drive render scale, panicked
  with "Resource does not exist" on precisely those configurations. Both are now
  always published, reporting *effective* values (scale 1.0, mode `Disabled`)
  rather than the requested ones, so an adaptive governor cannot mistake an
  inactive plugin for headroom.

### Added

- A log line when the render world honours a `MetalFxHistoryReset`. The request
  is set in the main world and read in the render world after extraction, so a
  mistimed clear would drop it with no symptom but ghosting. This distinguishes
  "the reset did not help" from "the reset never arrived".

## [0.4.0] — 2026-07-29 — YANKED

Requires **Bevy 0.19** and **Rust 1.95**. If you are on Bevy 0.18, stay on 0.3 —
Bevy 0.19 removed the render graph, so no single version of this crate can
support both.

### Changed (breaking)

- **Requires Bevy 0.19 / wgpu 29.** MSRV moves 1.82 → 1.95, set by `bevy_ecs`
  0.19; nothing in this crate's own code needs it.
- **`MetalFxLabel` is now a `SystemSet`**, was a render-graph `RenderLabel`.
  Its job is unchanged — it is the handle you order against — so
  `.after(MetalFxLabel)` still means what it meant. Any `add_render_graph_edges`
  referencing it should simply be deleted; the plugin registers the ordering.
- **`MetalFxUpscaleNode` is no longer a `ViewNode`.** Bevy 0.19 drives rendering
  from ECS schedules, so the pass is the `metalfx_upscale` system and the type
  is now just the state it carries. Apps that let the plugin wire itself — the
  documented path — need no change.

### Added

- `MetalFxScaleRange`: the render-scale band MetalFX will accept, with
  `contains()` to check a scale before setting it and `as_upscale_ratios()` to
  see the converted values. Out-of-band scales previously produced a `nil`
  scaler with no diagnostic. The band is derived from the same configuration the
  scaler is created with, so the two cannot drift.
- `MetalFxHistoryReset`: request that temporal history be dropped on the next
  frame, for camera cuts, teleports and scene loads. Only the first frame reset
  before this, so a hard cut ghosted. The request clears itself after one frame;
  holding it set would suppress temporal accumulation entirely.

### Removed

- The `foreign-types` dependency. wgpu-hal 29 migrated its Metal handles from
  the `metal` crate to objc2, so `raw_handle()` returns an objc2 object this
  crate can point at directly and `ForeignType::as_ptr()` has nothing to
  convert. This also removed six unnecessary `unsafe` blocks.

### Notes

The wgpu 27 → 29 jump was expected to be the risky part and was not: every
`as_hal` call site compiled unchanged. The work was all on the Bevy side.

**This note was wrong, and 0.4.1 corrects it.** "Compiled unchanged" is not
"works unchanged": wgpu 29 added a *runtime* guard against mixing the wgpu and
raw encoding APIs on one command encoder, and this crate tripped it on the first
frame it encoded. The compile-time audit that produced this note could not have
found it. Only running the thing could.

Yanked: the release panics on the first frame it encodes, in every mode, on
every supported platform — `wgpu-core`'s "Mixing the wgpu encoding API with the
raw encoding API is not permitted". There is no configuration in which 0.4.0
renders, so it is not a degraded release but an unusable one. Fixed in 0.4.1.

## [0.3.0] — 2026-07-29

Presentation telemetry only. The plugin, the modes, the render scale and every
upscaling type are untouched — if you do not read `PresentSink` or
`PresentStats`, this is a drop-in upgrade. See **Upgrading from 0.2** in the
README for the migration, including why the first change is not merely a type
change.

### Changed (breaking)

- `PresentSink::counts()` now returns `(u64, u64, u64, u64, u64)`; it was
  `(u64, u64, usize, u64, u64)`. The third element changed **meaning as well as
  type**: it was `presented.len()`, the occupancy of a ring capped at
  `RING_CAPACITY` (480), and is now `displayed`, a cumulative count of frames
  that reached the display. The old value saturated at 480 while the four
  counters beside it kept climbing, so any run longer than 480 presents read as
  though presentation had stopped. The `usize` → `u64` move is deliberate: it
  turns a silent semantic change into a compile error at every use site.
- `PresentStats::interp_fps` renamed to `presented_fps`. No behavioural change,
  but the old name was wrong in a way that invited a wrong fix. A single
  presented-handler serves every drawable the crate presents, so the rate has
  always covered real and interpolated frames together; code that added a render
  rate to it to recover a total was double-counting the real frames.

### Fixed

- The reported presented rate was `mean_fps + <sink rate>`, which double-counted
  the real frames — a single-present run reported 2.00× its own render rate and
  a dual-present run 3.00×. `presented_fps` is now the measured total, with the
  synthesised share reported separately and labelled as derived.

## [0.2.1] — 2026-07-27

### Fixed

- Removed `package.metadata.docs.rs.default-target = "aarch64-apple-darwin"`.
  docs.rs cross-compiles from a Linux container and still has to build the
  dependency graph; `blake3` (pulled in by `bevy_asset`, so unavoidable) has a
  `cc`-based build script, and targeting Apple passes `-arch arm64
  -mmacosx-version-min=11.0` to Linux gcc, which rejects them. The build failed
  outright, and no docs page is strictly worse than the non-macOS stub.
- Corrected the copyright holder in the license files.

## [0.2.0] — 2026-07-26

### Added

- Dual presentation (`present` module, opt-in via `MetalFxPlugin::dual_present`):
  an owned `CAMetalLayer` above the one `wgpu` renders into, presenting the
  interpolated and real frames on consecutive vsyncs. Measured 1.99× the
  accepted-present rate at an unchanged render rate. Whether the frames reach
  the panel is **not** established — `MTLDrawable.presentedTime` never populates
  on the development machine, for this crate or for a minimal Metal window.
- `PresentSink` / `PresentStats`: presentation-interval telemetry — presented
  rate, judder, ordering inversions and drops.
- GPU-timing surface (`GpuTimingSink`, `GpuTimingStats`) for per-command-buffer
  GPU-elapsed time of the MetalFX pass.
- Adaptive render scaling, and a readable `MetalFxModeResource` reporting the
  mode that survived any runtime fallback.
- License texts, crate authors, and a consumer-check script that verifies a
  published build from outside the workspace.

### Changed (breaking)

- `MetalFxPlugin` gained `adaptive`, `gpu_timing_sink` and `dual_present`. Use
  `..default()` — the documented construction path — and this is not a break.
- `MetalFxConfig` fields are now private; it is a render-world mirror the plugin
  maintains, not a control surface. Set scale via `MetalFxRenderScale` and mode
  via `MetalFxPlugin::mode`.
- `MetalFxModeResource.0` is now private; read it with `.get()`.

### Fixed

- `--features temporal` did not compile on Linux or Windows in 0.1.0: `mod
  jitter` carried a `target_os = "macos"` gate while its only caller was gated
  on the feature alone, so any non-macOS build with the feature failed at
  `unresolved module jitter`. Nobody noticed for four months because nothing in
  the project had ever cross-compiled.
- The feature flags now gate real code. `objc2-metal-fx` is taken with
  `default-features = false`; its default feature set enables every binding it
  ships, including four `MTL4FX*` families this crate never touches, which made
  the per-feature gating a no-op.

## [0.1.0] — 2026-03-23 — YANKED

Initial release: MetalFX spatial and temporal upscaling as a Bevy render-graph
node, with the plugin disabling itself on unsupported platforms.

Yanked: `--features temporal` did not compile on any non-macOS target, so the
release was unusable for cross-platform consumers. Fixed in 0.2.0.

[0.4.2]: https://crates.io/crates/bevy_metalfx/0.4.2
[0.4.1]: https://crates.io/crates/bevy_metalfx/0.4.1
[0.4.0]: https://crates.io/crates/bevy_metalfx/0.4.0
[0.3.0]: https://crates.io/crates/bevy_metalfx/0.3.0
[0.2.1]: https://crates.io/crates/bevy_metalfx/0.2.1
[0.2.0]: https://crates.io/crates/bevy_metalfx/0.2.0
[0.1.0]: https://crates.io/crates/bevy_metalfx/0.1.0
