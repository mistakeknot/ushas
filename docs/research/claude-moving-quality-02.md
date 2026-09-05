# Claude moving-reset quality, second protocol

Two independent inspections accept **readability throughout this short moving-reset
sequence**, including every cut8–16 state, for native-resolution Temporal and
Temporal half. **Immediate-cut native quality fails both Temporal arms**: the
first reset image has conspicuous aliasing, with substantially stronger loss at
half resolution. This is conditional acceptance of the declared fixture's
readability and moving recovery, with immediate-cut quality explicitly unaccepted.

This applies the [prospectively declared moving-v2 rules](claude-quality-acceptance-02.md#prospective-moving-after-cut-experiment).
It extends the earlier held-pose test with ongoing camera and model motion after
the cut. The old twelve-image protocol, evidence and conclusions remain separate.

## Matched evidence

Three sequential runs on September 5, 2026, used clean source
`9a90c3d83c71a5083bad160af7cbd445409970c1` and the same frozen executable:

- Binary SHA256: `29a28b2a15a0b7663c6b2d662d8cfea2861577afbf0703c22bffe83a1fc54c7d`.
- Runner SHA256: `4839e7abff54c891d643ba9271db2c0483acc24bdc2266add7eb52d9286b07fe`.
- Native control: Disabled, scale 1, MSAA4.
- Temporal controls: scale 1 and scale 0.5, both with MSAA off.

All arms output 1280×720 SDR images of `claude-toy-v1`. The retained logs identify
Apple M5 Max, Metal and enabled Metal API validation. No per-run OS, driver or
thermal metadata is retained. These are offscreen runs and make no display-state,
session-unlock or panel-delivery claim.

Each arm exited zero and passed its frozen validator: **145 contiguous render
proofs and 23 opaque PNGs**, totaling **435 proofs and 69 images**. Independent
CPU revalidation compared the original wrapper hashes, exact executable/runner,
capture entities, extracted frame and view, image target, effect scope, reset
acknowledgement and completed-frame fence. Readback arrival is distinct from the
rendered frame it carries. All 145 actual camera matrices, expected matrices,
model pose clocks and jitter indices match across arms. The two Temporal arms'
actual jitter and reset ledgers also match.

The first 128 logical ticks match the earlier scenario. Tick128 hard-cuts the
camera and requests a Temporal reset; the model clock then continues at 1/60
simulation steps and the camera pans from the new viewpoint. Six pre-cut
checkpoints and every post-cut state `moving-cut0` through `moving-cut16` are
captured. This is a serial, unpaced simulation, not a real-time video recording.

## Independent inspections

The harness author viewed all 69 original images at their supplied 1280×720
dimensions, grouped by matched state. The
[per-capture record](/Users/sma/projects/docs/ushas/evidence/claude-moving-quality-02/review/author-inspection.json)
records all three faces and ray silhouettes, limbs and tails, six rails and their
occlusion boundaries, checkerboard and full UI. The review is not blinded and
uses the declared qualitative feature/readability rules. A second reviewer
independently inspected all 69 original images and recorded the
[separate verdict](/Users/sma/projects/docs/ushas/evidence/claude-moving-quality-02/review/independent-inspection.json)
before reading the author's review. That reviewer also verified that all 69
reviewed originals matched the archived PNG hashes. Both reviewers agreed on
every gate below; neither review was blinded.

| Gate, agreed by both reviewers | Native Temporal | Temporal half |
|---|---|---|
| Settled and five pre-cut moving checkpoints | Pass; mild smoothing relative to MSAA4 | Pass; additional softness and facial/rail grain |
| All seventeen post-cut states retain readable features | Pass | Pass, including recognizable but heavily aliased cut0 |
| Every moving cut8–16 state meets readability | Pass | Pass; residual softness remains |
| Immediate cut0 quality relative to native MSAA4 | Fail; jagged rails, checkerboard and face outlines | Fail; stronger scene-wide stair-stepping and facial grain |

No obvious old-pose overlay, double contour, missing or merged facial mark,
missing reference-visible rail segment or corrupted UI was observed in any
post-cut image. Native MSAA4 itself retains ordinary fine edge stair-stepping.
Temporal half improves through cut1–4 while motion continues, and every later
required state is readable. Recognizable features at cut0 do not make that image
native quality, and later readability does not erase the immediate-quality
failure. Both reviewers identified cut0 as already meeting the narrower feature
readability rule; that is not a native-quality recovery point. No exact
wall-clock recovery time is inferred.

## Retention and limits

The separate archive at
`/Users/sma/projects/docs/ushas/evidence/claude-moving-quality-02/` contains **104
payload files, 126,160,873 bytes**: all three original reports, manifests and logs;
69 PNGs; the exact executable; build and launch receipts; frozen source files and
a complete committed-source bundle; the declared protocol; the CPU audit and
both separate inspections. Copied files matched source-before, source-after and
destination hashes. No old quality archive was modified. Its
[manifest](/Users/sma/projects/docs/ushas/evidence/claude-moving-quality-02/archive-manifest.json)
has SHA256 `0b98522bcea2b74778627efe8ec48170f5130a8c22e9bf422665ddc037757794`.

```sh
PYTHONDONTWRITEBYTECODE=1 python3 \
  /Users/sma/projects/docs/ushas/evidence/claude-moving-quality-02/audit.py
```

The [machine-readable report](claude-moving-quality-02.json) separates validated
evidence from the visual verdict. This short SDR sequence does not establish
longer continuous stability, adaptive-transition quality, HDR behavior, GPU
cost, presentation or general consumer enablement. Existing performance and
consumer-native recommendations remain separate. Both reviewers accept only the
declared short moving sequence's readability and recovery; neither accepts
immediate-cut native quality or broader continuous-video stability.
