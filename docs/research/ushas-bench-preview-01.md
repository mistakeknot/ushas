# Ushas Bench preview validation

The benchmark, comparison, stress and results flows are implemented in the
native macOS preview. Final hardware qualification remains open. This receipt
separates earlier integration checks from the packaged candidate's acceptance
run; it does not claim a completed 24-arm comparison or ten-minute stress test.

The [approved plan](../plans/2026-09-05-ushas-bench.md) defines the acceptance
contract. The [app guide](../../tools/benchmark/README.md) explains the metric
and preview distribution. Task: `shadow-work-vzox.9`.

## Candidate

The package was built from clean source
`88a2f54ca823910dbe03ca1fb2515c0abf74740f`. It contains an arm64 SwiftUI launcher,
the Rust/Bevy renderer, character attribution and the project's license texts.
The ZIP contains an ad-hoc-signed macOS 26 preview app, not a notarized public
release.

Artifacts are in `tools/benchmark/dist/preview-02/`:

| Artifact | SHA-256 |
|---|---|
| `Ushas Bench.zip` | `dc98a5f63d896461ca3382fe775dde3388a3ddd7de4980ae15af5e56539e38f4` |
| Bundled `Contents/Helpers/ushas-bench` | `f8501d3b91b15cec8b17ffb073a5a16a84cd1bf36eeb9f242835af1acb855786` |
| `Contents/MacOS/UshasBench` | `ba261e53b601550f2f1acd17902844d0ff9b7df38c9185730c6fde560586479d` |

Root verified the hashes and deep, strict code signature. The extracted ZIP's
two executable files match the packaged originals byte for byte. The extracted
app launched outside the repository from `/private/tmp`, with
`PATH=/usr/bin:/bin:/usr/sbin:/sbin`, without a Rust toolchain or external assets.
Its initial 1180 × 800 window exists; the sampled main thread was idle in the
normal AppKit event loop.
On the first final-candidate UI attempt, macOS reported the desktop locked;
visible interaction and measurement had not started.

## Integration evidence

These checks exercised earlier candidate revisions. They are useful regression
evidence, not substitutes for the final package's complete qualification.

| Check | Retained evidence | Observed result |
|---|---|---|
| Actual app, native benchmark, dirty integration build `f87830c` | App run `717e593a-33f5-41e5-8667-1f8e6c2d2f6a` | Three valid 1,200-frame chapters at 2560 × 1440; Results displayed the 120.9 completed-render FPS geometric mean. |
| Temporal two-thirds image replay, clean `6a744734` | `/private/tmp/ushas-bench-temporal-frozen-01` | Four identified 1280 × 720 material-scene images from a custom 120-tick replay; original screenshot joins valid and the scene/HUD inspected. |
| Actual app comparison and Stop, clean `6a744734` | App run `e7987218-56e3-4585-b2d8-841d09e19f00` | First three arms contained 10,800 valid proofs; later Temporal-half output became stale. Stop ended the comparison, retained five attempts, and withheld the score and paired summaries. No helper process remained. |
| Actual offline export of that cancelled comparison | `/private/tmp/ushas-bench-export-cancelled-01` | HTML, launcher log, original comparison and all five child reports retained; 31 exported files checked byte for byte. |
| Temporal-half retry, clean `35d80194` | `/private/tmp/ushas-bench-half-diagnostic-01` | All 3,600 frames qualified at 2560 × 1440; geometric mean 125.95. The window was visible, focused and reported unoccluded. |

App runs and their launcher logs are under
`~/Library/Application Support/Ushas Bench/Runs/`. The earlier failed comparison
remains invalid: its observed effect frame froze at 191 while requested frames
continued. The cause was not established. The successful retry does not prove
that cause or erase the failure.

The renderer now retains bounded window, target and effect-readiness diagnostics
and stops on a measured qualification failure. Escape in a comparison child
propagates cancellation to the parent. During active rendering a scoped public
`NSProcessInfo` activity holds idle display/system sleep; the activity ends with
the engine lifetime and changes no persistent power setting.

CPU validation before packaging covered 46 Rust benchmark contracts, 17 Swift
app contracts and three shared-model tests. Strict Clippy and formatting checks
passed. All 20 jobs in [candidate CI run
33993163591](https://github.com/mistakeknot/ushas/actions/runs/33993163591)
passed at the packaged source revision, including the benchmark and native-app
contracts. The remaining hardware gates must finish before this task closes.

## Remaining acceptance

- Run one uninterrupted four-round comparison: 24 fresh benchmark processes,
  three 1,200-tick chapters per process, followed by six separate image replays.
- Independently audit frame proofs, configuration/build identity, paired
  summaries and retained PNGs. Inspect native/reconstruction image pairs,
  including the camera cut and recovery ticks, in Results.
- Run stress for the full 600 seconds, exercise live load changes, and verify
  distinct reporting epochs. Verify actual Stop and Escape behavior.
- Export a completed comparison and verify the portable report and images.
- Record final CI, package provenance and hardware outcomes, then close the task.

## Interpretation and bounded timing assessment

The number measures completed-render throughput of this windowed Bevy path.
It includes CPU scheduling, drawable acquisition and completion-callback
dispatch. Values near display cadence do not establish GPU capacity. Immediate
presentation is requested; the resolved surface policy is unavailable through
the selected public Bevy interface. The app makes no GPU-busy, panel-FPS or
frame-pacing claim. Quality review remains separate from timing qualification.

The bounded public-API assessment did not identify a complete GPU-only frame-cost
producer. The dedicated MetalFX command-buffer interval includes dependencies;
selected public wgpu pass timestamps do not cover MetalFX's private encoders.
The earlier [proxy stop decision](proxy-validation-control-03.md) remains in
force. No new trace pair was collected: missing scope already prevents the
proposed runtime signal from qualifying. Any future unwrapped offline Metal
System Trace still needs explicit original-frame and physical-buffer ownership.
Automatic adaptation retains `TimingUnavailable`.
