# bevy_metalfx

[![Crates.io](https://img.shields.io/crates/v/bevy_metalfx.svg)](https://crates.io/crates/bevy_metalfx)
[![Docs.rs](https://docs.rs/bevy_metalfx/badge.svg)](https://docs.rs/bevy_metalfx)
[![License](https://img.shields.io/crates/l/bevy_metalfx.svg)](https://github.com/mistakeknot/ushas#license)

Bevy plugin for Apple MetalFX upscaling and frame interpolation.

Renders your scene at a lower resolution and uses MetalFX's ML-based upscaling
to reconstruct a full-resolution image, improving performance on Apple Silicon Macs.

> This repository is `ushas`; the crate it publishes is `bevy_metalfx`. A
> crates.io name is permanent and the `bevy_` prefix is how the ecosystem finds
> plugins, so the crate keeps the plain name. Versions through 0.4.1 were
> published with a `repository` pointing at `github.com/mistakeknot/bevy_metalfx`;
> that URL redirects here, and 0.4.2 is the first release to name this repo
> directly.

## Features

| Mode | Description | macOS Version | Cargo Feature |
|------|-------------|---------------|---------------|
| **Spatial** | Single-frame ML upscaling | 13+ | `spatial` (default) |
| **Temporal** | Multi-frame upscaling with motion vectors | 13+ | `temporal` |
| **Frame Interpolation** | Generate intermediate frames | 26+ (Metal 4) | `frame-interpolation` |

## Quick Start

```toml
[dependencies]
bevy_metalfx = "0.4"
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
bevy_metalfx = "0.4"

# Temporal upscaling (adds motion vector + depth prepass) — stable
bevy_metalfx = { version = "0.4", features = ["temporal"] }

# Frame interpolation (requires macOS 26+) — see the limitation below
bevy_metalfx = { version = "0.4", features = ["frame-interpolation"] }
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
| `MetalFxScaleRange` | Resource: the render-scale band MetalFX will accept |
| `MetalFxHistoryReset` | Resource: request a temporal-history reset |
| `MetalFxUpscaleNode` | State for the upscale pass (auto-registered) |
| `MetalFxLabel` | `SystemSet` containing the pass, for ordering |

### Render scale bounds

`MetalFxScaleRange` answers "what scale am I allowed to set?" before you set it.

```rust
fn set_scale(range: Res<MetalFxScaleRange>, mut scale: ResMut<MetalFxRenderScale>) {
    let wanted = 0.6;
    if range.contains(wanted) {
        scale.0 = wanted;
    }
}
```

This exists because the failure it prevents is silent. MetalFX is configured in
*upscale ratios* (`output / input`, always `>= 1.0`), while a render scale is a
fraction of the output — the two are reciprocals, so converting swaps the ends.
Hand MetalFX a fraction where it wants a ratio and
`newTemporalScalerWithDevice` returns `nil` and says nothing else.
`as_upscale_ratios()` shows the converted values if you need them.

With adaptive scaling on, the band is the governor's step range and the scaler
flexes inside it with no rebuild. With it off, the band is the single configured
scale — any other value works, but forces the scaler to be rebuilt.

### The device's own floor, and the presets

`MetalFxScaleRange` is the band the plugin was *configured* with.
`MetalFxDeviceScaleBand` is the band it *could* be configured with — the floor
the hardware reports, which is not the same on every chip:

| Device | Max temporal upscale ratio | Lowest render scale |
|---|---|---|
| Apple Silicon through M4 | 2.0 | 0.5 |
| M5 family | 3.0 | 0.333 |

At one third the rasterizer touches nine times fewer pixels than at native,
against four at one half. The band is read from
`MTLFXTemporalScalerDescriptor` in the plugin's `finish` (there is no render
device in `build`), so from the first frame on it is a device fact; until then
it is the assumed pre-M5 band, and `is_from_device()` tells you which.

```rust
fn settings_menu(band: Res<MetalFxDeviceScaleBand>) {
    for preset in MetalFxQuality::ALL {
        if preset.is_available_on(&band) {
            println!("{preset:?} -> render scale {:.3}", preset.render_scale());
        }
    }
}
```

`MetalFxQuality` is the DLSS ladder as render scales — `UltraPerformance`
(1/3), `Performance` (1/2), `Balanced` (0.58), `Quality` (2/3), `Native`
(1.0, at which the temporal scaler is temporal anti-aliasing). A preset is a
render scale and nothing more. `MetalFxQuality::ladder(&band)` returns the
rungs the device admits, ascending, and that ladder is what `adaptive: true`
climbs: on an M5 it reaches one third, on earlier chips it stops at one half.
Spatial mode has no device floor — the spatial scaler accepts any ratio — so
its band is the plugin's own `0.1..=1.0`, labelled as not a device fact.

### Temporal history reset

Temporal upscaling accumulates across frames, which is wrong across a
discontinuity. On a camera cut, teleport or scene load, ask for a reset:

```rust
fn on_teleport(mut reset: ResMut<MetalFxHistoryReset>) {
    reset.request();
}
```

It applies to the next rendered frame and clears itself. Do not hold it set —
that suppresses temporal accumulation entirely, which is the thing you are
paying for. Ignored in `Spatial` mode, which keeps no history.

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

| bevy_metalfx | Bevy | MSRV |
|-------------|------|------|
| 0.4.1 | 0.19 | 1.95 |
| ~~0.4.0~~ | 0.19 | 1.95 |
| 0.3 | 0.18 | 1.82 |
| 0.2 | 0.18 | 1.82 |
| 0.1 | 0.18 | 1.82 |

**0.4.0 is yanked — do not use it.** It panicked on the first MetalFX encode, in every mode —
wgpu 29 refuses to let one command encoder carry both wgpu calls and raw
`as_hal` encoding, and this pass does both. It was compile-verified and
unit-tested but had never rendered a frame; 0.4.1 is the fix. See the CHANGELOG.

### Hardware verification of the Bevy 0.19 port

Verified on an M5 Max under `MTL_DEBUG_LAYER=1`, which turns silent MetalFX
misuse into an immediate assertion rather than garbage pixels. Harness: a private consumer application, not this repository.

- All four modes — spatial, temporal, frame interpolation, disabled — run with
  no panic and no Metal validation assertion.
- The temporal prepass is wired: `Depth32Float` depth and `Rg16Float` motion,
  resolved from full physical resolution to content size.
- `MetalFxScaleRange` reports the band the scaler was actually created with
  (render `0.5..=0.5` → upscale ratios `2.0..=2.0`), configured scale in band.
- **The pass wins the write to `out_texture`.** This is the property the
  `.after(upscaling)` ordering exists to guarantee and the one that fails
  silently: Bevy's own upscaling blits into the same texture, so losing the
  order would substitute bilinear with no error anywhere. At an identical render
  scale, MetalFX output differs from Bevy's bilinear by mean-abs 44.26 across
  74.65% of pixels — while the same configuration run twice is byte-identical,
  so the difference is the upscaler and not run-to-run noise.
- **`MetalFxHistoryReset` reaches the scaler and changes the output**: 2.74% of
  pixels on the cut frame at a 0.25 rad jump, 11.31% with the camera held still,
  and it leaves measurably less of the pre-cut view behind (25.280 vs 25.224).

One documented non-effect, because it looks like a bug and is not: across a
*half-turn* teleport the reset changes nothing at all, byte for byte. A total
disocclusion leaves no history MetalFX considers reusable, so it discards the
lot on its own and there is nothing left for the flag to drop. The reset earns
its keep on partial discontinuities — which is also the only case where stale
history could have smeared in the first place.

Still unverified, and unrelated to the port: whether the interpolated frame
under `dual_present` reaches the panel. `MTLDrawable.presentedTime` does not
populate on this machine for any program.

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
