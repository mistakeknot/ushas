# Current-consumer camera-cut samples

Retain native for this frozen consumer fixture. All three camera-cut runs exited
successfully and produced six opaque images each. Temporal half preserves more
of the settled globe's detail than bilinear half, but these samples do not
establish a clean temporal-recovery quality pass: a pronounced zig-zag texture
seam is also present in the native post-cut baseline, and the legacy probe
records screenshot request frames without joining them to rendered frames.

The [machine-readable report](consumer-cuts-01.json) records the full image
inventory, request identities, observations and reset log events. The immutable
source is Shadow Work `2a49dfcb294a69283e9e4cf9aa0662b61c51495a`, patched only in a
private copy against Ushas `56a3b16c8c1c8f12a5320adc5082c6d20b6378c1`. The binary
SHA256 is `e1eaf124a3d01637a115af46bb9f02049c4ad093adb3596d5af49416df31a126`.

| Arm | Content → output | MSAA | Exit / report | Captures |
|---|---|---:|---|---:|
| Native | 1600×900 → 1600×900 | 4 | 0 / valid | 6 |
| Temporal half | 800×450 → 1600×900 | Off | 0 / valid | 6 |
| Bilinear half | 800×450 → 1600×900 | Off | 0 / valid | 6 |

Each run held the same initial orbit `(yaw=0, pitch=0.3, distance=25)`, then made
a half-turn yaw cut at script counter 30. Capture requests occurred at counters
29, 30, 31, 32, 34 and 38 (`before`, `p0`, `p1`, `p2`, `p4`, `p8`). The script
waited for at least three seconds and twenty distinct ready observations before
advancing. All recorded script-counter rows 1–42 were ready in each arm.
Startup pending observations remain in the raw logs and were not discarded.

I inspected `before`, `p0`, `p1` and `p8` for every arm. Before-cut images show the
same Africa/Europe-facing pose. Native retains the finest terrain and coastline
detail; Temporal is closer to it than the softer, visibly stepped bilinear
control. All inspected post-cut images show the opposite hemisphere with the
same prominent central seam. That seam already exists with MetalFX disabled at
native resolution; its cause was not diagnosed here. Native's five post-cut PNGs
are byte-identical, as are bilinear's. Temporal's five differ: its `p0` outline
is coarser than `p8`. No obvious retained bright Africa image is visible in the
inspected Temporal post-cut samples. This is a limited sampled observation,
not a measured recovery-frame count or continuous-motion assessment.

Temporal logs one reset request at script counter 30 and one successful reset
encoding. The encoding message's “temporal frame 1498” is the scaler-local
`state.frame_count`, **not** the public effect observation frame. Capture reports
retain main-frame request IDs; the contemporaneous status observations are two
frames older. Consequently `p0` is a request label, not proof that the PNG is the
exact reset-encoded render frame. There is no reset-disabled comparison here.
Native and bilinear print the legacy “FAIL — MetalFxHistoryReset absent” message;
Disabled mode has no temporal history resource, so those messages do not negate
their successful rendering/readback checks.

These image-only runs add no timing estimate. The separate
[four-pair completed-render trial](consumer-completion-trial-01.json) found native
already within the declared 60 FPS mean budget, with neither candidate showing
the required 8% improvement. Temporal/native paired interval ratio averaged
1.211, with a pointwise bootstrap 95% interval of 1.049–1.434. Those are serial
completed-render intervals, including CPU scheduling and polling, not GPU busy
time or production pipelined FPS. The cut samples do not overturn that trial's
recommendation to retain native. The webview overlay was disabled; no UI, HDR,
physical presentation, or exact reset-recovery acceptance is claimed.

All 18 PNGs were independently decoded as 1600×900 RGBA with every alpha byte
255. All 30 original per-run artifact hashes were checked; the full three run
directories were copied with source-before/source-after/destination SHA256
agreement. The archive contains 33 payload files (15,807,014 bytes) plus its
[verified manifest](/Users/sma/projects/docs/ushas/evidence/consumer-cuts-01/archive-manifest.json).
Prepared-source, patch, check and build receipts are linked to the existing
[pilot provenance archive](/Users/sma/projects/docs/ushas/evidence/consumer-completion-pilots-01/archive-manifest.json),
whose referenced hashes and current binary identity were rechecked. No source,
runner, live consumer worktree or GPU workload was changed during this audit.
