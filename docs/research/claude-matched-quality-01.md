# Matched Claude quality samples

The six fixed-scale arms produced **72 valid opaque captures with matching
logical poses**. Temporal half is slightly softer than native during the
inspected moving sample, but faces, rays and thin rails remain readable. The
reset frame has conspicuous aliasing; the held `cut16` sample is substantially
cleaner. Third resolution loses more fine detail and has more severe reset-frame
aliasing. These results support settled reconstruction and a documented reset
transient, not immediate native quality or full continuous temporal stability.

The [data report](claude-matched-quality-01.json) retains identities and hashes.
All runs used clean source `ac3091f4b3f2cfe9d6dfc099298addfba8f35555` and binary
SHA256 `06934fbfee226e6740fe57978ed730c79d0c7bb2ff8eeee7a5d5bb614e42e5b8`.
Each log identifies Apple M5 Max / Metal and records “Metal API Validation
Enabled.” The parent launched with `MTL_DEBUG_LAYER=1`; this review performed
only CPU revalidation, image inspection and archival.

| Arm | Content → output | MSAA | Main texture | Evidence |
|---|---|---|---|---|
| Native | 1280×720 → 1280×720 | 4 | Rgba8UnormSrgb | 145 frames / 12 PNGs |
| Temporal half | 640×360 → 1280×720 | Off | Rgba8UnormSrgb | 145 / 12 |
| Bilinear half | 640×360 → 1280×720 | Off | Rgba8UnormSrgb | 145 / 12 |
| Temporal third | 427×240 → 1280×720 | Off | Rgba8UnormSrgb | 145 / 12 |
| Native HDR | 1280×720 → 1280×720 | 4 | Rgba16Float | 145 / 12 |
| Temporal half HDR | 640×360 → 1280×720 | Off | Rgba16Float | 145 / 12 |

The fixed 1/60 simulation sequence holds ticks 0–31, animates the models and pans
the camera during 32–127, then cuts the camera at 128 and holds through 144.
Twelve captures sample ticks 31, 63, 93, 94, 95, 127, 128, 129, 130, 132, 136 and
144. Execution is serial and unpaced. The actual and expected camera matrices,
pose clock and jitter indices match **exactly across all 145 ticks in all six
arms**. The three Temporal arms also have identical actual jitter offsets.
Camera/entity/render IDs are checked within each process, not equated across
runs. The camera changes at tick 128 and remains fixed thereafter; the model
pose remains at tick 127's animation time throughout the cut samples.

All 870 scripted frame proofs passed the frozen validator again. Each of the
72 screenshots joins its requested entity to extraction, the same render frame,
current effect state and matching queue-completion fence, then its asynchronous
readback. All Temporal arms requested resets at ticks 0 and 128 and recorded
acknowledgement during those render frames. Thus `cut0` has a stronger identity
contract than the older consumer request-only cut probe. Encoding acknowledgement
and completion establish command execution/readback; visual recovery still needs
image inspection.

The harness author inspected `motion63`, `cut0` and `cut16` in every arm. Native
SDR and HDR retain sharp facial strokes, ray silhouettes and rails. Temporal
half remains readable in motion and is smoother after settling than the
bilinear control, which retains stepped edges. Half-resolution `cut0` has
visible stair-stepping on the checkerboard, rails, faces and rays; no obvious
old-pose overlay is visible in that reset sample. At third resolution the moving
sample is softer, thin rails have more breakup, and small facial strokes degrade
more at `cut0`; `cut16` improves substantially but remains softer than half/native.
The HDR pair shows the same composition and broad color appearance, with the
same half-resolution reset transient. These PNGs are tone-mapped readbacks from
an HDR main texture, not HDR panel output. The UI caption stays crisp and
readable even when the scene pixels are strongly aliased at `cut0`.

There is one run per arm and no blinded perceptual score or acceptance threshold.
The twelve sampled images do not certify continuous video stability, production
pacing, input latency, power, GPU cost, adaptive behavior, transparency stress,
or frame generation. Held `cut16` is not a moving recovery test. There is no
reset-disabled comparison, and 0.58, two-thirds and 0.75 quality runs are absent
from this archive. The harness author's inspection is reported separately from
the parent's and peer's independent review. These quality samples add no timing
estimate to the separate performance campaigns.

The [archive manifest](/Users/sma/projects/docs/ushas/evidence/claude-matched-quality-01/archive-manifest.json)
verifies **111 payload files, 147,989,417 bytes**: all six original reports,
manifests and logs; all 72 PNGs; the exact 101,088,960-byte executable; build
receipt/log; and 18 harness source files extracted from the committed Git objects.
Every copied file matched its source hash before and after copying. Original
run/capture hashes were checked, and every PNG decoded as 1280×720 RGBA with all
921,600 alpha bytes equal to 255. The archive includes the exact runner, quality
module and completion module used for these checks. No GPU workload, live source
or runner was changed during archival.
