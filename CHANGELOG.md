# Changelog

All notable changes to `bevy_metalfx` are documented here.

The format follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/).
This is a `0.x` crate, so a **minor** bump is the carrier for breaking changes
and a patch bump never breaks. Entries before 0.3.0 were reconstructed from the
commit history when this file was added.

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

[0.3.0]: https://github.com/mistakeknot/bevy_metalfx/releases/tag/v0.3.0
[0.2.1]: https://github.com/mistakeknot/bevy_metalfx/releases/tag/v0.2.1
[0.2.0]: https://github.com/mistakeknot/bevy_metalfx/releases/tag/v0.2.0
[0.1.0]: https://github.com/mistakeknot/bevy_metalfx/releases/tag/v0.1.0
