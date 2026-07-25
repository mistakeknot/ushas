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

---

## Phase 0 RESULTS — measured 2026-06-22 on M5 Max (shadow-work-6zit.11)

**Harness built (committed):** `bevy_metalfx` now captures true GPU command-buffer elapsed time via
`addCompletedHandler` (`GPUEndTime − GPUStartTime`) on the MetalFX command buffer, in all three scaler
branches (`src/gpu_timing.rs`, wired through `src/node.rs`). `sw-renderer --bench` reports GPU-elapsed
mean/p50/p99 alongside CPU frame time + a bound-ness verdict (`crates/sw-renderer/src/main.rs`).

**Methodology corrections applied (Codex plan review, 2 passes):** primary signal is *GPU command-buffer
elapsed*, not vsync-pinned total frame time; render-scale sweep is the *confirmation* test, not the primary
discriminator; completion handler is observational, registered pre-commit, borrowed ptr only, `try_lock` sink
on the Metal callback thread. **Metric caveat (Codex item C):** `GPUStartTime/EndTime` measures one command
buffer — confirm against Metal System Trace that the frame is one relevant buffer before reading as total GPU cost.

**Spatial mode, uncapped scale sweep (`--bench-quick`, globe spike scene):**

| render_scale | frame mean (ms) | GPU mean (ms) | GPU/frame fraction |
|--------------|-----------------|---------------|--------------------|
| 0.50         | 16.67           | 1.25          | 7.5%               |
| 0.75         | 16.67           | 1.27          | 7.6%               |
| 1.00         | 16.67           | 1.30          | 7.8%               |

**VERDICT (Codex-verified, narrowed): the MetalFX command buffer is NOT the bottleneck.** The broader claim
"the whole app is not GPU-bound" is **not yet proven** — see the open caveat below.

What the data *does* establish:
1. **The timed MetalFX upscale command buffer is ~1.3ms of a 16.67ms frame (≈8%) and nearly flat across render
   scale** (1.25→1.30ms across 0.5→1.0). A GPU-bound *upscale pass* would scale ~quadratically with render
   scale; it doesn't budge → the MetalFX pass itself is not the limiter, and optimizing it optimizes ≤8% of the frame.
2. **Frame time is paced at exactly 16.67ms (60Hz) even with `--uncapped`.** *Something* pins the loop at 60Hz.

What the data does **NOT** yet establish (Codex verification pass):
- **The 8% is for the MetalFX command buffer only.** The main globe render pass may be a *separate, untimed*
  command buffer. If it is large, the app could still be GPU-bound on that buffer and the 8% figure is misleading.
- **The 16.67ms pin ≠ proof of CPU/present-bound.** It is equally consistent with `AutoNoVsync` not actually
  applying (macOS/CAMetalLayer/compositor forcing vsync). "CPU-bound" and "vsync-not-lifted" are both still open.

**Most important next measurement (gates everything):** capture the *total per-frame GPU timeline across ALL
command buffers* — `main_render_gpu_ms + metalfx_gpu_ms + queue/present_wait + actual_frame_interval` — via
Metal System Trace (Instruments) or per-command-buffer timestamps. If total GPU active time ≪ 16.67ms and the
rest is present/wait → "not GPU-bound" is justified. If the main render pass is large and currently hidden →
this conclusion collapses and MetalFX optimization may be warranted after all.

**Implication for the program (conditional):** *if* the next measurement confirms low total GPU time, then
`6zit.12/13/14` (MetalFX dynamic-res / quality / governor) are premature as frame-rate wins (they can't move a
frame whose GPU cost is small) and remain valid only as *quality* + *hitch-elimination* work — with the real
frame-rate lever being the CPU/present cap, outside MetalFX. This implication is **gated on the all-buffer
measurement above**, not yet actionable.

**Known harness gap (corroborating evidence for `6zit.12`):** Temporal mode yielded **0 GPU samples** because it
rebuilds the scaler *every frame* (`needs_recreate` churn — the exact bug `6zit.12` targets), which prevents
steady command-buffer completion. Spatial mode proves the capture mechanism works; temporal capture comes online
once the rebuild churn is fixed.

