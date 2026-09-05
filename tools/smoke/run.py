#!/usr/bin/env python3
"""Bound a smoke process and retain each run's identity, environment and failures."""
import argparse
import hashlib
import json
import math
import os
from pathlib import Path
import signal
import subprocess
import sys
from datetime import datetime, timezone


def digest(path):
    try:
        with Path(path).open("rb") as stream:
            return hashlib.file_digest(stream, "sha256").hexdigest()
    except OSError:
        return None


def command(args, cwd=None):
    try:
        result = subprocess.run(args, cwd=cwd, capture_output=True, text=True, timeout=8)
        return {"exit_code": result.returncode, "stdout": result.stdout.strip(), "stderr": result.stderr.strip()}
    except (OSError, subprocess.TimeoutExpired) as error:
        return {"error": str(error)}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--timeout", type=float, default=90)
    parser.add_argument("--binary", type=Path, default=Path(__file__).parent / "target/release/ushas-smoke")
    parser.add_argument("args", nargs=argparse.REMAINDER)
    config = parser.parse_args()
    args = config.args[1:] if config.args[:1] == ["--"] else config.args
    if args.count("--out") != 1 or args.index("--out") + 1 >= len(args):
        parser.error("pass a unique --out path after --")
    if args.count("--screenshot") > 1:
        parser.error("--screenshot may appear only once")
    if not math.isfinite(config.timeout) or config.timeout <= 0:
        parser.error("timeout must be positive and finite")
    output = Path(args[args.index("--out") + 1]).resolve()
    args[args.index("--out") + 1] = str(output)
    screenshot = Path(str(output) + ".png")
    if "--screenshot" in args:
        index = args.index("--screenshot") + 1
        if index >= len(args):
            parser.error("--screenshot needs a path")
        screenshot = Path(args[index]).resolve()
        args[index] = str(screenshot)
    manifest_path = Path(str(output) + ".manifest.json")
    log_path = Path(str(output) + ".log")
    paths = [output, manifest_path, log_path, screenshot, Path(str(screenshot) + ".warmup.png")]
    if len(set(paths)) != len(paths):
        parser.error("report, manifest, log and screenshot paths must be distinct")
    for path in paths:
        if path.exists():
            parser.error(f"refusing to overwrite retained evidence: {path}")
    output.parent.mkdir(parents=True, exist_ok=True)
    root = Path(__file__).resolve().parents[2]
    binary = config.binary.resolve()
    manifest = {
        "schema": 1,
        "started_utc": datetime.now(timezone.utc).isoformat(),
        "binary": str(binary), "binary_sha256": digest(binary),
        "argv": [str(binary), *args], "timeout_seconds": config.timeout,
        "source_head": command(["git", "rev-parse", "HEAD"], root),
        "source_status": command(["git", "status", "--porcelain"], root),
        "lock_sha256": digest(root / "tools/smoke/Cargo.lock"),
        "toolchain_sha256": digest(root / "rust-toolchain.toml"),
        "os": command(["/usr/bin/sw_vers"]),
        "machine": command(["/usr/sbin/sysctl", "-n", "hw.model"]),
        "thermal_before": command(["/usr/bin/pmset", "-g", "therm"]),
        "environment": {k: os.environ[k] for k in ["MTL_DEBUG_LAYER", "MTL_SHADER_VALIDATION", "WGPU_BACKEND", "WGPU_POWER_PREF"] if k in os.environ},
        "idle_display_sleep_protection": "caffeinate -d for this process only",
    }
    exit_code = 1
    process = None
    def interrupted(signum, _frame):
        raise InterruptedError(f"wrapper interrupted by signal {signum}")
    previous_sigterm = signal.signal(signal.SIGTERM, interrupted)
    try:
        with log_path.open("x") as log:
            process = subprocess.Popen(["/usr/bin/caffeinate", "-d", str(binary), *args], cwd=root, stdout=log, stderr=subprocess.STDOUT, start_new_session=True)
            try:
                exit_code = process.wait(timeout=config.timeout)
            except subprocess.TimeoutExpired:
                manifest["timed_out"] = True
                os.killpg(process.pid, signal.SIGTERM)
                try:
                    process.wait(timeout=5)
                except subprocess.TimeoutExpired:
                    os.killpg(process.pid, signal.SIGKILL)
                    process.wait()
                exit_code = 124
    except (OSError, KeyboardInterrupt) as error:
        manifest["launch_error"] = str(error)
    finally:
        if process is not None and process.poll() is None:
            os.killpg(process.pid, signal.SIGTERM)
            try:
                process.wait(timeout=5)
            except subprocess.TimeoutExpired:
                os.killpg(process.pid, signal.SIGKILL)
                process.wait()
        signal.signal(signal.SIGTERM, previous_sigterm)
        manifest.update({"child_exit_code": process.returncode if process else None, "finished_utc": datetime.now(timezone.utc).isoformat(), "thermal_after": command(["/usr/bin/pmset", "-g", "therm"]), "report_sha256": digest(output), "log_sha256": digest(log_path), "binary_sha256_after": digest(binary)})
        if output.exists():
            try:
                report = json.loads(output.read_text())
                manifest["smoke_valid"] = report.get("valid")
                manifest["captures"] = {name: {"path": report[name].get("path"), "sha256": digest(report[name].get("path") or "")} for name in ["screenshot", "warmup_screenshot"] if isinstance(report.get(name), dict)}
            except (ValueError, OSError, AttributeError, TypeError) as error:
                manifest["report_error"] = str(error)
        captures = manifest.get("captures", {})
        proof_ok = (manifest.get("smoke_valid") is True and manifest["report_sha256"]
                    and all(captures.get(name, {}).get("sha256") for name in ["screenshot", "warmup_screenshot"])
                    and manifest["binary_sha256"] == manifest["binary_sha256_after"])
        if exit_code == 0 and not proof_ok:
            exit_code = 1
            manifest["wrapper_failure"] = "missing/invalid report or captures, or binary changed during the run"
        manifest["exit_code"] = exit_code
        manifest_path.write_text(json.dumps(manifest, indent=2) + "\n")
    print(f"exit={exit_code} manifest={manifest_path}")
    return exit_code if exit_code >= 0 else 128 - exit_code


if __name__ == "__main__":
    sys.exit(main())
