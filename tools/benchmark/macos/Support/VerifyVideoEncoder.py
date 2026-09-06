#!/usr/bin/env python3
"""Serial integration checks for the real macOS video encoder (no renderer).

Requires ffmpeg/ffprobe. Writes diagnostics and a 10-second calibration movie
under a NEW --out directory; run only while holding the GPU validation slot.
"""
import argparse
import json
from pathlib import Path
import resource
import signal
import struct
import subprocess
import time

WIDTH, HEIGHT, COUNT = 2560, 1440, 600
PAYLOAD = WIDTH * HEIGHT * 4
HEADER = b"USHASV01" + struct.pack("<4I", WIDTH, HEIGHT, 60, COUNT)


def frame(gray=128):
    top = (bytes([255, 0, 0, 255]) * (WIDTH // 2)
           + bytes([0, 255, 0, 255]) * (WIDTH // 2))
    bottom = (bytes([0, 0, 255, 255]) * (WIDTH // 2)
              + bytes([gray, gray, gray, 255]) * (WIDTH // 2))
    return top * (HEIGHT // 2) + bottom * (HEIGHT // 2)


def metadata(index=0, chapter=0, tick=0, size=PAYLOAD):
    return struct.pack("<4I", index, chapter, tick, size)


def clean_failure(folder, code):
    assert code != 0, "Rejected stream unexpectedly succeeded"
    assert not (folder / "video.mp4").exists(), "Failure published a video"
    assert not (folder / "video.partial.mp4").exists(), "Failure retained a partial video"
    assert not (folder / "video.mp4.encoding-lock").exists(), "Failure retained its output lock"


def run_case(binary, root, name, payload, preexec_fn=None):
    folder = root / name
    folder.mkdir()
    with (folder / "encoder.log").open("wb") as log:
        child = subprocess.Popen([binary, "--out", str(folder / "video.mp4")],
                                 stdin=subprocess.PIPE, stderr=log, preexec_fn=preexec_fn)
        try:
            for chunk in [payload] if isinstance(payload, bytes) else payload:
                child.stdin.write(chunk)
            child.stdin.close()
        except BrokenPipeError:
            pass
        code = child.wait(timeout=30)
    clean_failure(folder, code)
    return {"case": name, "exit": code,
            "diagnostic": (folder / "encoder.log").read_text().strip()}


def complete_stream(trailing=b""):
    yield HEADER
    rgba = frame()
    for index in range(COUNT):
        yield metadata(index, 0, index * 2)
        yield rgba
    if trailing:
        yield trailing


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--encoder", required=True)
    parser.add_argument("--out", required=True)
    args = parser.parse_args()
    root = Path(args.out).resolve()
    root.mkdir(parents=True, exist_ok=False)
    binary = str(Path(args.encoder).resolve())
    movie = root / "video.mp4"
    start = time.monotonic()
    with (root / "encoder.log").open("wb") as log:
        child = subprocess.Popen([binary, "--out", str(movie)], stdin=subprocess.PIPE, stderr=log)
        try:
            child.stdin.write(HEADER)
            for segment, gray in enumerate([0, 32, 64, 128, 192, 255]):
                rgba = frame(gray)
                for offset in range(100):
                    index = segment * 100 + offset
                    child.stdin.write(metadata(index, 0, index * 2))
                    child.stdin.write(rgba)
                print(f"calibration frames: {(segment + 1) * 100}/600", flush=True)
            child.stdin.close()
            assert child.wait(timeout=30) == 0, (root / "encoder.log").read_text()
        except BaseException:
            child.terminate()
            child.wait(timeout=10)
            raise
    info = json.loads(subprocess.check_output([
        "ffprobe", "-v", "error", "-count_frames", "-show_streams", "-show_format", "-of", "json", str(movie)]))
    (root / "ffprobe.json").write_text(json.dumps(info, indent=2) + "\n")
    assert len(info["streams"]) == 1, "Expected silent video with one track"
    stream = info["streams"][0]
    assert stream["codec_name"] == "h264"
    assert (stream["width"], stream["height"]) == (WIDTH, HEIGHT)
    assert stream["nb_read_frames"] == "600"
    assert stream["r_frame_rate"] == "60/1"
    assert abs(float(stream["duration"]) - 10) < 1e-6
    assert all(stream[key] == "bt709" for key in ["color_space", "color_transfer", "color_primaries"])
    times = json.loads(subprocess.check_output([
        "ffprobe", "-v", "error", "-select_streams", "v:0", "-show_frames",
        "-show_entries", "frame=best_effort_timestamp_time", "-of", "json", str(movie)]))["frames"]
    assert len(times) == COUNT
    assert all(abs(float(item["best_effort_timestamp_time"]) - i / 60) <= 1e-6
               for i, item in enumerate(times)), "Frame timestamps are not index/60"
    # Decode six frames, never the entire sequence. ffmpeg's RGB output retains
    # the video transfer curve, so compare samples against the Rec.709 OETF.
    decoded = subprocess.check_output([
        "ffmpeg", "-v", "error", "-i", str(movie), "-vf", "select=not(mod(n\\,100))",
        "-fps_mode", "passthrough", "-f", "rawvideo", "-pix_fmt", "rgb24", "-"])
    assert len(decoded) == WIDTH * HEIGHT * 3 * 6
    samples = []
    for index, gray in enumerate([0, 32, 64, 128, 192, 255]):
        source = gray / 255
        linear = source / 12.92 if source <= 0.04045 else ((source + 0.055) / 1.055) ** 2.4
        converted = 4.5 * linear if linear < .018 else 1.099 * linear ** .45 - .099
        expected_gray = round(converted * 255)
        expected = [(255, 0, 0), (0, 255, 0), (0, 0, 255), (expected_gray,) * 3]
        actual = []
        for (x, y), target in zip([(640, 360), (1920, 360), (640, 1080), (1920, 1080)], expected):
            offset = (index * WIDTH * HEIGHT + y * WIDTH + x) * 3
            color = tuple(decoded[offset:offset + 3])
            assert all(abs(a - b) <= 4 for a, b in zip(color, target)), (index, color, target)
            actual.append(color)
        samples.append({"index": index * 100, "srgb_gray": gray, "rec709_gray": expected_gray, "quadrants": actual})
    subprocess.run(["ffmpeg", "-v", "error", "-i", str(movie), "-vf", "select=eq(n\\,300)",
                    "-frames:v", "1", str(root / "calibration.png")], check=True)
    encoding_seconds = time.monotonic() - start
    (root / "color-samples.json").write_text(json.dumps(samples, indent=2) + "\n")
    results = []
    results.append(run_case(binary, root, "malformed-header", b"invalid"))
    results.append(run_case(binary, root, "truncated-payload", HEADER + metadata() + b"short"))
    results.append(run_case(binary, root, "wrong-order", HEADER + metadata(index=1)))
    results.append(run_case(binary, root, "nonopaque", HEADER + metadata() + bytes(PAYLOAD)))
    results.append(run_case(binary, root, "trailing-data", complete_stream(b"extra")))
    def disk_limit():
        signal.signal(signal.SIGXFSZ, signal.SIG_IGN)
        resource.setrlimit(resource.RLIMIT_FSIZE, (1024, 1024))
    results.append(run_case(binary, root, "disk-error", complete_stream(), disk_limit))
    assert "Truncated" not in results[-1]["diagnostic"], "Disk test must exercise writer failure"
    folder = root / "cancel-mid-frame"
    folder.mkdir()
    with (folder / "encoder.log").open("wb") as log:
        child = subprocess.Popen([binary, "--out", str(folder / "video.mp4")], stdin=subprocess.PIPE, stderr=log)
        child.stdin.write(HEADER + metadata() + b"short")
        child.stdin.flush()
        deadline = time.monotonic() + 10
        while not (folder / "video.partial.mp4").exists():
            assert child.poll() is None, "Encoder exited before cancellation test"
            assert time.monotonic() < deadline, "Encoder did not start"
            time.sleep(.02)
        cancelled = time.monotonic()
        child.send_signal(signal.SIGTERM)
        code = child.wait(timeout=10)
        child.stdin.close()
        clean_failure(folder, code)
        results.append({"case": "cancel-mid-frame", "exit": code, "seconds": time.monotonic() - cancelled})
    summary = {"encoding_seconds": encoding_seconds, "total_seconds": time.monotonic() - start, "frame_count": COUNT,
               "duration": 10, "samples": samples, "failure_cases": results}
    (root / "verification.json").write_text(json.dumps(summary, indent=2) + "\n")
    print(json.dumps(summary, indent=2), flush=True)


if __name__ == "__main__":
    main()
