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
- **`node.rs`** — Bevy render graph `ViewNode` implementation (spatial, temporal, frame interpolation)
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

# Frame interpolation (requires macOS 26+) — EXPERIMENTAL, see note below
bevy_metalfx = { version = "0.2", features = ["frame-interpolation"] }
```

> **Frame interpolation is experimental — it computes a frame it cannot show.**
> The MetalFX side is now correct and passes the Metal debug layer: the mode runs
> a temporal upscale, then feeds two consecutive *upscaled* frames plus the
> content-sized depth/motion pair to an `MTLFXFrameInterpolator`, with real
> camera parameters from Bevy's `Projection` and a real frame delta.
>
> What it does **not** do is present the synthesized frame. Interpolation only
> buys frame rate if you display the interpolated frame *and* the real one, paced
> to the display's refresh — two presents per simulated frame. A Bevy render
> graph presents its swapchain once per `App::update()`, so this node presents
> the genuine upscaled frame and leaves the interpolated one in an offscreen
> texture. Net effect today: identical visuals to `temporal`, plus the GPU cost
> of the interpolation pass (measured at roughly 5–7 ms/frame at 3024×1800 on an
> M5 Max, versus well under 1 ms for `temporal` alone).
>
> Realizing the benefit requires display-timed dual presentation below the Bevy
> render graph — a renderer-architecture change, not a node change. Until that
> lands, use `spatial` or `temporal` for shipping work and treat
> `frame-interpolation` as a correct-but-unrealized building block.

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
| 0.1 | 0.18 |

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

MIT OR Apache-2.0
