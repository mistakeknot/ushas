# Native minimize recovery: one bounded observed cycle

Run 01 independently passes the native event, Temporal reset and captured-pixel
checks for one actual minimize/restore cycle. Interactive window acceptance
remains incomplete, and OS sleep/wake has not run. This is narrower than full
native lifecycle acceptance.

Both attempts used the retained release executable built from clean source
`4e3b115d8427cb4b8e7c9025cef993f98c6d2c30`, SHA256
`74d8401f4b80ef614249a5351d687acf1cd4bb9e6b32551be5f508265d53aa82`.
The independent auditor matched all 139 archived source blobs against that
revision, verified build/embedded/manifest binary identity and all capture hashes.
The wrapper ran from clean `cbbc7f54153e370fda5313a95a583a0504cfd973`; its only
difference from the build revision was clarification of the native acceptance
plan. No executable input changed between the two revisions.

## Run 01: observed native cycle and fresh output

The run used the native 1280x720 Claude fixture, Temporal at scale 0.5, with a
20-second measurement tail. Adaptive, experimental timing, completion,
interpolation and creation-fault injection were inactive. Child and wrapper
exited zero without evidence errors.

| Independent observation | Result |
|---|---|
| Native arm / minimize request sequences | 3 / 4 |
| Actual occlusion / native minimized=true sequences | 5 / 6 |
| Restore request sequence | 7 |
| Actual unocclusion / native minimized=false sequences | 8 / 9 |
| Observed minimized hold | 0.500519 seconds |
| Recovery reset request / acknowledgement app frames | 358 / 360 |
| Distinct consecutive eligible initial / restored frames | 23 / 23 |
| Native event loss / observation evictions | 0 / 0 |
| Native lifecycle duration through completion | 5.865857 seconds |

The live arm JSON exactly matches the final report's native arm and sequence
boundary. Both enter and exit observations follow their respective requests and
identify the same native window. The restored phase acknowledges the requested
Temporal reset and contains fresh `OutputWritten` observations at the original
640x360 content and 1280x720 output sizes. There is one unchanged active view;
diagnostic creation faults remain Off.

The native ledger snapshot ends when lifecycle recovery completes. It does not
establish continuous native visibility throughout the subsequent measurement
tail. The retained observation ring contains 25 initial, four changed and 24
restored rows; unchanged states can be coalesced. These rows are observations,
not a complete per-frame history or GPU-completion record.

All four original PNGs—initial, restored, warmup and final—were independently
decoded at 1280x720 with every pixel opaque. Their sampled scene regions contain
9,017, 9,022, 8,790 and 7,681 unique RGB colors respectively. Visual inspection
retains three Claude characters, readable faces and radial head shapes, clothing,
tails, thin poles, checkerboard and header. No missing output, gross corruption
or obvious restored-state regression was observed. These are static captured
pixels, not a motion-quality or physical-panel finding.

## Run 02: failed before native arm

A separately recorded protocol extended the measurement tail from 20 to 40
seconds to allow direct native-window inspection. The binary and acceptance
criteria were unchanged. This attempt never emitted an arm marker, logged a
Temporal scaler-creation attempt, or retained a report or capture. The outer
90-second watchdog terminated the child with signal 15; wrapper exit was 124.
The failed manifest and log remain preserved.

The retained file named `display-midrun-02.json` was actually sampled at
16:21:19.397684 UTC, **23.657946 seconds after** the watchdog finished at
16:20:55.739738 UTC. It reports awake, on-console and locked=true. Its filename
does not make it a during-run observation. It cannot establish that locking
caused the earlier stall or that the session was locked during it; the cause
remains undetermined.

## Remaining acceptance and reproduction scope

The earlier preflight recorded awake and on-console state but unknown lock state.
It does not establish an unlocked interactive session throughout run 01. The
integrator reports that CUA could not select the standalone executable during
the first run. During run 02 the fallback capture helper timed out waiting for
the encoded recovery-reset log, so it did not capture or focus the window. These
UI-attempt facts come from the integrator's tool outcomes; no successful native
window screenshot or interaction artifact exists in this archive.

Consequently, interactive window acceptance remains false. An externally
initiated OS sleep/wake cycle is still pending. Neither this successful bounded
minimize cycle nor the failed second attempt reproduces or resolves the
historical `shadow-work-k922` MPSGraph SIGSEGV or locked-session scaler-creation
hang. Full GPU measurement, adaptive behavior, physical panel delivery and
long-duration reliability are outside this result.

## Evidence and replay

The [machine-readable report](native-recovery-04.json) links the source, protocol,
decoded images and failed-attempt findings. The retained originals and independent
auditor are under
`/Users/sma/projects/docs/ushas/evidence/native-recovery-04/`.
Root reproduced the audit and all seven tests. The complete
[archive manifest](/Users/sma/projects/docs/ushas/evidence/native-recovery-04/archive-manifest.json)
covers 25 payloads, 106,644,957 bytes; SHA256
`7054236ffd0d9fddec52beef70ed6036346381cfd8979608656c147c74a89e63`.
It includes the failed attempt, helper outcome and root visual review, with
no successful native-window inspection artifact or OS sleep run.

```sh
python3 /Users/sma/projects/docs/ushas/evidence/native-recovery-04/audit.py
python3 /Users/sma/projects/docs/ushas/evidence/native-recovery-04/audit.py --self-test
```

All seven auditor tests pass, including rejection of a mismatched arm boundary,
unknown native minimized state, wrong-window transition, early reset
acknowledgement, duplicate effect frames and event overflow. The replay audits
retained evidence with CPU tools; it performs no GPU or OS lifecycle action.
