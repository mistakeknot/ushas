# Actual-pass Metal counter probe

This isolated Objective-C program tests native Metal counter feasibility. It
does not use Bevy, wgpu or MetalFX, publish a governor input, or certify GPU busy
time. See [the design and remaining gates](../../docs/research/gpu-producer-02.md).
Objective-C exposes Apple's four render-stage sample indices and synchronous
CPU counter-resolution API directly, without a bridge or dependency patch.

The fixed offscreen workload contains 32 frames at 320×180, crossing 0/1000
fragment iterations with 0/20 ms between two submissions. Each frame owns a
scene render, dependent compute pass and composition render. A separate sampled
blit reads the pixels back for a frame sentinel and opaque/nonuniform image
check. Frames 29–32 additionally save one PNG per arm. Four slots bound
in-flight ownership; a full ring skips admissions. No frame-loop GPU wait or
later query-resolve submission is used. The program has a 15-second deadline
after pipeline setup and retains a failed summary if admitted work is pending.

Root schedules compilation and all GPU runs. The frozen source compiled with
ARC, blocks, `-O2 -Wall -Wextra -Werror` and the four frameworks: exit 0, no
diagnostics. Both the untraced and traced hardware runs completed all 32 frames.
The trace matched all 192 stage pairs exactly; complete Bevy/MetalFX scope and
instrumentation overhead remain unvalidated. See the design document for exact
receipts and quantitative limits. Use fresh output paths; the probe refuses an existing
output directory, and the analyzer refuses to overwrite its output file.

```sh
# CPU-only validator, with eleven regression tests.
python3 tools/render-timing-probe/analyze.py --self-test

# Compile only; does not create a Metal device or launch a GPU workload.
xcrun clang -fobjc-arc -fblocks -O2 -Wall -Wextra \
  -framework Foundation -framework Metal -framework CoreGraphics -framework ImageIO \
  tools/render-timing-probe/StageProbe.m \
  -o /private/tmp/ushas-stage-probe-02

# Root's serialized hardware slot. Retain stdout/stderr and the process exit code.
MTL_DEBUG_LAYER=1 /private/tmp/ushas-stage-probe-02 \
  --out /private/tmp/ushas-roadmap-evidence/stage-probe-02

python3 tools/render-timing-probe/analyze.py \
  /private/tmp/ushas-roadmap-evidence/stage-probe-02/samples.jsonl \
  --out /private/tmp/ushas-roadmap-evidence/stage-probe-02/analysis.json
```

Retain the exact compile/run argv, compiler and SDK version, source revision,
SHA-256 of `StageProbe.m`, `analyze.py` and the invoked binary, OS/device metadata,
Metal debug environment, both exit statuses, JSONL and four PNGs. An analyzer
exit of zero means the declared records are structurally complete. It does not
override a nonzero native process exit or prove load/delay controls responded.

Every completion contains the original frame/view/epoch/slot generation,
actual CPU submission/callback/resolution/delivery times, all four render
samples, compute and blit pairs, and pixel evidence. CPU and GPU clocks are not
subtracted from one another. `render_stage_union_ns` merges this frame's vertex,
fragment and compute intervals; `outer_render_envelope_ns` retains their gaps.
Readback is excluded and reported separately. The analyzer also retains
cross-frame overlap and a global interval union, so adding per-frame values
cannot silently become a claim of exclusive hardware occupancy.

The first hardware review must examine raw samples and PNGs, GPU-load response,
the measured CPU submission gap and its effect on stage unions versus outer
envelopes. Tiny or inverted stages, missing timestamps, stale records,
unresolved slots and failed image evidence are failures. Even a positive
native result still requires independent Metal trace validation, overhead
measurement, complete Bevy/raw-MetalFX scope and asynchronous production
integration before any live adaptation claim.

[TRACE.md](TRACE.md) gives the separate, bounded trace command and the expected
encoder-family join. Do not run the old marker-envelope analyzer against these
records: its marker schema describes a different experiment.
