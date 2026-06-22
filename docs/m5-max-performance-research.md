# bevy_metalfx — M5 Max performance research

**Goal:** dramatically improve `bevy_metalfx` performance on Apple M5 Max (40-core GPU,
128 GB unified memory, Metal 4, macOS 26-era).
**Date:** 2026-06-22. **Status:** research; recommendations not yet implemented.

This document records *verified* technical facts (from primary Apple sources + direct
inspection of the dependency and our own source) that gate the optimization program.
A ranked program of changes follows at the end; deep-research synthesis is folded in
once available.

---

## Verified ground truth

### Hardware (this machine, confirmed via `system_profiler`)
- **Apple M5 Max**, 40 GPU cores, 128 GB unified memory, **Metal 4** support.
- M5 generation: a **Neural Accelerator in every GPU core** (>4× peak GPU AI compute
  vs M4). Source: [Apple Newsroom — M5](https://www.apple.com/newsroom/2025/10/apple-unleashes-m5-the-next-big-leap-in-ai-performance-for-apple-silicon/).
- **The MetalFX temporal upscaler was redesigned for M5** and "harnesses both the Neural
  Engine and Neural Accelerators on M5 Pro and M5 Max," and "can now reconstruct fine
  detail from significantly lower render resolutions, allowing you to maintain fluid frame
  rates at higher-quality settings." Source: [Apple Metal — What's New](https://developer.apple.com/metal/whats-new/).

  **Implication:** the dominant performance lever on M5 Max is to **render at a lower
  internal resolution than on prior chips** and let the redesigned upscaler reconstruct it.
  Upscaling cost is dominated by *input pixel count* (the whole render pipeline scales with
  it), not the scaler call. "Dramatically faster" ≈ "render fewer pixels, M5 covers for it,"
  gated by input quality so artifacts don't appear.

### Dependency surface (`objc2-metal-fx 0.3.2`, in our Cargo.lock — inspected directly)
The crate **already binds everything we need**. Nothing below is blocked by the dependency:
- **Metal 4 native classes**: `MTL4FXTemporalScaler`, `MTL4FXTemporalDenoisedScaler`,
  `MTL4FXSpatialScaler`, `MTL4FXFrameInterpolator` (command-buffer-native Metal 4 path).
- **Temporal-denoised scaler**: `newTemporalDenoisedScalerWithDevice` (+ `diffuseAlbedo`,
  `normal`, `roughness`, `specularHitDistance` inputs).
- **Reactive / denoise masks**: `setReactiveMaskTexture`, `setDenoiseStrengthMaskTexture`.
- **Dynamic resolution (no rebuild)**: `setInputContentMinScale`, `setInputContentMaxScale`,
  `setInputContentPropertiesEnabled`.
- **Exposure**: `setExposureTexture`, `setPreExposure` (alternative to auto-exposure).
- **Color processing**: `setColorProcessingMode`.
- **Init/sync control**: `setRequiresSynchronousInitialization`, `fence`.

### What our code uses today (inspected `platform.rs` / `node.rs`)
- Legacy `MTLFXTemporalScaler` / `MTLFXSpatialScaler` / `MTLFXFrameInterpolator` only —
  **no `MTL4FX*` path.**
- Setters used: input/output/color/depth/motion dims+formats, `setJitterOffsetX/Y`,
  `setMotionVectorScaleX/Y`, `setDepthReversed(true)`, `setReset` / `setShouldResetHistory`,
  `setAutoExposureEnabled(true)`, and frame-interp camera params.
- **NOT used:** any `MTL4FX*`, denoised scaler, reactive mask, dynamic-resolution properties,
  exposure texture, `setColorProcessingMode`, `setRequiresSynchronousInitialization`.

So the current implementation exercises roughly the basic third of the available surface.

### Our per-frame hot path (the structural cost, inspected)
1. GPU copy: `main_texture` content region → `input_texture`.
2. **Full-screen "motion resolve" render pass** → RG16Float at content res.
3. **Full-screen "depth resolve" render pass** → Depth32Float at content res.
   *(Passes 2 & 3 exist ONLY to downsample full-res prepass textures to content res.)*
4. `MTLFXTemporalScaler.encodeToCommandBuffer` (appended to wgpu cmd buffer via `as_hal_mut`).
5. **Full-screen blit**: scaler output → swapchain.
- Steady-state heap allocation: **zero** (scalers/bind-groups/pipelines cached). The code is
  already clean on allocation; wins are architectural and hardware-exploitation, not cleanup.
- Frames are **skipped** while the scaler compiles on a background thread (cold-start stutter).

---

> **Note:** an earlier "preliminary wins" list lived here. It has been **superseded** by the
> adversarially-verified program below (some preliminary items — sub-0.5 floor, auto-exposure-as-the-cost,
> "passes are the bandwidth bottleneck" — were corrected or refuted by the synthesis). See the
> *Deep-research synthesis* section for the authoritative, ranked program.

## Confirmed defects vs. the 120 fps M5 Max goal (independently verified in source)

- **The adaptive governor targets 60 fps, not 120.** `lib.rs`: `P99_SCALE_DOWN_MS = 16.67`
  (60 fps) and `P99_SCALE_UP_MS = 12.0`. On a 120 fps ProMotion target the budget is **8.33 ms**;
  as written the governor keeps render scale *high* as long as it clears 60 fps, actively
  preventing 120 fps. Fix: make targets configurable / default to the display's max refresh;
  for 120 fps use ~8.33 ms down / ~6.5 ms up.
- **Scale ladder is coarse (`SCALE_STEPS = [0.5, 0.75]`).** Add *finer* steps (~5% apart) once
  rebuilds are free (below). NOTE — corrected by adversarial review: do **not** chase sub-0.5
  steps blindly. Apple gates the floor to `supportedInputContentMinScaleForDevice` (≈0.5), and the
  M5's "reconstruct from lower res" advantage shows up mostly as *quality at the same scale*, not a
  new lower floor. The lever is finer + hitch-free adaptation, not a magic 0.33 floor.
- **Every scale change rebuilds the temporal scaler** (the 10 s `SCALE_CHANGE_COOLDOWN` exists to
  cover background re-creation; frames are skipped while it compiles). `inputContentPropertiesEnabled`
  + `setInputContentMinScale/MaxScale` lets one scaler span a scale *range* — change scale per frame
  with **no rebuild, no cooldown, no frame skip**. This both removes stutter and lets the governor
  react fast enough to actually hold 120 fps.

Resolution control itself is already done correctly: the crate drives Bevy's
`MainPassResolutionOverride` per camera, so the *plumbing* to render fewer pixels exists — only the
range, targets, and scaler-rebuild cost need fixing.

---

# Deep-research synthesis (6 domains → 45 findings → 27 survived adversarial review)

## Headline (the honest bottom line)
**There is no dramatic single-digit-millisecond GPU win hiding in the upscale pass.** On a 40-core
M5 Max, a slow colony/city sim at high render scale is **likely CPU/sim- or present-bound**, and each
individual GPU lever here is sub-millisecond (≈0.05–0.4 ms). The real dividends are three different things:

1. **Quality/stability fixes that let you confidently HOLD or LOWER render scale** — 32-phase jitter,
   mip-bias, correct exposure, motion-vector/frame-interp correctness. (Render scale is the only
   ~quadratic perf lever; everything else just lets you push it without artifacts.)
2. **Hitch elimination** — wire dynamic resolution so the adaptive governor stops *rebuilding* the
   scaler on every scale change (removes a multi-frame stutter, cuts adaptation latency 10 s → ~1 s).
3. **One genuinely large but expensive bet** — frame interpolation toward 120 Hz, **gated on a
   Bevy/wgpu dual-present rewrite that does not exist yet** (without it, interpolation = pure overhead, 0 fps).

> **Mandatory Phase 0:** measure GPU-bound-ness FIRST (`powermetrics` GPU residency + Metal System Trace).
> If you already hold 120 fps at scale 1.0, most of this program is *quality* work, not perf.

## Program, ranked by leverage vs effort

### Quick wins (S effort, near-zero GPU cost, ship together)
| Change | Effect | Confidence |
|---|---|---|
| **32-phase Halton(2,3) jitter**, length `ceil(8·(1/scale)²)`, scaled by render ratio (current = fixed 8) | Less shimmer on thin static detail (UI grid, building edges); may unlock ~10-15% lower scale | verified-docs |
| **`MipBias = log2(render_scale)`** on the temporal camera (currently unset) | Sharper textures at fixed scale; conservative start (−0.5 @ 0.5), specular crawl risk if over-biased | verified-docs |
| **Validate exposure**: run with `MTLFX_EXPOSURE_TOOL_ENABLED=1`; only if it drifts, replace `setAutoExposureEnabled(true)` with a 1×1 `exposureTexture` | Kills flicker/ghosting on lighting changes → lets you hold a lower scale | verified-docs |
| **Correctness confounders**: verify/negate jitter sign vs Bevy convention; replace `usize→MTLPixelFormat` transmute with explicit match; bump wgpu/wgpu-hal pin ≥27.0.3 (`as_hal_mut` assertion fix) | Removes a smear source, a UB hazard, a panic risk; de-risks all A/B | verified-docs |
| **Motion-vector debug blit** (R,G→screen) | Makes every motion/ghosting claim falsifiable (prerequisite for resolve-removal) | verified-docs |
| **A/B `desired_maximum_frame_latency = 3`** (Bevy default 2) | Better 1%-lows / fewer dropped presents under sim spikes; ~0 avg fps | verified-docs |

### High-leverage
- **Wire dynamic input resolution** (`setInputContentPropertiesEnabled(true)` + `setInputContentMin/MaxScale`
  clamped to `supportedInputContentMin/MaxScaleForDevice`; build the scaler ONCE at max input dims;
  allocate input/depth/motion at max dims; re-key `needs_recreate` to **output** size only; per frame
  set `setInputContentWidth/Height` + recompute `motionVectorScale`/jitter from current content size).
  **This is THE live bug with the clearest payoff** and the prerequisite for all adaptive/DRR/governor
  work — today every adaptive step triggers a full background rebuild that *skips frames*. Drop the 10 s
  cooldown toward ~1 s. *Temporal/FrameInterp only; Spatial has no dynamic-res API.* Effort M.
- **Finish frame interpolation correctly** *and stage it last* — feed the scaler's **upscaled tonemapped
  display-res output** as `colorTexture` (not the input-res texture); implement the `prev_color` blit
  (currently a TODO → warping); feed real FOV/near/far/dt from Bevy `Projection` (currently hardcoded);
  render UI offscreen via `setUITexture`. **The gate:** build dual-drawable present pacing — *does not
  exist in Bevy 0.18/wgpu 27*. Realistic ceiling **1.5–1.8×** presented fps (not 2×), macOS 26 only,
  **0× without the present rewrite.** Effort L.

### Structural (do only if Phase 0 confirms GPU-bound)
- **Remove the redundant double-blit** — Bevy's built-in `UpscalingNode` blits main→swapchain, then our
  Phase-C blit overwrites it. Make `UpscalingNode` a no-op for MetalFX cameras (don't reverse the graph
  edge — that shows the non-MetalFX image). ~0.05–0.15 ms + ~260 MB/frame UMA traffic reclaimed. Effort M.
- **Collapse the two resolve passes + input copy** by feeding prepass subrects directly (requires dynamic-res
  first; guard: all inputs must share physical size + content top-left, preserve MV space/sign, breaks under
  MSAA). Real but modest: **0.10–0.35 ms** (1–4% of an 8.33 ms budget) + 3 fewer allocations + a temporal-
  stability quality gain. *Not* the 4–10% earlier framings claimed. Effort L.
- **Reactive mask** (`setReactiveMaskTextureEnabled` + per-frame `setReactiveMaskTexture`) authored
  *surgically* at content-res for alpha-blended particles/overlays not in motion+depth — to protect a
  one-notch lower render scale from ghosting. Mask alone = 0 fps; net win only if it unlocks the scale drop. Effort L.

### Esoteric bets (spikes; both default-OFF; both gated on dynamic-res + GPU-bound)
- **Motion-gated + thermal-aware governor** *(survived as speculative)* — M5 Max (esp. 14") throttles
  ~72 W → ~44–55 W within seconds, producing a sawtooth at fixed high scale. *Extend* `adaptive_scale_system`
  (don't add a 2nd controller): keep frame-time P99 primary, add `NSProcessInfo.thermalState` as a
  scale-DOWN-only trigger, add a near-static camera-velocity signal to allow a finer floor during low-motion
  windows, replace the 2-step ladder with ~5% steps. Est. **+5–15% sustained fps on 14"** (mostly sawtooth
  removal), ~0–8% on 16". Capped if sim-bound. Effort M.
- **Center/ROI foveated coarse-shading** via a Metal rasterization-rate map (full rate at cursor/selected
  entity, coarser at edges; MetalFX temporal accumulation hides the coarseness). Attacks *shading* cost,
  additive to render-scale's rasterization savings. Risk: peripheral coarseness on the wide static shots a
  city-sim favors. Plausibly low-single-digit-to-~10% *if fragment-bound*. Spike-only. Effort L.

## What the adversarial pass KILLED (don't waste time here)
- **"M5's redesigned upscaler is a free 4–8× via the legacy path"** — downgraded hard. The 4–8× is *peak
  FP16 GEMM throughput*, not upscale wall-clock; the win is likely **quality, not freed ms**. Central risk:
  if the neural path is gated to `MTL4FXTemporalScaler` (Metal 4 native), **bevy_metalfx cannot reach it on
  wgpu 27** (classic `MTLCommandBuffer` only). Measure NA utilization % in a GPU capture before believing any of it.
- **R2/plastic/blue-noise jitter** — needs hundreds of samples; pure downside at 8–32 phases vs Apple's
  explicit Halton(2,3). M5-agnostic.
- **`MTLResidencySet` / the MetalFX `fence` trick** — Metal-4-only / untracked-resources-only; preconditions
  false on our tracked, Metal-3-via-wgpu path. NVIDIA/Vulkan barrier lore misapplied to Apple's tracked model.
- **`colorProcessingMode` change** — that property is **spatial-scaler only**; nothing to set on the temporal path.
- **Sub-0.5 render-scale floor** — gated by `supportedInputContentMinScaleForDevice` (≈0.5).
- **"Resolve/blit passes are the bandwidth bottleneck"** — refuted; content-res passes (~2 MP) are trivial on
  ~460–614 GB/s. The scaler dominates, not the glue. (Also: M5 Max bandwidth is ~460–614 GB/s, **not** the 122 figure.)
- **Pre-warm/synchronous-init to fix "blank frames"** — the node runs after `Node3d::Upscaling`, so during
  compile the user already sees Bevy's complete bilinear frame; encode is already async; the transmute is lossless.

## Phase 0 benchmark harness (build before touching code)
- **Per-pass GPU time**: Instruments "Metal System Trace" + GPU capture; wrap each pass in
  `ctx.diagnostic_recorder().time_span(...)` (mirror upstream DLSS node's span). Read the **Neural-accelerator
  utilization %** track on the scaler region to confirm the M5 NA path is actually engaged.
- **Frame pacing**: `MTL_HUD_ENABLED=1` interval histogram (Apple's "good pacing" = within ≤2 buckets).
- **Bandwidth/compression**: GPU-capture resource inspector (per-encoder bytes + texture compression attr).
- **Sustained/thermal**: `powermetrics --samplers gpu_power` over 10 min fixed-camera; log `GPUStartTime/EndTime`
  + `thermalState`; report p50/p99/mean for **minutes 2–10** (post-burst), not just average.
- **Quality**: `MTLFX_EXPOSURE_TOOL_ENABLED=1` (constant mid-grey = correct); fixed-camera convergence test →
  variance-of-Laplacian (sharpness) + per-pixel temporal stddev over ≥60 settled frames vs a high-res reference.
- **In-crate counters (keep permanently)**: `newTemporalScalerWithDevice` calls; frames-skipped-waiting-for-scaler;
  `as_hal` calls (debug). These directly validate the rebuild-elimination work.
- **A/B protocol**: feature-flag every structural change; three scripted camera paths (static / slow-pan /
  motion-heavy) at fixed `render_scale = 0.5` → ~4K output; M4 baseline for any cross-gen claim.

## Recommended sequencing
**Phase 0** prove the bottleneck (gate) → **Phase 1** ship the quick-win quality+correctness bundle (independent,
near-free, raises quality-per-input-pixel that everything downstream depends on) → **Phase 2** dynamic resolution
(unblocks adaptive work, kills the rebuild hitch) → **Phase 3** bandwidth/encode cleanup *if GPU-bound* →
**Phase 4** frame interpolation (the big bet, only after correctness + the present-path spike). Defer indefinitely:
`TemporalDenoisedScaler` (only with RT lighting), MTL4/residency (only when wgpu exposes it), foveated VRS (esoteric spike).
