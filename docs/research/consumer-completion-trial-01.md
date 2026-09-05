# Frozen consumer completed-render trial

**Retain native resolution for this static globe fixture.** All twelve runs
passed, all four native means fit the declared 60 FPS budget, and neither
half-resolution candidate demonstrated the predeclared 8% practical improvement.
This is a bounded consumer result, not a production FPS recommendation.

The balanced trial ran September 5, 2026, 03:22:14–03:28:09 UTC, using Shadow
Work `2a49dfcb294a69283e9e4cf9aa0662b61c51495a` with Ushas
`56a3b16c8c1c8f12a5320adc5082c6d20b6378c1`. The frozen executable SHA-256 is
`e1eaf124a3d01637a115af46bb9f02049c4ad093adb3596d5af49416df31a126`.
The fixture holds the existing globe camera at PRE_CUT, renders to a 1600×900
image, and disables the webview overlay. Each run requires loaded textures,
three seconds of warmup, twenty distinct ready observations and an opaque
warmup image before at least six measured seconds. Two forward/reverse order
pairs give each arm the same mean launch position over four process runs.

## Results

These are serial callback-observed completed-render intervals, including CPU
preparation, scheduling, logging and polling. One render frame completes before
the next admission. The first interval starts at its admission; subsequent
intervals start at the preceding callback, so their sum spans the measured
epoch, including CPU gaps. Screenshots are requested only in the surrounding
warmup and drained final epochs.

| Arm | Mean ms | Mean run P95 ms | Mean run P99 ms | Mean run fraction over 16.667 ms |
|---|---:|---:|---:|---:|
| Native Disabled, MSAA4 | 6.497 | 7.799 | 8.605 | 0.00% |
| Half Temporal, MSAA off | 7.849 | 9.924 | 12.248 | 0.29% |
| Half bilinear, MSAA off | 6.781 | 9.140 | 12.437 | 0.81% |

Each cell equally weights four run-level values; tail columns average the four
nearest-rank quantiles and miss fractions. Frames are not independent
repetitions. All twelve run means meet the budget. The
[full analysis](consumer-completion-trial-01.json) retains every run's counts,
duration, mean, tails and misses.

The four paired Temporal/native mean-interval ratios average **1.2107**, with
pointwise bootstrap 95% interval **[1.0489, 1.4338]**. Temporal was slower in
each pair, but the lower bound does not exceed the declared 1.08 practical
regression threshold. The formal practical classification remains uncertain;
it is neither an improvement nor evidence of equivalence. Bilinear/native is
**1.0439 [0.9755, 1.1469]** and Temporal/bilinear is
**1.1656 [1.0153, 1.4150]**, also uncertain at the 8% threshold. These use
10,000 bootstrap draws with seed 21434 and only four paired runs.

Native already leaves substantial mean-budget headroom in this fixture.
Reducing resolution has no demonstrated practical timing benefit here, so the
evidence does not support enabling Temporal by default for this consumer pose.
It does not contradict the benefit in the separately loaded
[Claude fixture](claude-completion-campaign-01.md).

## Images and scope

One final capture from each arm was inspected at its original dimensions:
native `native-xbac967x`, Temporal `temporal-cvp4qk6n`, and bilinear
`bilinear-kv4dl4nh` in the archive. All show the same complete globe pose.
Temporal half retains substantially more coastline and terrain detail than
bilinear half; bilinear is visibly softer and its globe silhouette is more
stepped. These static endpoints do not establish motion, camera-cut recovery
or continuous temporal stability. The matched cut-image gate remains pending.
Native changes AA policy relative to Temporal; the two half-resolution arms
share dimensions and MSAA off, but their difference includes the whole Temporal
path and prepasses, not an isolated MetalFX kernel.

The frozen consumer retains its legacy GPU timing sink; native logs explicitly
report an empty timed command buffer. This analysis uses the completed-render
ledger, not that partial GPU timer. These numbers are not GPU busy cost,
production pipelined FPS, input latency or physical presentation evidence, and
cannot feed the validated GPU-budget governor. The runner fixes the overlay
and completion-mode environment variables, but does not retain the inherited
environment, OS or thermal conditions per run. No interference-free claim is
made.

## Evidence verification

The independent audit checked the plan and all twelve manifests, all 72
manifest-listed run artifacts, all three epoch counters per run, original
frame/view/effect identities and serial drain boundaries. Every measured
interval and run metric was independently recalculated and matched the
retained analysis. All 24 PNGs were decoded: each is 1600×900 RGBA, fully
opaque, and matches its recorded visible-pixel and sampled-color counts.

The entire trial tree and preparation/check/build receipts, patches, exact
runner/analysis/bridge/completion sources and audit script are retained in
`/Users/sma/projects/docs/ushas/evidence/consumer-completion-trial-01/`.
All 101 payloads (52,352,844 bytes) matched their source hashes before and after
copying; originals were unchanged. `archive-manifest.json` SHA-256 is
`aafd5c6b43b2e800cf3e854a45083e53db2858c9f16c8ea9172afc66fda5839c`.
The separately archived executable under `frozen-binaries-01/` was also
rehash-verified. Large source archives and extracted trees were not duplicated
or rehashed by this audit; their committed revisions and recorded hashes remain
in the retained preparation and build provenance.
