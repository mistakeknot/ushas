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


# Keep these phase sets aligned with LifecycleRun's capture transitions.
LIFECYCLES = {
    "resize": ("Resize", {"initial", "changed", "restored"}),
    "camera-cut": ("CameraCut", {"initial", "changed"}),
    "late-camera": ("LateCamera", {"changed", "restored"}),
    "multiple-views": ("MultipleViews", {"initial", "restored"}),
    "inactive-cut-resume": ("InactiveCutResume", {"initial", "restored"}),
    "creation-failure": ("CreationFailure", {"initial", "changed", "restored"}),
    "creation-slow": ("CreationSlow", {"initial", "changed", "restored"}),
}


def digest(path):
    try:
        if not Path(path).is_file():
            return None
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


def option_value(args, flag, parser, required=False):
    count = args.count(flag)
    if count > 1 or (required and count != 1):
        parser.error(f"pass a unique {flag} value after --")
    if not count:
        return None
    index = args.index(flag) + 1
    if index >= len(args) or args[index].startswith("--"):
        parser.error(f"{flag} needs a value")
    return args[index]


def retained_capture(path):
    # Never follow a capture symlink introduced after the preflight check.
    return {"path": str(path), "sha256": None if path.is_symlink() else digest(path)}


def capture_matches(proof, expected):
    if not isinstance(proof, dict) or not isinstance(proof.get("path"), str):
        return False
    try:
        path = Path(proof["path"])
        return path.is_absolute() and path.resolve() == expected and not path.is_symlink()
    except (OSError, ValueError):
        return False


