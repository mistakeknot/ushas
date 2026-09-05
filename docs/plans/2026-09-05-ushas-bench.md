# Ushas Bench implementation plan

Task: `shadow-work-vzox.9`. User approved September 5, 2026.

Deliver a self-contained Apple Silicon macOS26 preview app and ZIP, with a
SwiftUI launcher and a Rust/Bevy renderer. The Claude render lab supports fixed
benchmark, six-arm comparison, adjustable stress, and offline results. Target:
2560x1440 physical pixels and120FPS. A valid no-benefit result is acceptable.

## Fixed scene

Three deterministic1200-tick sequences at nominal1/120second simulation steps:
materials (hero Claude,12material displays,6lights/2shadowed spots), geometry
(64animated Claudes, props and thin structures), lighting (16Claudes,8lights/
4shadowed spots,4096particles). Reuse attributed procedural Claude geometry;
cache procedural assets before measurement. No synthetic fragment burn in the
standard profile. Stress load is adjustable, labelled custom, default10minutes.

## Measurement and comparisons

Keep Bevy pipelining enabled and request uncapped presentation. Verify actual
window, extracted target and effect output all2560x1440 throughout the cohort.
Immutable contiguous scene/frame/view/config tokens and exact fresh output
proofs qualify each tick. Asynchronous opening/closing queue fences cover the
whole cohort; never wait or install completion callbacks per frame. Score
qualified frames divided by first-admission-to-closing-callback elapsed time.
Report per-scene completed-renderFPS and their geometric mean; noGPUbusy,
panelFPS,1%low or frame-generation claims. Invalidity withholds score.

Separate deterministic image-target replay retains identified face/edge/motion/
cut captures. Never include capture readback in scored intervals. Compare native
MSAA4, Temporal native, Temporal two-thirds, Temporal half, Spatial half and
bilinear half. Qualification:4balanced rounds,24freshprocesses,8%practical
completed-render-time threshold and paired uncertainty. Preserve every failure.

## Implementation tracks

1. Root: shared CLI/config/report contracts, comparison/export, integration, CI,
   documentation, hardware coordination, review, commits and push.
2. Scene owner: private Claude model reuse and render-lab scene/animation/assets.
3. Engine owner: native benchmark/stress/capture runtime and completion ledger.
4. App owner: SwiftUI launcher/results, child-process controls and.app packaging.

Disjoint file ownership is recorded in `tools/benchmark/CONTRACT.md`. Root owns
the index and commits each complete logical unit. Agent reports require root
verification. Build/test work can overlap; GPU campaigns run serially.

## Acceptance

CPU contracts cover deterministic timelines, missing/duplicate/stale proof,
configuration changes, timeout/cancellation,report compatibility and child
handling. Verify the actual app's launch/run/compare/images/stress/Stop/export.
Run24-arm qualification and10minute stress; record available thermal pressure
without inventing GPU temperature/utilization. Package an ad-hoc-signed.app and
ZIP, launch outside the repo withoutRust/assets dependencies, and passCI before
closing the task. GPUtiming side track is bounded to publicAPI assessment and at
most one matched native/Temporal trace pair; missing scope ends it.

No game migration, sleep/wake gate, adaptive backend rewrite, frame generation,
notarization, public publication, accounts, telemetry or leaderboard.