### Two-reviewer reconciliation (Codex + Sakana Fugu)

Both reviewers independently assessed the Phase 0 result. Where they converge, confidence is high; where Fugu
sharpened the read, the doc is updated accordingly.

**Agreement (both reviewers):**
- The timed MetalFX command buffer (~1.3ms) is **not** the bottleneck — well-supported.
- It is **wrong** to claim the whole app is non-GPU-bound from this data alone (only one command buffer is timed).
- `GPUStartTime/EndTime` is the right primitive for a *single* command buffer but is **not** total GPU-active
  time; on Apple TBDR with overlapping command buffers, per-CB End−Start can over/under-count — invalid as a basis
  for an app-level bound-ness claim (which the harness already concedes).

**Where Fugu sharpened the analysis (adopted):**
1. **The 16.67ms pin is ~90/10 a frame-cap artifact, not 50/50.** Fugu: "a dead-flat 16.67ms across a 4× input-
   pixel change screams vsync/present cap… a genuinely CPU-bound app would show *some* variance and almost never
   land on 16.670 exactly." → The "CPU-bound vs vsync-not-lifted" framing above was *underclaiming*; bet heavily
   on `AutoNoVsync` not applying (`CAMetalLayer.displaySyncEnabled` / compositor forcing vsync). **Cheapest next
   check is therefore confirming the present mode actually took effect — before the full all-buffer trace.**
2. **Flatness across render scale is the NULL result, not evidence.** Fugu's sharpest point: MetalFX spatial writes
   the full 3024×1800 output *regardless of input scale*, so its cost is **output-resolution-bound and is expected
   to be flat**. The input-resolution-sensitive cost lives in the **untimed main globe render pass** — so this
   benchmark is *structurally blind to exactly the component that should scale with `render_scale`*. (This corrects
   statement (1) in the verdict above: "doesn't budge → not the limiter" is only valid for the output-bound upscale
   pass; it says nothing about the input-bound main pass.)
3. **Report stddev/percentiles** — the 1.25→1.30ms rise is likely noise at 240 samples without dispersion stats.

**Net reviewed verdict:** the MetalFX pass is confirmed cheap; the *real* render-scale-sensitive cost is in the
untimed main pass; and the frame is almost certainly present/vsync-capped, not CPU-bound. **Two follow-ups now
gate the program, cheapest first:** (a) confirm `AutoNoVsync` actually applied (one-line check); (b) the all-
command-buffer GPU timeline (`6zit.15`). Both must land before `6zit.12/13/14` are actionable as frame-rate work.

## Phase 0b RESULTS — frame rate is display/present-cadence bound, not render-work bound (`6zit.15`)

Step (a), the cheap behavioral probe, turned out to be **decisive on its own** — it answered the gate question
before the expensive Instruments trace was needed. Method: run the bench at the cheapest possible setting
(`--metalfx=off --scale=0.25 --uncapped`, ~756×450 render, no upscale) and ask one question: *does mean frame
time move when render cost collapses 4×?* It did not.

**Measured (uncapped, cheap scene — `metalfx=off scale=0.25`):**

| metric | value | reading |
| --- | --- | --- |
| mean frame time | **16.67 ms (60.0 fps)** | unchanged from `scale=1.0` despite 4× less render work |
| frame count / 15 s window | **900 = 60.000 fps exactly** | locked to display vblank count, not work |
| min frame time | **1.73 ms** | real per-frame work can be ~2 ms — huge masked headroom |
| FPS during first ~2 s | **~120 fps** | `AutoNoVsync` *is* active — app can exceed 60 |
| then settles to | **~60 fps, rock-steady** | display-link cadence drops 120→60 once slack is large |

**Full frame-time distribution (1602 frames, startup trimmed) is BIMODAL:**

| cluster | count | share |
| --- | --- | --- |
| ~8.33 ms (120 Hz beat) | 285 | 17.8% |
| < 8.34 ms (>120 fps) | 153 | 9.6% |
| ~16.67 ms (60 Hz beat) | 1253 | 78.2% |
| **within ±1 ms of an 8.33 ms multiple** | **~96%** | — |

