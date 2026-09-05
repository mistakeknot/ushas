# Ushas Bench preview validation

Background rendering passed the packaged 24-arm comparison and configured
600-second stress run on Apple M5 Max. Chrome was observed frontmost during the
comparison while no Ushas window was onscreen. The launcher can stay minimized;
other applications still affect the measured throughput.

The [approved plan](../plans/2026-09-05-ushas-bench.md) and
[app guide](../../tools/benchmark/README.md) define the contract. Task:
`shadow-work-vzox.9`. Hardware qualification, native benchmark/results, live stress controls,
Stop/foreground Escape, and completed-report export passed. Browser policy
prevented an automated local-HTML preview; exported-file validation passed.

## Package provenance

`tools/benchmark/dist/preview-05/` contains the ad-hoc-signed macOS26 arm64 app
and ZIP, with the attributed procedural Claude and project license texts.
The launcher was built from clean `9ee5af024b0a0c034e7de70c62b4376c139abde6`.
Its status-label and local Escape fixes reuse the exact renderer built from clean
`b4fdabb676c0d446a13459e9e8a4001f8e411bbf`, which completed the hardware runs.
The reused renderer remains byte-identical after signing and ZIP extraction.

| Artifact | SHA-256 |
|---|---|
| `Ushas Bench.zip` | `4482ee500cf141b151f2526ecea39a9bc3757130e587498321b136864091ebdf` |
| `Contents/Helpers/ushas-bench` | `40e557604068f75acd14aa161655e0e0952f210f81512f31072bd5e75b0fc6f3` |
| `Contents/MacOS/UshasBench` | `ba52441b988707af7f6c4362405b61340fbb5b24b5830b4a7f5e33950304ea98` |

Deep/strict signatures, ZIP integrity, executable equality and resources passed.
The extracted app launched outside the repository from `/private/tmp` with
`PATH=/usr/bin:/bin:/usr/sbin:/sbin`, without Rust or external asset dependencies.
This is an ad-hoc-signed preview, not a notarized public release.

## Background qualification

Run `27d1fd1b-cbcd-4c53-a57f-1b0472a25064` ran from
`2026-09-05T22:43:29Z` to `22:56:09Z`. All 24 benchmark children and six separate
capture replays passed. The launcher accepted the final report and recorded
exit0. The profile is `claude-lab-offscreen-v1`, with a 2560×1440 image target,
normal Bevy pipelining, no scored readbacks and no per-frame GPU waits/callbacks.

A foreground-only desktop probe at `22:48:50Z` recorded `com.google.Chrome`,
zero Ushas windows onscreen, and active round-three progress. Other in-progress
observations recorded Rio or the system notification application; unrelated
window contents were not retained. Root minimized the launcher before these
observations. A separate native standard background run passed all 3,600 frames
at 423.5 geometric-mean completed-render FPS before the comparison.

The independent v3 audit checked 86,400 benchmark frame proofs, 21,600 replay
frame proofs, 144 decoded opaque RGBA8 PNGs, 24 cross-arm camera poses, exact
source/binary/configuration joins, balanced arm order and all paired statistics.
The comparison report SHA-256 is
`9489e449edb1f8f3809f5704b67c29d527f46dc92348332874d699eba5dcfb21`.

| Arm | Four-round geometric mean FPS | Render-time reduction vs native | Paired 95% interval | Performance decision |
|---|---:|---:|---:|---|
| Native MSAA4 | 214.7 | — | — | Baseline |
| Temporal native | 139.8 | −53.6% | −81.9% to −23.8% | Slower |
| Temporal two-thirds | 158.5 | −35.5% | −90.8% to −1.5% | Slower |
| Temporal half | 200.8 | −6.9% | −21.7% to +8.1% | No demonstrated practical benefit |
| Spatial half | 255.9 | +16.1% | +7.85% to +27.2% | Lower bound misses the >8% gate |
| Bilinear half | 252.6 | +15.0% | +12.4% to +17.5% | Performance gate passed |

These are observations with other desktop applications active and only four
pairs per candidate. They do not isolate GPU capacity or establish a MetalFX
performance win. Retain native as the default. Bilinear's timing result does
not establish acceptable image quality.

The original v2 image audit failure is preserved. Parent JSON re-serialization
changed copied camera/jitter values by one f64 ULP while preserving their f32
bits. V3 permits exactly one adjacent f64 step only on the three known f32 array
paths, with identical f32 bits; all other fields remain exact. Six regression
cases and an independent source review passed. No renderer or report was
modified to satisfy the audit.

Root visually inspected native material, geometry and lighting images and a
matched Temporal two-thirds camera-cut image. Faces and scene composition were
present and matched; thin-edge aliasing remains visible at the cut. The app
loaded native/temporal-half selectors from retained files while stress continued.
This spot inspection does not claim quality equivalence or smooth live motion.

## Ten-minute stress

Run `33456ba1-ac1d-400e-a791-be4741161492` used background Temporal half at
2560×1440, starting with 64 Claudes, eight lights, 4,096 particles and no extra
pixel load. It completed automatically, `valid:true`, `stopped:false`, no
errors, `profile_version:custom`, and no aggregate benchmark score. The launcher
recorded exit0; no renderer process remained afterward. Results displayed
Completed / CUSTOM / VALID / BACKGROUND.

