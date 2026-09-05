#!/usr/bin/env python3
"""Freeze, patch, build, and bound an existing Shadow Work image playtest."""
import argparse
from datetime import datetime, timezone
import difflib
import hashlib
import json
import math
import os
from pathlib import Path
import shutil
import signal
import subprocess
import sys
import tarfile
import tempfile
import time
import tomllib

PIN = "2a49dfcb294a69283e9e4cf9aa0662b61c51495a"
CONSUMER = Path("/Users/sma/projects/shadow-work/.claude/worktrees/metalfx-m5-research")
HERE = Path(__file__).resolve().parent
SHOTS = ("before", "p0", "p1", "p2", "p4", "p8")
ARMS = {"native": ("off", "Disabled", 1.0), "bilinear": ("off", "Disabled", 0.5),
        "temporal": ("temporal", "Temporal", 0.5)}
ASSETS = ("blue_marble.jpg", "heightmap.jpg")


def sha(path):
    with Path(path).open("rb") as stream:
        return hashlib.file_digest(stream, "sha256").hexdigest()


def write_json(path, value):
    with Path(path).open("x") as stream:
        json.dump(value, stream, indent=2)
        stream.write("\n")


def tree_sha(root):
    digest = hashlib.sha256()
    for path in sorted(root.rglob("*")):
        if path.is_symlink():
            value = "link:" + os.readlink(path)
        elif path.is_file():
            value = sha(path)
        else:
            continue
        digest.update(f"{path.relative_to(root)}\0{value}\n".encode())
    return digest.hexdigest()


def git(repo, *args):
    return subprocess.check_output(["git", "-c", "core.fsmonitor=false", "-C", str(repo), *args], text=True)


def archive(repo, commit, destination, tar_path):
    with tar_path.open("xb") as stream:
        subprocess.run(["git", "-c", "core.fsmonitor=false", "-C", str(repo),
                        "archive", "--format=tar", commit], stdout=stream, check=True)
    omitted = []

    def safe_member(member, root):
        try:
            return tarfile.data_filter(member, root)
        except (tarfile.LinkOutsideDestinationError, tarfile.AbsoluteLinkError):
            omitted.append({"path": member.name, "target": member.linkname})
            return None

    with tarfile.open(tar_path) as source:
        source.extractall(destination, filter=safe_member)
    return omitted


def replace_once(text, old, new):
    if text.count(old) != 1:
        raise ValueError(f"pinned source anchor changed: {old[:90]!r}")
    return text.replace(old, new)


def bounded(command, cwd, log, timeout, env=None):
    stopped = []
    previous = {sig: signal.getsignal(sig) for sig in (signal.SIGTERM, signal.SIGINT)}
    for sig in previous:
        signal.signal(sig, lambda received, _frame: stopped.append(received))
    child = None
    try:
        try:
            child = subprocess.Popen(command, cwd=cwd, stdout=log, stderr=subprocess.STDOUT,
                                     env=env, start_new_session=True)
        except OSError as error:
            log.write(f"launch failed: {error}\n")
            log.flush()
            return 127
        deadline = time.monotonic() + timeout
        while True:
            if stopped:
                return 128 + stopped[0]
            remaining = deadline - time.monotonic()
            if remaining <= 0:
                return 124
            try:
                return child.wait(timeout=min(remaining, 0.25))
            except subprocess.TimeoutExpired:
                continue
    finally:
        # Runs on timeout, SIGINT, SIGTERM and unexpected exceptions; reap the child.
        if child is not None:
            try:
                os.killpg(child.pid, signal.SIGTERM)
                child.wait(timeout=3)
            except subprocess.TimeoutExpired:
                pass
            except ProcessLookupError:
                pass
            # A child can exit on TERM before a helper that ignores it. Remove
            # any remaining group members even after the direct child has exited.
            try:
                os.killpg(child.pid, signal.SIGKILL)
            except ProcessLookupError:
                pass
            child.wait()
        for sig, handler in previous.items():
            signal.signal(sig, handler)


def valid_report(report, output, arm):
    if not isinstance(report, dict):
        return False
    _, mode, scale = ARMS[arm]
    captures = report.get("captures", [])
    expected = {str(output / f"{arm}_{name}.png") for name in SHOTS}
    return (report.get("valid") is True and report.get("mode") == mode
            and report.get("scale") == scale and report.get("distinct_ready", 0) >= 20
            and report.get("warmup_seconds", 0) >= 3.0
            and isinstance(captures, list) and len(captures) == 6
            and all(isinstance(item, dict) and item.get("valid") is True for item in captures)
            and {item.get("path") for item in captures} == expected
            and all(not Path(path).is_symlink() and Path(path).is_file() for path in expected))


