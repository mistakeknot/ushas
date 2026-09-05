# Adaptive performance roadmap

The next release targets a chosen frame budget at the highest acceptable image
quality on Apple Silicon. The work is tracked in Beads; this document states the
acceptance contract rather than duplicating task status.

The [Ushas Bench Claude render lab](plans/2026-09-05-ushas-bench.md) is available
as a packaged native preview with benchmark, comparison, adjustable stress and
offline results. Background runs continue with other windows in front. Its
[validation receipt](research/ushas-bench-preview-01.md) records the complete
24-arm campaign, ten-minute stress run, package and native UI checks.
This milestone uses completed-render throughput and independent quality images;
it does not supply the GPU-only timing input required by automatic adaptation.

## Baseline

| Revision | Meaning |
|---|---|
| `1eaff858a44560f9903366aded62a6930040be09` | Published 0.4.2 baseline |
| `c51e941cd0e29a77944638a5cc7f3ff798c6706b` | Device quality ladder and timing control arm, integrated for 0.5 |
| `c558b784ec172a5a17f54049c84efc26ac9554bc` | Deferred direct-write experiment; excluded from the release |

## Acceptance order

1. Report actual per-view effect state: pending, fallback, encoded, and output
   written. Neither an encode nor a present request proves panel delivery.
2. Validate a Metal frame-cost signal with frame identity, scope and freshness.
   The existing raw-command-buffer diagnostic is not total-frame GPU cost.
3. Drive a pure, time-based adaptive controller from valid observations and an
   explicit frame budget and quality floor. Cover 60/120 Hz, stale samples,
   spikes, CPU-limited work, pause/resume and infeasible budgets.
4. Use a repository-owned sample with native and bilinear controls to verify
   quality under motion and the benefit of each useful rung. Expand the matrix
   only when the result could change the decision.
5. Run feature/platform, documentation, package and hardware gates on the release
   candidate. Publish only after the concrete candidate is approved.

Frame-generation evaluation proceeds independently: first reproduce the earlier
presentation evidence, then compare dual presentation with temporal-only,
including cost, ordering, dropped frames, judder and latency. Preserve a bounded
inconclusive result when the instrument cannot discriminate. Direct-write stays
deferred unless a representative profile identifies a material bottleneck.

## Experiment contract

Each run records source revision and dirty state, toolchain, device and OS,
features, requested mode, effective state, dimensions, readiness, and timing
scope. Missing rendering or timing is an invalid measurement, never a fast one.
Display protection is separate from tracing; required encoder/table presence is
checked before reading a capture. Interleave comparisons and report uncertainty
and a practical benefit threshold, not only a p-value.

Keep cold-start, resize, sleep/resume and failed scaler creation in the lifecycle
gate. The previously unreproduced MPSGraph crash remains an unreproduced report
unless a reproduction establishes a fix; a few successful starts do not prove
its absence.
