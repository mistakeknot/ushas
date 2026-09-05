# Offscreen creation faults: complete failure proof, incomplete slow proof

The **creation-failure arm passes the bounded lifecycle audit**: actual image
rendering becomes bilinear fallback while reset remains pending, then returns to
Temporal output after the fault is released and the reset is acknowledged.
The **creation-slow arm reported success but lacks complete retained evidence**.
Its global observation buffer filled during the pending phase and retained no
restored-phase observations. Its events and images cannot replace that missing
identity and freshness evidence, so the two-exercise acceptance gate remains open.

Both sequential September 5 runs used clean source
`eb7a742c282bde45bdb47425c9feae2d7f50b27d` and binary SHA256
`e21b0f919b3294120afe883b1e2a7014195b02b226d19cb1dd0bdfd5311adcaa`.
Both children and wrappers exited zero with valid reports and Metal API validation
enabled on Apple M5 Max. Each selected Temporal at scale 0.5 with adaptive and
serial-completion measurement disabled. These are offscreen image runs, distinct
from the [failed window attempts](lifecycle-fault-window-attempts-02.md).

## Retained transitions

The fixture obtains 1280×720 output dimensions from the actual image texture
descriptor, keeps its image asset identity fixed, and verifies the active camera
targets that image. The independent audit checks the retained target declaration,
camera view identity, actual 640×360 input and 1280×720 output observations,
requested scale, states, reset flags and generation transitions.

| Evidence | Creation failure | Creation slow |
|---|---|---|
| Initial / changed / restored retained observations | 26 / 24 / 35 | 26 / 4,070 / 0 |
| Distinct eligible initial / fallback / recovered frames | 23 / 23 / 23 | Recovery unavailable |
| Injected control | Generation 1, `ReturnNone` | Generation 1, `HoldPending` |
| Required reason | `ScalerCreationFailed`, effect frame 28 | Event reports `ScalerCreationSlow`, effect frame 4,989, after 10.0073 seconds; its observation was not retained |
| Release / reset acknowledgement | Generation 2 at app frame 51 / acknowledgement at 63 | Events report generation 2 at 4,994 / acknowledgement at 5,006; recovery observation was not retained |
| Lifecycle captures | Three valid opaque images | Three valid opaque images |
| Evicted or dropped observations | 0 | 932 |
| Independent lifecycle acceptance | Pass for this bounded synthetic failure | Incomplete evidence |

The complete failure ledger contains at least 20 distinct, fresh, correctly sized
frames in every phase. During the injected phase, the reset remains pending and
current observations report effective Disabled with pending/failed creation
reasons. Release precedes acknowledgement and fresh Temporal `OutputWritten`
recovery. An old `OutputWritten` observation remains visible briefly at the start
of the changed phase, with effect frame 26 preceding injection at app frame 27;
the audit excludes that pre-injection observation from fallback readiness.

For the slow arm, the fixture's events report its intended ten-second threshold,
release and subsequent acknowledgement. The retained buffer ends before these
events' effect observations. The unmodified run remains execution-reported
success with incomplete acceptance evidence. A separate retention fix introduces
bounded per-phase recent buffers with explicit eviction counts; fresh runs must
verify that change. This archive is not repaired by substituting new records.

## Images and limits

Independent CPU decoding verifies all **ten original PNGs**: the three lifecycle
captures and two regular readbacks per arm. Every image is 1280×720, fully opaque,
and nonuniform in the scene region; recomputed scene statistics match the reports.
A separate reviewer inspected all six lifecycle originals, and the audit author
crosschecked the four changed/restored originals. Both changed images show a
recognizable Claude scene, rails and UI with conspicuous fallback aliasing. Both
restored images have smoother outlines. The
[pixel review](/Users/sma/projects/docs/ushas/evidence/lifecycle-fault-offscreen-02/independent-pixels.json)
explicitly withholds the slow arm's missing ledger acceptance.

These captures are associated with settled lifecycle phases. The harness does
not retain an exact screenshot-to-render-frame completion join. Reset
acknowledgement means Temporal reset commands were encoded. No exclusive GPU
completion, performance, panel delivery, native visibility, OS sleep/resume,
adaptive convergence or broad visual-quality conclusion follows. The faults
simulate an absent creation result and a held pending result; they do not
reproduce a driver failure or the historical MPSGraph crash.

## Reproduction and retention

The [machine-readable report](lifecycle-fault-offscreen-02.json) separates wrapper
success, decoded pixels and independent lifecycle acceptance. The separate
archive at `/Users/sma/projects/docs/ushas/evidence/lifecycle-fault-offscreen-02/`
retains the exact executable, build/launch receipts, all original artifacts and
an immutable committed-source archive.

Manifest revision 2 contains **29 payload files, 111,597,756 bytes**, excluding
itself. It adds the independent pixel review and corrected audit while preserving
the original manifest and all 26 original payloads byte-for-byte. The
[revision 2 manifest](/Users/sma/projects/docs/ushas/evidence/lifecycle-fault-offscreen-02/archive-manifest-v2.json)
SHA256 is `c4097ad5f76efa1390b29a63dc4b3d6146a652c04e32a041d0fb4a9f3e9a9da6`.
The original manifest SHA256 remains
`5ed4dd3c822c6aabd35a80c12b9475df374f500202ad5405db8d32dac9bbd291`.

```sh
PYTHONDONTWRITEBYTECODE=1 python3 \
  /Users/sma/projects/docs/ushas/evidence/lifecycle-fault-offscreen-02/audit-v2.py
PYTHONDONTWRITEBYTECODE=1 python3 \
  /Users/sma/projects/docs/ushas/evidence/lifecycle-fault-offscreen-02/audit-v2.py --self-test
```

The corrected audit has six passing CPU tests, including a regression that
rejects repeated/regressed in-age frame IDs which could otherwise inflate a
settling count. The current failure arm has 23 distinct eligible frames per
phase, so this correction does not change its verdict.
