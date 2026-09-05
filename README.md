# bevy_metalfx

[![Crates.io](https://img.shields.io/crates/v/bevy_metalfx.svg)](https://crates.io/crates/bevy_metalfx)
[![Docs.rs](https://docs.rs/bevy_metalfx/badge.svg)](https://docs.rs/bevy_metalfx)
[![License](https://img.shields.io/crates/l/bevy_metalfx.svg)](https://github.com/mistakeknot/ushas#license)

Bevy plugin for Apple MetalFX upscaling and experimental frame interpolation.
It renders the scene at a reduced resolution and reconstructs a full-resolution
image. Whether this improves frame cost depends on the workload and hardware.

The repository is `ushas`; its published crate is `bevy_metalfx`.
**The quick start below uses the published 0.4 series. Sections marked
Development describe unpublished changes intended for 0.5, including a breaking
adaptive-controller replacement.** The checkout's package version remains 0.4.2
until a release is prepared. See [CHANGELOG.md](CHANGELOG.md).

## Quick Start

For Bevy 0.19:

```toml
[dependencies]
bevy_metalfx = "0.4"
```

```rust
use bevy::prelude::*;
use bevy_metalfx::{MetalFxMode, MetalFxPlugin};

fn main() {
    App::new()
        .add_plugins(DefaultPlugins)
        .add_plugins(MetalFxPlugin {
            render_scale: 0.5, // Half the output width and height.
            mode: MetalFxMode::Spatial,
            ..default()
        })
        .run();
}
```

Defaults are `render_scale: 0.5`, `mode: Spatial`, and `adaptive: false`.
Use `..default()` to accommodate optional platform-specific fields. Unsupported
platforms disable MetalFX while retaining the public configuration resources.

## Modes and Features

| Mode | Cargo feature | Minimum macOS |
|------|---------------|---------------|
| Spatial upscaling | `spatial` (default) | 13 |
| Temporal upscaling with depth and motion vectors | `temporal` | 13 |
| Experimental frame interpolation | `frame-interpolation` | 26 |

```toml
# Spatial only.
bevy_metalfx = "0.4"
# Temporal includes spatial.
bevy_metalfx = { version = "0.4", features = ["temporal"] }
# Interpolation includes temporal and spatial.
bevy_metalfx = { version = "0.4", features = ["frame-interpolation"] }
```

Features gate both the crate's encode paths and the corresponding MetalFX
bindings. Runtime support and scaler readiness must also be checked.

## Development: Render Path and Scope

Spatial and temporal upscaling run after the scene pass, before tonemapping and
later post-processing. The reconstructed scene then receives output-resolution
UI and Bevy's final output pass. `Disabled` at a reduced scale uses a bilinear
copy of the rendered content region as the comparison arm; `Disabled` at 1.0
is native rendering. Bevy 0.19's built-in final blit alone does not provide that
reduced-scale control: it samples the full allocated texture with nearest filtering.

Frame interpolation retains a separate, experimental late output path. Its UI
and HDR composition have not been established for the new early upscaling path.
With dual presentation off, it computes an intermediate frame without presenting
that frame. Use temporal mode when only upscaling is needed.

The current renderer supports one active 3D camera with a full-target viewport.
Multiple active views and offset or subrectangle viewports report unsupported
status. A per-view status registry does not imply multi-view rendering support.
Scaler cache identity includes the view, dimensions, format, and mode; readiness
is checked again after a replacement.

## API Reference

| Type | Purpose |
|------|---------|
| `MetalFxPlugin` | Initial mode, render scale, and adaptive opt-in |
| `MetalFxRenderScale` | Main-world render scale |
| `MetalFxModeResource::get()` | Mode selected after platform fallback |
| `MetalFxScaleRange` | Configured scaler scale band |
| `MetalFxHistoryReset` | Request a temporal-history reset |
| `MetalFxLabel` | Bevy `SystemSet` for pass ordering |
| `MetalFxConfig`, `MetalFxUpscaleNode` | Renderer state maintained by the plugin |

### Development: Observe the Effect

`MetalFxEffectStatus` records each view's requested and effective mode, requested
scale, actual input/output dimensions in physical pixels, frame identity, and
monotonic observation time. View IDs are main-world camera `Entity::to_bits()`
values, including the entity generation.

States distinguish `Disabled`, `Unavailable`, `NoRender`, `Pending`, `Failed`,
`Encoded`, and `OutputWritten`, with reasons where available. A new pending or
fallback observation replaces earlier success. **`Encoded` and `OutputWritten`
are evidence that CPU-side commands were encoded; neither proves GPU completion
or presentation.** Reading the configured mode alone is insufficient.

```rust
use bevy::prelude::*;
use bevy_metalfx::{MetalFxEffectStatus, MetalFxObservationFrame};
use std::time::Duration;

fn inspect_effect(
    frame: Res<MetalFxObservationFrame>,
    status: Res<MetalFxEffectStatus>,
    cameras: Query<Entity, With<Camera3d>>,
) {
    for camera in &cameras {
        let snapshot = status.snapshot(camera.to_bits(), frame.0);
        // Allow the expected render-world lag, with a wall-clock limit too.
        if snapshot.is_fresh(2, Duration::from_millis(250)) {
            if let Some(observed) = snapshot.last_observation {
                println!("{:?}: {:?} -> {:?}, {:?}",
                    observed.state, observed.content_size,
                    observed.output_size, observed.reason);
            }
        }
    }
}
```

`snapshot.state()` is strict: missing, older, and future-frame observations read
as `NoRender`. Consumers that tolerate render-world lag must explicitly check
`is_fresh()` before using `last_observation`. The status and observation-frame
resources are available even when rendering is disabled or no `RenderApp` exists.

### Development: Scale Bounds and Quality Presets

`MetalFxDeviceScaleBand` reports the device's allowed band;
`MetalFxScaleRange` reports the band configured for the scaler. Render scale is
`input / output`, the reciprocal of MetalFX's upscale ratio. Both types provide
`contains()`, and the configured range exposes `as_upscale_ratios()`.

The device query runs in plugin `finish`, once a render device exists.
`is_from_device()` distinguishes a queried band from a fallback. The previously
tested M5 Max reported a maximum temporal ratio of 3.0, admitting a scale of
one third; the conservative temporal fallback admits one half. Spatial mode
uses the plugin's `0.1..=1.0` range rather than a reported temporal band.

```rust
use bevy::prelude::*;
use bevy_metalfx::{MetalFxDeviceScaleBand, MetalFxQuality};

fn settings_menu(band: Res<MetalFxDeviceScaleBand>) {
    for preset in MetalFxQuality::ALL {
        if preset.is_available_on(&band) {
            println!("{preset:?}: {:.3}", preset.render_scale());
        }
    }
}
```

The presets are `UltraPerformance` (1/3), `Performance` (1/2), `Balanced`
(0.58), `Quality` (2/3), and `Native` (1.0).
`MetalFxQuality::ladder(&band)` returns admitted scales in ascending order.
The adaptive quality floor further restricts that ladder: its default of 0.5
keeps one third disabled even on hardware that admits it.

### Development: Adaptive Resolution

Set `adaptive: true` and configure `MetalFxAdaptiveConfig`. The default target
is explicitly 60 rendered FPS, independent of monitor refresh, with a minimum
scale of 0.5. Set the target to the application's intended budget.

```rust
use bevy::prelude::*;
use bevy_metalfx::{MetalFxAdaptiveConfig, MetalFxPlugin};
use bevy_metalfx::adaptive::AdaptiveConfig;

fn main() {
    App::new()
        .insert_resource(MetalFxAdaptiveConfig {
            policy: AdaptiveConfig {
                target_fps: 60.0,
                minimum_scale: 0.5,
                ..default()
            },
            ..default()
        })
        .add_plugins(DefaultPlugins)
        .add_plugins(MetalFxPlugin { adaptive: true, ..default() })
        .run();
}
```

**No validated GPU frame-cost source is installed by default.** The controller
holds its configured quality and reports the reason through
`MetalFxAdaptiveStatus` until an external adapter supplies suitable samples.
App frame intervals, virtual `Time`, presentation cadence, and the isolated
MetalFX command-buffer diagnostic are not GPU frame-cost inputs.

An external adapter publishes `ValidatedGpuFrameCost` through
`MetalFxFrameCostInput::publish_validated()`. Each sample carries its view,
frame, configuration epoch, measured scale, monotonic sampling time, positive
GPU milliseconds, instrument name, and validation reference. The reference is
an attestation by the adapter author; filling in that field does not validate a
measurement. Establish the instrument's GPU semantics and frame coverage first.
Capture `MetalFxAdaptiveContext::snapshot()` in the render world when recording
the frame, and preserve that identity through asynchronous completion.

The pure `adaptive::AdaptiveController` uses elapsed-time smoothing, settling,
and separate overload/headroom intervals. The controller and adapter reject
missing, stale, duplicate, future, wrong-scale, and wrong-epoch evidence.
CPU-only slow frames do not lower quality. After a downshift the controller
requires measured GPU-cost improvement before
continuing; otherwise it restores the previous rung. It reports an infeasible
budget when overloaded at the permitted floor and can recover quality when
headroom returns. These decisions are policy behavior, not a performance guarantee.

View, size, mode, and policy changes invalidate old evidence. Call
`MetalFxAdaptiveContext::request_reset()` after a camera cut, workload change,
or timing-instrument change. It does not replace a temporal-history reset.

### Temporal History Reset

```rust
use bevy::prelude::*;
use bevy_metalfx::MetalFxHistoryReset;

fn on_teleport(mut reset: ResMut<MetalFxHistoryReset>) {
    reset.request();
}
```

The request applies to the next rendered frame and clears itself. Spatial mode
ignores it. For adaptive consumers, also reset the measurement context after a
workload discontinuity so old cost evidence does not drive the new scene.

### GPU Timing and Measurement

On macOS, `GpuTimingSink` / `GpuTimingStats` expose the dedicated MetalFX
command buffer's elapsed interval. It can include dependency waits and excludes
other scene work. It cannot establish total GPU frame cost, isolated scaler
execution cost, or whether the application is GPU-bound. In the development
checkout, the instrumented `Disabled` arm submits an empty timed buffer as a
diagnostic control.

**Development:** `frame_timing::ExperimentalFrameTimingPlugin` adds optional
per-view timestamp markers. Enable it explicitly after `MetalFxPlugin` and
request `frame_timing::requested_features()` in the renderer's device settings.
Read observations from `frame_timing::ExperimentalFrameTiming`. Marker coverage,
output preservation, and overhead require validation; `ObservedUnvalidated`
remains unvalidated even when timestamps are positive. This plugin never feeds
the adaptive governor.

Use the [standalone render smoke](tools/smoke/README.md) for the build command,
readiness gates, matching control arms, captures, and artifact contract. A
passing capture is evidence of rendered content, not physical panel delivery.
No new GPU speedup or automatic timing validation is claimed for this development
API.

## Experimental Dual Presentation

With `frame-interpolation`, `present::MetalFxDualPresent` opts into an owned
`CAMetalLayer` above Bevy's layer. It presents the interpolated and real frames
through the same telemetry path. `PresentSink` separates encoded work,
completion callbacks, committed presents, and drawable presentation timestamps.
Presented FPS is the total measured rate; adding render FPS double-counts real
frames. Accepted presents alone do not establish displayed frames or latency.

The historical evidence has two stages. An early test recorded approximately
1.99 times as many accepted presents with nearly unchanged render cadence;
presentation timestamps were unavailable in that run. Later, Shadow Work's
frozen `e5h3` gate recorded **13 passed, 0 failed, 0 skipped, PASS (strong)** on
an awake display. The fix and verdict are preserved in
[Shadow Work commit 4253273976](https://github.com/mistakeknot/shadow-work/commit/4253273976ba58a454b6cf00ceaa11bd896db8a1).
That commit also records a 10/0/3 weak pass with the display asleep.

The later strong result supersedes the old blanket README claim that drawable
timestamps never populated. It remains historical evidence for that checkout
and setup. The current development path still needs a fresh, pinned replay of
presentation cadence, ordering, latency, and UI/HDR composition; it has no new
panel-delivery claim.

## Bevy Compatibility

| bevy_metalfx | Bevy | MSRV |
|-------------|------|------|
| Development (unpublished) | 0.19 | 1.95 |
| 0.4.1–0.4.2 | 0.19 | 1.95 |
| ~~0.4.0~~ (yanked) | 0.19 | 1.95 |
| 0.1–0.3 | 0.18 | 1.82 |

0.4.0 panicked on its first MetalFX encode; 0.4.1 fixed the raw/wgpu encoder
mixing. 0.4.2 fixed extraction of the reduced-resolution override. Historical
hardware checks and their limits are recorded in the [changelog](CHANGELOG.md);
compilation alone does not establish rendered behavior.

## Development: Upgrading from 0.4

The upcoming 0.5 changes replace the app-time P99 governor and remove the public
`AdaptiveScaleState` type. Read `MetalFxAdaptiveStatus` and configure
`MetalFxAdaptiveConfig` instead. Existing `MetalFxPlugin { adaptive: true, .. }`
code must supply an externally validated GPU frame-cost adapter to obtain
performance-driven scale changes. Without one, scale holds after configuration
and floor clamping. The default quality floor remains 0.5 even when the device
admits lower presets.

Spatial and temporal output now enters the view before tonemapping and UI.
Consumers that ordered custom systems around the old late MetalFX output must
review that ordering. Frame interpolation retains its experimental late path.

## Upgrading from 0.3

**If you are on Bevy 0.18, stay on `bevy_metalfx` 0.3.** 0.4 requires Bevy 0.19
and cannot work with 0.18: Bevy 0.19 removed the render graph, and this crate's
pass is now a system in a schedule. There is no version of this crate that
supports both.

The upgrade is mostly Bevy's, not ours. Most apps change nothing in their
MetalFX code — `MetalFxPlugin`, `MetalFxMode`, `MetalFxRenderScale`,
`is_available()` and `probe_spatial_scaler()` are all untouched.

| Change | Why | Fix |
|---|---|---|
| Requires Bevy 0.19 and Rust 1.95 | `bevy_ecs` 0.19 sets the MSRV; nothing here needs it | upgrade both |
| `MetalFxLabel` is a `SystemSet`, was a `RenderLabel` | Bevy 0.19 has no render graph to label | `.after(MetalFxLabel)` still works; drop any `add_render_graph_edges` |
| `MetalFxUpscaleNode` is no longer a `ViewNode` | the pass is the `metalfx_upscale` system now | remove any manual graph wiring; the plugin registers it |

If you never referenced the node or the label directly — the common case, since
the plugin wires itself — 0.4 is a drop-in once you are on Bevy 0.19.

**New in 0.4**, both closing gaps that used to fail silently:

- `MetalFxScaleRange` reports the render-scale band MetalFX will accept, and
  `contains()` lets you check a scale *before* setting it. Previously an
  out-of-band scale produced a `nil` scaler with no diagnostic at all.
- `MetalFxHistoryReset` lets you drop accumulated temporal history on a camera
  cut, teleport or scene load. Previously only the very first frame reset, so a
  hard cut ghosted.

## Upgrading from 0.2

0.3 changes two things on the presentation-telemetry surface. Both are breaking
only for code that reads `PresentSink`/`PresentStats` — the plugin, the modes,
the render scale and every upscaling type are untouched, so if you do not sample
presentation telemetry, 0.3 is a drop-in.

| Change | Why | Fix |
|---|---|---|
| `PresentSink::counts()` returns `(u64, u64, u64, u64, u64)`, was `(u64, u64, usize, u64, u64)` | the third element is a real counter now, not a length | see below — **read the note, do not just fix the type** |
| `PresentStats::interp_fps` renamed `presented_fps` | it was never the interpolated-frame rate | rename at the use site |

**The third element of `counts()` changed meaning, not just type.** It used to
be `presented.len()` — the occupancy of the sample ring, which is capped at
`RING_CAPACITY` (480). The other four counters are cumulative and unbounded, so
past 480 presents the old third element sat at 480 forever while `encoded`,
`callbacks` and `committed` kept climbing. Read as a row, that looks exactly
like presentation having stopped 480 frames in. It is now `displayed`, a
cumulative count of frames that actually reached the display, reset only by
`reset()`.

```rust
// 0.2 — third element saturates at 480
let (encoded, dropped, presented_len, callbacks, committed) = sink.counts();

// 0.3 — third element is cumulative, like the other four
let (encoded, dropped, displayed, callbacks, committed) = sink.counts();
```

The type moved from `usize` to `u64` in the same change, and that is deliberate:
it makes the compiler stop you. Had the type stayed put, a saturating gauge
would have silently become an unbounded counter and every dashboard built on it
would have kept rendering without a single diagnostic. If you were storing the
old value as a "how full is the ring" signal, there is no replacement for it —
that was never a useful number to expose, and it is the ring's own business.

**`interp_fps` → `presented_fps`** is a rename with no behavioural change, but
the old name was actively wrong. One presented-handler serves every drawable the
crate presents, so the rate it measures has always covered real and interpolated
frames together. Code that added a render rate to `interp_fps` to recover a
total was double-counting the real frames; delete the addition.

```rust
// 0.2 — the name invited this, and it is wrong
let total = stats.interp_fps + render_fps;

// 0.3 — the name says what it always measured
let total = stats.presented_fps;
```

## Upgrading from 0.1

0.2 carries breaking changes, which is what the minor bump signals for a `0.x`
crate. In practice one pattern covers almost all of it:

```rust
// 0.1 — exhaustive struct literal
MetalFxPlugin { render_scale: 0.5, mode: MetalFxMode::Spatial }

// 0.2 — `..default()` absorbs the new fields
MetalFxPlugin { render_scale: 0.5, mode: MetalFxMode::Spatial, ..default() }
```

| Change | Why | Fix |
|---|---|---|
| `MetalFxPlugin` gained `adaptive`, `gpu_timing_sink`, `dual_present` | adaptive scaling, GPU timing, dual presentation | add `..default()` |
| `MetalFxConfig` fields are now private | it is a render-world mirror the plugin maintains, not a control surface | set scale via `MetalFxRenderScale`, mode via `MetalFxPlugin::mode` |
| `MetalFxModeResource.0` is now private | reading is meaningful, writing is not | `.get()` |

`MetalFxMode`, `MetalFxRenderScale`, `MetalFxLabel`, `MetalFxUpscaleNode`,
`is_available()` and `probe_spatial_scaler()` are unchanged.

**Fixed in 0.2:** `features = ["temporal"]` did not compile on Linux or Windows
in 0.1 — an internal module was platform-gated while its caller was not, so the
build failed with `unresolved module jitter`. If you build cross-platform with
temporal upscaling, 0.1 never worked for you off macOS; 0.2 does.

## Platform Support

- **macOS 13+** (Apple Silicon): Spatial and temporal upscaling; mode support is checked at runtime
- **macOS 26+**: Experimental frame interpolation
- **macOS < 13**: Plugin disables itself gracefully
- **Linux / Windows**: Plugin disables itself; type stubs available for cross-platform code

## Documentation

- [API docs on docs.rs](https://docs.rs/bevy_metalfx)
- [Apple MetalFX documentation](https://developer.apple.com/documentation/metalfx)
- [objc2-metal-fx bindings](https://docs.rs/objc2-metal-fx)
- [Render smoke and measurement procedure](tools/smoke/README.md)

## License

Licensed under either of

- Apache License, Version 2.0 ([LICENSE-APACHE](LICENSE-APACHE) or
  <https://www.apache.org/licenses/LICENSE-2.0>)
- MIT license ([LICENSE-MIT](LICENSE-MIT) or
  <https://opensource.org/licenses/MIT>)

at your option.

Unless you explicitly state otherwise, any contribution intentionally submitted
for inclusion in this crate by you, as defined in the Apache-2.0 license, shall
be dual licensed as above, without any additional terms or conditions.
