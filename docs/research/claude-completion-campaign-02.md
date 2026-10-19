# Balanced Claude completed-render campaign, Bevy 0.20

**All 60 runs passed** with no recorded errors. This repeats
[campaign 01](claude-completion-campaign-01.md) on the Bevy 0.20 / wgpu 30 code
with the same protocol, so the 0.5.0 README cites a measurement of the code it
ships. It establishes serial completed-render cadence for the frozen offscreen
fixture. It does not establish GPU busy cost, production pipelined FPS, input
latency or presentation.

## Frozen experiment

The campaign ran October 19, 2026, 02:48–02:59 UTC on Apple M5 Max, macOS 26.5.2,
Rust 1.97.1. The source was the `release/0.5.0` branch at
`2213bea858d006ae55198c5ca673725a4bcfba8a` merged with pull request #2 at
`258a9715700a08767fba0a8c82a5bac54d3939ff`, which ports the fixture's load
shader to WESL. Without that port the load plane does not compile under Bevy 0.20.
The merge was a local, clean build (recorded revision `9ab0ac1`, not published).
Its `src/` tree is `f1809f781b39595348e39676a92ea6854902984b` and its
`tools/smoke/` tree is `f7a2d554ad8de0eb29da238a42c5affe4f0f03b6`. The executable
SHA-256 was `2964a81a4fdc80f61f0453888814b4601900751fc0815ff88c544893a6b45c4e`.

The scene was static `claude-toy-v1`, LDR, 1280×720, with four seconds of warmup
and six seconds of measurement, built with the `frame-interpolation` feature
set. Each admitted render frame completed before the next admission. No other
GPU job or Cargo build ran during the campaign. This does not establish an
interference-free or thermally identical machine.

The [predeclared protocol](../../tools/smoke/CAMPAIGN.md) is unchanged: five arms,
three fragment loads and four repetitions, in balanced forward/reverse arm and
load orders, with a seeded 10,000-draw bootstrap and an 8% practical threshold.
The [paired analysis](claude-completion-campaign-02.json) holds every run. Its
evidence paths are written relative to the evidence directory.

## Results

Mean completed-render time per frame, milliseconds (four runs each):

| Arm | Load 0 | Load 8,000 | Load 20,000 |
|---|---:|---:|---:|
| Native Disabled, MSAA4 | 2.818 | 45.863 | 116.363 |
| Native Temporal | 4.055 | 48.333 | 116.748 |
| Half Temporal | 4.101 | 18.640 | 39.216 |
| Third Temporal | 4.023 | 10.522 | 20.981 |
| Half bilinear | 2.881 | 16.902 | 35.725 |

Paired ratios against native Disabled/MSAA4 (lower is faster), with pointwise
95% bootstrap intervals:

| Candidate | Load 0 | Load 8,000 | Load 20,000 |
|---|---|---|---|
| Native Temporal | 1.444 [1.323, 1.549], regression | 1.057 [1.004, 1.109], below threshold | 1.025 [0.892, 1.130], below threshold |
| Half Temporal | 1.458 [1.384, 1.533], regression | **0.408 [0.385, 0.431]**, improvement | 0.345 [0.295, 0.384], improvement |
| Third Temporal | 1.435 [1.315, 1.588], regression | 0.230 [0.217, 0.241], improvement | 0.184 [0.158, 0.201], improvement |
| Half bilinear | 1.024 [0.987, 1.067], below threshold | 0.368 [0.350, 0.398], improvement | 0.313 [0.272, 0.341], improvement |

## Reading the results

- **Light scene.** At load 0 every Temporal arm is a clear regression, about
  1.44× native, and half bilinear is no faster than native. When the scene is
  already cheap, MetalFX's fixed cost outweighs the pixels it saves.
- **Fragment-bound scene.** At load 8,000, half Temporal takes 0.41× the native
  time. Plain half bilinear is faster still (0.37×). The 1.7 ms difference is
  the cost of Temporal reconstruction over a bilinear stretch; what it buys is
  image quality, which these timings do not measure.
- **Compared with campaign 01.** The direction of every decision is the same as
  under Bevy 0.19, but the absolute times differ. Native at load 8,000 rose from
  30.5 ms to 45.9 ms, and native at load 0 fell from 6.5 ms to 2.8 ms. The load
  plane now runs a WESL shader instead of the naga_oil one, and the engine
  version changed, so the two campaigns' absolute times are not comparable. Only
  the paired ratios within each campaign are.

## Hero image

`docs/images/hero-three-panel.png` shows the neutral `shapes` scene (cubes and
thin rails, no character), rendered offscreen at 1920×1080 output with
`--scale 0.5`. The three panels, left to right, are bilinear (`--mode disabled`),
MetalFX Spatial and MetalFX Temporal. All three use the same static camera, the
same binary and a two-second warmup. Each panel is the same unscaled 640×360 crop
of its full output. All three runs reported `valid: true` and `pipeline_errors: 0`.
Apple M5 Max only.

## Limits

These are four repetitions per arm on one machine, with serial offscreen timing.
They do not measure image quality or pipelined throughput. The Ushas Bench
comparison measures pipelined throughput separately, under different scenes.