All 574 retained checkpoint summaries passed, covering 68,479 completed frames.
The last eight detailed cohorts supplied 843 fresh frame/proof/fence joins;
earlier details are intentionally bounded. No summaries were evicted. Applied
configuration generations were 64→65→129→128 Claudes, ending at 12 lights and
8,192 particles. Intermediate loads remain in the report.

The intended change began around 117 seconds of reported progress. Direct
accessibility slider value-setting was unsupported; actual slider actions
applied the final generation around 293 seconds and verified values around
310 seconds. This differs from the planned two-minute change, while still
exercising live configuration transitions and several minutes at the larger load.

Report UTC lifetime was 601 seconds; the monotonic log observer measured
602.436 seconds and saw engine progress through 599.966 seconds. The fixed
renderer source's valid, unstopped completion supports the configured 600-second
run. Arrival times do not independently identify exact admission/stop instants,
and checkpoint durations were never summed to infer total duration.

The stress report SHA-256 is
`fe322c8b834eb1f040898ab1a59dec0dd0f00158774f88fd4e50d76111a683ea`.
The original stress audit rejected the stdout-only envelope fields; its failed
receipt remains preserved. Stress-auditor v3 validates every envelope before descent and joins every
observed checkpoint exactly to the final report. Its 33 corruption checks and
independent review passed; it retains the duration-evidence limitation above.

## Completed export and launcher checks

The final preview loaded native and Temporal-half Materials·908 in Results.
Changing the divider revealed each image, including the matching native and
temporal HUD labels. No renderer process was running during these interactions.
The app displayed “Offline report exported.” after its real Export action.

The export at `~/Documents/Ushas-qualification-background-preview-04` contains
144 byte-identical PNGs, 32 byte-identical original JSON files and contained
relative artifact links. All 267 copied source files checked out; `index.html`
is intentionally regenerated by the exporter. Its 576 image references resolve
to the 144 unique captures and use no remote resources. A durable copy is in
`tools/benchmark/dist/preview-05/validation/comparison-export/`. Automated browser
preview of the local HTML was blocked by browser URL policy; file integrity and
resource containment were verified without bypassing that policy.

The actual Stop button preserved run `e31e60de-9296-4a2f-96e0-1868dba145bf`,
and ⌘. stopped `bc8bab63-5851-4be9-a3db-b96ad0cefe31`. The original menu Escape
equivalent did not invoke Stop. The final launcher handles Escape with a local
app event monitor, guarded by active background-run state and modal/menu checks.
Actual Raise→Escape then stopped run `858e2d14-33df-41a3-8cb7-4da17d564c06`,
preserved its unqualified report and reaped the helper. No global keyboard
monitor is installed.

The final preview also completed native benchmark
`7d00f39a-abdc-4278-b054-e14e871e1d45` through the actual launcher. All 3,600
frames passed the independent check. Results displayed 471.0 completed-render
FPS, with Materials 562.0, Geometry 497.3 and Lighting 373.9. This
launcher check is retained separately from the four-round comparison above.

## Retained failures and checks

The earlier windowed comparison `6049d5bf-9a4a-4926-9fc8-5e2121590160` remains
invalid/stopped. Its first round completed, then later arms lost qualified view
output with Chrome frontmost. Numeric paired conclusions remain withheld.
The earlier cancelled comparison `e7987218-56e3-4585-b2d8-841d09e19f00` and its
31-file export also remain separate integration evidence. Neither is silently
replaced by the successful background campaign.

The native progress label initially stayed at “Warming” after rendering began.
The launcher-only fix advances it on measured progress while preserving
comparison, error and Stop messages. Root observed “Stress running” in the final
app. All 27 Swift tests, strict formatting and launcher build passed, including
local Escape consumption and inactive/modal/modified-key passthrough checks. Renderer checks covered 59 Rust tests, strict Clippy and
rustfmt. All 20 jobs in [renderer CI run33996152702](https://github.com/mistakeknot/ushas/actions/runs/33996152702)
passed. All 20 jobs in [final launcher CI run33999129521](https://github.com/mistakeknot/ushas/actions/runs/33999129521)
passed for packaged launcher revision `9ee5af0`.

Original app runs and logs remain under
`~/Library/Application Support/Ushas Bench/Runs/`. Auditor sources, successful
and failed receipts, foreground observations, observer data and test logs are
retained in `tools/benchmark/dist/preview-05/validation/`.

## Interpretation and bounded timing assessment

Background scores measure normal-pipelined completed offscreen-render throughput,
including CPU/render scheduling and completion-callback dispatch. They exclude
surface acquisition and scored image readback. Windowed scores use the separate
`claude-lab-standard-v1` profile and must not be combined with background scores.
Neither profile establishes GPU-busy time, frame pacing or panel FPS.

The bounded public-API assessment did not identify a complete GPU-only frame-cost
producer. The dedicated MetalFX command-buffer interval includes dependencies;
selected public wgpu pass timestamps do not cover MetalFX's private encoders.
The earlier [proxy stop decision](proxy-validation-control-03.md) remains in
force. No new trace pair was collected: missing scope already prevents the
proposed runtime signal from qualifying. Automatic adaptation retains
`TimingUnavailable`.
