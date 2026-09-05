# First balanced Claude campaign

**53 of 60 runs passed the complete execution gate.** All runs produced valid
image reports, but seven heavy-load processes then panicked during GPU queue
shutdown. The campaign correctly returned nonzero and withholds every
highest-load comparison. A valid PNG or an application-written `valid: true`
report cannot override a failed process exit.

This campaign measured CPU application-loop cadence. Its results motivate a
bounded GPU-completion measurement; they do not establish GPU execution cost,
completed-frame throughput, presented FPS, or an adaptive budget verdict.

## Frozen experiment

The run took place September 5, 2026, 02:18:41–02:35:37 UTC on the Apple M5 Max,
macOS 26.5.2, Rust 1.97.1. It used the clean source revision
`9e53efea7bd2f550e9aac0669c14616bc32618e6` and executable SHA-256
`6430b1db65cc58fc68125397b6756c0d5d725b8267d91137cf13e85735ce604e`.
The source/scene/device/compiler identity remained identical across runs. The
invoking checkout changed as independent development continued; the binary and
runner stayed frozen, and each checkout snapshot was retained separately.

The default [campaign protocol](../../tools/smoke/CAMPAIGN.md) supplied five
arms, three fragment loads, and four balanced repetitions at 1280×720. Claude
geometry was `claude-toy-v1`, static, LDR, offscreen, with four seconds of warmup
and six seconds of measurement after readiness. Experimental marker passes and
Metal validation were disabled. This task launched no second GPU job or heavy
Cargo build during the campaign; small CPU-only development commands continued. This does
not establish an interference-free or thermally identical machine.

## Valid lower-load observations

Each cell below is an equally weighted mean of four complete runs. Misses refer
only to **CPU-loop intervals exceeding 16.67 ms**. They are not GPU or displayed
frame misses.

| Arm | Load 0: mean loop ms | Load 8,000: mean loop ms | Load 8,000: CPU-loop misses |
|---|---:|---:|---:|
| Native Disabled, MSAA4 | 1.679 | 30.896 | 99.8% |
| Native Temporal | 2.304 | 32.533 | 99.9% |
| Half Temporal | 2.697 | 11.883 | 1.0% |
| Third Temporal | 2.557 | 5.677 | 0.1% |
| Half bilinear | 1.860 | 10.220 | 0.2% |

At load 8,000, the four paired half-Temporal/native CPU-loop-time ratios average
**0.388**, with pointwise bootstrap 95% interval **[0.354, 0.428]**. Third
Temporal gives **0.186 [0.175, 0.195]**; half bilinear gives
**0.332 [0.321, 0.344]**. These pass the predeclared 8% practical threshold for
this CPU-cadence metric. Native Temporal gives **1.050 [1.025, 1.065]**, below
that practical threshold. At zero load, all Temporal arms have slower CPU-loop
cadence than native MSAA4. The full paired run ratios and intervals are in the
[machine-readable analysis](claude-campaign-01.json).

The unpaced loop can retain GPU work beyond the measurement window, so those
ratios must not be turned into GPU FPS or latency. The native comparison also
changes AA policy; bilinear and Temporal half both use MSAA off. The
[image assessment](claude-quality.md) separately favors retaining the 0.5
default quality floor and leaving one third explicit.

## Heavy-load failure

At load 20,000, all four native-MSAA4 runs and three of four native-Temporal
runs exited 101 after this wgpu-core 29.0.4 shutdown panic:

```text
We timed out while waiting on the last successful submission to complete!
```

The affected IDs are `013`, `014`, `015`, `016`, `037`, `038`, and `052` in the
retained campaign. Every reduced-scale arm exited successfully, but no complete
native comparison remains. Selecting only those surviving arms would hide the
failure, so the analysis produces no high-load ratio or confidence interval.
This is a queue-completion/teardown observation, distinct from the historical
unreproduced MPSGraph cold-start SIGSEGV.

The follow-up is an explicit one-frame-in-flight offscreen mode with bounded
completion and drained measurement boundaries. Its metric must include CPU
scheduling and polling overhead and remain separate from normal pipelined app
FPS, GPU busy cost, and presentation. Increasing a timeout or bypassing orderly
shutdown would not establish the required result.

### Bounded completion follow-up

The optional `--offscreen --completion` mode at clean revision
`56a3b16c8c1c8f12a5320adc5082c6d20b6378c1` passed both previously failing heavy
configurations with Metal validation enabled. Native MSAA4 retained 94 qualified
measured frame fences; native Temporal retained 90. Both produced opaque Claude
captures, closed their measurement epochs with a drained boundary, and exited
normally with status 0. Each frame wait is bounded at five seconds. The source's
full CI matrix also passed.

These are unpaired diagnostic pilots, not a speed comparison or a repair claim
for the original unpaced mode. Their [compact provenance](completion-pilots-01.json)
includes the exact executable, report and capture hashes. All 12 raw reports,
images and logs are archived with verified hashes at
`/Users/sma/projects/docs/ushas/evidence/completion-pilots-01/`.
Repeated completed-render comparisons and a current consumer trial remain
separate gates.

## Retained artifacts

All 362 files, including failed reports, logs, captures, execution manifests,
the predeclared plan and analysis, are copied and hash-verified at
`/Users/sma/projects/docs/ushas/evidence/claude-campaign-01/`.
`archive-manifest.json` records every file's size and SHA-256. Original files
remain in `/private/tmp/ushas-roadmap-evidence/claude-campaign-01/`.
The public JSON is a compact analysis; it does not replace those raw records.
