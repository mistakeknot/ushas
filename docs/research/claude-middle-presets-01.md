# Middle-preset refinement

**Half resolution remains the highest tested Temporal preset meeting the mean
60 FPS budget at load 8,000.** Neither 0.58 nor two thirds meets that budget.
This is a fixed-scale, serial offscreen result; half's pacing and image-quality
limits still apply. It is not evidence of an adaptive hardware trajectory.

All 20 runs passed their effect, completion, image and normal-shutdown gates.
The campaign ran September 5, 2026, 03:28:25–03:31:59 UTC using the same clean
`56a3b16c8c1c8f12a5320adc5082c6d20b6378c1` source and frozen executable as the
[initial completion campaign](claude-completion-campaign-01.md): SHA-256
`5c773aba90f70229d03f1ac87045045f11a6f2e435bb9bc21f55e1b0266b4744`.
It retained 1280×720, static Claude, LDR, load 8,000, four seconds of warmup and
six measured seconds, without experimental timestamps or Metal validation.

The [retained profile](../../tools/smoke/CAMPAIGN.md) declared native MSAA4,
Temporal 2/3 and 0.58, and matching bilinear controls before launch. Four
forward/reverse repetitions give every arm the same mean launch position.
The practical threshold remained 8%. For the absolute mean budget, the upper
pointwise 95% bootstrap bound must be at most 16.667 ms. Each process supplies
one mean; frames are not independent repetitions.

| Arm | Mean ms | Mean 95% interval ms | Mean run P95 ms | Mean run fraction over 16.667 ms |
|---|---:|---:|---:|---:|
| Native Disabled, MSAA4 | 30.454 | [28.382, 32.526] | 35.083 | 100% |
| Temporal 2/3 | 21.206 | [20.605, 22.062] | 25.721 | 95.78% |
| Temporal 0.58 | 18.705 | [18.514, 18.919] | 22.758 | 72.93% |
| Bilinear 2/3 | 20.484 | [19.500, 21.592] | 25.180 | 89.86% |
| Bilinear 0.58 | 18.246 | [18.020, 18.456] | 22.206 | 68.37% |

The [paired analysis](claude-middle-presets-01.json) and
[pacing report](claude-middle-presets-budget-01.json) retain the full data.
Temporal/native ratios are **0.6991 [0.6594, 0.7388]** at two thirds and
**0.6174 [0.5759, 0.6590]** at 0.58: both improve the serial interval
substantially, but neither reaches the declared budget. Temporal/matched-bilinear
ratios are **1.0369 [0.9992, 1.0746]** and **1.0253 [1.0169, 1.0356]**.
Neither establishes an 8% practical incremental cost; this compares the whole
Temporal path, including its prepasses, rather than an isolated kernel.

The earlier half-Temporal runs averaged 13.848 ms. A
[retrospective application](claude-half-budget-followup-01.json) of the same
mean-budget rule gives [13.232, 14.739] ms. This is not a predeclared paired
comparison between campaigns. Native means were similar across campaigns, but
that does not eliminate drift or interference. Half's earlier 8.90% miss
fraction still prevents a smooth-60-FPS claim. At load 20,000 even the tested
0.5 floor misses the mean budget; at zero load native already meets it.

The two new Temporal captures preserve Claude's facial strokes, rays and
output-resolution UI at the same static pose. Matched motion/cut review is a
separate quality gate. These measurements neither supply a GPU busy-cost signal
nor establish production pipelined FPS, input latency or presentation.

All 126 payload files (49,072,839 bytes) are hash-verified in
`/Users/sma/projects/docs/ushas/evidence/claude-middle-presets-01/`, including
the complete plan, reports, images, logs, manifests and exact analysis sources.
The archive manifest SHA-256 is
`6df0164cc02507791cf2a7cdcdb131cd73767403e0844c9f8d9210a1d1904622`.
The original artifacts and separately archived executable remain intact.
