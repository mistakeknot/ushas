# Ushas Bench

A local Claude render lab for Apple Silicon Macs running macOS 26. The preview
app contains a SwiftUI launcher and a bundled Bevy renderer. No account, network
service, Rust installation, or repository assets are needed to use the package.

Benchmark runs three deterministic chapters at 2560 × 1440 physical pixels:
Materials, Geometry, and Lighting. Each chapter renders 1,200 authored ticks,
with a nominal scene time of tick / 120 seconds. The target is 120 completed
renders per second. The Claude model is an independent procedural interpretation
of vgel / thebes' character; see [attribution](../claude-model/CHARACTER.md).

## Use the app

**Background run** is enabled by default. The full scene renders into a fixed GPU
image without a live preview, so Chrome or another window can stay in front.
Other apps can still affect the result through CPU/GPU activity. Turn Background
run off to watch the lab in its own window; covering or minimizing that render
window can invalidate a windowed measurement.

- **Benchmark:** choose Native MSAA4, Temporal, Spatial, or Bilinear and a legal
  render scale. Native always uses full resolution.
- **Compare:** run six fixed arms in fresh processes. Quick comparison uses one
  round; qualification uses four balanced rounds. A separate replay retains
  quality images for each arm.
- **Stress:** run for ten minutes by default, adjusting Claude count, lights,
  particles, and optional synthetic fill. Changes start a new reporting epoch.
  Stress runs are custom workloads and have no benchmark score.
- **Results:** inspect retained runs, compare original image pairs, and export a
  self-contained folder containing JSON, offline HTML, images, and child reports.

Stop, or Escape while Ushas is active, ends a run and preserves its artifacts. A stopped or failed
benchmark receives no score. Invalid comparison arms remain visible. Reduced
dimensions, short timelines, a selected chapter, or a different seed produce
explicitly custom results.

## What the number means

The metric is **completed-render throughput**: exact cohort frames divided by
the interval from first scene admission to the closing render-queue callback.
It includes CPU scheduling and callback dispatch. Background runs use an image
target and a continuous schedule runner, with no native presentation or image
readback in the measured interval. Windowed runs also include surface acquisition;
their closing callback does not certify the final native presentation buffer.
Neither mode measures GPU-only time, displayed FPS, or a 1% low.

Background results use `claude-lab-offscreen-v1`; windowed results retain
`claude-lab-standard-v1`. Comparisons require one execution target throughout.
The windowed mode requests Immediate presentation. Bevy does not expose the
resolved surface policy through its public interface; drawable acquisition and
compositor backpressure can still limit that path. A flat rate near display
cadence does not establish GPU capacity or prove a reconstruction algorithm has
no benefit under a different workload.

Normal Bevy render pipelining stays enabled. The measured benchmark uses
asynchronous opening and closing completion boundaries, with no per-frame GPU
waits or completion callbacks. Every expected tick needs matching frame, view,
configuration, target dimensions, and fresh output evidence. Capture readback
runs separately against an image target and joins the extracted screenshot
entity to its original render frame.

Four-round comparisons use paired completed-render-time ratios and a
deterministic paired bootstrap interval. A practical benefit requires the
interval's lower bound to exceed an 8% time reduction. Timing does not qualify
image quality: inspect the retained face, thin-edge, motion, and camera-cut pairs
before recommending a reconstruction arm. A no-benefit result is useful.

## Build and CLI

From the repository root, using the pinned Rust toolchain and Xcode 26:

```sh
cargo test --locked --manifest-path tools/benchmark/Cargo.toml
cargo test --locked --manifest-path tools/smoke/Cargo.toml -p ushas-claude-model
DEVELOPER_DIR=/Applications/Xcode.app/Contents/Developer swift test --package-path tools/benchmark/macos
cargo build --release --locked --manifest-path tools/benchmark/Cargo.toml
bash tools/benchmark/package.sh
```

The helper is also a CLI at `Ushas Bench.app/Contents/Helpers/ushas-bench`:

```sh
ushas-bench benchmark --background --out /tmp/ushas-native
ushas-bench benchmark --background --mode temporal --scale 2/3 --out /tmp/ushas-temporal
ushas-bench compare --background --rounds 4 --out /tmp/ushas-comparison
ushas-bench stress --background --duration 600 --claudes 64 --lights 8 --particles 4096 --out /tmp/ushas-stress
ushas-bench capture --mode native --out /tmp/ushas-quality
```

Output must be a new directory. stdout contains newline-delimited JSON events;
stderr contains renderer diagnostics. The final `complete` event identifies the
absolute `result.json` path. Reports retain build revision, dirty-build flag,
invoked helper hash, configuration, device details, cohort proofs, and failures.
Public OS thermal pressure is recorded where available; no GPU temperature or
utilization value is inferred from it.

The CLI keeps its original windowed default when `--background` is omitted.
Existing reports without the background field remain windowed reports. An
explicit windowed preset cannot be combined with `--background`.

The package is an ad-hoc-signed preview for local sharing. It is not a notarized
public release. [Implementation plan](../../docs/plans/2026-09-05-ushas-bench.md)
and [protocol contract](CONTRACT.md) describe the versioned profile and ownership.
