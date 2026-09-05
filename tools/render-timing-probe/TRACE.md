# Bounded trace plan

Run only after the native probe has correct pixels and structurally complete
samples. Root owns the GPU slot. This is a second run of the same frozen binary
with different fresh artifact paths; tracing can perturb timing, so retain it
separately from the first untraced probe.

The installed `xctrace help record` and `help export` were checked before writing
these commands. Do not launch another workload concurrently. `--launch` targets
this process; `--all-processes` is unnecessary. Instrumentation can still record
other processes' GPU intervals, which must remain separate from owned work.

```sh
xcrun xctrace record --template 'Metal System Trace' \
  --output /private/tmp/ushas-roadmap-evidence/stage-probe-trace-02.trace \
  --time-limit 20s --no-prompt --env MTL_DEBUG_LAYER=1 \
  --launch -- /private/tmp/ushas-stage-probe-02 \
  --out /private/tmp/ushas-roadmap-evidence/stage-probe-traced-02

# Wait for recording to finish and retain its actual exit code before exports.
xcrun xctrace export \
  --input /private/tmp/ushas-roadmap-evidence/stage-probe-trace-02.trace \
  --toc --output /private/tmp/ushas-roadmap-evidence/stage-probe-trace-02.toc.xml

xcrun xctrace export \
  --input /private/tmp/ushas-roadmap-evidence/stage-probe-trace-02.trace \
  --xpath '/trace-toc/run[@number="1"]/data/table[@schema="metal-application-encoders-list"]' \
  --output /private/tmp/ushas-roadmap-evidence/stage-probe-trace-02.encoders.xml
```

Inspect the TOC, then repeat the final export for each present schema below,
changing both the schema and output suffix. Export tables separately so their
column schemas and XML references are retained. Missing required tables make
the corresponding scope check unavailable. This host's exports require access
to Instruments' normal cache outside the workspace sandbox.

| Schema | Output suffix | Purpose |
|---|---|---|
| `metal-application-encoders-list` | `.encoders.xml` | CPU encoder identity and exact labels |
| `metal-gpu-intervals` | `.gpu-intervals.xml` | GPU intervals joined by encoder ID, not creation order |
| `metal-gpu-state-intervals` | `.gpu-states.xml` | Global Active/Idle intervals; union before intersection |
| `metal-application-intervals` | `.application.xml` | Submission and CPU waits, separate from GPU intervals |
| `metal-driver-intervals` | `.driver.xml` | Driver dependency waits and corroboration |

Analyze the traced run's `samples.jsonl` with `analyze.py` too. A successful
Instruments process exit cannot override a failed/missing native summary or
invalid images. Retain stdout/stderr, trace, all exports, JSONL, PNGs, both
process outcomes and the hashes/source identity of the same invoked binary.

## Expected join and falsification checks

Filter CPU encoder and GPU interval tables to the exact PID in this run's JSONL
header. Resolve XML `ref` values before interpreting IDs or labels. Within that
process join `encoder-id` to the encoder metadata, retaining `cmdbuffer-id` as
an additional consistency check. A missing join or duplicate identity remains
an explicit coverage error. Never use CPU encoder creation order to assign a
GPU interval to a frame.

Every frame has exactly four labeled encoder families:

```text
stage-probe/frame=F/view=1/epoch=E/slot=S/gen=G/scene
stage-probe/frame=F/view=1/epoch=E/slot=S/gen=G/compute
stage-probe/frame=F/view=1/epoch=E/slot=S/gen=G/compose
stage-probe/frame=F/view=1/epoch=E/slot=S/gen=G/diagnostic-readback
```

Match all five identity numbers to the retained admission/completion ledger,
then require one CPU encoder for each family per frame: 128 encoders for the
32-frame probe. Additional target encoders and target GPU rows without CPU
metadata must be reported, not assigned by a nearest-frame heuristic.

For `scene` and `compose`, inspect all GPU rows of the same encoder and separate
vertex and fragment stage channels. Compare each stage's first and last GPU
boundary with its two counter samples. Compute and readback use their complete
corresponding GPU rows. If a stage has multiple rows, retain their gaps and union;
do not replace them silently with one busy interval. Unexpected channels or
missing stages prevent complete scope validation.

The counter clock is absolute while exports use a trace-relative origin. First
compare durations, which need no cross-clock offset. If comparing endpoints,
declare the offset from the first matched scene vertex boundary and test all
other endpoints against that fixed offset; report residuals and drift. Do not
fit independent offsets per stage or frame. Timestamp agreement alone cannot
establish busy cost.

Union owned scene vertex/fragment, compute and composition stage intervals
within each frame. Exclude the labeled diagnostic readback. Retain cross-frame
stage overlap and the global union independently. Intersect stage unions with
global Active/Idle states after unioning overlapping state channels. Active
state does not prove this process was executing; another process may be active.
Any remaining idle or wait inside owned stage bounds is relevant negative
evidence against calling their union exclusive GPU busy time.

Compare the four arms using the traced run's original identities and observed
CPU submission gaps. The expected discrimination is a GPU-load response in
fragment work, with CPU-gap growth appearing in the outer envelope rather than
the stage union. This short feasibility probe cannot establish instrumentation
overhead: that requires the same work without sample attachments in a later
paired experiment. This trace also contains no Bevy or MetalFX work and cannot
close those integration gates.

## Current offline parser

The new parser has seven passing CPU regressions and independent code review.
It validates native records first, joins all 128 named encoder identities, and
checks all 192 stage pairs against one fixed clock offset. It also reports
same-device global Idle intersections and foreign/unattributed GPU overlap.
Its `valid` flag means complete structural scope and timestamp agreement for
this native capture; `validated_for_governor` always remains false.

```sh
python3 tools/render-timing-probe/analyze_trace.py --self-test
python3 tools/render-timing-probe/analyze_trace.py \
  --samples /private/tmp/ushas-roadmap-evidence/stage-probe-traced-02/samples.jsonl \
  --encoders /private/tmp/ushas-roadmap-evidence/stage-probe-trace-02.encoders.xml \
  --gpu /private/tmp/ushas-roadmap-evidence/stage-probe-trace-02.gpu-intervals.xml \
  --states /private/tmp/ushas-roadmap-evidence/stage-probe-trace-02.gpu-states.xml \
  --out /private/tmp/ushas-roadmap-evidence/stage-probe-trace-02.fresh-audit.json
```

The parser hashes all input files and refuses to overwrite its output. The
retained reviewed result is `stage-probe-trace-02.audit-v2.json`; earlier audit
output remains untouched. `stage_row_gap_ns` includes diagnostic readback;
global/per-frame rendering-stage unions exclude readback.
