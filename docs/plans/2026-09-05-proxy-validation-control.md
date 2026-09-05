# MetalFX proxy validation-layer control

This newly authorized diagnostic follows the stopped experiment in
[metalfx-proxy-02](../research/metalfx-proxy-02.md). It does not amend that
experiment's result or selector inventory. The hypothesis is that Metal API
Validation contributes the observed `globalTraceObjectID` invocation. Its
caller and semantics are currently unknown.

## Bounded protocol

Freeze one reviewed source revision and optimized executable. Run four fresh
processes, serially: Spatial OFF with validation 1, CALLS with validation 1,
OFF with validation 0, CALLS with validation 0. Each has the existing 16-frame,
160x90 to 320x180 deterministic fixture and two live slots. Compare CALLS only
to its same-validation OFF reference. Retain cross-validation pixel comparisons
separately. A failure in either pair is retained; both pairs are needed to test
the hypothesis. Each child has the existing 65-second external watchdog.

The runner sets `MTL_DEBUG_LAYER` explicitly to 1 or 0 and fixes
`MTL_SHADER_VALIDATION=0` before process launch. Apple documents that these
[validation controls](https://developer.apple.com/videos/play/wwdc2020/10616/)
are latched before device creation. Other inherited MTL settings are removed
consistently; inherited library injection is rejected. Retain the effective
controls, exact argv, compiler/SDK/OS, clean source revision, binary hashes,
actual runtime class identity, exit status, full logs and untouched outputs.

Enable the bounded unknown-selector caller diagnostic in both pairs. It adds
synchronous CPU work, so these runs supply attribution and compatibility
evidence only, not timing or instrumentation-overhead measurements. Missing
symbols remain missing evidence. No selector arguments or application data
are inspected. Root schedules all GPU runs and heavy builds; other application
GPU activity is not controlled, so no exclusive occupancy claim is possible.

## Decision

The existing strict analyzer, unsupported-selector rejection, command-buffer
status guards and exact raw/composed pixel requirements remain unchanged.
Unknown selectors make a frame unavailable even when execution and pixels
succeed. A stack identifies a calling path, not the undocumented selector's
semantics. Disabling validation cannot clear a failure observed with validation
enabled or establish production safety.

If CALLS with validation 0 is accepted against its valid reference, review its
inventory and attribution before a separate counters attempt. Spatial and
Temporal counter coverage, whole-process trace reconciliation, instrumentation
cost and an independently reviewed Bevy ownership integration remain gates.
None of these four diagnostic arms supplies governor input.

If both settings still encounter an unknown selector, or forwarding/output
fails, stop this proxy route. Do not add a private-selector exception, swizzle,
or expand into a backend rewrite. Record the concrete next measurement design
decision and preserve `TimingUnavailable` for automatic adaptation. Existing
fixed presets and offline trace measurements remain usable within their stated
scope.
