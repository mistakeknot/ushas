# bevy_metalfx

[![Crates.io](https://img.shields.io/crates/v/bevy_metalfx.svg)](https://crates.io/crates/bevy_metalfx)
[![Docs.rs](https://docs.rs/bevy_metalfx/badge.svg)](https://docs.rs/bevy_metalfx)
[![License](https://img.shields.io/crates/l/bevy_metalfx.svg)](https://github.com/mistakeknot/ushas#license)

Apple MetalFX upscaling for Bevy 0.20. Render at a reduced resolution and let
MetalFX reconstruct the full-resolution image.

The repository is `ushas`; the crate is `bevy_metalfx`.

![Bilinear, MetalFX Spatial and MetalFX Temporal at 0.5 render scale](https://raw.githubusercontent.com/mistakeknot/ushas/main/docs/images/hero-three-panel.png)

*Neutral test scene at 1920×1080 output, rendered at half resolution: bilinear
stretch, MetalFX Spatial, MetalFX Temporal. Unscaled crops, Apple M5 Max.*

**Does it help?** It depends on your scene; do not expect a speed-up by default.
MetalFX saves time by rendering fewer pixels, and it has a fixed cost of its
own. On a light scene, Temporal was about 1.44 times slower than native MSAA4,
and half-resolution bilinear was no faster than native. Under heavy fragment load
(1280x720, load 8,000), half-resolution Temporal completed a frame in 0.41 times
the native time (95% interval 0.38 to 0.43): 18.6 ms against 45.9 ms for native.
Plain half-resolution bilinear took 16.9 ms. Temporal costs a little more than
the bilinear stretch, and in return it reconstructs the image shown above.
Measured with Bevy 0.20 on Apple M5 Max as serial offscreen timing, not
production FPS. Details and limits are in the
[measurement report](https://github.com/mistakeknot/ushas/blob/main/docs/research/claude-completion-campaign-02.md).

## Quick Start

```toml
[dependencies]
bevy_metalfx = "0.5"
```

| bevy_metalfx | Bevy | Rust |
|--------------|------|------|
| 0.5 | 0.20 | 1.97.1 |
| 0.4 | 0.19 | 1.95 |
| 0.1 to 0.3 | 0.18 | 1.82 |

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

Runnable examples live in [`examples/`](https://github.com/mistakeknot/ushas/blob/main/examples):
[`spatial`](https://github.com/mistakeknot/ushas/blob/main/examples/spatial.rs),
[`temporal_with_ui`](https://github.com/mistakeknot/ushas/blob/main/examples/temporal_with_ui.rs) (feature `temporal`) and
[`quality_menu`](https://github.com/mistakeknot/ushas/blob/main/examples/quality_menu.rs). Upgrading from an earlier release:
[docs/upgrading.md](https://github.com/mistakeknot/ushas/blob/main/docs/upgrading.md).

## Modes and Features

Spatial and temporal upscaling are supported; frame interpolation and adaptive resolution are experimental. One active 3D camera with a full-target viewport.

| Mode | Cargo feature | Minimum macOS | Status |
|------|---------------|---------------|--------|
| Spatial upscaling | `spatial` (default) | 13 | Supported |
| Temporal upscaling with depth and motion vectors | `temporal` | 13 | Supported |
| Frame interpolation | `frame-interpolation` | 26 | Experimental |
| Adaptive resolution | none (`adaptive: true`) | 13 | Experimental |

```toml
# Spatial only.
bevy_metalfx = "0.5"
# Temporal includes spatial.
bevy_metalfx = { version = "0.5", features = ["temporal"] }
# Interpolation includes temporal and spatial.
bevy_metalfx = { version = "0.5", features = ["frame-interpolation"] }
```

Features gate both the crate's encode paths and the corresponding MetalFX
bindings. Runtime support and scaler readiness must also be checked. The
adaptive controller installs no validated GPU timing producer by default; until
an external adapter supplies one it holds the configured quality.

Hardware: tested on Apple M5 Max; also run by a contributor on M5 Pro. All
benchmark figures are from the M5 Max only.

## Render Path and Scope

Spatial and temporal upscaling run after the scene pass, before tonemapping and
later post-processing. The reconstructed scene then receives output-resolution
UI and Bevy's final output pass. `Disabled` at a reduced scale uses a bilinear
copy of the rendered content region as the comparison arm; `Disabled` at 1.0
is native rendering. Bevy's built-in final blit (0.19 and 0.20) alone does not
provide that reduced-scale control: it samples the full allocated texture with
nearest filtering.

Frame interpolation retains a separate, experimental late output path. Its UI
and HDR composition have not been established for the new early upscaling path.
With dual presentation off, it computes an intermediate frame without presenting
that frame. Use temporal mode when only upscaling is needed.

The current renderer supports one active 3D camera with a full-target viewport.
That camera may render to a window or to an image: the reduced-resolution main
pass is sized from the camera's own render target, so a windowless capture app
gets the same upscale as a windowed one.
Multiple active views and offset or subrectangle viewports report unsupported
status. A per-view status registry does not imply multi-view rendering support.
Scaler cache identity includes the view, dimensions, format, and mode; readiness
is checked again after a replacement.

## Smoke Test and Measurements

The [smoke sample](https://github.com/mistakeknot/ushas/blob/main/tools/smoke/README.md) is a standalone render fixture with
readiness checks, matching control arms, captures and an artifact contract. The
first balanced [completed-render campaign](https://github.com/mistakeknot/ushas/blob/main/docs/research/claude-completion-campaign-01.md)
passed all 60 runs and shows a workload-dependent fixed-scale benefit. Its serial
offscreen rate includes CPU scheduling and polling; it is not production
pipelined FPS or a GPU-cost signal for automatic adaptation.

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

### Observe the Effect

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

### Scale Bounds and Quality Presets

`MetalFxDeviceScaleBand` reports the device's allowed band;
`MetalFxScaleRange` reports the band configured for the scaler. Render scale is
`input / output`, the reciprocal of MetalFX's upscale ratio. Both types provide
`contains()`, and the configured range exposes `as_upscale_ratios()`.

The device query runs in plugin `finish`, once a render device exists.
`is_from_device()` distinguishes a queried band from a fallback. An
Apple M5 Max reported a maximum temporal ratio of 3.0, admitting a scale of
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

### Adaptive Resolution (experimental)

Set `adaptive: true` and configure `MetalFxAdaptiveConfig`. The default target
uses the refresh reported for the primary window's current monitor, with a
labelled 60 FPS fallback when that information is unavailable. The minimum
scale defaults to 0.5. Set `Explicit` when the application has its own budget;
an explicit 60 FPS target takes precedence even on a faster monitor.

```rust
use bevy::prelude::*;
use bevy_metalfx::{MetalFxAdaptiveConfig, MetalFxAdaptiveTarget, MetalFxPlugin};

fn main() {
    App::new()
        .insert_resource(MetalFxAdaptiveConfig {
            target: MetalFxAdaptiveTarget::Explicit(60.0),
            minimum_scale: 0.5,
            ..default()
        })
        .add_plugins(DefaultPlugins)
        .add_plugins(MetalFxPlugin { adaptive: true, ..default() })
        .run();
}
```

`MetalFxAdaptiveStatus` exposes the resolved target and its source. Monitor
refresh is operating-system metadata, not measured presentation cadence or
variable refresh. Bevy 0.19 can retain that metadata after an in-place display
mode change; set an explicit budget when the distinction matters. Moving to a
different reported monitor or changing the resolved budget resets old evidence.
The target defines the controller's budget; it does not pace the application.

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

The request stays pending until a Temporal or FrameInterpolation command
encodes the reset. Inactive views, pending scalers, and failed encode attempts
preserve it; Spatial mode does not consume it. An older frame's acknowledgement
cannot clear a newer request. This acknowledgement proves command encoding,
not GPU completion. For adaptive consumers, also reset the measurement context
after a workload discontinuity so old cost evidence does not drive the new scene.

### GPU Timing and Measurement

On macOS, `GpuTimingSink` / `GpuTimingStats` expose the dedicated MetalFX
command buffer's elapsed interval. It can include dependency waits and excludes
other scene work. It cannot establish total GPU frame cost, isolated scaler
execution cost, or whether the application is GPU-bound. The instrumented `Disabled` arm submits an empty timed buffer as a
diagnostic control.

`frame_timing::ExperimentalFrameTimingPlugin` adds optional
per-view timestamp markers. Enable it explicitly after `MetalFxPlugin` and
request `frame_timing::requested_features()` in the renderer's device settings.
Read observations from `frame_timing::ExperimentalFrameTiming`. Marker coverage,
output preservation, and overhead require validation; `ObservedUnvalidated`
remains unvalidated even when timestamps are positive. This plugin never feeds
the adaptive governor.

Use the [standalone render smoke](https://github.com/mistakeknot/ushas/blob/main/tools/smoke/README.md) for the build command,
readiness gates, matching control arms, captures, and artifact contract. A
passing capture is evidence of rendered content, not physical panel delivery.
No GPU speedup or automatic timing validation is claimed for this API.

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
and setup. The current path still needs a fresh, pinned replay of
presentation cadence, ordering, latency, and UI/HDR composition; it makes no panel-delivery claim.

## Bevy Compatibility

| bevy_metalfx | Bevy | MSRV |
|-------------|------|------|
| 0.5 | 0.20 | 1.97.1 |
| 0.4.1–0.4.2 | 0.19 | 1.95 |
| ~~0.4.0~~ (yanked) | 0.19 | 1.95 |
| 0.1–0.3 | 0.18 | 1.82 |

0.4.0 panicked on its first MetalFX encode; 0.4.1 fixed the raw/wgpu encoder
mixing. 0.4.2 fixed extraction of the reduced-resolution override. Historical
hardware checks and their limits are recorded in the [changelog](https://github.com/mistakeknot/ushas/blob/main/CHANGELOG.md);
compilation alone does not establish rendered behavior.

Upgrading from an earlier release? See [docs/upgrading.md](https://github.com/mistakeknot/ushas/blob/main/docs/upgrading.md).

## Platform Support

- **macOS 13+** (Apple Silicon): Spatial and temporal upscaling; mode support is checked at runtime
- **macOS 26+**: Experimental frame interpolation
- **macOS < 13**: Plugin disables itself gracefully
- **Linux / Windows**: Plugin disables itself; type stubs available for cross-platform code

## Documentation

- [API docs on docs.rs](https://docs.rs/bevy_metalfx) (built on Linux, so macOS-only modules are missing there; on a Mac run `cargo doc --open -p bevy_metalfx --all-features`)
- [Apple MetalFX documentation](https://developer.apple.com/documentation/metalfx)
- [objc2-metal-fx bindings](https://docs.rs/objc2-metal-fx)
- [Render smoke and measurement procedure](https://github.com/mistakeknot/ushas/blob/main/tools/smoke/README.md)
- [Upgrading](https://github.com/mistakeknot/ushas/blob/main/docs/upgrading.md) and [changelog](https://github.com/mistakeknot/ushas/blob/main/CHANGELOG.md)

## License

Licensed under either of

- Apache License, Version 2.0 ([LICENSE-APACHE](https://github.com/mistakeknot/ushas/blob/main/LICENSE-APACHE) or
  <https://www.apache.org/licenses/LICENSE-2.0>)
- MIT license ([LICENSE-MIT](https://github.com/mistakeknot/ushas/blob/main/LICENSE-MIT) or
  <https://opensource.org/licenses/MIT>)

at your option.

Unless you explicitly state otherwise, any contribution intentionally submitted
for inclusion in this crate by you, as defined in the Apache-2.0 license, shall
be dual licensed as above, without any additional terms or conditions.
