# MetalFX proxy experiment: stopped at the compatibility gate

The bounded instance-local proxy route stopped after its first calls-only
Spatial run. During encoding, the proxy received `globalTraceObjectID`, which
the declared public selector inventory did not support. All 16 observations became unavailable,
the executable exited 1, and strict analysis rejected the run. No allowlist
change, counter arm, Temporal arm, trace or Bevy integration followed. This
experiment supplies no validated GPU-cost producer or governor input.

## Retained results

| Run | Child exit | Strict result | What the evidence establishes |
|---|---:|---|---|
| `proxy-spatial-off-02` | 0 | Invalid schema | 16 completed frames and intact pixels; 49 Boolean fields were numeric 0/1. Preserved as a failed reference. |
| `proxy-spatial-off-03` | 0 | Valid reference | 16 original frame identities, completed command buffers and independently decoded outputs pass the strict validator. |
| `proxy-spatial-calls-03` | 1 | Invalid observation | 16 completed frames, each with `unsupported_selector`; observation unavailable on every frame. |

The schema failure affected the timestamp-set capability once, reset identity
in 16 admissions and 16 completions, and the delivery-limit flag in 16
completions. CPU tests then reproduced the defect using the actual driver
record builders and ledger snapshots. Explicit `numberWithBool:` serialization
fixed it; 67 observer checks and the driver CLI/JSON checks passed. The analyzer
remained strict, and the original failed files were not repaired or used as a
reference.

The corrected runs use source `b4484a17469aa3b62c181287e46277f792a70d33`
and binary SHA-256
`aed166fdb283eb88197d0dca3819d7c57df0d5054f5572cceaab461a84c2fec5`.
The build used Apple Clang 17.0.0, macOS SDK 26.2, strict warnings and optimization.
Execution used an Apple M5 Max on macOS 26.5.2 with Metal API validation enabled.
Root serialized these GPU invocations; other applications' GPU activity was
not controlled or measured by a trace.

## Why pixels do not clear the gate

Every calls-only frame recorded one `globalTraceObjectID` invocation followed
by three actual `renderCommandEncoderWithDescriptor:` calls. The preserved
framework encoder labels were `MetalFX_Normalize`, `MetalFX_Scale` and
`MetalFX_Sharpen`: 48 observed render encoders across 16 frames. Each supplied
MetalFX buffer sealed in NotEnqueued state and completed successfully. All 48
setup, MetalFX and final command buffers completed without an error. No selector
records were dropped, and **zero GPU timestamp samples** were requested.

The archive audit independently decoded all retained raw outputs and PNGs.
For corrected OFF versus CALLS, all 16 exact raw RGBA16Float files, composed
RGBA8 pixels and PNG files match. Root also viewed frames 1 and 16 in both arms;
they show the intended opaque gradient. This is a narrow observation of pixel
equality for the deterministic Spatial fixture. It does not establish accepted
proxy compatibility, all-process encoder coverage, instrumentation overhead,
GPU cost or Temporal quality. The strict compatibility validator deliberately
refuses to promote pixel equality from an invalid observation.

The inspected public SDK 26.2 Metal and MetalFX headers contain no declaration
of `globalTraceObjectID`; the cached public bindings likewise expose no such
member. The public [`MTLCommandBuffer` contract](https://developer.apple.com/documentation/metal/mtlcommandbuffer)
documents command encoding and submission, and its
[`label` property](https://developer.apple.com/documentation/metal/mtlcommandbuffer/label)
provides debugging metadata. No public semantics for the observed selector
were established. Its name alone cannot justify treating it as a harmless
query. The negative disposition follows the experiment's predeclared unknown-
selector boundary; it does not assert that this method itself submitted
additional work.

## Evidence and reproduction

The [machine-readable report](metalfx-proxy-02.json) keeps audit consistency,
strict run validity, pixel equality and rejected compatibility as separate
fields. The durable archive is
`/Users/sma/projects/docs/ushas/evidence/metalfx-proxy-02/`.
Its initial copy receipt covers 144 payloads totaling 30,844,812 bytes: exact
source tarballs and binaries, build/execution receipts, original and corrected
runs, raw outputs, PNGs, strict analyses, CPU failing/passing logs and frozen
validator sources. The auditor verifies that initial inventory and separately
hashes the subsequent independent and root visual reviews. The final
[archive manifest](/Users/sma/projects/docs/ushas/evidence/metalfx-proxy-02/archive-manifest.json)
covers **150 payloads, 30,998,835 bytes**, including the auditor, root audit and
later reviews. Its SHA-256 is
`f3204077e2d0745775591533e1bfa74441e69f3a1d12e8feec51049f85e31ea0`.
Root also matched all 134 Git blobs in each frozen source tarball against its
recorded revision. The original copy receipt and all failed evidence remain
unchanged.

```sh
python3 /Users/sma/projects/docs/ushas/evidence/metalfx-proxy-02/audit.py --self-test
python3 /Users/sma/projects/docs/ushas/evidence/metalfx-proxy-02/audit.py
```

The auditor is read-only, disables bytecode writes and never launches a native
binary. Its exit 0 means the retained evidence consistently supports the stated
negative disposition; `experiment_valid`, `compatibility_accepted` and
`validated_for_governor` remain false. Optional `--out` requires a new file.
The auditor's four CPU tests cover changed classifications, altered calls
inventory and payload/hash/path corruption; the actual archive pass also
reproduces all three strict analyses and independently verifies pixel files.

The earlier native stage-counter experiment remains separate evidence. This
failed MetalFX compatibility gate leaves the complete Bevy/MetalFX producer
unvalidated; it cannot be replaced with the already rejected marker envelopes
or CPU-inclusive completed-render cadence.
