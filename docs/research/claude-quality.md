# Claude reconstruction checks

The smoke fixture now defaults to a procedural 3D interpretation of
vgel/thebes' Claude character. Its coral rays, white face, narrow facial marks,
articulated limbs and curved tail give the upscaler recognizable silhouettes
and thin details to reconstruct. The original shape scene remains available
with `--subject shapes`; every report records the subject and geometry version.
See [the reference and credit](../../tools/smoke/CHARACTER.md) and the
[actual rendered preview](../../tools/smoke/preview.png).

The following checks ran on the M5 Max with Metal validation enabled, using a
clean build of `9e53efea7bd2f550e9aac0669c14616bc32618e6`. The frozen executable's
SHA-256 is `6430b1db65cc58fc68125397b6756c0d5d725b8267d91137cf13e85735ce604e`.
All runs rendered offscreen to the same 1280×720 RGBA8 sRGB image target, with
native-resolution UI. Their warmup and final captures are fully opaque and
nonblank; each report passed its distinct-frame readiness gate.

| Run | Mode/input scale | Scene | Distinct ready effect frames | Image assessment |
|---|---|---|---:|---|
| `claude-rc-temporal-half-motion-01` | Temporal / 0.5 | Moving | 909 | Faces and silhouettes remain legible; native UI is clear. |
| `claude-rc-temporal-third-motion-01` | Temporal / 1/3 | Moving | 913 | More visible fine grain and edge breakup than half resolution. |
| `claude-rc-spatial-half-01` | Spatial / 0.5 | Static, input MSAA off | 1169 | Recognizable reconstruction, with visible geometric aliasing. |
| `claude-rc-temporal-half-hdr-01` | Temporal / 0.5 | Moving, HDR main texture | 880 | Faces and UI remain clear after tonemapping. |

These are captured-image assessments. Motion poses differ between runs, so
they are not pixel-matched temporal comparisons, motion-sequence measurements,
or evidence about judder and latency. `OutputWritten` counts CPU-side encoded
output observations, not completed or displayed GPU frames. The Spatial arm
has no input AA; its jagged edges are not a like-for-like comparison to native
MSAA4. HDR changes the tonemapped background and must be assessed separately.

The images support retaining the conservative **0.5 default quality floor**.
One third stays an explicit quality choice. No automatic rung or frame-budget
claim follows from these captures. The separately balanced
[fixed-scale campaign](../../tools/smoke/CAMPAIGN.md) records CPU-loop cadence
with its own limits; the [marker investigation](marker-scope-01.md) explains why
the experimental timer cannot drive the governor.

## Artifact identity

Original reports are under `/private/tmp/ushas-roadmap-evidence/` with `.json`,
`.json.png`, and `.json.png.warmup.png` suffixes. Verified copies of all 20
reports, images, logs and run manifests are archived at
`/Users/sma/projects/docs/ushas/evidence/claude-quality-01/`; its
`archive-manifest.json` records every file's size and SHA-256.

| Run | Report SHA-256 | Final PNG SHA-256 |
|---|---|---|
| Half Temporal motion | `d5f7af9be1595cb628bff1aa2e39ae7e9b2e48d0da27c025c9b7044dea6692e0` | `b6cb948bf8a121c449e8c32be377218c84cc6844693a3045b57d6898833840f2` |
| Third Temporal motion | `129153f59afc9a7649c03a47d2c57ab5bdeb5444f7d1d6f2f27db9c20e3560a5` | `932d6e12ac38f3ebad9815ea59f8b3dca439622acfefbe449d342b0b1e48d142` |
| Half Spatial static | `8c451f39eb688caee315abde7deb661b7f8ddc933cf9f9123fae9f14130bcc26` | `4e6ac3672db0dd276a260aca5fe8dfc5ee0d0e81274fb009295aa8ff0ef1d65c` |
| Half Temporal HDR motion | `1209d041bdd1616fab703b3ba0fc8b0092e3ab63ddd64b77024a859251868e3f` | `63b7ef6425058978e1772fdc14b70204a4db14d6dd8a419d115f7f46870ec385` |

Visible-window resize, camera-cut sequences, inactive resume, operating-system
sleep/resume, and frame-interpolation composition require their own evidence.
The missing/sleeping display during later runs does not invalidate these image
targets, but it prevents a window or panel verdict.
