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

The Python runners and offline tests use Python 3.14 in CI. Install their pinned
image-validation dependency in a virtual environment before using them:

```sh
python3.14 -m venv /tmp/ushas-smoke-venv
. /tmp/ushas-smoke-venv/bin/activate
python -m pip install --requirement tools/smoke/requirements.txt
```

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

`--target-fps` explicitly defines the analysis/controller budget, not a frame cap.
Without it, the adaptive window fixture uses the primary window's reported monitor
refresh, or a labelled 60 FPS fallback when unavailable. Fixed-scale runs keep a
60 FPS analysis budget unless explicitly overridden.
Reports retain both the requested and resolved target and the resolution source.
Reported refresh does not measure VRR or presentation, and Bevy's cached monitor
metadata may lag an in-place display-mode change. The
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

For a bounded diagnostic across temporal-only, single interpolation and dual
interpolation, freeze a clean release binary and run:

```sh
python3 tools/smoke/presentation_probe.py --binary /absolute/path/ushas-smoke \
  --source-revision FULL_COMMIT_SHA --out /private/tmp/new-presentation-probe
```

The probe retains artifacts and continuously samples the main display and
session. Locked, asleep or unknown preflight state records
`environment_unavailable` and exits 3 without launching the renderer. Run it
with access to macOS display services; sandbox-denied state is unknown. Its
aggregate timestamps cannot establish frame identity, ordering, latency or
net benefit, even when all three arms execute successfully.

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
`inactive-cut-resume`, `creation-failure`, and `creation-slow`. The two creation
exercises require Temporal mode. They use the smoke harness's explicitly enabled
`diagnostic-fault-injection` feature to return no scaler or hold an attempt
pending for at least ten seconds. They require opaque fallback captures, an
unconsumed history reset, and real Temporal output after releasing the fault.
These are simulated creation outcomes; they do not reproduce a driver crash.
The library feature is off by default.

Lifecycle reports retain the most recent 1024 observations separately for each
phase. `observation_retention` reports total, retained and evicted counts and
frame bounds; `dropped_observations` totals those evictions. This keeps a long
pending phase from displacing recovery evidence. The retained windows and
separate transition events do not constitute a complete per-frame history.

The two creation exercises also support `--offscreen`. They resolve geometry
from the actual image asset, require the camera to keep rendering that same
target, and retain initial, fallback and recovered captures. They reject
`--completion` and quality-sequence modes, which have their own capture protocols.

```sh
python3 tools/smoke/run.py --timeout 60 -- --offscreen --mode temporal \
  --scale 0.5 --lifecycle creation-slow --out /tmp/ushas-creation-slow-001.json
```

`window-minimize` requests minimize/restore of its own test window and requires
both actual `WindowOccluded` events and native minimized-state transitions.
`os-sleep-resume` observes externally initiated NSWorkspace system sleep/wake
notifications; it makes no power or lock request. Both require Temporal mode,
initial and restored opaque captures, and fresh output acknowledging the recovery
reset. Their deadline is 60 seconds of sleep-inclusive wall time; use the runner's
90-second timeout. A sleep test must be coordinated with the operator after the
initial capture. Camera inactivity, screen sleep, and a time gap cannot satisfy it.

Camera inactivity is a render-pause test, not an
operating-system sleep/resume test. `--hdr` enables Bevy's HDR main texture;
`--native-aa` selects the native-scale Disabled MSAA4 image-quality control.

`--offscreen` renders fixed-scale Disabled, Spatial, or Temporal into an
RGBA8 sRGB image with an unpaced application loop. It creates no native window
or drawable, and captures that same image target, including native-resolution
UI. This isolates image rendering from display availability and drawable waits;
it does not measure presentation. Compare offscreen arms only with other
offscreen arms because the target format and scheduling differ from the window
fixture. Adaptive control, interpolation and lifecycle exercises other than
the two creation faults require the window fixture and are rejected with
`--offscreen`. Image-target recovery does not establish native window or OS
sleep recovery.

```sh
python3 tools/smoke/run.py -- --offscreen --mode temporal --scale 0.5 \
  --experimental-timing --out /tmp/ushas-offscreen-001.json
```

For completed-render throughput, use the optional serial mode:

```sh
python3 tools/smoke/run.py -- --offscreen --completion \
  --mode temporal --scale 0.5 --pixel-iterations 8000 \
  --warmup 4 --seconds 6 --out /tmp/ushas-completed-001.json
```