def patch_consumer(consumer, ushas):
    """Exact anchors make a changed consumer fail before an altered fixture can run."""
    version = tomllib.loads((ushas / "Cargo.toml").read_text())["package"]["version"]
    files = {}

    def change(relative, replacements):
        path = consumer / relative
        before = path.read_text()
        after = before
        for old, new in replacements:
            after = replace_once(after, old, new)
        path.write_text(after)
        files[relative] = (before, after)

    change("Cargo.toml", [('bevy_metalfx = { path = "/Users/sma/projects/ushas" }',
                           'bevy_metalfx = { path = "../ushas" }')])
    change("crates/sw-renderer/Cargo.toml", [
        ('bevy_metalfx = { version = "0.4",', f'bevy_metalfx = {{ version = "={version}",'),
        ('env_logger = "0.11"', 'env_logger = "0.11"\nserde_json = "1"')])
    change("crates/sw-renderer/src/main.rs", [
        ("mod playtest;", "mod playtest;\nmod consumer_probe;\nmod consumer_readiness;"),
        ('std::fs::File::create("/tmp/sw-frametime.csv")',
         'std::fs::File::create(std::env::var("SW_CONSUMER_FRAME_LOG").unwrap_or_else(|_| "/tmp/sw-frametime.csv".into()))'),
        ('resolution: (1600u32, 900u32).into(),',
         'resolution: if playtest_mode && playtest_offscreen {\n'
         '                        bevy::window::WindowResolution::new(1600, 900).with_scale_factor_override(1.0)\n'
         '                    } else { (1600u32, 900u32).into() },'),
        ("app.add_plugins(DefaultPlugins\n", "let mut renderer_plugins = DefaultPlugins\n"),
        ("        )\n        // Performance diagnostics", "        ;\n"
         "    if playtest_mode && playtest_offscreen {\n"
         "        renderer_plugins = renderer_plugins.disable::<bevy::winit::WinitPlugin>();\n"
         "    }\n    app.add_plugins(renderer_plugins)\n        // Performance diagnostics"),
        ("            app.add_systems(PostStartup, playtest::retarget_offscreen);",
         "            app.add_plugins(bevy::app::ScheduleRunnerPlugin::run_loop(std::time::Duration::ZERO));\n"
         "            let probe = consumer_probe::ConsumerProbe::new(\n"
         "                std::path::Path::new(&playtest_dir), metalfx_mode, render_scale,\n"
         "                app.world().resource::<AssetServer>());\n"
         "            app.insert_resource(probe);\n"
         "            app.add_systems(PostStartup, playtest::retarget_offscreen);")])
    change("crates/sw-renderer/src/playtest.rs", [
        ("    reset: Option<ResMut<bevy_metalfx::MetalFxHistoryReset>>,\n",
         "    reset: Option<ResMut<bevy_metalfx::MetalFxHistoryReset>>,\n"
         "    mut probe: Option<ResMut<crate::consumer_probe::ConsumerProbe>>,\n"
         "    status: Res<bevy_metalfx::MetalFxEffectStatus>,\n"
         "    clock: Res<bevy_metalfx::MetalFxObservationFrame>,\n"
         "    views: Query<Entity, With<Camera3d>>,\n"
         "    assets: Res<AssetServer>,\n"),
        ("    let frame = pt.frame;\n", "    if let Some(probe) = probe.as_deref_mut() {\n"
         "        if !probe.advance(&status, clock.0, views.single().ok(), &assets, pt.frame) {\n"
         "            if pt.frame == 0 {\n"
         "                for mut camera in &mut cam { camera.snap_to(PRE_CUT.0, PRE_CUT.1, PRE_CUT.2); }\n"
         "            }\n            return;\n        }\n    }\n    let frame = pt.frame;\n"),
        ("            let _ = std::fs::remove_file(&path);",
         "            assert!(!path.exists(), \"refusing to overwrite a capture\");"),
        ("            commands.spawn(shot).observe(save_to_disk(path.clone()));",
         "            if probe.is_some() {\n"
         "                commands.spawn(shot).observe(crate::consumer_probe::capture(path.clone(), frame, clock.0));\n"
         "            } else {\n"
         "                commands.spawn(shot).observe(save_to_disk(path.clone()));\n            }"),
        ("            finish(0);", "            let valid = probe.as_deref_mut().is_none_or(|p| p.finish(true));\n"
         "            finish(if valid { 0 } else { 1 });"),
        ("            finish(1);", "            if let Some(probe) = probe.as_deref_mut() { probe.finish(false); }\n"
         "            finish(1);")])
    for name in ("probe.rs", "readiness.rs"):
        source = HERE / name
        relative = "crates/sw-renderer/src/consumer_" + name
        content = source.read_text()
        (consumer / relative).write_text(content)
        files[relative] = ("", content)
    return "".join("".join(difflib.unified_diff(before.splitlines(True), after.splitlines(True),
                      fromfile="a/" + name, tofile="b/" + name))
                   for name, (before, after) in sorted(files.items()))


