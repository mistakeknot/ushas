# Claude quality acceptance: native Temporal control

Protocol declared on September 5, 2026, before inspecting the missing native-scale
Temporal arm. The six earlier arms and their visible defects are already known;
this is a prospective rule for the new control and an explicitly retrospective
application to those existing images. It is not a blinded study or a population
quality score. Two independent, nonblinded reviews now agree on conditional
sampled-motion/settled acceptance and held recovery, with immediate-reset quality
unaccepted.

The decision is whether Temporal's own native-resolution image is usable beside
native MSAA4, what additional loss half-resolution input introduces, and whether
the observed reset transient is acceptable for a stated use. A pass for settled
or sampled moving images does not imply immediate post-cut quality or continuous
video stability.

## Frozen control

Reuse the exact source and executable from the
[six-arm matched sequence](claude-matched-quality-01.md):

- Source: `ac3091f4b3f2cfe9d6dfc099298addfba8f35555`, clean at build.
- Binary SHA256: `06934fbfee226e6740fe57978ed730c79d0c7bb2ff8eeee7a5d5bb614e42e5b8`.
- Archived runner SHA256: `f06ed4c5a825080b2ff640cb074c0038dc4914778248f70b8daefe2206e29a39`.
- Build receipt: `/private/tmp/ushas-quality-artifacts-01/build.json`.

Both binary and runner were rehashed before proposing this invocation. The new
output paths were unused. Only the coordinating operator launches GPU work:

```sh
PYTHONDONTWRITEBYTECODE=1 MTL_DEBUG_LAYER=1 python3 \
  /Users/sma/projects/docs/ushas/evidence/claude-matched-quality-01/source/tools/smoke/quality_runner.py \
  --binary /private/tmp/ushas-quality-artifacts-01/ushas-smoke \
  --out /private/tmp/ushas-roadmap-evidence/quality-native-temporal-02.json \
  --mode temporal --scale 1
```

This requests 1280×720 Temporal input and output, MSAA off, SDR, four seconds of
warmup, the original 145 logical ticks and twelve captures. The existing
75-second application deadline, five-second queue-drain bound and 90-second
wrapper deadline remain in force. No source change or rebuild is needed.

Compare this new control first with native Disabled/MSAA4, then compare Temporal
half with native Temporal. The first comparison includes the different AA paths;
the second holds the Temporal path and MSAA policy constant while changing input
resolution. Neither comparison isolates a MetalFX kernel. Bilinear half supplies
the existing same-input-size fallback comparison. Third resolution and the two
HDR arms remain separate observations; no native-Temporal HDR control is present.

## Evidence gate

Before judging images, require normal child exit zero, successful frozen-runner
validation, complete hashes, all twelve opaque PNGs, and all 145 valid contiguous
render proofs. Compare every logical tick's actual camera matrix, expected camera
matrix, model pose clock and jitter index to the archived arms. Actual Temporal
jitter offsets must also match. Per-process entity and frame IDs may differ;
within each arm the screenshot entity, extraction, effect state, reset
acknowledgement, completed render frame and readback must satisfy the existing
validator. A missing, stale or unmatched arm is invalid evidence, not a quality
failure that can be averaged away.

Keep the original six-arm data and historical review intact. Archive the new
report, log, twelve PNGs and manifest with source/binary/runner identity. The
known build identity, scene version, resolution and SDR setting must match;
elapsed wall time and warmup length need not match. A later run on the same
machine is not proof of identical driver or environmental conditions, so record
the new run's available metadata and any differences.

## Review and decision rules

Two reviewers independently inspect all twelve matched captures at native image
size, with detail enlargement permitted only as an aid. Each records the capture
name, affected feature and severity before comparing conclusions. Disagreement
is retained as unresolved; it is not converted into a majority quality pass.
The review regions are all three faces and ray silhouettes, curved tails and
limb silhouettes, all six thin foreground rails and their occlusion boundaries,
the checkerboard, and the complete UI caption. Ignore a feature that is also
occluded in the corresponding native reference; do not excuse a feature missing
only in the candidate.

Use three descriptive grades per region: **clear**, **degraded but readable**,
or **unacceptable**. Slight softness, fine edge grain or stair-stepping may be
degraded but readable. Missing or merged facial strokes, a duplicated contour or
old-pose overlay, a disconnected or missing rail segment visible in the matched
reference, or corrupted/unreadable UI is unacceptable. These are task-specific
visual judgments, not calibrated perceptual measurements. Record actual
examples rather than assigning a numerical score.

