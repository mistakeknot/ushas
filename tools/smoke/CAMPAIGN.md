# Fixed-scale Claude campaign

The campaign compares five fixed arms at 1280×720: native Disabled with MSAA4,
native Temporal, half-resolution Temporal, one-third Temporal, and half-resolution
bilinear. Claude is the subject in every arm. The fragment workload takes values
0, 8,000 and 20,000; each arm/load combination runs four times, with four seconds
of warmup and six seconds of measurement after actual render readiness.

```sh
python3 tools/smoke/campaign.py --dry-run --binary /absolute/path/to/frozen/ushas-smoke
python3 tools/smoke/campaign.py --run-dir /absolute/path/to/new/evidence \
  --binary /absolute/path/to/frozen/ushas-smoke
python3 tools/smoke/campaign.py --analyze-existing /absolute/path/to/evidence
```

Build from a clean immutable revision and copy the resulting executable to a
separate path before running. The campaign records its own script, wrapper and
binary hashes, refuses reused evidence paths, launches only one arm at a time,
and preserves failed attempts. Do not replace those files while it runs. Avoid
other GPU work and heavy builds during measurement. The offscreen fixture needs
no display, but still uses the actual Apple GPU.

Two forward/reverse pairs balance both arm and load order; each arm/load has
the same mean global position. The analysis gives each of four paired run means
equal weight. It uses a seeded, 10,000-draw bootstrap with pointwise 95% intervals
and a predeclared 8% practical threshold. Missing or invalid pairs produce no
complete comparison. Four repetitions leave substantial uncertainty and are
not a substitute for checking drift, interference, and the raw runs.

The measured quantity is **CPU application-loop cadence**. This unpaced fixture
can queue GPU work; its loop rate is neither GPU completion rate nor presented
FPS, and its budget misses are CPU-loop misses. Captures and distinct effect
observations prove that rendering occurred, not that every loop completed a
frame within the budget. Experimental markers remain off by default and never
become validated governor input. See
[the timing investigation](../../docs/research/marker-scope-01.md).

Use `--completion` for a separate campaign that measures **serial completed-render
cadence**. It disables render pipelining and waits for a queue-completion fence
after each full Bevy render frame, including its final screenshot/readback
submission. It permits one render frame in flight and rejects experimental
timestamp instrumentation. Keep the same mode for every arm and repetition:

```sh
python3 tools/smoke/campaign.py --completion --dry-run \
  --binary /absolute/path/to/frozen/ushas-smoke
python3 tools/smoke/campaign.py --completion \
  --run-dir /absolute/path/to/new/completion-evidence \
  --binary /absolute/path/to/frozen/ushas-smoke
python3 tools/smoke/campaign.py --analyze-existing /absolute/path/to/completion-evidence
```

Completion analysis requires a closed measurement epoch with at least 20
qualified frame fences, no unfinished frame or recorded error, and a later
drained epoch. It checks frame and view identities, image-target and effect
agreement, scale and dimensions, non-overlapping intervals, counters, and the
reported rate against retained frame records. Failed checks withhold the affected
paired comparison. A fence can cover several queue submissions; fences are counted
as render frames, not submissions.

Each completion run contributes `1000 / completed_render_fps` milliseconds per
completed render to its paired ratio. The elapsed interval runs from the first
measured admission to the last measured completion callback, including gaps
between frames. CPU-loop fields remain separately available and do not supply this
ratio. The same four repetitions, balanced order, bootstrap and practical threshold
apply. Existing CPU-cadence campaigns retain their original analysis; reanalysis
reads the recorded mode and rejects mode override flags.

Completion cadence includes CPU preparation, scheduling, callback delivery and
polling. Serial execution intentionally removes rendering overlap, so its rate is
not normal pipelined application FPS, GPU busy time, hardware frame latency, or
panel delivery. Neither campaign mode supplies validated adaptive-governor input.

Review images separately for acceptable quality. Half and one-third resolution
are different quality choices, and native Temporal is not the same AA baseline
as MSAA4. These results alone cannot select an automatic policy or establish a
consumer, latency, power, or panel-delivery benefit.

The optional `--middle-presets` refinement is restricted to completion mode and
load 8,000. It uses 20 runs: four repetitions of native Disabled/MSAA4, Temporal
at 2/3 and 0.58, and bilinear at each of those same input scales. The same frozen
binary, 1280×720 output, four-second warmup, six-second measurement and two
forward/reverse order pairs apply. It is intended for the boundary where native
misses the chosen budget and half resolution meets it; it does not expand every
load or resolution.

```sh
python3 tools/smoke/campaign.py --middle-presets --completion --dry-run \
  --binary /absolute/path/to/frozen/ushas-smoke
python3 tools/smoke/campaign.py --middle-presets --completion \
  --run-dir /absolute/path/to/new/middle-preset-evidence \
  --binary /absolute/path/to/frozen/ushas-smoke
python3 tools/smoke/campaign.py --analyze-existing /absolute/path/to/middle-preset-evidence
python3 tools/smoke/completion_budget.py /absolute/path/to/middle-preset-evidence
```

The retained plan records the complete arm set and a 60 FPS mean-throughput
criterion: the upper bound of the exploratory, pointwise 95% bootstrap interval
for four equally weighted run-mean completed-render times must be at most
16.667 ms. A lower bound above the budget is a miss; a crossing interval remains
uncertain. Missing or invalid runs withhold the classification. This mean gate
is separate from the 8% practical threshold for paired comparisons. Native
comparisons measure overall cost; the same-resolution bilinear controls measure
the incremental cost of the whole Temporal render path, including its prepasses
and resolves, and its image-quality tradeoff. They do not isolate the MetalFX
kernel cost. Temporal need not be faster than
bilinear to improve its image quality within budget.

`completion_budget.py` separately reports per-run P95/P99 and the fraction of
completed-frame intervals above the budget, including interframe CPU gaps.
Passing the mean gate does not imply smooth 60 FPS pacing. Four repetitions give
limited uncertainty resolution; images still need matched motion, disocclusion,
thin-geometry, cut and UI checks before selecting acceptable quality.

Choose 2/3 over 0.58 only if it meets both the declared mean budget and that image
quality gate; otherwise consider 0.58, then the previously tested half-resolution
rung. A half-resolution budget miss is infeasible at the default 0.5 floor.
One-third remains an explicit lower-quality choice. These conclusions describe
serial completed-render throughput and do not establish normal pipelined FPS,
GPU busy time, a working autonomous governor, or presentation cadence.
