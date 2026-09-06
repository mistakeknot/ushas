# Video replay v1

The renderer owns the encoder process. It finds `ushas-video-encoder` beside
itself (development override: `USHAS_VIDEO_ENCODER`) and launches it with
`--out <run-directory>/video.mp4`. Diagnostics go to `encoder.log`.

stdin is binary, little endian. The 24-byte header is ASCII `USHASV01`, then
four u32 values: width 2560, height 1440, cadence 60, total frames 600 or 1800.
Each frame has four u32 values: contiguous zero-based video index, chapter
ordinal (0 materials, 1 geometry, 2 lighting), simulation tick (0,2,...1198),
payload byte count (width * height * 4). Payload is tightly packed, top-down
RGBA8 sRGB with opaque alpha. An individual chapter may start at any ordinal;
the full sequence must contain chapters 0,1,2 in order. EOF must follow exactly
the declared number of frames. Reject truncation, trailing bytes, reordering,
invalid alpha, dimensions, cadence, counts, chapter boundaries or ticks.

The encoder converts sRGB samples explicitly to SDR Rec.709 and encodes silent
H.264 at target 30 Mbps. Presentation timestamps are index/60. Use AVFoundation
PixelBufferReceiver. Await admission; never drop frames. The renderer permits
only one outstanding capture and uses a bounded handoff; waiting must not
advance simulation or update temporal history. Render all 1200 ticks per
chapter, including odd ticks, with the existing chapter and camera-cut resets.

The encoder writes `video.partial.mp4`, finishes and validates the exact frame
count, then publishes `video.mp4` without replacing an existing file. On error
or cancellation, remove partial video data and preserve diagnostics. Renderer
must stop and reap encoder on every exit path, including stream-write failure.

Success is exit 0. Renderer independently hashes the finished movie and puts
this object in result.json `video`: `path` (video.mp4), `width`, `height`,
`fps` (60), `simulation_hz` (120), `frame_count`, `duration_seconds`,
`codec` (h264), `bitrate` (30000000), `color_space` (rec709), `sha256`.
The existing envelope records config and renderer identity. kind is `video`,
render_fps is null everywhere; no benchmark score. Progress uses existing
`progress` events plus optional `video_frames` and `video_total_frames`.

Ownership for this implementation: renderer track owns benchmark src/**;
encoder track owns macos/Sources/VideoEncoderCore/**,
macos/Sources/UshasVideoEncoder/**, macos/Tests/VideoEncoderTests/**,
macos/Package.swift and package.sh; root owns launcher BenchCore/UshasBench,
their tests, documentation, integration validation and git commits. No agent
commits. GPU validation is serial and coordinated by root.
