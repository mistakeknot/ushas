# Candidate window lifecycle checks

**All five fixed-scale Temporal lifecycle runs passed** on clean
`56a3b16c8c1c8f12a5320adc5082c6d20b6378c1`. Each child and wrapper exited zero,
with valid phase captures and no dropped lifecycle observations. These checks
establish window/camera transitions, observed render state and recovered images.
They do not measure performance or physical panel delivery.

The runs used Apple M5 Max, macOS 26.5.2 / 25F84, and the frozen executable
`5c773aba90f70229d03f1ac87045045f11a6f2e435bb9bc21f55e1b0266b4744`.
They ran sequentially September 5, 2026, 03:37:02–03:38:33 UTC with static Claude,
Temporal scale 0.5, MSAA off, LDR, and an initial 1280×720 window. Adaptive mode
and serial-completion instrumentation were disabled. Scale remained 0.5 and
adaptive epoch remained zero throughout.

| Exercise | Observed transition and recovery |
|---|---|
| Resize | Output 1280×720 → 960×540 → 1280×720; actual content 640×360 → 480×270 → 640×360. Rebuilds reported `Pending/ScalerPending`, then `Encoded/BlitPipelinePending`, then `OutputWritten`. Changed/restored phases retained 23/24 distinct ready view frames at the intended dimensions. |
| Camera cut | A 0.25-radian yaw and reset were requested at main frame 28; reset acknowledgement appeared at main frame 30. The changed view recovered with a valid capture. |
| Late camera | Startup had no camera. Main frame 32 spawned view `4294966911`; frame 62 replaced it with `4294966908`. Both reached half-resolution input and `OutputWritten`; the restored phase contained only the replacement view. |
| Multiple views | Views `4294966908` and `4294967072` both reported `Unavailable/MultipleViewsUnsupported` across 20 distinct observed frames. Removing the second camera at main frame 50 restored the original view and a valid image. The unsupported phase was checked through per-view status and was not captured. |
| Inactive cut/resume | Reset requested at main frame 28 remained pending while the camera was inactive. Its last render observation, frame 27, became stale. Resume at main frame 354 still carried the request; acknowledgement appeared at 356, followed by a valid recovered image. The inactive interval was 0.301 seconds across 326 main updates. |

Every captured phase retained at least 20 distinct ready view frames. The
[compact audit](lifecycle-candidate-01.json) records events, identities, phase
counts and capture hashes. All **21 PNGs** were independently decoded and were
fully opaque, correctly sized and nonblank; eleven are lifecycle phase images.
Six changed/restored images were visually inspected. Claude's faces, rays and
thin rails remained recognizable, and the native-resolution UI remained readable.
Resize returned to the original geometry; cut/resume showed the requested changed
viewpoint. These are recovered-state captures after readiness, not a frame-by-frame
measurement of post-cut ghosting or temporal recovery.

Reset acknowledgement proves CPU temporal reset-command encoding. Effect
observations can lag the main frame; their frame identities remain separate.
These runs do not prove GPU completion, adaptive hardware epoch resets or
GPU-driven convergence. Actual OS sleep/resume, occlusion/minimize/unlock recovery,
and forced driver/scaler creation failure still need separate evidence. The
historical MPSGraph cold-start SIGSEGV remains unreproduced, not resolved by these
successful starts.

The operator's outside-sandbox CoreGraphics preflight reported `cg_error=0`,
main display 1 and `asleep=false`; that output was observed in the tool session
and has no retained probe file. Each run reports `display_awake_at_finish=true`.
Continuous awake state, session-unlocked state and on-screen visibility were not
measured during these runs. A later outside-sandbox CoreGraphics session probe
reported display 1 awake, `on_console=true`, and `locked=true`; that tool-session
observation is not contemporaneous run metadata. These are window-target lifecycle
checks, not verified visible presentation. The preceding sandbox probe reported
unavailable display state and is excluded.

The archive at
`/Users/sma/projects/docs/ushas/evidence/lifecycle-candidate-01/` contains **43
hash-verified payloads, 8,749,674 bytes**: 36 candidate artifacts, six earlier
failed-resize artifacts under `history/`, and a CPU-only `audit.py` that can
recheck the archived candidate files. Its manifest SHA-256 is
`11361dc40f39f9d8a9b4bfa1430bacc1079741e8acdfa4b4507a9b3da5599927`.
Earlier `lifecycle-resize-02` exhausted readiness with `NoRender`;
`lifecycle-resize-03` exited its child zero but its wrapper failed with a missing
report/captures. Both remain invalid evidence. Original artifacts remain intact.
