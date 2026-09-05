# Temporal jitter coordinates

The 2026-09-04 early-postprocess smoke capture produced a correctly sized scene
and crisp native UI, but static geometry had repeated sawtooth and ghost edges.
The bilinear and spatial controls did not show those pronounced artifacts.
Source inspection found that Ushas supplied the wrong horizontal jitter sign
to MetalFX: `(bevy.x, -bevy.y)` instead of `(-bevy.x, -bevy.y)`.

Apple's [temporal antialiasing sample](https://developer.apple.com/documentation/metalfx/applying-temporal-antialiasing-and-upscaling-using-metalfx)
provides the concrete convention. In the
[downloaded sample](https://docs-assets.developer.apple.com/published/cd4b8936af25/ApplyingTemporalAntialiasingAndUpscalingUsingMetalFX.zip),
`Renderer/AAPLRenderer.swift:145-183` adds `-2 * halton / renderResolution`
to the projection matrix's Z column, then changes the pixel jitter to
`(halton.x, -halton.y)`. Lines 268-269 pass that pixel jitter to MetalFX.
Consequently, for a projection-column change `D`, its MetalFX jitter is
`(-D.x * width / 2, D.y * height / 2)`.

[Bevy 0.19's `TemporalJitter::jitter_projection`](https://docs.rs/bevy_render/0.19.0/src/bevy_render/camera.rs.html#788-801)
adds `D = (2*x/width, -2*y/height)`. With its right-handed perspective
projection (`clip.w = -view.z`) and Metal's downward screen Y, the actual
input-pixel displacement is `(-x, -y)`. The previous code accounted for Y
conversion but missed X's sign after perspective division. The shared
`metalfx_jitter_offset` helper now supplies this same conversion to temporal
upscaling and frame interpolation.

The regression applies Bevy's actual projection mutation to two 3D points at
different depths, over all 32 Halton offsets and two input resolutions. It
compares the projected pixel displacement with the supplied MetalFX jitter.
Before the correction it failed at offset `(-0.25, 0.16666667)`: MetalFX was
given `(-0.25, -0.16666667)` while the projected displacement was
`(0.25, -0.1666565)`. This test establishes the perspective coordinate
relationship; it does not establish reconstructed image quality.

The other inspected inputs agree with the static perspective case: the
resolve shaders use integer pixel loads to crop the top-left content region,
and Bevy's prepass emits unjittered current-minus-previous UV motion. MetalFX
expects previous-minus-current pixel motion, so the existing negative input
width/height scales are appropriate. Apple's sample removes jitter before
forming motion vectors (`Renderer/Shaders.metal:259-269`), matching Bevy's
unjittered motion contract.

The matching static hardware capture confirms this correction removes the
pronounced sawtooth and ghost rails seen before it: thin lines and cube edges
reconstruct cleanly while the native UI remains sharp. The paired artifacts
are `/private/tmp/ushas-roadmap-evidence/early-temporal-01.json` and
`early-temporal-01.json.png` before the correction, and
`/private/tmp/ushas-roadmap-evidence/jitter-fixed-temporal-01.json` and
`jitter-fixed-temporal-01.json.png` afterward. Both reports have `valid: true`,
opaque nonblank captures, Metal validation enabled, and the same static
1280×720 temporal arm at scale 0.5 with MSAA off and HDR disabled. Their
reported executable SHA-256 values are
`f26b9dfc8d0f052d6d5359f71449b2b4992c6cc2c497293385d830e01d7cb19e`
before and `a4783b375ea8eff34600ab74ea249e0a3216d129ba4bd37fb8d2eac2b078266f`
after. These are local run artifacts, pending archival with a hashed manifest.

This is visual confirmation for that static perspective scene. Moving scenes
and HDR still require matching captures. It does not validate orthographic
jitter, frame interpolation quality, or GPU timing scope.