def prepare(args):
    consumer = args.consumer.resolve()
    if git(consumer, "rev-parse", "HEAD").strip() != PIN:
        raise ValueError("consumer worktree must be at the audited pin " + PIN)
    ushas = args.ushas.resolve()
    revision = git(ushas, "rev-parse", args.revision + "^{commit}").strip()
    directory = Path(tempfile.mkdtemp(prefix="ushas-consumer-", dir=args.parent)).resolve()
    print(directory, flush=True)
    manifest = {"schema": 1, "created_utc": datetime.now(timezone.utc).isoformat(),
                "consumer_commit": PIN, "ushas_commit": revision,
                "consumer_source": str(consumer), "ushas_source": str(ushas),
                "directory": str(directory), "scope": "offscreen image correctness; no GPU-time or panel claim"}
    for name, repo, commit in (("consumer", consumer, PIN), ("ushas", ushas, revision)):
        destination = directory / name
        destination.mkdir()
        tar_path = directory / (name + ".tar")
        manifest[name + "_omitted_external_links"] = archive(repo, commit, destination, tar_path)
        manifest[name + "_archive_sha256"] = sha(tar_path)
    assets = []
    for name in ASSETS:
        relative = Path("crates/sw-renderer/assets/textures") / name
        source, destination = consumer / relative, directory / "consumer" / relative
        # This explicit two-file exception is the only live-worktree copy.
        if source.is_symlink() or not source.is_file() or source.read_bytes()[:3] != b"\xff\xd8\xff":
            raise ValueError(f"required real JPEG missing: {source}")
        before = sha(source)
        destination.parent.mkdir(parents=True, exist_ok=True)
        if not destination.exists():
            shutil.copyfile(source, destination)
        if sha(source) != before or sha(destination) != before:
            raise ValueError(f"asset changed or archive differs: {source}")
        assets.append({"source": str(source), "relative_path": str(relative), "sha256": before,
                       "bytes": source.stat().st_size, "origin": "explicit local NASA JPEG asset"})
    manifest["assets"] = assets
    patch = patch_consumer(directory / "consumer", directory / "ushas")
    (directory / "consumer.patch").write_text(patch)
    manifest["patch_sha256"] = sha(directory / "consumer.patch")
    manifest["tools"] = {p.name: sha(p) for p in (HERE / "run.py", HERE / "probe.rs", HERE / "readiness.rs")}
    manifest["consumer_tree_sha256"] = tree_sha(directory / "consumer")
    manifest["ushas_tree_sha256"] = tree_sha(directory / "ushas")
    write_json(directory / "prepared.json", manifest)
    return 0


def load_prepared(directory):
    return json.loads((directory / "prepared.json").read_text())


def build(args):
    directory = args.directory.resolve()
    prepared = load_prepared(directory)
    previous = directory / "check.json"
    expected = json.loads(previous.read_text()) if previous.is_file() else prepared
    for name in ("consumer", "ushas"):
        if tree_sha(directory / name) != expected[name + "_tree_sha256"]:
            raise ValueError("frozen source changed: " + name)
    operation = "check" if args.check else "build"
    receipt = directory / (operation + ".json")
    if receipt.exists():
        raise ValueError("already attempted this operation; prepare a fresh directory")
    target = args.target_dir.resolve() if args.target_dir else directory / "target"
    if any(target == root or root in target.parents for root in (directory / "consumer", directory / "ushas")):
        raise ValueError("target directory must be outside the frozen source trees")
    env = os.environ.copy()
    env["CARGO_TARGET_DIR"] = str(target)
    lock = directory / "consumer/Cargo.lock"
    lock_before = lock.read_text()
    command = ["cargo", "+1.97.1", operation, "--offline", "-p", "sw-renderer"]
    if not args.check:
        command.append("--release")
    with (directory / (operation + ".log")).open("x") as log:
        code = bounded(command, directory / "consumer", log, args.timeout, env)
    result = {"command": command, "exit_code": code, "target_dir": str(target),
              "runner_sha256": sha(Path(__file__)),
              "consumer_tree_sha256": tree_sha(directory / "consumer"),
              "ushas_tree_sha256": tree_sha(directory / "ushas")}
    (directory / (operation + "-lock.patch")).write_text("".join(difflib.unified_diff(
        lock_before.splitlines(True), lock.read_text().splitlines(True),
        fromfile="before/Cargo.lock", tofile="after/Cargo.lock")))
    result["lock_sha256"] = sha(lock)
    if code == 0:
        try:
            host = next(line.split(": ", 1)[1] for line in subprocess.check_output(
                ["rustc", "+1.97.1", "-vV"], text=True, timeout=10).splitlines() if line.startswith("host: "))
            metadata = json.loads(subprocess.check_output(
                ["cargo", "+1.97.1", "metadata", "--offline", "--locked", "--filter-platform", host,
                 "--format-version=1"], cwd=directory / "consumer", env=env, text=True, timeout=60))
            renderer = next(p for p in metadata["packages"] if p["name"] == "sw-renderer")
            node = next(n for n in metadata["resolve"]["nodes"] if n["id"] == renderer["id"])
            dependency = next(d["pkg"] for d in node["deps"] if d["name"] == "bevy_metalfx")
            package = next(p for p in metadata["packages"] if p["id"] == dependency)
            if Path(package["manifest_path"]).resolve() != directory / "ushas/Cargo.toml":
                raise ValueError("consumer resolved a different MetalFX dependency")
            result["resolved_metalfx"] = package["id"]
            if not args.check:
                binary = directory / "sw-renderer"
                shutil.copyfile(target / "release/sw-renderer", binary)
                binary.chmod(0o700)
                result["binary_sha256"] = sha(binary)
        except (OSError, ValueError, KeyError, StopIteration, subprocess.SubprocessError) as error:
            code = result["exit_code"] = 2
            result["verification_error"] = str(error)
    write_json(receipt, result)
    print(json.dumps(result, indent=2))
    return code


