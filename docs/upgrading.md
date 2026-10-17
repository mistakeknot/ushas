# Upgrading

Per-version upgrade notes for `bevy_metalfx`, newest first. See the [changelog](https://github.com/mistakeknot/ushas/blob/main/CHANGELOG.md) for the full list of changes.

## Upgrading from 0.4

The 0.5 candidate requires Bevy 0.20 (wgpu 30) and Rust 1.97.1. `bevy` 0.20
and its WESL shader dependencies set that minimum; nothing in this crate's own code needs it.

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
