# Ushas render smoke

A standalone Bevy 0.19 scene with a locked dependency graph, visible geometry,
a controllable fragment workload, moving edges, and an output-resolution UI.
The default subject is a procedural 3D version of vgel/thebes' Claude character,
with an articulated head, limbs, and tail. See [CHARACTER.md](CHARACTER.md) for
the reference and attribution. `--subject shapes` restores the original cube
and thin-rail scene for historical comparisons; reports identify the subject
and geometry version, which must match across benchmark arms.
The [captured half-resolution Temporal preview](preview.png) has its
[render provenance](preview.json) alongside it.
Build and run from the Ushas repository. Rust 1.97.1 is selected by its tracked
toolchain file.

```sh
cargo build --release --locked --manifest-path tools/smoke/Cargo.toml
caffeinate -d tools/smoke/target/release/ushas-smoke \
  --mode temporal --scale 0.5 --width 1280 --height 720 \
  --warmup 4 --seconds 6 --out /tmp/ushas-temporal.json
```

The run captures warmup and final PNGs beside its JSON and exits. It waits for
20 distinct ready render observations, the requested warmup, and a valid initial
image before measuring. Active modes require fresh `OutputWritten` observations
with the requested mode, scale, and physical dimensions throughout measurement.
Both captures must contain opaque, varied scene pixels beyond the UI header;
a flat colored or zero-alpha frame fails. Missing
rendering, timeout, capture failure, or report-write failure returns nonzero.
A PNG proves captured content, not panel delivery.

Use an unlocked, awake display and keep other GPU workloads fixed across arms.
`caffeinate -d` prevents idle display sleep; it does not unlock an existing
locked session. Preserve failed reports and logs. The fixture stays above other windows during its bounded run to avoid macOS
occlusion skipping the render path. The JSON retains compiler, OS, binary hash,
source revision, and distinct rendered-frame IDs; `source_dirty_at_build` marks
exploratory builds. CPU loop samples remain separate from render observations.

For retained campaigns, use the wrapper below. It refuses existing evidence
paths, bounds the entire process (including GPU hangs), prevents idle display
sleep, and saves exit status, thermal state, binary/lock hashes, logs, and capture
hashes even on failure:

```sh
python3 tools/smoke/run.py --timeout 90 -- \
  --mode temporal --scale 0.5 --out /tmp/ushas-run-001.json
```

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

Lifecycle checks are separate from performance measurement. Each waits for actual
render observations and captures each supported state before advancing; failure
or a 25-second lifecycle timeout invalidates the run. Successful exercises restore
the original dimensions and one camera before the normal measurement begins.

```sh
python3 tools/smoke/run.py -- --mode temporal --scale 0.5 --adaptive \
  --lifecycle inactive-cut-resume --out /tmp/ushas-resume-001.json
```

Available exercises are `resize`, `camera-cut`, `late-camera`, `multiple-views`,
and `inactive-cut-resume`. Camera inactivity is a render-pause test, not an
operating-system sleep/resume test. `--hdr` enables Bevy's HDR main texture;
`--native-aa` selects the native-scale Disabled MSAA4 image-quality control.

`--offscreen` renders fixed-scale Disabled, Spatial, or Temporal into an
RGBA8 sRGB image with an unpaced application loop. It creates no native window
or drawable, and captures that same image target, including native-resolution
UI. This isolates image rendering from display availability and drawable waits;
it does not measure presentation. Compare offscreen arms only with other
offscreen arms because the target format and scheduling differ from the window
fixture. Adaptive control, lifecycle exercises, and interpolation require the
window fixture and are rejected with `--offscreen`.

```sh
python3 tools/smoke/run.py -- --offscreen --mode temporal --scale 0.5 \
  --experimental-timing --out /tmp/ushas-offscreen-001.json
```
