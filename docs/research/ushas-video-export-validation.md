# Ushas video export validation

The packaged app exports scoreless 2560 × 1440, 60 fps H.264 replays of the
standard lab sequence. Native and Temporal half full sequences decoded to
exactly 1,800 frames over 30 seconds. Native and Spatial half Geometry exports
decoded to exactly 600 frames over 10 seconds. All decoded timestamps equal
frame index / 60, with no audio and Rec.709 primaries, transfer and matrix tags.

Task: `mk-4kbu`. Validation ran on Apple M5 Max with macOS 26 on 2026-09-05.
The [app guide](../../tools/benchmark/README.md) and
[stream contract](../../tools/benchmark/VIDEO-CONTRACT.md) define the behavior.
Export preserves the renderer's 120 Hz simulation and temporal history; these
movies carry no benchmark score or claim of reconstruction quality equivalence.

## Package and source

`tools/benchmark/dist/video-preview-04/` contains the final app, ZIP and
`SHA256SUMS`. The launcher was built from clean `a842056`; the unchanged
renderer was built from clean `e10344b9247bd89108b2fbc123b8a98ea07ff591`.
The final app was invoked from that directory. Deep/strict signature checks,
ZIP integrity and every manifest hash passed.

| Artifact | SHA-256 |
|---|---|
| `Ushas Bench.zip` | `f9f19e6bf02ce30185e7336c19ee9ae60a0f454e6e1d5441ed27429306a2681f` |
| `Contents/MacOS/UshasBench` | `5510c5ae51cda15f4d01c255b2466ec500f0cb0ae5069960b14d43dd3bf34bd5` |
| `Contents/Helpers/ushas-bench` | `7930a829891c77a68610519f016eaced230d43ca8f6fe6189d9cf6673225fc26` |
| `Contents/Helpers/ushas-video-encoder` | `6769313ea2af7a2aeb201b1eab9975f62ef9323daeb56dfc54461d41d75e5c93` |

The encoder is bundled and signed with the app. ffmpeg/ffprobe are developer
verification dependencies only. This remains the existing ad-hoc-signed preview
distribution, not a notarized public release.

## Decoded movies and visual inspection

Receipts, movies, samples, UI observations and failed attempts remain under
`tools/benchmark/dist/video-validation/`. App run directories remain under
`~/Library/Application Support/Ushas Bench/Runs/`.

| Render | Evidence | Decoded result |
|---|---|---|
| Native, all chapters, packaged UI | Run `1e9162cd-7579-4c5e-a0d5-b14fb210cf06`; `packaged-native-decoded/decoded.json` | 1,800 frames, 30 seconds |
| Temporal half, all chapters, CLI | `temporal-01/result.json`; `temporal-01-decoded/decoded.json` | 1,800 frames, 30 seconds |
| Spatial half, Geometry, packaged helper | `spatial-geometry/result.json`; `spatial-geometry-decoded/decoded.json` | 600 frames, 10 seconds |
| Native, Geometry, packaged Save replacement | Run `70728cc0-350c-498c-abbe-a0927856c333`; `packaged-geometry-decoded/decoded.json` | 600 frames, 10 seconds |

The full packaged Native movie hashes to
`4be6f61c7291dc1a32c8dbe61aba1f6cab76ea09360a0e1475580b264851badf`;
the replacement Geometry movie hashes to
`a1e741f3d38e7a8364cb53ea1e716086387bd6875abb4d76a1bf504cb1298eaa`.
The decoder independently recomputes the movie hash and checks every timestamp,
frame count, duration, dimensions, chapter selection and stream property.

Native and Temporal contact sheets were visually inspected at every chapter's
opening, last frame, and immediately before/after each camera cut, including
recovery: video indices 0, 449, 450, 451, 458 and 599 per chapter. All chapters
were present, upright and correctly ordered. Claude attribution remains; the
interactive instructions and performance HUD are absent. The Native contact
sheet is from the initial integration build (`native-01`, pre-commit dirty
source); the clean packaged Native movie has a separate complete decode audit.
Temporal samples come from clean feature source. These inspections establish
composition and cut continuity, not pixel equivalence between render modes.