def run_arm(args):
    directory = args.directory.resolve()
    receipt = json.loads((directory / "build.json").read_text())
    binary = directory / "sw-renderer"
    if receipt["exit_code"] != 0 or sha(binary) != receipt.get("binary_sha256"):
        raise ValueError("missing or changed successful build")
    for name in ("consumer", "ushas"):
        if tree_sha(directory / name) != receipt[name + "_tree_sha256"]:
            raise ValueError("frozen source changed: " + name)
    output = Path(tempfile.mkdtemp(prefix=args.arm + "-", dir=directory)).resolve()
    mode, _, scale = ARMS[args.arm]
    command = [str(binary), "--playtest", "--playtest-offscreen", f"--metalfx={mode}",
               f"--scale={scale}", "--playtest-cut=30", "--history-reset",
               f"--playtest-dir={output}", f"--playtest-tag={args.arm}"]
    env = os.environ.copy()
    env["CARGO_MANIFEST_DIR"] = str(directory / "consumer/crates/sw-renderer")
    env["SW_CONSUMER_FRAME_LOG"] = str(output / "frametime.csv")
    env["OVERLAY"] = "0"
    with (output / "run.log").open("x") as log:
        code = bounded(command, directory / "consumer", log, args.timeout, env)
    result = {"command": command, "exit_code": code, "arm": args.arm,
              "runner_sha256": sha(Path(__file__)),
              "binary_sha256": receipt["binary_sha256"], "valid": False,
              "prepared_sha256": sha(directory / "prepared.json"),
              "build_sha256": sha(directory / "build.json"), "output": str(output)}
    try:
        report = json.loads((output / "effect.json").read_text())
        result["valid"] = code == 0 and sha(binary) == receipt["binary_sha256"] and valid_report(report, output, args.arm)
    except (OSError, ValueError, TypeError, KeyError) as error:
        result["report_error"] = str(error)
    result["artifacts"] = {p.name: sha(p) for p in output.iterdir() if p.is_file() and not p.is_symlink()}
    write_json(output / "manifest.json", result)
    print(json.dumps(result, indent=2))
    return code or (0 if result["valid"] else 1)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    sub = parser.add_subparsers(dest="command", required=True)
    prep = sub.add_parser("prepare")
    prep.add_argument("--consumer", type=Path, default=CONSUMER)
    prep.add_argument("--ushas", type=Path, default=HERE.parents[1])
    prep.add_argument("--revision", required=True, help="committed Ushas revision to archive")
    prep.add_argument("--parent", type=Path, default=Path("/private/tmp"))
    builder = sub.add_parser("build")
    builder.add_argument("directory", type=Path)
    builder.add_argument("--check", action="store_true", help="CPU compilation only; do not link or run")
    builder.add_argument("--target-dir", type=Path)
    builder.add_argument("--timeout", type=float, default=1800)
    runner = sub.add_parser("run")
    runner.add_argument("directory", type=Path)
    runner.add_argument("--arm", choices=ARMS, required=True)
    runner.add_argument("--timeout", type=float, default=90)
    args = parser.parse_args()
    if hasattr(args, "timeout") and (not math.isfinite(args.timeout) or not 0 < args.timeout <= 3600):
        parser.error("timeout must be finite and in (0,3600] seconds")
    try:
        return {"prepare": prepare, "build": build, "run": run_arm}[args.command](args)
    except (OSError, ValueError, subprocess.SubprocessError) as error:
        print(f"consumer-smoke: {error}", file=sys.stderr)
        return 2


if __name__ == "__main__":
    sys.exit(main())
