"""CPU-only evidence isolation and pinned patch regression tests."""
import importlib.util
import argparse
import contextlib
import io
import json
import os
import signal
import sys
import time
from pathlib import Path
import subprocess
import tempfile
import unittest
from unittest.mock import patch

HERE_RUN = Path(__file__).with_name("run.py").resolve()
SPEC = importlib.util.spec_from_file_location("consumer_run", HERE_RUN)
run = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(run)


class IsolationTests(unittest.TestCase):
    def test_archive_uses_commit_not_dirty_or_untracked_files(self):
        with tempfile.TemporaryDirectory() as base:
            base = Path(base)
            repo = base / "repo"
            repo.mkdir()
            subprocess.run(["git", "init", "-q", str(repo)], check=True)
            (repo / "source").write_text("committed\n")
            (repo / "external").symlink_to("../../live-data")
            subprocess.run(["git", "-C", str(repo), "add", "source", "external"], check=True)
            subprocess.run(["git", "-C", str(repo), "-c", "user.name=Test", "-c",
                            "user.email=test@example.invalid", "commit", "-qm", "fixture"], check=True)
            commit = run.git(repo, "rev-parse", "HEAD").strip()
            (repo / "source").write_text("dirty\n")
            (repo / "untracked").write_text("do not copy\n")
            destination = base / "frozen"
            destination.mkdir()
            omitted = run.archive(repo, commit, destination, base / "source.tar")
            self.assertTrue((destination / "source").is_file())
            self.assertEqual((destination / "source").read_text(), "committed\n")
            self.assertFalse((destination / "untracked").exists())
            self.assertFalse((destination / "external").is_symlink())
            self.assertEqual(omitted, [{"path": "external", "target": "../../live-data"}])

    def test_patch_refuses_a_missing_or_ambiguous_anchor(self):
        with self.assertRaises(ValueError):
            run.replace_once("same same", "same", "changed")
        with self.assertRaises(ValueError):
            run.replace_once("different", "same", "changed")
        self.assertEqual(run.replace_once("prefix same suffix", "same", "changed"),
                         "prefix changed suffix")

    def test_timeout_terminates_child_and_retains_partial_log(self):
        with tempfile.TemporaryDirectory() as base:
            base = Path(base)
            with (base / "log").open("x") as log:
                code = run.bounded(["python3", "-c", "import time;print('started',flush=True);time.sleep(5)"],
                                   base, log, 0.2)
            self.assertEqual(code, 124)
            self.assertIn("started", (base / "log").read_text())

    def test_launch_failure_has_an_exit_code_and_retained_diagnostic(self):
        with tempfile.TemporaryDirectory() as base:
            base = Path(base)
            with (base / "log").open("x") as log:
                code = run.bounded([str(base / "missing-binary")], base, log, 1)
            self.assertEqual(code, 127)
            self.assertIn("launch failed", (base / "log").read_text())

    def test_sigterm_reaps_the_child_and_keeps_a_nonzero_receipt(self):
        with tempfile.TemporaryDirectory() as base:
            base = Path(base).resolve()
            pid_path = base / "pid"
            for name in ("consumer", "ushas"):
                (base / name).mkdir()
            child_program = ("import os,time;from pathlib import Path;"
                             f"Path({str(pid_path)!r}).write_text(str(os.getpid()));time.sleep(30)")
            binary = base / "sw-renderer"
            binary.write_text(f"#!{sys.executable}\n{child_program}\n")
            binary.chmod(0o700)
            run.write_json(base / "prepared.json", {})
            run.write_json(base / "build.json", {
                "exit_code":0, "binary_sha256":run.sha(binary),
                **{name + "_tree_sha256": run.tree_sha(base / name) for name in ("consumer", "ushas")}})
            wrapper = subprocess.Popen([sys.executable, str(HERE_RUN), "run", str(base),
                                        "--arm", "native", "--timeout", "30"], stdout=subprocess.DEVNULL)
            child_pid = None
            try:
                deadline = time.monotonic() + 5
                while not pid_path.exists() and time.monotonic() < deadline:
                    time.sleep(0.02)
                self.assertTrue(pid_path.exists())
                child_pid = int(pid_path.read_text())
                wrapper.send_signal(signal.SIGTERM)
                self.assertEqual(wrapper.wait(timeout=5), 143)
                receipt = json.loads(next(base.glob("native-*/manifest.json")).read_text())
                self.assertEqual(receipt["exit_code"], 143)
                self.assertFalse(receipt["valid"])
                self.assertIn("run.log", receipt["artifacts"])
                with self.assertRaises(ProcessLookupError):
                    os.kill(child_pid, 0)
            finally:
                if wrapper.poll() is None:
                    wrapper.kill()
                    wrapper.wait()
                if child_pid:
                    try:
                        os.killpg(child_pid, signal.SIGKILL)
                    except ProcessLookupError:
                        pass

    def test_capture_validation_rejects_missing_invalid_or_unrelated_files(self):
        with tempfile.TemporaryDirectory() as base:
            base = Path(base)
            self.assertFalse(run.valid_report({}, base, "native"))
            report = {"valid": True, "mode": "Disabled", "scale": 1.0,
                      "captures": [{"path": "/unrelated/file.png", "valid": True}]}
            self.assertFalse(run.valid_report(report, base, "native"))

    def test_all_six_expected_captures_and_matching_mode_are_required(self):
        with tempfile.TemporaryDirectory() as base:
            base = Path(base).resolve()
            captures = []
            for name in run.SHOTS:
                path = base / f"native_{name}.png"
                path.write_bytes(b"retained image")
                captures.append({"path": str(path), "valid": True})
            report = {"valid": True, "mode": "Disabled", "scale": 1.0,
                      "distinct_ready": 20, "warmup_seconds": 3.0, "captures": captures}
            self.assertTrue(run.valid_report(report, base, "native"))
            self.assertFalse(run.valid_report(report, base, "temporal"))
            captures.pop()
            self.assertFalse(run.valid_report(report, base, "native"))

    def test_metadata_failure_retains_a_failed_build_receipt(self):
        with tempfile.TemporaryDirectory() as base:
            base = Path(base).resolve()
            for name in ("consumer", "ushas"):
                (base / name).mkdir()
            (base / "consumer/Cargo.lock").write_text("lock\n")
            run.write_json(base / "prepared.json", {
                name + "_tree_sha256": run.tree_sha(base / name) for name in ("consumer", "ushas")})
            args = argparse.Namespace(directory=base, check=True, target_dir=None, timeout=1)
            with patch.object(run, "bounded", return_value=0), patch.object(
                run.subprocess, "check_output", side_effect=["host: aarch64-apple-darwin\n",
                    subprocess.CalledProcessError(1, "cargo metadata")]), contextlib.redirect_stdout(io.StringIO()):
                self.assertEqual(run.build(args), 2)
            receipt = json.loads((base / "check.json").read_text())
            self.assertEqual(receipt["exit_code"], 2)
            self.assertIn("cargo metadata", receipt["verification_error"])


if __name__ == "__main__":
    unittest.main()