| Gate | Captures | Rule |
|---|---|---|
| Settled reconstruction | `settled` | Faces, rays, rails and UI must be recognizable; neither reviewer may mark any region unacceptable. Report softness relative to both native references. |
| Sampled motion | `motion32`, `motion62`, `motion63`, `motion64`, `before-cut` | Every sample must meet the same readability rule. Inspect the consecutive 62/63/64 samples for changing duplicated edges or broken thin geometry, distinguishing the reference's real motion/occlusion. Any unacceptable sample fails this gate. |
| Immediate reset quality | `cut0` | Same readability rule, plus no conspicuous scene-wide increase in aliasing relative to the matched native MSAA4 reference. Also compare half with native Temporal to separate baseline and reduced-input loss. A visible reset transient fails immediate-quality acceptance even if later frames recover. |
| Held recovery | `cut1`, `cut2`, `cut4`, `cut8`, `cut16` | No old-pose overlay or corrupted UI in any sample. Both `cut8` and `cut16` must meet the settled readability rule. Record the first sampled acceptable point; do not infer an exact recovery duration between samples. |

Apply the gates first to native Temporal against native MSAA4, then to half
Temporal against both. A defect shared by native Temporal and half is a baseline
Temporal-path limitation; a defect seen only at half is additional reduced-input
loss. Neither is excused by a throughput improvement.

The resulting recommendation must state each gate separately:

- If native Temporal fails settled or sampled-motion readability, do not accept
  the Temporal path for this fixture, regardless of half-resolution speed.
- If native Temporal passes but half fails either readability gate, retain native
  quality and do not describe half as the highest acceptable-quality budget rung.
- If half passes settled and sampled motion but fails immediate reset quality,
  accept it only for the sampled motion/settled use with the reset transient
  disclosed. Keep immediate-cut quality unaccepted; held recovery does not
  establish quality while motion continues after a cut.
- If held recovery fails, do not accept camera-cut use under this protocol.

The previously inspected half-resolution `cut0` had conspicuous aliasing, so an
immediate native-quality claim was already unsupported before the new control.

## Completed control and independent assessments

The missing control ran September 5, 2026, starting at 07:07:37 UTC. It exited
zero and passed the unchanged frozen validator with **145 render proofs and
twelve opaque captures**. The retained log identifies Apple M5 Max / Metal and
Metal API Validation Enabled. This quality report does not retain per-run OS,
driver or thermal metadata; the later launch does not establish an identical
environment to the earlier arms.

Fresh CPU revalidation checked the original run hashes, exact frozen binary and
runner, all twelve screenshot/effect/reset/completion joins, and all 145 logical
poses against each of the six archived arms. Actual camera matrices, expected
matrices, model pose times and jitter indices match exactly. The three existing
Temporal arms also match the new arm's actual jitter and reset ledgers. Content
and output are both 1280×720, with MSAA off and the expected SDR main format.

The harness author inspected all twelve captures from native MSAA4, native
Temporal and Temporal half: **36 images**. The
[per-capture review](/Users/sma/projects/docs/ushas/evidence/claude-quality-acceptance-02/review/author-inspection.json)
records grades and observations separately from the machine evidence. No missing
or merged facial mark, missing reference-visible rail segment, duplicated contour,
obvious old-pose overlay or corrupted UI was observed in the settled and moving
samples. Native Temporal smooths edges relative to MSAA4; half adds softness and
fine grain, especially on facial strokes and thin rails, while retaining their
readability.

| Gate | Native Temporal | Temporal half | Observed distinction |
|---|---|---|---|
| Settled reconstruction | Pass, both reviewers | Pass, both reviewers | Native Temporal is clear; half retains readable facial marks with some softness. |
| Five sampled moving states | Pass, both reviewers | Pass, both reviewers | All three faces, silhouettes, rails and UI remain recognizable. Half is softer; consecutive `motion62/63/64` samples show no obvious duplicated contours or disappearing rail segments. |
| Immediate reset quality | Fail, both reviewers | Fail, both reviewers | Native Temporal `cut0` has visible rail/checker stair-stepping relative to native MSAA4. Half has substantially stronger aliasing across the faces, rays, limbs and checkerboard. |
| Held recovery | Pass, both reviewers | Pass, both reviewers | No obvious old-pose overlay or UI corruption in `cut1/2/4/8/16`. Both `cut8` and `cut16` meet the readability rule and are substantially cleaner than `cut0`; half retains more softness. |

The author's first post-cut sample meeting the narrower readability rule was
`cut1` for both arms, although visible aliasing remained. This is neither a
native-quality recovery point nor a measured recovery duration. The immediate-quality gate is
stricter than readability and fails in both arms. Thus reset aliasing is partly
a baseline Temporal-path limitation, with additional loss at reduced resolution.