The actual **Open video** action opened the 30-second packaged export in
QuickTime. Playback was started and a retained screenshot shows it advancing
into Lighting. QuickTime's hidden accessibility elapsed-time label remained
stale; it was not used as a timing measurement. Synthetic encoder calibration
also checked top-down orientation, RGB quadrants and six gray levels. For
example, sRGB 128 converts to Rec.709 115 and decodes to 114 at the target bitrate.

## Native app flows

The real Benchmark export action worked without a completed benchmark. The
Save sheet defaulted to all chapters, and Geometry selection produced the
10-second replacement above. The native replacement confirmation was exercised;
the previous destination hash remained unchanged while rendering, and changed
to the new movie hash only after successful completion.

Saved Benchmark and Compare results opened export sheets labeled as replays
rendered with the current app version. Compare offered all six modes with
Native selected initially. The final package selected Bilinear half and Lighting
and started the matching render. Results and history displayed video duration
without an FPS score, and exposed **Open video** and **Show in Finder**. Both
actions were invoked through the native UI.

Actual video cancellation was exercised in runs
`b2a97bbc-441a-4ffc-ac0e-5813eea53288` and
`848127e5-1859-4578-b33d-52d9041ef39e`. Reports and diagnostics remained, while
the MP4, partial movie and encoding lock were absent. No destination was
published. A final process check found only the launcher, with no renderer or
encoder helper left running.

An initial Save dialog used `runModal()` inside a SwiftUI action. Native picker
actions returned success but never opened their menus. A stack sample showed
the original SwiftUI update dispatch still enclosing the modal loop. Switching
to a retained asynchronous Save sheet released that action. The same native
picker reproduction then opened all chapter and mode menus, and the actual
Geometry replacement completed. The narrow fix received independent review.

## Foreground behavior and timing isolation

The final package's **Background run → Off** guidance described a visible
render window and recommended Stress for continuous viewing. Benchmark run
`71a060f8-f21e-423c-be81-e916d7bc9fb1` completed all three chapters with a
visible window, `valid:true`, no captures, `measured_readbacks:false` and
`per_frame_gpu_waits:false`.

Stress run `93228f43-1eb3-4a50-8407-492596a824aa` also opened the visible lab.
Five live sample epochs were valid and error-free. **Stop and save** reaped the
renderer and preserved the report. The stopped aggregate is intentionally
unqualified (`stopped:true`, `valid:false`) with no aggregate FPS score; this
was a foreground interaction check, not a new ten-minute qualification run.
Its readback and per-frame-wait flags remained false. Both windows were
captured and visually inspected.

## Automated checks and retained failures

- 69 Rust tests passed, covering cadence, chapter boundaries, temporal reset
  rules, frame identity/order, scoreless reporting, blocked-pipe cancellation
  and encoder failure. Strict Clippy and rustfmt passed.
- 44 Swift tests passed after the Save-sheet fix, including stream validation,
  awaited sink admission without read-ahead, output ownership, cancellation,
  destination replacement races, report/hash validation and child reaping.
  Strict Swift formatting passed.
- The actual AVFoundation encoder passed a 600-frame calibration decode and
  rejected malformed headers, truncated payloads, reordered frames, nonopaque
  data, trailing bytes and a forced disk-limit error. Every failure cleaned its
  partial output and lock. Mid-frame SIGTERM exited 130 in about 0.34 seconds.
- GPU/export validation ran serially. Sandbox-only encoder and icon-service
  failures were retained separately; authorized native execution succeeded.
  An early signal-handler actor-isolation failure was fixed and its real
  cancellation reproduction then passed. Independent code review was clean
  after the publication, history-loading and Save-sheet fixes.

Use `tools/benchmark/verify_video.py` for complete replay decode audits and
`tools/benchmark/macos/Support/VerifyVideoEncoder.py` for encoder calibration
and failure injection. Live recording, stress-adjustment replay, side-by-side
video, audio and 120 fps export remain outside this version.
