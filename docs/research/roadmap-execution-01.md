# Roadmap execution checkpoint

This checkpoint records the September 4–5, 2026 implementation and measurement
campaign. The candidate is **0.5.0-rc.1, unpublished**. Fixed-scale rendering has
new execution and image evidence. Automatic adaptation still lacks a validated
GPU busy-cost producer and holds its scale with an unavailable reason. The
[roadmap acceptance contract](../roadmap.md) is not fully satisfied.

## Implemented

- Per-view requested/effective effect state, observed dimensions, render-frame
  identity, readiness, output-write state and bounded history acknowledgements.
  Unsupported multi-view and subviewport configurations fail closed.
- Correct Bevy projection jitter signs, early full-resolution reconstruction,
  native-resolution UI, and explicit bilinear fallback controls.
- A pure time-based governor and Bevy adapter with explicit FPS caps,
  monitor-reported defaults, a 0.5 quality floor, freshness and identity checks,
  settling, hysteresis, benefit checks and infeasible-budget reporting. CPU loop
  time and experimental Metal timestamps cannot silently become valid input.
- A repository-owned [Claude fixture](../../tools/smoke/README.md), modeled from
  the user's selected character reference, with animated geometry, thin rails,
  HDR controls and preserved source/binary/capture provenance. The former shapes
  remain an optional control. No shop bitmap is bundled.
- Bounded serial-completion, matched-quality and presentation diagnostic tools;
  frozen consumer preparation; feature/platform/MSRV/docs/package checks in CI.

## Measured decisions

| Gate | Evidence and resulting decision |
|---|---|
| GPU signal feasibility | [Metal trace review](timing-feasibility.md) matched 237 descriptor intervals to the trace, but they include substantial idle/drawable waits, overlap and incomplete coverage. The instrument is unsuitable as a GPU busy-cost input. Keep automatic adaptation unavailable. |
| Claude fixed-scale benefit | [60 completed runs](claude-completion-campaign-01.md) all exited successfully. At load 8,000, native averaged 30.486 ms and Temporal half 13.848 ms per serial completed render. Half missed the 16.667 ms budget on 8.90% of measured intervals; this does not establish smooth 60 FPS. |
| Highest tested budget rung | [20 middle-preset runs](claude-middle-presets-01.md) all passed. Temporal 0.58 and two-thirds exceeded the mean 60 Hz budget. Half is the highest tested rung meeting the mean budget at load 8,000. At load 20,000 the lowest allowed rung fails, and no tested allowed rung meets it. |
| Claude image quality | [Six matched arms](claude-matched-quality-01.md): 72 opaque captures and 870 frame proofs, including motion, reset and HDR. Half is softer in motion and conspicuously aliased on the reset frame; held cut16 is substantially cleaner. Third loses more detail. Retain the conservative 0.5 floor; immediate native quality and continuous temporal stability are unproved. |
| Current consumer | [12 balanced completed runs](consumer-completion-trial-01.md): native 6.497 ms, Temporal half 7.849 ms, bilinear half 6.781 ms. Retain native for this frozen static Shadow Work scene. This is not a verdict on every consumer workload. |
| Consumer cuts | [Three arms, 18 captures](consumer-cuts-01.md) completed; a visible texture seam also appears in the native baseline. No broad consumer quality pass or exact reset-recovery frame claim follows. |
| Lifecycle | [Five window-target exercises](lifecycle-candidate-01.md) passed: resize, camera cut, late/replacement camera, unsupported multiple views and inactive-cut-resume. These do not prove window visibility, OS sleep recovery or driver-failure recovery. |
| Presentation | [Historical reconciliation](frame-generation-reconciliation.md) preserves prior strong-gauge evidence and its limits. The [new diagnostic](presentation-diagnostic-01.md) stopped at asleep, locked-session preflight without launching a renderer. Present cadence, content ordering, latency and net value remain unvalidated. |

Completed-render cadence includes CPU submission gaps and uses one frame in
flight. It is neither GPU busy time nor normal pipelined application FPS.
Performance runs and quality captures are separate experiments. Earlier invalid
runs remain archived, including seven failures in the original unpaced
60-attempt campaign; the serial fixture does not establish a fix to that path.

## Remaining work, in priority order

1. Establish a frame-identified GPU-cost producer with complete scope and fresh
   delivery before enabling live adaptation. Validate it against GPU-heavy,
   CPU-only and presentation-limited controls; then run hardware trajectories
   and moving adaptive quality checks. The tested policy alone is insufficient.
   This continuation is tracked by `shadow-work-vzox.8`. Complete a matched
   native-resolution Temporal quality control and make an explicit acceptable
   motion/reset quality decision; sampled settled images are not that decision.
2. Complete lifecycle coverage for real OS sleep/resume, occlusion/minimization
   and forced scaler-creation failure. The historical MPSGraph crash remains
   unreproduced; successful starts cannot close it as fixed.
3. With an eligible unlocked display, run the bounded presentation diagnostic.
   Aggregate timestamps still need frame-kind, content/order and latency
   evidence before a net-value decision. Keep frame generation experimental.
   This independent lane does not block the adaptive release.
4. Review the concrete release artifact after the remaining acceptance gates.
   No tag, registry publication or production consumer switch has occurred.
   Frame-generation acceptance remains separate from that release decision.

The implementation and frozen evidence are committed on `main`. Large raw
artifacts and exact executables are hash-archived under
`/Users/sma/projects/docs/ushas/evidence/`; individual reports identify their
manifests. The local roadmap checkpoint records the final source, CI and package
receipt without claiming that a green package closes the remaining hardware
gates. Work remains tracked by `shadow-work-vzox` and its open children.
