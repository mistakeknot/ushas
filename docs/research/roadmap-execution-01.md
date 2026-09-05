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
- Default-off creation-fault injection with generation-safe fallback/recovery,
  separate bounded observation windows for each lifecycle phase, and native
  window/system-sleep observers. Observer implementation alone is not a native
  lifecycle result.

## Measured decisions

| Gate | Evidence and resulting decision |
|---|---|
| GPU signal feasibility | [Metal trace review](timing-feasibility.md) matched 237 descriptor intervals to the trace, but they include substantial idle/drawable waits, overlap and incomplete coverage. The instrument is unsuitable as a GPU busy-cost input. Keep automatic adaptation unavailable. |
| Native stage investigation | [The second probe](gpu-producer-02.md) distinguishes GPU work from a CPU submission gap; its trace matches 128 encoder identities and 192 stage pairs. This is a partial positive for a synthetic native chain. Complete Bevy and raw MetalFX scope, asynchronous attribution and instrumentation overhead remain unresolved; the [source inventory](bevy-frame-scope-02.md) identifies the missing seams. |
| MetalFX observation compatibility | [The bounded proxy experiment](metalfx-proxy-02.md) preserved exact pixels across all 16 matched Spatial frame pairs, but each wrapper observation failed on the unreviewed `globalTraceObjectID` selector. The declared guard stopped counters, Temporal follow-on and Bevy integration. Pixel equality does not establish complete observation; automatic adaptation remains unavailable. |
| Validation-layer control | [Four fresh processes](proxy-validation-control-03.md) show the same unknown selector and MetalFX caller with validation enabled and disabled. Both references pass; both CALLS arms reject every observation. Stop this proxy route. Use unwrapped whole-process Metal System Trace for future offline profiling; it is not a runtime producer. No counter/Temporal proxy arm or Bevy rewrite follows from this result. |
| Claude fixed-scale benefit | [60 completed runs](claude-completion-campaign-01.md) all exited successfully. At load 8,000, native averaged 30.486 ms and Temporal half 13.848 ms per serial completed render. Half missed the 16.667 ms budget on 8.90% of measured intervals; this does not establish smooth 60 FPS. |
| Highest tested budget rung | [20 middle-preset runs](claude-middle-presets-01.md) all passed. Temporal 0.58 and two-thirds exceeded the mean 60 Hz budget. Half is the highest tested rung meeting the mean budget at load 8,000. At load 20,000 the lowest allowed rung fails, and no tested allowed rung meets it. |
| Initial Claude image evidence | [Six matched arms](claude-matched-quality-01.md) passed artifact validation: 72 opaque captures and 870 frame proofs, including motion, reset and HDR. Half is softer in motion and conspicuously aliased on the reset frame; held cut16 is substantially cleaner. Third loses more detail. Artifact validity is separate from quality acceptance. |
| Claude motion/reset decision | The [native Temporal control](claude-quality-acceptance-02.md) and [moving-v2 experiment](claude-moving-quality-02.md) now have explicit independent verdicts. Moving v2 retains 69 PNGs and 435 frame proofs across native MSAA4, native Temporal and Temporal half. Both reviewers accept the short sequence's readability and moving cut8–16 recovery for both Temporal arms; both arms fail immediate-cut native quality. Retain the 0.5 floor and conditional fixed-scale acceptance, without claiming continuous stability or adaptive-transition quality. |
| Current consumer | [12 balanced completed runs](consumer-completion-trial-01.md): native 6.497 ms, Temporal half 7.849 ms, bilinear half 6.781 ms. Retain native for this frozen static Shadow Work scene. This is not a verdict on every consumer workload. |
| Consumer cuts | [Three arms, 18 captures](consumer-cuts-01.md) completed; a visible texture seam also appears in the native baseline. No broad consumer quality pass or exact reset-recovery frame claim follows. |
| Lifecycle | [Five window-target exercises](lifecycle-candidate-01.md) passed: resize, camera cut, late/replacement camera, unsupported multiple views and inactive-cut-resume. These do not prove window visibility, OS sleep recovery or driver-failure recovery. |
| Creation-fault recovery | [Fresh offscreen failure and slow-creation runs](lifecycle-fault-offscreen-03.md) pass independent retained-phase gates: actual image/view/dimensions, at least 20 distinct eligible frames per phase, pending reset through fallback, later Temporal recovery and opaque captures. The slow arm declares 2,917 evictions. This closes the synthetic creation-fault slice within retained windows; it does not prove a real driver failure, native-window recovery or complete frame history. [Failed window attempts](lifecycle-fault-window-attempts-02.md) and the [incomplete earlier slow ledger](lifecycle-fault-offscreen-02.md) remain archived. |
| Native minimize/restore | [The new native fixture](native-recovery-04.md) retains an actual minimize/restore event cycle, later Temporal reset acknowledgement and restored Claude pixels. Inspection of the actual desktop window remains unverified: CUA could not select the standalone executable, and the second inspection attempt timed out before arming. A subsequent display sample reports locked; it does not establish the cause or duration of the earlier stall. OS sleep/wake was not run. |
| Presentation | [Historical reconciliation](frame-generation-reconciliation.md) preserves prior strong-gauge evidence and its limits. The [new diagnostic](presentation-diagnostic-01.md) stopped at asleep, locked-session preflight without launching a renderer. Present cadence, content ordering, latency and net value remain unvalidated. |

