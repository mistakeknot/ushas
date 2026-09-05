# Balanced Claude completed-render campaign

**All 60 runs passed**, including normal process shutdown, opaque captures,
matching effect/frame/view evidence and drained measurement boundaries. This
establishes serial completed-render cadence for the frozen offscreen fixture.
It does not repair the original unpaced scheduling mode or establish GPU busy
cost, production pipelined FPS, input latency or presentation.

## Frozen experiment

The campaign ran September 5, 2026, 03:11:15–03:21:56 UTC on Apple M5 Max,
macOS 26.5.2, Rust 1.97.1. Clean source was
`56a3b16c8c1c8f12a5320adc5082c6d20b6378c1`; executable SHA-256 was
`5c773aba90f70229d03f1ac87045045f11a6f2e435bb9bc21f55e1b0266b4744`.
The scene was static `claude-toy-v1`, LDR, 1280×720, with four seconds of warmup
and six seconds of measurement. Metal validation and experimental timestamps
were disabled. Each admitted render frame completed before the next admission.
No second GPU job or heavy Cargo build was launched by this task during the
campaign; small CPU-only reviews and tooling continued. This does not establish
an interference-free or thermally identical machine.

The [predeclared protocol](../../tools/smoke/CAMPAIGN.md) used five arms, three
fragment loads and four repetitions, with balanced forward/reverse arm and load
orders. Each arm/load had the same mean global launch position. Every result
retains the clean compiled source identity separately from the changing invoking
checkout. The [paired analysis](claude-completion-campaign-01.json) and
[budget report](claude-completion-budget-01.json) revalidate the complete evidence.

## Results

Times below equally weight four process runs. Miss fractions likewise average
four run-level fractions of completed-render intervals exceeding 16.67 ms.
The first interval starts at the first admission; later intervals start at the
previous callback. Their sum includes CPU preparation, scheduling and polling
gaps and exactly spans the measured epoch.

| Arm | Load 0 mean ms | Load 8,000 mean ms | Load 20,000 mean ms | Load 8,000 misses | Load 20,000 misses |
|---|---:|---:|---:|---:|---:|
| Native Disabled, MSAA4 | 6.549 | 30.486 | 69.870 | 100% | 100% |
| Native Temporal | 6.946 | 32.724 | 73.312 | 100% | 100% |
| Half Temporal | 6.839 | 13.848 | 27.422 | 8.90% | 100% |
| Third Temporal | 6.887 | 11.177 | 16.878 | 0.90% | 56.25% |
| Half bilinear | 6.468 | 12.892 | 26.423 | 2.14% | 100% |

At load 8,000, the four paired half-Temporal/native interval ratios average
**0.4565**, with pointwise bootstrap 95% interval **[0.4304, 0.4859]**.
At load 20,000 the ratio is **0.3946 [0.3722, 0.4183]**. Both clear the
predeclared 8% practical improvement threshold. Third Temporal and half bilinear
also clear that threshold at both loaded settings. Neither native Temporal nor
any zero-load candidate clears it. These intervals use four independent
process pairs and 10,000 seeded bootstrap draws, not individual frames as
independent repetitions; their uncertainty resolution is limited.

At the middle load, half Temporal's 13.848 ms mean corresponds to 72.21 serial
completed renders per second. Its mean of per-run 95th-percentile intervals is
16.738 ms and its miss fraction is 8.90%; passing the average budget does not
establish smooth 60 FPS pacing. The unmeasured intermediate presets can change
the highest-quality choice, so a focused load-8,000 refinement follows.
That [20-run refinement](claude-middle-presets-01.md) also passed: neither
0.58 nor two thirds met the mean budget, leaving half as the highest tested
Temporal preset that did.
One half-Temporal repetition was slower: 15.241 ms with 26.65% misses, versus
13.232–13.688 ms and 2.20–3.87% misses in the other three. The balanced design
retains this variability rather than selecting the best run.

At the heaviest load, the lowest rung permitted by the default 0.5 quality floor
fails the mean budget. Third Temporal, which requires an explicit lower floor,
also averages below 60 and misses more than half its intervals. No tested rung
at this load supports a 60 FPS recommendation. At zero load, retain the native
quality baseline: it already meets the budget without a practical measured
benefit from reducing resolution.

## Quality and scope

Native MSAA4, half Temporal and third Temporal captures show the same static
Claude poses and native-resolution UI. Half retains clean facial strokes and
silhouettes; third shows finer edge breakup on rays and the background. This
agrees with the earlier [motion/HDR assessment](claude-quality.md), while neither
set proves continuous temporal consistency. Matched motion and cut sequences
remain a separate gate. Native changes AA policy relative to Temporal; bilinear
and Temporal half both use MSAA off and the same input dimensions.

The successful heavy runs replace the missing comparisons from the
[first CPU-loop campaign](claude-campaign-01.md) only for this serial mode.
That earlier campaign remains 53/60 valid, with seven shutdown failures.
Do not discard those failures or describe serial completion as a fix for the
default unpaced queue. This CPU-inclusive metric also cannot drive the
GPU-budget governor's validated-signal input.

## Retained evidence

All 366 payload files, totaling 160,379,433 bytes, are hash-verified at
`/Users/sma/projects/docs/ushas/evidence/claude-completion-campaign-01/`.
They include every run, captures, logs, manifests, plan, analyses and the exact
analysis/runner sources. `archive-manifest.json` has SHA-256
`b406124c30374fa989ae717329e5d5a2a335b3c62a3d6045687bb7500cbe8f7f`.
The exact executable is separately retained under
`/Users/sma/projects/docs/ushas/evidence/frozen-binaries-01/`.
Original temporary artifacts remain intact.