**Verdict (closes the gate at the product-decision level; mechanism stated conservatively):** the observed frame
rate is **dominated by display/present/frame-pacing cadence**, not by render-scale-sensitive GPU work, and **MetalFX
cannot raise observed FPS at this scene/hardware.** Well-supported by the data:
- A **bimodal distribution snapping to 8.33 ms / 16.67 ms** (integer display beats) shows the measured frame loop is
  **paced by display/compositor/present cadence**. Free-running work produces a smooth unimodal curve at the true
  work cost (~2-5 ms here), not ~96% of frames on integer vblank multiples.
- **Mean frame time is invariant to a 4× render-cost change** — impossible if observed FPS were render-work-bound.
- **The app hit 120 fps** during warmup, so it is *not* hard-clamped at 60 Hz and `AutoNoVsync` *did* apply; it
  settles to a steady 60 fps once the renderer has large idle slack.

**Reviewer-corrected scope (both Codex and Sakana Fugu flagged the same three overclaims — adopted):**
1. **"Display-cadence quantized," not specifically "swapchain vsync."** The bimodal clustering does *not* uniquely
   prove `CAMetalLayer` vsync blocking. A CPU-side wait that is *itself* display-synchronized — drawable/`SurfaceTexture`
   acquisition, winit/CoreAnimation event-loop pacing, an internal frame limiter, or ProMotion downshift — produces
   the same 8.33/16.67 ms clustering. (A *normal* CPU compute stall would *not* snap to vblank, so "not a plain
   CPU-compute bottleneck" still holds; "the limiter is the swapchain present specifically" does not.) The "120→60 =
   CoreAnimation lowering preferred frame rate" line is *one plausible mechanism*, not established.
2. **`min=1.73 ms` does NOT bound whole-frame GPU+CPU work.** Bevy's `delta_secs` is CPU-side loop timing; GPU work
   is asynchronous (the app can submit fast while prior GPU work is still running). A 1.73 ms sample can be a
   catch-up frame, a nonblocking submit, or a cheap/skipped present. So it is evidence *against a constant CPU-side
   bottleneck*, but it does **not** prove the untimed main render pass is ~2 ms. **The main-pass GPU cost remains
   unmeasured** — the original Phase 0 gap is narrowed, not eliminated.
3. **"Only lever is present cadence" → "for this probe, the next most likely intervention is present cadence."**
   Stated as a universal it overreaches; stated for this scene/hardware it is the right call.

**What this means for the gate.** The gate question — *"is there a MetalFX-solvable frame-rate problem here?"* — is
answered **No** with high confidence, and that is enough to re-scope `6zit.12/13/14` (below). What is *not* yet
proven is the stronger claim *"the app is not GPU-bound at all"* — that still needs the untimed main command buffer +
present/acquire waits. The full **Metal System Trace (step b) is therefore demoted from blocker to optional**: it is
*not* required to act on the product decision, but it *is* the only way to upgrade "not behaviorally FPS-bound by
render scale" to "not GPU-bound," should that stronger claim ever matter.

**Falsifiers (what would overturn the verdict):** GPU timestamps showing main+post+present consistently ≥ 8.33/16.67
ms; frame rate rising with render-scale reduction after forcing true immediate/no-vsync present; sustained 120 fps
after explicitly setting `CAMetalLayer.preferredFrameRateRange` / a `CADisplayLink`; or a heavier scene where
MetalFX-on beats native at identical present settings.

**Answer to "are (a) the present cap and (b) the main pass in scope, even though they're outside MetalFX?"** —
Yes, and they were the whole game. The program's framing inverts:
- **There is no frame-rate problem that MetalFX can solve here.** At this scene complexity on an M5 Max, observed FPS
  is set by present/display cadence, not render-scale-sensitive work — a faster upscale or a lower render scale
  recovers nothing the display will show. (The renderer *can* hit 120 fps, so the slack is real; whether it is
  "parked on the vblank" specifically vs. another display-synchronized wait is the unresolved mechanism detail above.)
- The **most likely** path to >60 fps is the **present/display-link cadence** (get and hold the 120 Hz `CAMetalLayer`
  preferred-frame-rate, or drive presentation off a `CADisplayLink` at 120 Hz) — pure present-path work, no MetalFX.
  This should be *tried and measured* (it is also a falsifier: if forcing 120 Hz present does **not** sustain 120 fps,
  the bottleneck is elsewhere and the trace becomes necessary).
- `6zit.12` (rebuild-hitch), `6zit.13` (quality bundle), `6zit.14` (120 fps governor) are **re-scoped**:
  - `6zit.14` becomes the *primary* frame-rate lever but is **NOT a MetalFX governor** — it is a **present-cadence
    fix** (lock ProMotion to 120 Hz). Retitle/refocus accordingly.
  - `6zit.12`/`6zit.13` survive **only as correctness + quality work** (eliminate the per-frame scaler rebuild;
    fix jitter/mipbias/exposure). Neither will move frame rate on this hardware/scene — that expectation is retired.

**Caveat on generality:** this verdict is for the *current* scene (a single icosphere globe) on an *M5 Max*. A
heavier scene (full simulation overlay, many meshes, higher output res, or a slower GPU) could re-enter a
GPU-bound regime where MetalFX's render-cheap-upscale-to-native trade pays off. The harness + GPU-timing sink built
in Phase 0 remain the tool to re-test that the moment scene complexity grows; the *gate* (`6zit.15`) is closed for
now, not the *capability*.

### Phase 0b two-reviewer reconciliation (Codex + Sakana Fugu)

Both reviewers ran adversarially against the *first-draft* verdict (which asserted "vsync-quantized," "min=1.73 ms
proves the whole frame is ~2 ms," and "only lever is present cadence"). Run independently, **they converged on the
exact same three overclaims** — high signal that these were real, not stylistic:
- **Codex:** "It does not uniquely prove swapchain vsync… a CPU wait on present infrastructure would [quantize too]";
  "`min=1.73ms` overclaimed… frame diagnostics are usually CPU-side loop timing, GPU work is asynchronous";
  "'only lever' overclaiming… safer: frame rate is dominated by display/present pacing *for this probe*."
- **Fugu:** "clustering… is strong evidence [of] display-cadence quantized [pacing]… does **not** uniquely prove
  CAMetalLayer vsync/present blocking"; "min=1.73 ms… is the weakest part… bounds only the measured CPU-side frame
  loop interval, not full GPU completion"; agreed the direction is right but "several claims are overstrong."

Both agreed the **product decision is sound**: display-beat quantization is established, observed FPS is
render-scale-insensitive, and MetalFX does not explain the 60 fps plateau — enough to re-scope `6zit.12/13/14`. The
verdict above was tightened to match exactly what the data supports (and no more), per their convergent feedback.

---

## `6zit.14` RESULT — the present-cadence lever works: 60 → 120 fps (2026-07-10)

Phase 0b predicted the *only* remaining frame-rate lever was present cadence: get and **hold** the ProMotion 120 Hz
cadence, which CoreAnimation was dropping 120 → 60 after warmup. That prediction was falsifiable and is now
**confirmed** — the fix nearly doubled the frame rate.

### The fix (present-path, not MetalFX)

The bead's original note named `CAMetalLayer.preferredFrameRateRange`, but grounding in the actual objc2 bindings
corrected the API: **`preferredFrameRateRange` is a property of `CADisplayLink`, not `CAMetalLayer`** (nor `CALayer`).
The `CAMetalLayer` levers that exist are `displaySyncEnabled` / `maximumDrawableCount` — neither pins the ProMotion
rate. The correct macOS-14+ lever is a `CADisplayLink` created on the Metal `NSView`
(`NSView.displayLinkWithTarget:selector:`) with `preferredFrameRateRange` set to a 120/120/120 `CAFrameRateRange`,
added to the main run loop. The link's callback does **no work** — it is a passive *demand* that advertises to
CoreAnimation that this window wants a sustained 120 Hz cadence.

Implementation: `crates/sw-renderer/src/present_pacing.rs` — a `PresentPacingPlugin` + `run_once` Update system
modeled exactly on the existing `overlay/native_macos.rs` NSView seam (extract `NSView` from `RawHandleWrapper`,
dispatch to the main thread via GCD, set up the link). macOS-only, always on. No MetalFX, no wgpu present-loop
surgery, no change to `present_mode` (still `AutoNoVsync` under bench).

### Measured A/B (same build, same machine, `--bench-quick --metalfx=off`)

| Metric | Baseline (before) | With 120 Hz hint | Δ |
|---|---|---|---|
| Mean FPS | **60.0** | **118.5** (pass 2: 119.2) | **+97.5 %** |
| Mean frame time | 16.67 ms | 8.44 ms | halved |
| **P50 frame time** | **16.66 ms** (60 Hz beat) | **8.34 ms** (120 Hz beat) | snapped one ProMotion beat up |
| P95 | 17.44 ms | 8.86 ms | tight at 120 Hz |
| Frames rendered / 15 s | 900 | 1778 (pass 2: 1788) | ~2× |

The **P50 landing exactly on 8.34 ms** — the 120 Hz ProMotion beat, versus the prior 16.66 ms = 60 Hz beat — is the
decisive evidence: CoreAnimation granted the sustained 120 Hz cadence in direct response to the display-link hint.
This closes the Phase 0b loop end-to-end: the frame rate *was* present-cadence bound, and this hint *is* the lever.

### Smoke-test (normal run, MetalFX = Spatial default)

A non-bench launch holds **~120 fps sustained** (119–122 fps across the whole run, no settle-back to 60), pacing hint
fires, MetalFX plugin initializes, no panic. Confirms the fix is orthogonal to and compatible with the MetalFX path —
120 Hz holds *with* MetalFX active, not only with it off.

### Scope note

This is a `sw-renderer` present-path fix, deliberately **outside** the `bevy_metalfx` crate the epic is named for —
consistent with the Phase 0b re-scope (the frame-rate win was always going to be present-cadence, not MetalFX). The
adaptive-scaling governor that `6zit.14` originally described remains deferred until/unless a heavier scene re-enters
a GPU-bound regime (see the generality caveat above).

---

# Epic `6zit` wrap — quality bundle + crate extraction, then close (2026-07-16)

The final two pieces of chartered work landed, and the epic is retired. Nothing here changes the frame-rate
conclusion (Phase 0b + `6zit.14` already delivered that); this is quality, correctness, and packaging.

## `6zit.13` — quality-fix bundle (correctness/quality only)

Five fixes to the MetalFX **temporal** path in `bevy_metalfx`, each grounded in the actual Bevy 0.18 and
objc2-metal(-fx) 0.3.2 source conventions rather than from memory:

| # | Fix | Where | Why it was wrong / what it does |
|---|-----|-------|--------------------------------|
| 1 | **32-phase Halton(2,3) jitter** (was 8) | `jitter.rs` | 4× the sub-pixel sampling density for the temporal accumulator; first 8 samples still match Bevy's built-in TAA sequence exactly. |
| 2 | **`MipBias(log2(render_scale))`** on the camera | `lib.rs` | Was **unset**. MetalFX renders below native res, so PBR textures sampled a coarser mip than the final image warrants. `MipBias` is a Bevy `Component` (there is **no** MetalFX API for it — a key correction), inserted alongside the resolution override and updated on adaptive scale changes. At scale 0.5 → `-1.0` (one mip sharper). Live log confirms `mip_bias -1.000`. |
| 3 | **Exposure path validated** | `platform.rs` | `setAutoExposureEnabled(true)` was already set and is **correct** per Apple's temporal-scaler contract when no explicit exposure texture is supplied. No change — validation, documented. |
| 4a | **Jitter-sign fix** | `node.rs` | Bevy stores `TemporalJitter.offset` in a clip-space convention that flips Y (`offset * vec2(2, -2)`); MetalFX's `jitterOffsetX/Y` wants pixel space. The code passed Y un-negated. Fixed: negate Y at both the Temporal and FrameInterpolation encode sites. |
| 4b | **Safe format conversion** | `platform.rs`, `node.rs` | Three `std::mem::transmute` calls between `usize` and `MTLPixelFormat` replaced with safe newtype construction / field access (`MTLPixelFormat` is `#[repr(transparent)]`). transmute of an out-of-range discriminant would be UB. |

Verified: 7/7 crate tests pass (added 5 locking in jitter length/centering/TAA-match and the mip-bias formula/clamp);
clippy clean (no new warnings); and the **live release temporal bench ran end-to-end** — the temporal scaler builds
and upscales 1512×900 → 3024×1800 with all fixes active, no panic.

> **Note — the temporal bench independently reproduced `6zit.12`.** Even at a *fixed* scale with `adaptive=false`, the
> `MTLFXTemporalScaler` is rebuilt roughly every ~130 ms (p50 1.4 ms vs p99 72 ms — the hitch). This is worse than
> `6zit.12`'s original framing (which blamed scale *changes*): the rebuild fires without any scale change, so the
> `needs_recreate` predicate is keying on something that flickers frame-to-frame. `6zit.12` remains a real, open bug
> (correctness only — Phase 0b already established it is **not** a frame-rate lever at this scene).

## `6zit.9` — open-source extraction: publish-ready `bevy_metalfx` 0.2.0

Grounded in a full crates.io publication audit. Key finding: **mk already owns and published `bevy_metalfx` v0.1.0**
(github.com/mistakeknot/bevy_metalfx, 25 downloads, since 2026-03-23). So this phase is *next-version prep*, not a
first publish. Changes:

- **Version 0.1.0 → 0.2.0** — additive (temporal, adaptive scale, gpu-timing, the `6zit.13` quality fixes); no breaking removals.
- **README compile-fail fixed** — the flagship Quick Start `MetalFxPlugin { .. }` example was missing `adaptive` and the macOS-only `gpu_timing_sink` field with no `..default()`; it would not compile. Now uses `..default()` and documents the `Default` impl.
- **Frame interpolation honestly labeled EXPERIMENTAL** — the `frame-interpolation` feature compiles and wires an `MTLFXFrameInterpolator`, but it is **not production-ready**: camera params are hardcoded (`node.rs:940`) and the current→previous color copy is unimplemented (`node.rs:982`), so the interpolator never sees a correct previous frame. The README now warns loudly; completion is tracked in `6zit.8`.
  > **Superseded by `6zit.8`** (see the final section): both TODOs are fixed, and a third defect — the interpolator was at the wrong pipeline stage — was found and fixed. The EXPERIMENTAL label has been dropped; what remains is a documented limitation, not an instability.
- **GPU-timing diagnostics documented** — previously undocumented public API, now framed as an opt-in profiling facility.
- Publication posture verified: **zero path/git deps** in the crate, all metadata present, non-macOS stub complete, `# Safety` docs on all `unsafe fn`. `cargo publish --dry-run` packages cleanly as 0.2.0.

The **actual `cargo publish` is mk's credentialed, outward-facing step** — this phase makes the crate ready; it does
not push it.

## Epic close — what shipped, what's deferred

The `6zit` epic delivered its charter: MetalFX spatial + temporal upscaling integrated into a Bevy render graph node
and published as a reusable crate; the "why isn't it 120 fps" question answered empirically (Phase 0/0b: present-cadence
bound, not GPU-bound at this scene); and the 120 fps headline delivered via the present path (`6zit.14`). 13 of 15
children closed.

Two children are **genuinely incomplete and were spun out as standalone follow-ups** rather than falsely marked done:

- **`6zit.12`** — the scaler-rebuild hitch (real correctness bug, reproduced above; fix = `setInputContentPropertiesEnabled` + min/max scale for true dynamic resolution).
- **`6zit.8`** — frame-interpolation completion (implement the prev-frame color blit + extract real camera params from Bevy's `Projection`; gate on macOS 26).

Plus one new pre-1.0 API-tightening follow-up (`shadow-work-3hvh`): feature-gate the temporal/frame-interp code at
compile time (today the features gate runtime enablement but not the compiled surface) and narrow `pub` fields on
internal render-world resources. Held out of 0.2.0 to keep the release additive and safe.

---

# `6zit.12` RESULT — the temporal scaler-rebuild hitch was a control-flow bug, not a scale-change cost (2026-07-16)

The `6zit.13` temporal bench (above) surfaced this: even at a *fixed* render scale with adaptive off, the
`MTLFXTemporalScaler` was being rebuilt roughly every 130 ms. The bead assumed the hitch came from *scale changes*
triggering rebuilds and prescribed dynamic resolution as the fix. The real cause was more fundamental.

## Root cause — a fall-through that discarded every scaler it built

In `MetalFxUpscaleNode::run`, the background-thread scaler was received into `*cached = Some(..)`, but the
`Ok(Some(scaler))` receive branch (unlike its sibling branches) had **no `return`/skip**. Because `needs_recreate`
was still `true` (computed at function entry, when `cached` was `None`) and the creation guard had been weakened to
`pending.is_none()`-only — now `true`, since the receive had just cleared `pending` — execution fell straight into
the *create-new* block, nulled `cached`, and spawned another background thread. The freshly-built scaler was
discarded on the very same frame and **never rendered once**. Diagnostics confirmed `cached=None, pending=true` on
every frame, with the scaler received and instantly re-queued.

**Fix:** restore the `cached.is_none() && pending.is_none()` creation guard, and null `cached` in the
dimensions-changed branch so a genuine window resize still recreates.

## Measured — the hitch is gone

Temporal, `--scale=0.5 --uncapped`, before (with the bug) vs after:

| Metric | Before (hitch) | After | Change |
|--------|---------------|-------|--------|
| scaler creations | ~hundreds (every ~130 ms) | **1** across 1798 frames | eliminated |
| p50 | 1.40 ms (degenerate) | 8.33 ms (the 120 Hz beat) | — |
| **p99** | **72.81 ms** | **9.10 ms** | **−87.5%** |
| stdev | 14.62 ms | **0.46 ms** | −97% |
| max | 85.84 ms | ~12 ms | −86% |
| GPU samples | 0 | **240** | timing unblocked |

The old `mean_fps` of 173.7 was an artifact of the bug (the temporal path skipped frames every rebuild cycle, so
`delta_secs` measured degenerate loop timing). The honest, present-capped rate is ~120 fps. And `gpu_samples` going
`0 → 240` matters beyond this bug: the rebuild had been *blocking temporal-mode GPU-timing capture* the whole time —
now the temporal path reports `gpu_frame_fraction ≈ 0.04`, verdict "likely CPU/sim/present-bound", **re-confirming
Phase 0b's thesis directly on the temporal path** (not just the spatial control).

## True dynamic resolution (the bead's original prescription, still delivered)

With the hitch fixed, dynamic resolution is now an *adaptive-path enhancement* rather than the hitch fix: it lets the
governor flex render scale without recreating the scaler. Implemented via `setInputContentPropertiesEnabled` +
`setInputContentMin/MaxScale` on the descriptor, plumbed through a `MetalFxConfig.dynamic_res_range` (`Some` only in
adaptive mode). The scaler is created at output dimensions with dynamic res on, and the existing per-frame
`setInputContentWidth/Height` selects the actual content size each frame.

> **Gotcha worth recording:** MetalFX's `inputContentMin/MaxScale` are **upscale ratios** (`output/input`, always
> ≥ 1.0), *not* the render-scale fractions the rest of the codebase uses. Passing a fraction < 1.0 makes
> `newTemporalScalerWithDevice` return `nil` (silent creation failure). Convert by reciprocal, swapping min/max: a
> render range of `0.5..=0.75` maps to a MetalFX scale range of `1.333..=2.0`.

Verified: an adaptive run does **1 scaler creation** across a live `0.5 → 0.75` governor change — zero rebuilds. The
non-adaptive path is byte-identical to before (`dynamic_res_range = None` ⇒ the scaler input equals the current
input), so the common case is provably untouched.

With `6zit.12` resolved, only `6zit.8` (frame-interpolation completion) remains open under the (closed) epic.

---

# 6zit.8 — frame interpolation: correct, and still not useful

`6zit.8` was filed as a two-item cleanup: implement the missing current→previous colour blit, and replace the
hardcoded camera parameters with real ones from Bevy. Both are done. But grounding the work in Apple's headers and
the Metal debug layer turned up a third problem the bead did not know about, and that problem is the one that
actually decides whether the feature is worth shipping.

## What the bead asked for (done)

| Fix | Before | After |
|---|---|---|
| Previous-frame colour history | never written — `prevColorTexture` held uninitialised memory for the life of the process | `copy_texture_to_texture` from the upscaled frame, encoded *after* the interpolation pass so the pass still reads the genuine previous frame |
| Camera parameters | hardcoded `1/60`, `45°`, `0.1`, `1000` | read from the `Projection` component that `extract_cameras` clones into the render world; delta from a `Time` mirror resource |
| FOV units | Bevy radians passed straight into a degrees API | `fov.to_degrees()` — a silent ~57× error otherwise |
| Reverse-Z | relied on an undocumented default | `setDepthReversed(true)` stated explicitly, matching Bevy's infinite reverse-Z projection |
| Device gating | already correct | unchanged — `MTLFXFrameInterpolatorDescriptor::supportsDevice` |

`Time` needs the mirror because the render world genuinely has no `Time` resource — `bevy_time` only plumbs an
`Instant` channel between the worlds — whereas `Projection` needs nothing, because `extract_cameras` already does
`commands.insert(projection.clone())` onto the view entity.

## The problem the bead missed: wrong pipeline stage

The mode was wired as an *alternative* to upscaling — low-res colour in, full-res interpolated frame out. Running
under `MTL_DEBUG_LAYER=1` says otherwise, immediately and unambiguously:

```
MetalFXDebugError.h:29: failed assertion `Color texture width mismatch from descriptor'
```

Reading the headers explains it. `MTLFXFrameInterpolatorDescriptor.inputWidth` is documented as "the width of the
input **motion and depth** texture"; `outputWidth` is "the width of the **output colour** texture". The colour
textures — both `colorTexture` and `prevColorTexture` — live at *output* resolution. The interpolator belongs
**after** the upscaler, not in place of it, and `MTLFXTemporalScaler` conforms to `MTLFXFrameInterpolatableScaler`
precisely so it can be attached to the descriptor and chained.

So `FrameInterpolation` now holds *both* objects and encodes two stages per frame: temporal upscale to full res,
then interpolate between that and the previous full-res frame using content-sized depth/motion.

## Result

Fixing the staging removed a pathology that had been read as noise:

| Metric (bench-quick, 3024×1800, scale 0.5) | Before | After |
|---|---|---|
| Frame p99 | 126–148 ms | **17.9 ms** |
| GPU p99 | 131.9 ms | **5.8–8.7 ms** |
| Metal debug layer | assertion failure | **clean** |
| Normal (vsync) run | erratic, 6–200 fps | **~120 fps, stable** |

The old triple-digit p99 was not thermal noise or CPU contention: it was MetalFX being handed size-mismatched colour
textures every frame.

## Complete, stable — and still not useful

Frame interpolation only buys frame rate if you present the interpolated frame **and** the real one, paced to the
display refresh — two presents per simulated frame. A Bevy render graph presents its swapchain once per
`App::update()`. This node therefore presents the genuine upscaled frame and leaves the interpolated frame in an
offscreen texture.

Net effect today: visually identical to `temporal`, plus the cost of the interpolation pass — GPU mean ~5.5–7.1 ms
against well under 1 ms for `temporal` alone on the same scene. That is a real cost for no visible benefit, and no
amount of work *inside* the render-graph node changes it. Display-timed dual presentation lives below Bevy's render
graph, in the same layer as the `CAMetalLayer` work from `6zit.14`.

So the honest state is: **the MetalFX usage is correct and validated; the feature is unrealized.** The path is no
longer *experimental* — it is debug-layer clean, tested, correctly gated, and holds 120 fps — so the README no longer
labels it that way. What it carries instead is a known-limitation note telling callers not to enable the feature
unless they are also building the presentation half, since `temporal` gives the same picture far more cheaply. The
presentation work is tracked separately.

The useful research finding is the cost figure itself — MetalFX frame interpolation is roughly a 5–7 ms/frame GPU
tax at 3024×1800 on an M5 Max. Against this project's measured ~120 fps present-capped ceiling (8.3 ms/frame), that
tax only pays for itself if dual presentation actually doubles the presented rate. That is a real question, and now
a measurable one.
