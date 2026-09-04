# Ushas render smoke

A standalone Bevy 0.19 scene with a locked dependency graph, visible geometry,
a controllable fragment workload, moving edges, and an output-resolution UI.
Build and run from the Ushas repository. Rust 1.97.1 is selected by its tracked
toolchain file.

```sh
cargo build --release --locked --manifest-path tools/smoke/Cargo.toml
caffeinate -d tools/smoke/target/release/ushas-smoke \
  --mode temporal --scale 0.5 --width 1280 --height 720 \
  --warmup 4 --seconds 6 --out /tmp/ushas-temporal.json
```

The run captures a PNG beside its JSON and exits. It waits for 20 ready frames
and the requested warmup before measuring. Active modes require fresh
`OutputWritten` observations throughout measurement. The capture must contain
varied scene pixels beyond the UI header; a flat colored frame fails. Missing
rendering, timeout, capture failure, or report-write failure returns nonzero.
A PNG proves captured content, not panel delivery.

Use an unlocked, awake display and keep other GPU workloads fixed across arms.
`caffeinate -d` prevents idle display sleep; it does not unlock an existing
locked session. Preserve failed reports and logs. Record the exact source
revision; `source_dirty_at_build` marks exploratory builds.

Useful controls:

```sh
# Native scene (no native AA) and matching bilinear half-resolution control.
caffeinate -d tools/smoke/target/release/ushas-smoke --mode disabled --scale 1 --out /tmp/native.json
caffeinate -d tools/smoke/target/release/ushas-smoke --mode disabled --scale 0.5 --out /tmp/bilinear.json
# GPU workload, CPU-only delay, and moving edges are independent knobs.
caffeinate -d tools/smoke/target/release/ushas-smoke --mode temporal --scale 0.5 --pixel-iterations 1000 --out /tmp/gpu.json
caffeinate -d tools/smoke/target/release/ushas-smoke --mode temporal --scale 0.5 --cpu-ms 20 --out /tmp/cpu.json
caffeinate -d tools/smoke/target/release/ushas-smoke --mode temporal --scale 0.5 --moving --out /tmp/motion.json
```

`--target-fps` defines the analysis/controller budget, not a frame cap. The
window requests `AutoNoVsync`; its frame loop can still be presentation limited.
`frame_loop` reports CPU loop intervals, never GPU busy time or presented FPS.
The dedicated MetalFX command-buffer diagnostic includes dependency waits and
is neither total frame cost nor isolated upscale execution.

`--adaptive --target-fps 60 --minimum-scale 0.5` exercises the adapter. With no
validated input, it holds quality and reports an unavailable signal. The
`--experimental-timing` flag adds dependency-carrying marker passes and deferred
asynchronous timestamp resolution. That signal remains unvalidated and is
never fed into the governor. Compare matching runs with and without markers;
then corroborate interval coverage in Metal System Trace.

For interpolation, `--presentation single` and `--presentation dual` use the
same owned presentation path. `--refresh-hz 120` declares the scheduling
assumption; it does not measure the monitor. Positive drawable timestamps,
render cadence, output pixels, and physical panel delivery are separate
observations. Compare temporal-only against dual interpolation for product
value, and report latency/ordering separately.

Before an efficacy claim, select a practical threshold (the initial roadmap
uses at least 8% measured GPU-cost benefit for a downshift), balance arm order,
repeat runs, and report confidence intervals. A statistically inconclusive or
presentation-limited comparison is not a speedup. The thin geometry and motion
scene still requires human image review; passing its smoke checks is not a
quality verdict.
