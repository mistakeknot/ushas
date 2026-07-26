# bevy_metalfx

[![Crates.io](https://img.shields.io/crates/v/bevy_metalfx.svg)](https://crates.io/crates/bevy_metalfx)
[![Docs.rs](https://docs.rs/bevy_metalfx/badge.svg)](https://docs.rs/bevy_metalfx)
[![License](https://img.shields.io/crates/l/bevy_metalfx.svg)](https://github.com/mistakeknot/bevy_metalfx#license)

Bevy plugin for Apple MetalFX upscaling and frame interpolation.

Renders your scene at a lower resolution and uses MetalFX's ML-based upscaling
to reconstruct a full-resolution image, improving performance on Apple Silicon Macs.

## Features

| Mode | Description | macOS Version | Cargo Feature |
|------|-------------|---------------|---------------|
| **Spatial** | Single-frame ML upscaling | 13+ | `spatial` (default) |
| **Temporal** | Multi-frame upscaling with motion vectors | 13+ | `temporal` |
| **Frame Interpolation** | Generate intermediate frames | 26+ (Metal 4) | `frame-interpolation` |

## Quick Start

```toml
[dependencies]
bevy_metalfx = "0.2"
```

```rust
use bevy::prelude::*;
use bevy_metalfx::{MetalFxPlugin, MetalFxMode};

fn main() {
    App::new()
        .add_plugins(DefaultPlugins)
        .add_plugins(MetalFxPlugin {
            render_scale: 0.5, // Render at half resolution
            mode: MetalFxMode::Spatial,
            ..default() // adaptive = false, and any platform-specific fields
        })
        .run();
}
```

`MetalFxPlugin` implements `Default` (`render_scale: 0.5`, `mode: Spatial`,
`adaptive: false`), so `MetalFxPlugin::default()` or `..default()` picks up
sensible defaults — including the macOS-only GPU-timing hook, which you never
need to set by hand.

On non-macOS platforms, the plugin detects that MetalFX is unavailable and
gracefully disables itself — no `#[cfg]` guards needed in your app code.

## How It Works

```text
Scene render (low-res via MainPassResolutionOverride)
  -> MetalFX upscale (raw Metal encode on command buffer)
    -> Full-res output texture
      -> Blit to swapchain
```

The plugin inserts a render graph node after Bevy's built-in `UpscalingNode`.
It extracts raw Metal textures and command buffers from wgpu-hal and encodes
the MetalFX upscale pass directly, then blits the result to the swapchain.

### Architecture

- **`lib.rs`** — Plugin setup, `MetalFxPlugin`, `MetalFxMode` enum, resolution override systems
- **`platform.rs`** — Raw `objc2-metal-fx` bindings: scaler creation, encode functions, format mapping
- **`node.rs`** — Bevy render graph `ViewNode`; `run` orchestrates, phases live in child modules
  - **`node/scaler.rs`** — scaler lifecycle and the textures sized to it
  - **`node/resolve.rs`** — depth + motion prepass resolve to content size
  - **`node/encode.rs`** — the MetalFX encode, one arm per mode
- **`present/`** — Dual presentation on an owned `CAMetalLayer` (`frame-interpolation` only)
- **`jitter.rs`** — Halton(2,3) jitter sequence for temporal upscaling (matches Bevy's TAA)

### ObjC Runtime Interop

wgpu-hal uses the `metal` crate (built on `objc` v0.2), while MetalFX bindings
use `objc2` v0.6. Both wrap the same ObjC `id` pointers — the platform module
bridges between them via raw `*mut c_void` casts.

## Feature Flags

```toml
# Spatial only (default) — stable
bevy_metalfx = "0.2"

# Temporal upscaling (adds motion vector + depth prepass) — stable
bevy_metalfx = { version = "0.2", features = ["temporal"] }

# Frame interpolation (requires macOS 26+) — see the limitation below
bevy_metalfx = { version = "0.2", features = ["frame-interpolation"] }
```

The features are cumulative (`frame-interpolation` implies `temporal` implies
`spatial`) and they genuinely shrink the build: each one gates both this crate's
encode paths and the corresponding `objc2-metal-fx` bindings, so a `spatial`
build does not compile the temporal or interpolation surface at all.

The `frame-interpolation` path is complete and stable: it runs a temporal
upscale, then feeds two consecutive *upscaled* frames plus the content-sized
depth/motion pair to an `MTLFXFrameInterpolator`, using real camera parameters
from Bevy's `Projection` and a real frame delta. It passes the Metal debug
layer, and holds the same ~120 fps as `temporal` on an M5 Max.

> **Known limitation — the synthesized frame is not presented.** Interpolation
> only buys frame rate if you display the interpolated frame *and* the real one,
> paced to the display's refresh: two presents per simulated frame. A Bevy render
> graph presents its swapchain once per `App::update()`, so this node presents
> the genuine upscaled frame and leaves the interpolated one in an offscreen
> texture.
>
> Net effect with dual presentation off: visuals identical to `temporal`, plus
> the GPU cost of the interpolation pass — roughly 5–7 ms/frame at 3024×1800 on
> an M5 Max, against well under 1 ms for `temporal` alone. **Prefer `temporal`
> unless you are opting into dual presentation**, which gives the same picture
> for a fraction of the GPU time.
>
> The `present` module implements the presentation half. What it does and does
> not establish is below.

### Dual presentation: presents accepted, display unverified

`present::MetalFxDualPresent` (opt-in, off by default) creates a `CAMetalLayer`
of its own above the one `wgpu` renders into and presents both frames from it —
interpolated first, real held back one refresh with
`presentDrawable:afterMinimumDuration:` so the two land on consecutive vsyncs
instead of collapsing onto one.

Three details are load-bearing, each of which failed silently before it was
right: the layer must be `framebufferOnly = false`, its `pixelFormat` must be a
BGRA one (CoreAnimation accepts an RGBA present and then skips it), and the
presents must be issued from the graph command buffer's *completion* handler on
a queue of our own — a drawable acquired mid-graph has been recycled by the time
that buffer commits.

**Measured**, both arms through the same layer and telemetry so only the present
count differs:

| | presents | callbacks | render fps |
|---|---|---|---|
| baseline (single present) | 403 | 403 | 26.9 |
| dual present | 802 | 801 | 26.7 |

That is 1.99× the accepted-present rate at an unchanged render rate.

**Not established: that any of it reaches the panel.**
`MTLDrawable.presentedTime` never populates on the development machine — not for
this crate, and not for a minimal, maximally visible Metal window either. So
presented frame rate is unmeasurable there by any implementation, and the
accepted-present rate above is a proxy for it, not a substitute. `PresentSink`
already records presented rate, judder, ordering inversions and drops, so
validating this needs only hardware where that signal works.

## API Reference

### Core Types

| Type | Description |
|------|-------------|
| `MetalFxPlugin` | Bevy plugin — configure `render_scale` (0.1–1.0) and `mode` |
| `MetalFxMode` | Enum: `Spatial`, `Temporal`, `FrameInterpolation`, `Disabled` |
| `MetalFxRenderScale` | Main-world resource holding the render scale factor |
| `MetalFxConfig` | Render-world resource (auto-inserted) |
| `MetalFxUpscaleNode` | Render graph `ViewNode` (auto-inserted) |
| `MetalFxLabel` | Render graph label for ordering |

### Runtime Queries

```rust
// Check MetalFX availability at runtime
if bevy_metalfx::is_available() {
    // MetalFX is available on this system
}

// Probe whether a spatial scaler can be created (for integration tests)
let ok = bevy_metalfx::probe_spatial_scaler(&render_device);
```

### GPU Timing (diagnostics)

The crate exposes an optional GPU-timing surface for measuring the per-command-buffer
GPU-elapsed time of the MetalFX pass — useful for profiling whether a scene is
GPU- or present-bound. Construct a `GpuTimingSink`, pass a clone into
`MetalFxPlugin { gpu_timing_sink: Some(sink.clone()), .. }` (macOS only), and read
`GpuTimingStats` from your own clone. This is a diagnostic/bench facility, not part
of the upscaling pipeline — most apps leave `gpu_timing_sink` at its `None` default.

## Bevy Compatibility

| bevy_metalfx | Bevy |
|-------------|------|
| 0.2 | 0.18 |
| 0.1 | 0.18 |

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

## Platform Support

- **macOS 13+** (Apple Silicon): Full support
- **macOS < 13**: Plugin disables itself gracefully
- **Linux / Windows**: Plugin disables itself; type stubs available for cross-platform code

## Documentation

- [API docs on docs.rs](https://docs.rs/bevy_metalfx)
- [Apple MetalFX documentation](https://developer.apple.com/documentation/metalfx)
- [objc2-metal-fx bindings](https://docs.rs/objc2-metal-fx)
- [Bevy render graph guide](https://bevyengine.org/learn/quick-start/getting-started/render-graph/)

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
