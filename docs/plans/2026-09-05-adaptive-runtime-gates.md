# Adaptive runtime and quality gates

The user authorized these parallel tracks on September 5, 2026. Baseline:
`9e254c9c397e955cac15cd531ffa4696e5880b33`, clean and pushed, with successful CI
and an independently verified unpublished 0.5.0-rc.1 tarball. This continues
the existing `shadow-work-vzox` roadmap; it does not authorize publication or
the previously rejected external Beads tracker sync.

## Track A: validated GPU input

Owner: timing feasibility agent. Integrator and hardware operator: root.

1. Prototype a new measurement path on actual dependency-carrying render work.
   Investigate per-stage render timestamps and callback-side direct resolution
   before touching the governor. Declare which work is included and excluded.
2. Preserve frame, view, configuration, submission and observation identities.
   Use bounded asynchronous storage; exclude frame-loop waits and stale reuse.
3. Require workload output proof, a GPU-load positive control, a CPU-delay
   negative control and independent trace coverage. Check overlap, missing work,
   readback freshness and measurement overhead. A standalone pass is not a
   complete Bevy/MetalFX frame-cost producer.
4. Only integrate a producer into `ValidatedGpuFrameCost` after scope and
   calibration pass. Then exercise live 60/120 Hz load changes, recovery,
   quality floors, target changes and invalidation. Keep `TimingUnavailable`
   if the new path fails; do not substitute serial CPU-inclusive completion.

## Track B: quality and lifecycle acceptance

Owners: quality agent and lifecycle agent, with disjoint source ownership.

1. Run the missing native-resolution Temporal control using the frozen matched
   quality executable. Independently compare all twelve captures with native
   MSAA4 and half-resolution Temporal under an explicit feature-readability,
   reset-transient and held-recovery rubric. Preserve narrower conclusions.
2. Add a nondefault diagnostic fault-injection path at scaler creation. Exercise
   synthetic failure and delayed completion through the actual fallback,
   readiness and history acknowledgement paths; release the fault and require
   real output recovery. Never label synthetic behavior a reproduced driver bug.
3. Add bounded, externally driven window/OS lifecycle observation. A requested
   minimize, camera pause or wall-clock gap does not prove actual OS sleep.
   Retain native events and contemporaneous display/session evidence. Do not
   change persistent power settings or silently lock the user's session.
4. If the missing native control changes the quality decision, design a separate
   short moving-after-cut protocol. Do not infer continuous video quality from
   sparse samples, or hide reset defects behind settled quality scores.

## Coordination and acceptance

- Root schedules all GPU runs and heavy builds; measurements never overlap.
- Timing owns `tools/render-timing-probe/` and its research report. Quality owns
  the quality module/runner/tests/report. Lifecycle owns its fixture module,
  scaler fault seam and fault resource. Root owns shared wiring and the display
  preflight parser. Shared edits are coordinated before implementation.
- Use failing behavioral regressions for changed contracts, followed by
  independent specification and code review. Freeze source before hardware
  validation and retain failures with successful evidence.
- Commit logical units to main and push authorized Ushas code. Keep tracker
  mutations local with `bd --sandbox`; remote tracker sync remains unapproved.
- Frame generation remains a separate lane, outside this goal's critical path.
- Finish with exact source, tests, rendered evidence, package and CI receipts.
  Successful partial probes do not close an unmet adaptive release gate.
