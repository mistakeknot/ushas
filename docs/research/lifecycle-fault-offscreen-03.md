# Both offscreen creation-fault exercises pass their retained phase gates

Fresh creation-failure and creation-slow runs pass the independent lifecycle
audit. Each retains matching initial, fallback and recovered render observations,
ordered fault/reset events, and three opaque phase captures. The slow run uses a
bounded recent window and explicitly reports its evictions; this is **acceptance
within retained phase windows, not a complete history of every rendered frame**.
The [earlier incomplete slow run](lifecycle-fault-offscreen-02.md) and
[failed window attempts](lifecycle-fault-window-attempts-02.md) remain unchanged.

Both September 5 runs used clean source
`906b27cecf76616f100187c9b55f97ec6c6f9b1c` and binary SHA256
`637cc5a59554b489ac4784fbf996523bcde1d1dc9a2507574c64fea863c4bd1f`.
Each child and wrapper exited zero, with report and lifecycle validity true.
The logs identify Apple M5 Max and enabled Metal API validation. Configuration
was fixed Temporal at scale 0.5, offscreen, with adaptive and serial-completion
measurement disabled.

## Independent phase audit

The fixture reads the selected image's actual 1280×720 texture descriptor,
preserves its asset identity, and checks that the active camera renders that
image. The auditor verifies the recorded target, stable view identity, actual
640×360 input and 1280×720 output, requested scale, effect state, fault generation
and reset flags. Settling requires at least 20 distinct eligible effect frames
per phase; repeated or regressed IDs cannot inflate that count.

| Retained evidence | Creation failure | Creation slow |
|---|---|---|
| Initial / changed / restored records | 26 / 25 / 34 | 26 / 1,024 / 35 |
| Distinct eligible frames in those phases | 23 / 24 / 23 | 23 / 1,024 / 24 |
| Injected control | Generation 1, `ReturnNone` | Generation 1, `HoldPending` |
| Required reason and retained effect frame | `ScalerCreationFailed`, frame 28 | `ScalerCreationSlow`, frame 3,963 |
| Release / reset acknowledgement app frame | 52 / 63 | 3,968 / 3,979 |
| Restored-phase finishing app frame | 86 | 4,003 |
| Evicted records | 0 | 2,917 early changed-phase records |
| Independent bounded lifecycle acceptance | Pass | Pass |

Every eligible changed-phase observation reports effective Disabled and the
appropriate pending/failed reason; reset remains pending throughout the retained
changed window. The slow reason is observed 10.006924 seconds after injection.
The fixture then releases to generation 2, receives a later reset acknowledgement,
and retains fresh Temporal `OutputWritten` recovery with the original dimensions.
Pre-transition observations that lag the main frame are excluded from eligibility.

The slow arm's declared changed-phase total is 3,941: 1,024 retained plus 2,917
evicted. Its retained changed window spans app frames 2,945–3,968 and includes the
required slow reason. Initial and restored records are retained independently;
the restored window spans 3,969–4,003. The auditor checks per-phase totals, actual
record counts, capacity, eviction totals and first/last frame bounds. Separate
events retain injection, reason, release, acknowledgement and completion order.
Neither the report nor this review infers what happened in every evicted frame.

## Captured output and scope

All **ten original PNGs** were independently decoded: three phase captures plus
warmup and final readbacks per arm. Every image is 1280×720 and fully opaque, with
nonuniform scene content; recomputed image statistics agree with the reports.
A separate reviewer inspected all six lifecycle originals. Both fallback images
retain recognizable faces, all six rails and the UI, with visibly coarse edges
and checkerboard aliasing; both restored images are smoother and intact. The
[independent pixel record](/Users/sma/projects/docs/ushas/evidence/lifecycle-fault-offscreen-03/independent-pixels.json)
does not claim fallback quality equals native rendering.

These are synthetic creation-result faults exercising the real fallback output
and normal recovery path. They do not reproduce a driver failure or the historical
MPSGraph crash. Screenshot evidence is associated with settled phases; this
harness does not retain an exact screenshot-to-render-frame completion join.
Reset acknowledgement establishes command encoding. There is no independent
full-frame GPU completion, performance, panel delivery, native window, OS
sleep/resume, adaptive convergence or broad visual-quality claim.

## Retained reproduction

The [machine-readable report](lifecycle-fault-offscreen-03.json) includes audited
phase counts, events, decoded capture statistics and explicit retention limits.
The archive at
`/Users/sma/projects/docs/ushas/evidence/lifecycle-fault-offscreen-03/` contains
**27 payload files, 108,872,753 bytes**, excluding its manifest: original reports,
manifests, logs and ten PNGs; exact executable and build/launch receipts;
committed-source archive; independent pixel review; copy record and CPU auditor.
Copies matched source-before, source-after and destination hashes. The
[manifest](/Users/sma/projects/docs/ushas/evidence/lifecycle-fault-offscreen-03/archive-manifest.json)
SHA256 is `ecc0c03914cfef84c67e31f84662f36a63cc5f447ca1b74deb7632cf68f4dfb7`.

```sh
PYTHONDONTWRITEBYTECODE=1 python3 \
  /Users/sma/projects/docs/ushas/evidence/lifecycle-fault-offscreen-03/audit.py
PYTHONDONTWRITEBYTECODE=1 python3 \
  /Users/sma/projects/docs/ushas/evidence/lifecycle-fault-offscreen-03/audit.py --self-test
```

Eight CPU auditor tests pass, including declared-eviction acceptance and rejection
of inconsistent phase counts/bounds, missing reset/fault evidence, active output
during injection, wrong geometry and repeated/regressed frame IDs. The retention
change separately passed 57 Rust smoke tests, strict Clippy and formatting before
these frozen runs.