Completed-render cadence includes CPU submission gaps and uses one frame in
flight. It is neither GPU busy time nor normal pipelined application FPS.
Performance runs and quality captures are separate experiments. Earlier invalid
runs remain archived, including seven failures in the original unpaced
60-attempt campaign; the serial fixture does not establish a fix to that path.

## Remaining work, in priority order

1. Establish a frame-identified GPU-cost producer with complete scope and fresh
   delivery before enabling live adaptation. The public-selector proxy route is
   now stopped; another runtime measurement design must first establish a viable
   MetalFX coverage mechanism. Offline trace profiling is useful but cannot feed
   the governor. Do not begin an unbounded Bevy/HAL rewrite. Validate any future
   producer against GPU-heavy,
   CPU-only and presentation-limited controls, including complete submission
   ownership, trace agreement, sample age and overhead under normal pipelining.
   Then run hardware 60/120 FPS target trajectories, floor/target changes,
   overload/recovery, invalidation and moving adaptive quality checks against
   the best fixed rung. The tested policy and native stage probe are insufficient.
   This continuation is tracked by `shadow-work-vzox.8`; the immediate-cut quality
   failure remains disclosed rather than being converted into a pass.
2. Complete interactive native-window inspection and actual OS sleep/resume.
   The first minimize run supplies bounded native-event and restored-pixel
   evidence; it does not clear the separate desktop-window inspection gate.
   Require fresh native events, contemporaneous environment evidence, restored
   output/reset acknowledgement and opaque captures. Camera inactivity and
   offscreen fault recovery do not satisfy these gates. The historical MPSGraph
   crash remains archived as unreproduced; neither synthetic faults nor successful
   starts establish a fix.
3. With freshly observed awake, unlocked and on-console conditions, run the
   bounded presentation diagnostic.
   Aggregate timestamps still need frame-kind, content/order and latency
   evidence before a net-value decision. Keep frame generation experimental.
   This independent lane does not block the adaptive release.
4. Review the concrete release artifact after the remaining acceptance gates.
   Refresh exact-source feature/platform/MSRV/API/docs/package and render checks;
   earlier package or CI success does not verify subsequent source changes.
   No tag, registry publication or production consumer switch has occurred.
   Frame-generation acceptance remains separate from that release decision.

The implementation and frozen evidence are committed on `main`. Large raw
artifacts and exact executables are hash-archived under
`/Users/sma/projects/docs/ushas/evidence/`; individual reports identify their
manifests. The local roadmap checkpoint separates the current source from the
latest confirmed CI and package revisions; none is labelled the final accepted
release. The [September 5 16:21 UTC sample](/Users/sma/projects/docs/ushas/evidence/native-recovery-04/display-midrun-02.json)
found the display awake and session locked. Despite its filename, this sample
occurred after the second native attempt ended; it does not establish that
attempt's cause. Further interactive work awaits an available unlocked session.
Work remains tracked by `shadow-work-vzox`
and its open children. Broader resolution/device matrices and copy-removal
experiments remain optional unless evidence shows they could change the decision.
