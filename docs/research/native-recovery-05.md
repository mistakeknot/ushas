# Native minimize and window inspection accepted; sleep cycle still missing

Four fresh native minimize/restore runs independently pass their native event,
Temporal reset and captured-pixel checks. Run 04 also passes bounded native-window
inspection: the intended process accepted the OS frontmost action, and an actual
desktop capture restricted to its stable window bounds shows the complete Claude
scene. The OS sleep attempt did not observe a sleep/wake cycle, so that acceptance
gate remains open.

All five renderer attempts used the same frozen release executable from clean
source `4e3b115d8427cb4b8e7c9025cef993f98c6d2c30`, SHA256
`74d8401f4b80ef614249a5351d687acf1cd4bb9e6b32551be5f508265d53aa82`.
The independently verified source/build archive remains in
`native-recovery-04/build/`; its 139 source blobs match that revision. The wrappers
ran from clean `8b68098d52154a2cc3fb02faaa8fc84854ce9faf`, whose differences are
research and plan documents. No renderer input changed.

## Four actual minimize/restore cycles

Each run used the native 1280x720 Claude window, Temporal at scale 0.5, followed
by a 40-second measurement tail for UI inspection. This tail establishes no
performance result. Adaptive, experimental timing, completion, interpolation and
creation faults were inactive. Every child and smoke wrapper exited zero.

| Run | Initial / restored distinct eligible frames | Reset request / acknowledgement app frames | Observed minimized hold |
|---|---|---|---|
| 01 | 23 / 24 | 394 / 396 | 0.501059 seconds |
| 02 | 23 / 24 | 368 / 370 | 0.500534 seconds |
| 03 | 23 / 22 | 314 / 316 | 0.501696 seconds |
| 04 | 23 / 23 | 388 / 390 | 0.500784 seconds |

Each ledger contains the native arm at sequence 3, minimize request at 4, actual
occluded/minimized observations at 5/6, restore request at 7 and actual
unoccluded/non-minimized observations at 8/9. Window identity stays unchanged.
The live marker matches the retained arm boundary, acknowledgement follows the
recovery reset request, and initial/restored output has the required geometry,
Temporal state and freshness. No native event was lost and no phase observation
was evicted. These ledgers cover the bounded recovery phases, not continuous
native visibility throughout the later measurement tails.

All 16 fixture PNGs independently decode at 1280x720 with complete opacity and
varied scene pixels. They establish captured content; the separate native image
below establishes that the restored window was reachable and visibly contained
the scene.

## Run 04 native-window evidence

The helper selected the unique onscreen `ushas-smoke` window: PID 23883, native
window 3211, layer 3, bounds 640x392 points. It waited for the encoded recovery
reset, successfully issued the System Events frontmost action, checked the same
window and bounds, and captured only that desktop rectangle. The retained
before/after inventory is unchanged. Capture succeeded at 18:06:35.036809 UTC,
within the live run and after the encoded reset message.

The original native-window PNG has SHA256
`4756310a700bd9469251ee68ac4255e437fb8fcfce785afb54b9c943395bdc6a`.
It decodes to 1280x784: a 64-pixel titlebar plus the full opaque 1280x720 scene.
An independent reviewer inspected that original image and confirmed the Ushas
titlebar, all three Claude figures and readable faces, radial head shapes,
clothing and tails, rails, checkerboard and header. No missing output or obvious
corruption was visible.

This closes the bounded native-window inspection case. The evidence is an
acknowledged frontmost action and actual window content, not a queried key-window
focus state, continuous focus measurement, motion-quality assessment or physical
panel-delivery proof.

## Earlier UI attempts remain failed

Run 01's helper excluded the AlwaysOnTop layer-3 fixture by requiring layer 0.
Run 02's helper included auxiliary windows and failed its uniqueness check. A
manual retry 45.344 seconds after that wrapper finished could no longer reach the
process; its focus and window-id capture both failed.

Run 03 successfully issued the frontmost action, but window-id capture returned
`could not create image from window`. Its helper process exited zero despite the
capture command exiting one; the missing native image prevents UI acceptance for
that run. All helper logs, inventories and failed receipts remain preserved.

Screen-capture access preflight was true. Run 03 reported window sharing state 0;
run 04 reported sharing state 1 and also changed from window-id to rectangle
capture. Those two changes prevent attributing success to the capture method
alone. Renderer source, executable and native recovery criteria stayed unchanged.

## OS sleep acceptance remains incomplete

The separate `os-sleep-resume-01` attempt armed at native sequence 3, app frame 28,
with the NSWorkspace observer available and no event loss. Its retained ledger
contains no will-sleep or did-wake notification. The 60.002593-second native
deadline expired in the changed phase; child and wrapper exited one. No recovery
reset or restored-phase capture followed.

Its initial, later warmup and final images independently decode, but those three
ordinary captures cannot replace a native sleep/wake pair or restored-phase
evidence. This trial did not observe an actual system sleep cycle during its
armed interval. It is not a measured failure to recover after sleep.

The four minimize sessions retained 82, 83, 82 and 81 display samples. All sampled
awake and on-console; every lock value was unknown, not false. User readiness and
the successful native-window interaction are separate evidence. Neither unknown
lock metadata nor the display samples alone prove an unlocked session.

The historical `shadow-work-k922` MPSGraph SIGSEGV and locked-session
scaler-creation hang remain unresolved. Complete GPU-cost measurement, live
adaptive acceptance and long-duration reliability are also outside this campaign.

## Independent evidence

The [machine-readable report](native-recovery-05.json) contains per-run audits,
the UI review, preserved helper failures and the rejected sleep attempt. Originals
and independent receipts are retained under
`/Users/sma/projects/docs/ushas/evidence/native-recovery-05/`.
The archive manifest covers 87 payload files (21,356,882 bytes), SHA256
`d8f17cb7f889df11c2c025fddd3f8ab727a508b9785e5591dd3f5216315fa0b3`.
The root integrator reproduced all four independent fixture audits and the
14 CPU tests, confirmed that the actual sleep report remains rejected, and
separately inspected the native-window image.

```sh
python3 /Users/sma/projects/docs/ushas/evidence/native-recovery-05/audit.py \
  --report /Users/sma/projects/docs/ushas/evidence/native-recovery-05/window-minimize-04.json
```

The generalized auditor passed 14 CPU protocol tests, including synthetic
validator fixtures for sleep ordering. Those fixtures are explicitly test data,
not hardware sleep evidence. The auditor rejects the actual unsuccessful sleep
report; no acceptance guard was relaxed.