`--completion` disables pipelined rendering and waits for the full render
submission, including final screenshot/readback work, before another frame.
Each wait is bounded at five seconds; missing completion or a wait error fails
the process and remains in the wrapper's evidence. The measured epoch requires
matching frame/view/image/effect identities and a drained closing boundary.
It is incompatible with experimental timestamps, interpolation, and window mode.

`serial_completion` retains frame fences, epoch boundaries and actual elapsed
time. Its rate is **serial completed-render cadence**, including CPU scheduling,
render preparation and callback polling. It is neither normal pipelined app FPS
nor GPU busy time, hardware latency or presentation. No completion measurement
is sent to the adaptive controller. See [CAMPAIGN.md](CAMPAIGN.md) for balanced
comparisons. The original unpaced mode remains useful for CPU-cadence diagnostics;
its [heavy-load shutdown failures](../../docs/research/claude-campaign-01.md)
show why a valid capture alone is insufficient.

For matched quality samples, use the separate quality runner (Python with
Pillow). It requires a new output path and retains a report, twelve RGBA PNGs,
log, binary hashes and a validation manifest:

```sh
python3 tools/smoke/quality_runner.py --binary /absolute/path/ushas-smoke \
  --out /private/tmp/quality-native.json --mode disabled --scale 1 --native-aa
python3 tools/smoke/quality_runner.py --binary /absolute/path/ushas-smoke \
  --out /private/tmp/quality-temporal-half.json --mode temporal --scale 0.5
```

Repeat the Temporal arm with `--scale 0.58`, `--scale 0.66666667`,
`--scale 0.75`, or `--scale 0.33333334`; `--hdr` runs the same sequence with an
HDR main texture and tone-mapped PNG output. Reduced Disabled is the bilinear
control. Native uses MSAA4; reconstruction arms use MSAA off. Keep dimensions,
source/binary and HDR setting identical within a comparison.

`--quality-sequence` owns the simulation clock, camera and screenshot schedule.
After at least three seconds and twenty distinct ready frames, it renders 145
serial frames at fixed 1/60 simulation steps. Ticks 0–31 hold the initial pose;
32–127 animate the Claude models and pan the camera; tick 128 makes a hard cut
and requests a temporal history reset; 128–144 hold the final pose. Captures are
`settled` (31), `motion32` (63), `motion62/63/64` (93/94/95), `before-cut` (127),
and `cut0/1/2/4/8/16` (128/129/130/132/136/144). Jitter has a fixed logical phase
independent of warmup. The consecutive moving samples and post-cut recovery
samples support matched inspection of rays, facial lines, rails, disocclusion,
and the native-resolution UI. They are twelve sampled frames, not continuous
video or a proof of quality at a real-time presentation cadence.

Add `--moving-reset` to the quality runner for the distinct
`claude-60hz-moving-cut-v2` protocol. It passes `--quality-moving-reset` to the
renderer, preserves the first 128 poses, and continues both model animation and
camera motion after the cut. It captures all seventeen `moving-cut0` through
`moving-cut16` states plus the six earlier checkpoints: 23 PNGs and the same 145
render proofs. Every filename starts with `moving-`. Compare native MSAA4,
native Temporal, and half-scale Temporal under this same protocol; do not mix
it with the held-camera captures. This short sequence still does not establish
real-time presentation quality.

Every scripted frame must have current effect status and the expected camera,
MSAA, dimensions, HDR format and jitter. Each screenshot entity is frozen at
extraction, joined to its render-frame proof and later `ScreenshotCaptured`
readback, and paired with the same frame's final queue-completion fence. Request
frame, render frame and readback arrival are separate fields. Reset acknowledgement
proves CPU reset-command encoding; the image must still be inspected for visual
recovery. The runner independently decodes every expected PNG, requires opaque
scene pixels and complete identity/hash evidence, and rejects partial sequences.

This mode has a 75-second internal deadline, five-second bounded queue drains,
and a 90-second runner deadline. Normal completion uses `AppExit`; timeout or
termination cleans up the child process group and retains invalid evidence.
It rejects `--moving`, explicit `--completion`, custom screenshots, artificial
load, adaptive mode, lifecycle mode and interpolation. Ordinary smoke runs keep
their existing behavior; `run.py` deliberately cannot validate this distinct
report. No quality result measures GPU cost, normal application FPS, or panel
presentation.