def check_report(output, screenshot, lifecycle, phase_paths, manifest):
    """Hash only preflighted paths, including partial evidence from failed runs."""
    main_paths = {"screenshot": screenshot, "warmup_screenshot": Path(str(screenshot) + ".warmup.png")}
    manifest["captures"] = {name: retained_capture(path) for name, path in main_paths.items()}
    manifest["lifecycle_captures"] = {phase: retained_capture(path) for phase, path in phase_paths.items()}
    errors = []
    try:
        if not output.is_file() or output.is_symlink():
            raise ValueError("missing regular report file")
        report = json.loads(output.read_text())
        if not isinstance(report, dict):
            raise ValueError("report must be an object")
    except (ValueError, OSError) as error:
        manifest["report_error"] = str(error)
        return [str(error)]
    manifest["smoke_valid"] = report.get("valid")
    if report.get("valid") is not True:
        errors.append("report valid must be true")
    for name, path in main_paths.items():
        if not capture_matches(report.get(name), path) or not manifest["captures"][name]["sha256"]:
            errors.append(f"missing or mismatched {name}")
    observed = report.get("lifecycle")
    if lifecycle is None:
        if observed is not None:
            errors.append("unexpected lifecycle report")
        return errors
    exercise, expected_phases = LIFECYCLES[lifecycle]
    if not isinstance(observed, dict):
        return errors + ["missing lifecycle report"]
    manifest["lifecycle_valid"] = observed.get("valid")
    if observed.get("valid") is not True or observed.get("exercise") != exercise:
        errors.append("lifecycle validity or exercise mismatch")
    captures = observed.get("captures")
    if not isinstance(captures, list):
        return errors + ["lifecycle captures must be an array"]
    seen = set()
    for capture in captures:
        phase = capture.get("phase") if isinstance(capture, dict) else None
        if not isinstance(phase, str) or phase not in expected_phases or phase in seen:
            errors.append("unknown, unexpected, or duplicate lifecycle phase")
            continue
        seen.add(phase)
        retained = manifest["lifecycle_captures"][phase]
        retained["reported_valid"] = capture.get("valid")
        if (capture.get("valid") is not True or not capture_matches(capture, phase_paths[phase])
                or not retained["sha256"]):
            errors.append(f"missing, invalid, or mismatched lifecycle {phase} capture")
    if seen != expected_phases:
        errors.append("lifecycle phase capture set is incomplete")
    for phase in phase_paths.keys() - expected_phases:
        if manifest["lifecycle_captures"][phase]["sha256"]:
            errors.append(f"unexpected lifecycle {phase} artifact")
    return errors


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--timeout", type=float, default=90)
    parser.add_argument("--binary", type=Path, default=Path(__file__).parent / "target/release/ushas-smoke")
    parser.add_argument("args", nargs=argparse.REMAINDER)
    config = parser.parse_args()
    args = config.args[1:] if config.args[:1] == ["--"] else config.args
    raw_output = option_value(args, "--out", parser, required=True)
    raw_screenshot = option_value(args, "--screenshot", parser)
    lifecycle = option_value(args, "--lifecycle", parser)
    if lifecycle is not None and lifecycle not in LIFECYCLES:
        parser.error(f"unknown lifecycle exercise: {lifecycle}")
    if not math.isfinite(config.timeout) or config.timeout <= 0:
        parser.error("timeout must be positive and finite")
    for raw in (raw_output, raw_screenshot):
        if raw is not None and (Path(raw).exists() or Path(raw).is_symlink()):
            parser.error(f"refusing to overwrite retained evidence: {raw}")
    output = Path(raw_output).resolve()
    args[args.index("--out") + 1] = str(output)
    screenshot = Path(str(output) + ".png")
    if raw_screenshot is not None:
        index = args.index("--screenshot") + 1
        if index >= len(args):
            parser.error("--screenshot needs a path")
        screenshot = Path(args[index]).resolve()
        args[index] = str(screenshot)
    manifest_path = Path(str(output) + ".manifest.json")
    log_path = Path(str(output) + ".log")
    phase_paths = ({phase: Path(f"{output}.lifecycle-{phase}.png")
                    for phase in ("initial", "changed", "restored")} if lifecycle else {})
    paths = [output, manifest_path, log_path, screenshot, Path(str(screenshot) + ".warmup.png"), *phase_paths.values()]
    if len(set(paths)) != len(paths) or any(a in b.parents for a in paths for b in paths if a != b):
        parser.error("evidence paths must be distinct files and cannot contain each other")
    for path in paths:
        if path.exists() or path.is_symlink():
            parser.error(f"refusing to overwrite retained evidence: {path}")
    output.parent.mkdir(parents=True, exist_ok=True)
    root = Path(__file__).resolve().parents[2]
    binary = config.binary.resolve()
    manifest = {
        "schema": 1,
        "started_utc": datetime.now(timezone.utc).isoformat(),
        "binary": str(binary), "binary_sha256": digest(binary),
        "argv": [str(binary), *args], "timeout_seconds": config.timeout,
        "lifecycle_requested": lifecycle,
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
    owns_log = False
    def interrupted(signum, _frame):
        raise InterruptedError(f"wrapper interrupted by signal {signum}")
    previous_sigterm = signal.signal(signal.SIGTERM, interrupted)
    try:
        with log_path.open("x") as log:
            owns_log = True
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
        if owns_log:
            manifest.update({"child_exit_code": process.returncode if process else None, "finished_utc": datetime.now(timezone.utc).isoformat(), "thermal_after": command(["/usr/bin/pmset", "-g", "therm"]), "report_sha256": digest(output), "log_sha256": digest(log_path), "binary_sha256_after": digest(binary)})
            errors = check_report(output, screenshot, lifecycle, phase_paths, manifest)
            if (not manifest["binary_sha256"] or manifest["binary_sha256"] != manifest["binary_sha256_after"]):
                errors.append("binary missing or changed during the run")
            if not manifest["report_sha256"]:
                errors.append("report hash unavailable")
            manifest["evidence_errors"] = errors
            proof_ok = not errors
            if exit_code == 0 and not proof_ok:
                exit_code = 1
                manifest["wrapper_failure"] = "missing/invalid report or captures, or binary changed during the run"
            manifest["exit_code"] = exit_code
            # A concurrent runner must never have its manifest truncated here.
            with manifest_path.open("x") as stream:
                stream.write(json.dumps(manifest, indent=2) + "\n")
    if owns_log:
        print(f"exit={exit_code} manifest={manifest_path}")
    else:
        print(f"exit={exit_code} log reservation failed; no manifest written: {log_path}", file=sys.stderr)
    return exit_code if exit_code >= 0 else 128 - exit_code


if __name__ == "__main__":
    sys.exit(main())