The second reviewer independently inspected the same 36 distinct images and
agreed with every gate result. The
[separate review record](/Users/sma/projects/docs/ushas/evidence/claude-quality-acceptance-02/review/independent-inspection.json)
retains that review's extent and observations. Both reviews were nonblinded and
use the stated qualitative rules; they are not a calibrated population study.

The decision is **conditional acceptance of half for this sampled
moving/settled 720p SDR fixture**, with immediate-cut native-quality acceptance
withheld. The known throughput benefit cannot excuse that failed quality gate.
Do not use this result to approve ongoing motion after a cut or general consumer
enablement.

The new run is archived separately at
`/Users/sma/projects/docs/ushas/evidence/claude-quality-acceptance-02/`: **23
payload files, 4,447,516 bytes**, including its report, twelve PNGs, log, manifest,
frozen validator, build receipt, linked binary provenance, declared protocol,
data audit, both separate inspections and a CPU-only `audit.py`. The
[archive manifest](/Users/sma/projects/docs/ushas/evidence/claude-quality-acceptance-02/archive-manifest.json)
has SHA256 `36908f607d4d3e1507787668b3aa01e2e59a6a87f35cf474ab97b98acfb4a0b0`.
Every copied file matched source-before, source-after and destination hashes.
All 111 old payloads were rehashed and the old archive's exact file inventory
remains unchanged. The verifier decoded all **84 PNGs** across the seven arms;
the visual review above covers only the stated 36 images.

```sh
PYTHONDONTWRITEBYTECODE=1 python3 \
  /Users/sma/projects/docs/ushas/evidence/claude-quality-acceptance-02/audit.py
```

## Prospective moving-after-cut experiment

The separately implemented `claude-60hz-moving-cut-v2` protocol is not measured
yet. Declare the following rules before its hardware runs. Use native Disabled
MSAA4, native-resolution Temporal with MSAA off, and half-resolution Temporal
with MSAA off, all at 1280×720 SDR on the same newly frozen binary. Retain every
run and the exact source, executable and runner hashes; the earlier executable
does not implement this protocol.

The first 128 logical ticks match v1. At tick 128 the camera hard-cuts and Temporal
requests its reset; the model clock continues at `(tick - 32) / 60` seconds.
After the cut, the camera pans from x=-1.4 by
`0.75 * sin((tick - 128) / 60 * 0.8)`, retaining the cut viewpoint's other position
coordinates and look-at target. The same 145 full-frame proofs are required.
There are 23 readbacks: six pre-cut checkpoints and every `moving-cut0` through
`moving-cut16`. All names have the `moving-` prefix. The original twelve-image
held v1 protocol is unchanged.

Before image review, require normal exit and all 145 matching
frame/view/effect/reset proofs, all 23 opaque PNGs and matching completion fences,
and independently validated camera/model trajectories. Missing an interior
post-cut capture or supplying a held pose fails validation. Across the three
arms, compare actual camera matrices, pose clocks and Temporal jitter by logical
tick. The wrapper's `--moving-reset` flag selects this distinct expected protocol;
held v1 evidence cannot pass as moving v2.

Both reviewers must independently inspect **all seventeen post-cut states** and
the six pre-cut checkpoints for each arm. Apply the same feature regions and
readability grades declared above. Any obvious old-pose overlay, double contour,
missing or merged reference-visible facial mark, missing rail segment, or
corrupted/unreadable UI in any post-cut image fails moving-reset acceptance.
Each `moving-cut8` through `moving-cut16` must additionally meet the settled
readability rule while camera and models continue moving; a single unacceptable
state fails this recovery gate. Record early degradation and the first acceptable
sample without assuming that later states stay acceptable. Judge immediate
`moving-cut0` aliasing separately against native MSAA4 and native Temporal: later
readability cannot turn an immediate-quality failure into a pass. Preserve any
reviewer disagreement as unresolved.

Passing these rules would accept only this short, fully sampled moving-reset
sequence at the declared resolution and settings. It would not establish
continuous real-time video, adaptive transitions, presentation or a general
camera-cut quality guarantee. Hardware execution and independent visual review
are still required; the CPU protocol tests establish neither.

## Limits

The captures are sampled states from a serial, unpaced 1/60 simulation. There is
no continuous video, real-time pacing, input latency, panel delivery, GPU-cost or
adaptive-quality acceptance here. Sixteen logical post-cut steps must not be
reported as a measured wall-clock recovery time. Existing timing results remain
separate, including their pacing and workload limits.

The v2 extension has a distinct protocol identifier, trajectories, filenames and
validator, so its eventual results must remain separate from the completed held
sequence above. No v2 hardware outcome is implied by the earlier acceptance.
