# Native recovery acceptance, run 04

The next retained campaign uses Claude in the native window fixture at Temporal
scale 0.5. It tests actual minimize/restore and externally initiated system
sleep/wake separately. Offscreen recovery, screen sleep, camera inactivity,
elapsed time and successful process exit cannot establish either result.

## Source and executable

Freeze a clean source revision after the native arm observability change, build
the release smoke executable, and retain its build command, source identity,
lock hash and binary SHA256. Use that one frozen executable for both arms; do
not rebuild or overwrite it between runs. The root integrator schedules all
builds and GPU execution.

Before this change, both `tools/smoke/target/release/ushas-smoke` and the retained
`/Users/sma/projects/docs/ushas/evidence/final-runtime-candidate-02/build/ushas-smoke`
were verified as SHA256
`2a560c7eba2f9a6d97b2c658519ddb9cfc70fbcb46ea7223addffd280fddf586`, built from
`45d7a05c74aa100f522a2dd1ebfc34fff80c6d1d`. That executable is a provenance
reference; the new arm marker requires a new build.

## Commands and coordination

Capture contemporaneous display/session state with native service access before
launch. Awake, unlocked, on-console state is required. Unknown state is unavailable
evidence. Record the same state after restoration and inspect the actual test
window using the UI tools. Keep other GPU work stopped for these short runs.

With `USHAS_SMOKE_BINARY` set to the newly frozen executable, run from the repo:

```sh
python3 tools/smoke/run.py --timeout 90 --binary "${USHAS_SMOKE_BINARY:?}" -- \
  --subject claude --mode temporal --scale 0.5 \
  --lifecycle window-minimize \
  --out /private/tmp/ushas-native-recovery-04/window-minimize-01.json

python3 tools/smoke/run.py --timeout 90 --binary "${USHAS_SMOKE_BINARY:?}" -- \
  --subject claude --mode temporal --scale 0.5 \
  --lifecycle os-sleep-resume \
  --out /private/tmp/ushas-native-recovery-04/os-sleep-resume-01.json
```

Execute the two commands sequentially. Never overwrite a failed attempt. The
wrapper retains failure logs and rejects existing evidence paths. It uses
`caffeinate -d` only for idle display sleep; the fixture contains no OS power or
lock requests.

Both modes require twenty distinct eligible initial frames and an opaque initial
capture before arming. Read the flushed `USHAS_NATIVE_LIFECYCLE_ARM ` JSON line
from the run log. The line contains the native ledger arm, UTC milliseconds,
process elapsed milliseconds, window identity and exact `after_sequence` boundary.
It is retained identically in the final lifecycle event detail.

Minimize is automatic: the fixture requests minimize of its own window, requires
both post-request occlusion and native minimized observations, holds that observed
state for at least 500 ms, then requests restore. Observe this actual interaction;
do not manually restore early or substitute a different window.

For system sleep, arrange the operator's participation before launching. After the
live arm marker, the operator initiates actual system sleep, wakes the Mac and
unlocks promptly. No test-controlled sleep, wake schedule or lock-policy change
is part of this protocol. A display-only sleep or lock action is insufficient.
If an operator cannot participate, complete minimize and keep OS sleep pending.

The native lifecycle deadline is 60 seconds of sleep-inclusive `SystemTime` from
observer installation. It includes initial rendering and capture, not just the
armed wait. At default settings the main smoke readiness/capture deadlines are
70/75 seconds of `Instant` time, preserving the lifecycle's full 60-second budget
and the normal six-second measurement tail. The wrapper timeout is 90 seconds.
`Instant` may exclude sleep on macOS; it is not evidence of sleep duration.

## Independent acceptance

Archive the executable/build receipt, invocation, complete log, report, wrapper
manifest, four PNGs (initial, restored, warmup, final), environment observations,
and the independent audit. Hash originals and retain failed attempts unchanged.
The previous offscreen creation-fault auditor explicitly rejects native evidence
and must not be used as a native validator.

The independent reader must reconstruct these checks rather than trusting
`valid:true`:

1. Both child and wrapper exited zero. No timeout, report error, missing capture,
   changed executable or manifest evidence error occurred. Embedded source/binary
   identity matches the frozen build; wrapper source identity may be newer only
   with an explicit source-to-binary receipt. The report requests Temporal, Claude,
   scale 0.5 and the native window target; no adaptive, timing, completion,
   interpolation or creation fault is enabled.
2. The native ledger is unpoisoned with zero dropped events, contiguous sequence
   numbers and valid timestamps. Exactly one `observation_armed` record matches
   the exercise, window, main frame and full arm payload in both the live marker
   and final lifecycle event. Do not compare the origins of independent `Instant`
   clocks. Sequence ordering establishes native event order; UTC supports external
   correlation.
3. Minimize: the arm precedes the `minimize_requested(true)` sequence identified
   by `after_sequence`. For that same window, both actual occluded=true and
   minimized=Some(true) occur after the request. The fixture's observed hold lasts
   at least 500 ms before its restore request. Both occluded=false and
   minimized=Some(false) occur after the restore request and remain the latest
   observed values through restoration. Requests alone, unknown native state and
   events from another window fail.
4. Sleep: `after_sequence` equals the arm sequence. A new
   `workspace_will_sleep` then `workspace_did_wake` pair occurs strictly after it,
   matching the pair named by `system_sleep_wake_observed`. There is no later
   unmatched sleep. Native observer installation succeeded. The externally
   recorded user action and system evidence agree; a long time gap is insufficient.
5. Native recovery requests a Temporal history reset. A subsequent observation
   acknowledges it, with a strictly later app frame than the reset request.
   Initial and restored phases each contain at least twenty distinct, consecutive
   eligible effect frames: fresh, single unchanged active view and window target,
   correct 640x360 content/1280x720 output, Temporal `OutputWritten`, no reason,
   scale 0.5. Restored frames follow the recovery phase boundary and have no pending
   reset. Diagnostic creation faults remain Off. Check retention counts, bounds
   and disclosed evictions; the per-phase ring is not a complete frame history.
6. Decode all four PNGs independently, check dimensions and full scene opacity,
   and require varied pixels beyond the UI. Review initial versus restored Claude
   and actual restored native window content for missing output or corruption.
   Captures alone do not prove physical panel delivery or GPU completion.

A passing run closes only its actual native lifecycle case. It does not reproduce
or resolve the historical MPSGraph crash, establish long-duration robustness,
prove complete GPU measurement, or validate adaptive performance.

## Harness review findings

The prior OS-sleep arm existed only in the eventual report and omitted its native
sequence boundary. The reviewed change records the arm in the native event ledger,
retains the exact boundary in the lifecycle event, and emits and flushes the same JSON
for operator coordination. It keeps post-arm native transition requirements intact.

The former generic 40/45-second smoke deadlines could preempt the native
60-second lifecycle deadline. Native modes now receive the corresponding
60-second readiness allowance; other smoke modes retain their existing deadlines.
The native sleep-inclusive deadline and bounded wrapper remain unchanged.

CPU validation covers pre-arm complete cycles, wake without a new sleep,
request-only and wrong-window observations, latest-state transitions and overflow.
These contract tests validate the observation guard, not hardware behavior.
