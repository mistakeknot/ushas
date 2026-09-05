# Creation-fault window attempts stopped before injection

Neither September 5 attempt reached an accepted initial render or injected-fault
transition. Both wrappers failed. These runs provide **no creation-failure,
slow-creation, fallback or recovery acceptance**, and do not reproduce the
historical MPSGraph crash.

Both used clean source `9a90c3d83c71a5083bad160af7cbd445409970c1`, binary SHA256
`29a28b2a15a0b7663c6b2d662d8cfea2861577afbf0703c22bffe83a1fc54c7d`, Temporal at
scale 0.5, adaptive enabled with explicit 60 FPS, and Metal API validation.
The manifests record macOS 26.5.2 (25F84), Mac17,7; the logs identify Apple M5 Max.

| Requested exercise | Retained result | Child / wrapper exit |
|---|---|---|
| `creation-failure` | Monitor removed, then window closed about five seconds after launch. No final JSON or capture survived. | 0 / 1 |
| `creation-slow` | Initial phase timed out at 25.0017 seconds. Its only retained observation was stale `NoRender`, generation 0 / fault `Off`. No lifecycle captures or injection event. | 1 / 1 |

The first child's zero exit is ordinary window-close termination, not a passing
test; its wrapper correctly rejected the missing report. With no final ledger,
that run cannot establish an injected transition. The second run logs normal
Temporal scaler creation only around 34 seconds after launch, after its initial
lifecycle deadline. Its final 1280×720 PNG decodes to entirely zero RGBA pixels,
including alpha, and is retained as a failed readback rather than rendered output.
No changed-phase fallback image or restored-phase image exists for either run.

The second report records `display_awake_at_finish: false`. A later outside-sandbox
CoreGraphics/session probe, reported in tool output, found awake false, locked
true and on-console true. That sample was not retained with these runs and is
not a contemporaneous causal measurement. These failures justify separating
offscreen creation-fault checks from native window availability; they do not
establish a fault-seam regression or successful OS sleep/resume behavior.

The [machine-readable audit](lifecycle-fault-window-attempts-02.json) retains the
failed classifications. Its separate archive at
`/Users/sma/projects/docs/ushas/evidence/lifecycle-fault-window-attempts-02/`
contains all eight original artifacts, a copy record and a CPU audit: **10 payload
files, 42,817 bytes**, excluding the manifest. All copies matched source-before,
source-after and destination hashes. The
[manifest](/Users/sma/projects/docs/ushas/evidence/lifecycle-fault-window-attempts-02/archive-manifest.json)
SHA256 is `bd8a7d1c1592f8e1bc2dfbe0d9fc879f8bd44722371cdd1471ebfb89c9666753`.
Missing reports and phase captures are deliberately not synthesized.

```sh
PYTHONDONTWRITEBYTECODE=1 python3 \
  /Users/sma/projects/docs/ushas/evidence/lifecycle-fault-window-attempts-02/audit.py
```

The remaining gates are actual injected failure/slow fallback and recovery,
observed native minimize/restore, and externally initiated system sleep/wake with
render recovery. Offscreen image checks can address the first gate only. Earlier
failed resize evidence and the unreproduced MPSGraph report remain unchanged.
