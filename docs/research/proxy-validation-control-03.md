# MetalFX proxy validation control: same rejection with validation off

The four-arm diagnostic reached its predeclared stop condition. Both unwrapped
Spatial references passed strict analysis. Both calls-only proxy arms rejected
all 16 observations because `globalTraceObjectID` remained outside the declared
public selector inventory. Turning Metal API validation off did not remove the
invocation in this fixture. The proxy route is stopped; automatic adaptation
still has no validated GPU-cost input and retains `TimingUnavailable`.

This is a new experiment under the
[prospective validation-control protocol](../plans/2026-09-05-proxy-validation-control.md).
It does not amend the [earlier failed experiment](metalfx-proxy-02.md), relax
selector rejection, or convert matching pixels into accepted compatibility.

## Controlled runs and observed result

All four fresh processes used clean source
`4e3b115d8427cb4b8e7c9025cef993f98c6d2c30` and one optimized executable,
SHA-256 `8779e580aae2563f338930beeb305024ea6ce7da1635dac7d7ab5b9b15869f8e`.
The build used Apple Clang 17.0.0, SDK 26.2 and strict warnings. Execution used
an Apple M5 Max on macOS 26.5.2 build 25F84. The fixture renders 16 deterministic
160×90 RGBA16Float inputs to 320×180 outputs, with two live admission slots.

| Run, in execution order | `MTL_DEBUG_LAYER` | Child exit | Completed frames | Strict result |
|---|---:|---:|---:|---|
| Spatial OFF | 1 | 0 | 16 | Valid reference |
| Spatial CALLS | 1 | 1 | 16 | All observations unavailable |
| Spatial OFF | 0 | 0 | 16 | Valid reference |
| Spatial CALLS | 0 | 1 | 16 | All observations unavailable |

The runner fixed `MTL_SHADER_VALIDATION=0` and enabled the bounded unknown-call
stack diagnostic in every process. Each run retained its explicit environment,
argv, unique PID, unchanged binary hashes, log and exit status. All finished
without watchdog timeout or interruption. The runs occurred serially between
16:14:09 and 16:15:16 UTC on September 5, 2026; other applications' GPU activity
was not controlled. These are compatibility diagnostics, not performance
replicates or measurements of instrumentation overhead.

With validation 1, the device was `MTLDebugDevice` and all recorded real command
buffers were `MTLDebugCommandBuffer`. With validation 0, they were
`AGXG17CDevice` and `AGXG17XFamilyCommandBuffer`. These observed class differences
corroborate the environment intervention. They do not establish every internal
validation setting or any undocumented selector's semantics.

Each CALLS frame recorded exactly one `globalTraceObjectID`, followed by three
`renderCommandEncoderWithDescriptor:` calls. The labels were
`MetalFX_Normalize`, `MetalFX_Scale` and `MetalFX_Sharpen`. Across the two CALLS
arms this is 32 unknown invocations and 96 observed render encoders, with no
dropped selector records and no requested GPU counter samples. Each observation
retained `unsupported_selector` and `available:false`.

All 192 setup/MetalFX/final command-buffer completion records across the four
arms report Completed with no error. This does not clear observation rejection:
the strict analyzer rejects both CALLS arms, including their same-validation
reference comparisons. The independent artifact audit decoded all 64 raw
half-float outputs and all 64 opaque PNGs. For each of the 16 corresponding
frames, raw file bytes, PNG file bytes and decoded RGBA8 pixels match across all
four arms. This includes 32 within-validation OFF↔CALLS frame comparisons and
the cross-validation comparisons. That is pixel equality for this synthetic
gradient fixture; it does not clear either strict compatibility rejection.

## What the stacks establish

All 32 captured unknown-call stacks contain the MetalFX frame
`-[_MFXSpatialScalingEffectEFFECT_NAME_V1 encodeToCommandBuffer:]` at symbol offset
`0x198`, through CoreFoundation's forwarding path to the observer. The captured
selector is ordinal 1 in each original frame ledger. The actual target classes
match the corresponding validation setting. Stack collection remains bounded,
before forwarding, and adds synchronous CPU work.

This identifies an observed MetalFX calling path in both settings. It rejects
the narrow explanation that enabling the tested validation setting is necessary
for this invocation. It does not prove that validation has no influence, reveal
what `globalTraceObjectID` does, establish harmlessness, or identify all GPU work
the framework may submit. No private method was added to the supported inventory.

## Measurement decision

Stop this public-selector proxy route. No counter arm, Temporal proxy arm,
new trace capture or Bevy/HAL integration follows from this diagnostic. A backend
rewrite and a private-selector exception are outside the bounded decision.

The selected alternative for future profiling is **unwrapped, offline,
whole-process Metal System Trace** with explicit physical command-buffer and
encoder attribution. It supplies a profiling workflow, not a runtime producer.
Any future capture still needs native/Temporal controls, complete scope and
exclusions, original frame ownership, and inspection of waits and overlapping
stages. Labels alone cannot supply ownership. The
[Bevy inventory](bevy-frame-scope-02.md) documents the unresolved uploads, shared
work and submission boundaries; the
[native stage investigation](gpu-producer-02.md) remains a partial positive
control, not validation of a complete Bevy/MetalFX frame cost.

No new capture is needed to make this stop/design decision. Dedicated-buffer
`GPUStartTime`/`GPUEndTime`, outer markers and CPU-inclusive serial completion
cadence remain unsuitable substitutes for the missing governor input. Existing
fixed-preset and consumer recommendations keep their original evidence scope.

## Retained evidence

The artifacts are under
`/Users/sma/projects/docs/ushas/evidence/proxy-validation-control-03/`:
`build/` retains the source archive, executable, build receipt and logs; the four
`spatial-{off,calls}-validation{1,0}` directories retain original JSONL, raw
half-float outputs and PNGs, with sibling run receipts, strict analyses and logs.
The [machine-readable report](proxy-validation-control-03.json) records exact
artifact hashes and separates rejected compatibility from audit status.

The independent auditor reproduced all four strict analyses, checked original
frame/slot lifetimes and caller paths, and matched all 139 frozen Git blobs to
the recorded source. Its five corruption checks passed. The final audit result
is `results-02.json`, SHA-256
`268f1725f6c57967b5a00ecf715ff4b09ab215509535b835c8dd4ebf8b25b159`;
the auditor is `audit.py`, SHA-256
`6c6c32fd37fe553e9939e3c14e3b4c3792fd0c08d5f5741e21ec6330c8ee0fb1`.
The earlier `results.json` remains unchanged; revision 02 also verifies the
retained standalone build script.

The audit's input inventory covers 158 payloads, 36,894,598 bytes, excluding its
own source and result files. Root reproduced the audit; the result is byte
identical to `results-02.json`. The complete
[archive manifest](/Users/sma/projects/docs/ushas/evidence/proxy-validation-control-03/archive-manifest.json)
covers 163 payloads, 37,102,350 bytes, including auditor and results. Its SHA-256
is `b5d7c63a5a8a97198105f2e0cdfd69526e4337134cc0ce41fb28b2a8e9900d68`.
`audit_consistent:true` describes faithful retained evidence;
`compatibility_accepted` and `validated_for_governor` remain false.
