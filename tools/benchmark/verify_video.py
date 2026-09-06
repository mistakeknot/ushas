#!/usr/bin/env python3
"""Validate a completed replay with ffprobe/ffmpeg; these are QA tools, not app dependencies."""
import argparse
import hashlib
import json
from pathlib import Path
import subprocess


def probe(movie, *args):
    return json.loads(subprocess.check_output(
        ["ffprobe", "-v", "error", *args, "-of", "json", str(movie)], text=True))


def verify(report_path, output):
    report = json.loads(report_path.read_text())
    assert report["kind"] == "video" and report["valid"] and not report["stopped"]
    assert not report["errors"] and report["render_fps"] is None
    assert report["source_revision"] and len(report["binary_sha256"]) == 64
    config, video = report["config"], report["video"]
    chapters = [config["scene"]] if config.get("scene") else ["materials", "geometry", "lighting"]
    count = len(chapters) * 600
    assert [s["scene"] for s in report["scenes"]] == chapters
    assert all(s["valid"] and not s["errors"] and s["frames"] == 1200 and s["render_fps"] is None
               for s in report["scenes"])
    assert config["width"] == 2560 and config["height"] == 1440 and config["frames"] == 1200
    assert video["frame_count"] == count and video["duration_seconds"] == count / 60
    assert video["fps"] == 60 and video["simulation_hz"] == 120
    assert video["codec"] == "h264" and video["color_space"] == "rec709"
    movie = (report_path.parent / video["path"]).resolve()
    assert movie.parent == report_path.parent.resolve()
    assert hashlib.file_digest(movie.open("rb"), "sha256").hexdigest() == video["sha256"]
    assert not list(report_path.parent.glob("*.partial.mp4"))
    assert not list(report_path.parent.glob("*.encoding-lock"))
    metadata = probe(movie, "-count_frames", "-show_streams", "-show_format")
    assert len(metadata["streams"]) == 1, "Movie must contain one video stream and no audio"
    stream = metadata["streams"][0]
    for key, expected in dict(codec_name="h264", codec_type="video", width=2560, height=1440,
                              r_frame_rate="60/1", avg_frame_rate="60/1", color_space="bt709",
                              color_transfer="bt709", color_primaries="bt709").items():
        assert stream[key] == expected, (key, stream.get(key), expected)
    assert int(stream["nb_read_frames"]) == count
    assert abs(float(stream["duration"]) - count / 60) < 1e-6
    frames = probe(movie, "-select_streams", "v:0", "-show_frames",
                   "-show_entries", "frame=best_effort_timestamp_time,duration_time")["frames"]
    assert len(frames) == count
    for index, frame in enumerate(frames):
        assert abs(float(frame["best_effort_timestamp_time"]) - index / 60) < 1e-6, (index, frame)
        assert abs(float(frame["duration_time"]) - 1 / 60) < 1e-6, (index, frame)
    environment = report["environment"]
    assert environment["render_target"] == "offscreen_image" and not environment["live_preview"]
    assert environment["measured_readbacks"] and environment["per_frame_gpu_waits"]
    output.mkdir(parents=True, exist_ok=False)
    # Retain the opening, both sides of the cut, recovery and closing image for
    # every chapter. Decode to sRGB explicitly for visual inspection.
    samples = [base * 600 + tick for base in range(len(chapters)) for tick in [0, 449, 450, 451, 458, 599]]
    select = "+".join(f"eq(n\\,{index})" for index in samples)
    subprocess.run(["ffmpeg", "-v", "error", "-i", str(movie), "-vf",
                    f"select='{select}',colorspace=all=bt709:trc=iec61966-2-1:format=yuv444p",
                    "-fps_mode", "passthrough", str(output / "sample-%02d.png")], check=True)
    labels = [{"file": f"sample-{i+1:02}.png", "frame": n,
               "chapter": chapters[n // 600], "tick": (n % 600) * 2} for i, n in enumerate(samples)]
    receipt = dict(report=str(report_path.resolve()), video_sha256=video["sha256"],
                   source_revision=report["source_revision"], renderer_sha256=report["binary_sha256"],
                   frames=count, duration=count / 60, stream=stream, samples=labels,
                   visual_inspection="pending")
    (output / "decoded.json").write_text(json.dumps(receipt, indent=2) + "\n")
    print(json.dumps({k: receipt[k] for k in ["frames", "duration", "video_sha256"]}))


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("report", type=Path)
    parser.add_argument("--out", required=True, type=Path, help="new QA artifact directory")
    args = parser.parse_args()
    verify(args.report, args.out)
