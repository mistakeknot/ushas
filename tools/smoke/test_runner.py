"""CPU-only runner contracts; no Bevy binary, display, or GPU is launched."""
import contextlib
import hashlib
import importlib.util
import io
import json
from pathlib import Path
import subprocess
import sys
import tempfile
import time
import unittest
from unittest import mock

SPEC = importlib.util.spec_from_file_location("smoke_runner", Path(__file__).with_name("run.py"))
runner = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(runner)

EXERCISES = {
    "resize": ("Resize", ["initial", "changed", "restored"]),
    "camera-cut": ("CameraCut", ["initial", "changed"]),
    "late-camera": ("LateCamera", ["changed", "restored"]),
    "multiple-views": ("MultipleViews", ["initial", "restored"]),
    "inactive-cut-resume": ("InactiveCutResume", ["initial", "restored"]),
    "creation-failure": ("CreationFailure", ["initial", "changed", "restored"]),
    "creation-slow": ("CreationSlow", ["initial", "changed", "restored"]),
    "window-minimize": ("WindowMinimize", ["initial", "restored"]),
    "os-sleep-resume": ("OsSleepResume", ["initial", "restored"]),
}

FAKE = r'''
import json, sys, time
from pathlib import Path
binary = Path(__file__)
scenario = json.loads(binary.with_suffix('.scenario.json').read_text())
args = sys.argv[1:]
output = Path(args[args.index('--out') + 1])
shot = Path(args[args.index('--screenshot') + 1]) if '--screenshot' in args else Path(str(output) + '.png')
binary.with_suffix('.launched').write_text('yes')
captures = []
for phase in scenario.get('phases', []):
    path = Path(str(output) + '.lifecycle-' + phase + '.png')
    path.write_bytes(('phase:' + phase).encode())
    captures.append({'phase': phase, 'path': str(path), 'valid': True})
if scenario.get('timeout'):
    time.sleep(10)
report = {'valid': True, 'lifecycle': None}
for name, path in [('screenshot', shot), ('warmup_screenshot', Path(str(shot) + '.warmup.png'))]:
    path.parent.mkdir(parents=True, exist_ok=True)
    if scenario.get('missing_main') != name:
        path.write_bytes(('capture:' + name).encode())
    report[name] = {'path': str(path), 'nonuniform': True}
if '--lifecycle' in args:
    report['lifecycle'] = {'exercise': scenario['exercise'], 'valid': True, 'captures': captures}
kind = scenario.get('kind')
if kind == 'missing_phase':
    Path(captures[-1]['path']).unlink()
elif kind == 'missing_phase_report':
    captures.pop()
elif kind == 'duplicate_phase':
    captures.append(captures[0].copy())
elif kind == 'invalid_phase':
    captures[-1]['valid'] = False
elif kind == 'invalid_phase_name':
    captures[-1]['phase'] = []
elif kind == 'invalid_captures_type':
    report['lifecycle']['captures'] = {}
elif kind == 'symlink_phase':
    path = Path(captures[-1]['path'])
    path.unlink()
    path.symlink_to(shot)
elif kind == 'wrong_phase_path':
    captures[-1]['path'] = str(shot)
elif kind == 'wrong_exercise':
    report['lifecycle']['exercise'] = 'Resize'
elif kind == 'invalid_lifecycle':
    report['lifecycle']['valid'] = False
elif kind == 'missing_lifecycle':
    report['lifecycle'] = None
elif kind == 'wrong_main_path':
    report['screenshot']['path'] = str(binary)
elif kind == 'invalid_report':
    report['valid'] = False
elif kind == 'array_report':
    report = []
if kind != 'missing_report':
    output.write_text('{bad json' if kind == 'malformed_report' else json.dumps(report))
if kind == 'binary_changed':
    with binary.open('a') as stream:
        stream.write('\n# binary changed during run\n')
sys.exit(scenario.get('exit_code', 0))
'''


class RunnerTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory(prefix="ushas runner ")
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name)
        self.binary = self.root / "fake smoke"
        self.binary.write_text(f"#!{sys.executable}\n" + FAKE)
        self.binary.chmod(0o755)
        self.caffeinate = self.root / "fake caffeinate"
        self.caffeinate.write_text('#!/bin/sh\nshift\nexec "$@"\n')
        self.caffeinate.chmod(0o755)
        self.output = self.root / "run.json"
        self.scenario = {}

    def invoke(self, exercise=None, extra=(), timeout=2):
        if exercise:
            name, phases = EXERCISES.get(exercise, (exercise, []))
            self.scenario.setdefault("exercise", name)
            self.scenario.setdefault("phases", phases)
        self.binary.with_suffix('.scenario.json').write_text(json.dumps(self.scenario))
        args = ["run.py", "--binary", str(self.binary), "--timeout", str(timeout),
                "--", "--out", str(self.output)]
        if exercise:
            args += ["--lifecycle", exercise]
        args += list(extra)
        real_popen = subprocess.Popen

        def launch(argv, **kwargs):
            self.assertEqual(argv[:2], ["/usr/bin/caffeinate", "-d"])
            return real_popen([str(self.caffeinate), *argv[1:]], **kwargs)

        with mock.patch.object(sys, "argv", args), \
                mock.patch.object(runner, "command", return_value={"test": "CPU-only"}), \
                mock.patch.object(runner.subprocess, "Popen", side_effect=launch), \
                contextlib.redirect_stdout(io.StringIO()), contextlib.redirect_stderr(io.StringIO()):
            try:
                return runner.main()
            except SystemExit as error:
                return error.code

    def manifest(self):
        return json.loads(Path(str(self.output) + '.manifest.json').read_text())

    def test_every_exercise_retains_the_correct_phase_hashes(self):
        for exercise, (_, phases) in EXERCISES.items():
            with self.subTest(exercise=exercise):
                self.output = self.root / (exercise + '.json')
                self.scenario = {}
                self.assertEqual(self.invoke(exercise), 0)
                retained = self.manifest()["lifecycle_captures"]
                self.assertEqual(set(retained), {"initial", "changed", "restored"})
                for phase in phases:
                    expected = hashlib.sha256(('phase:' + phase).encode()).hexdigest()
                    self.assertEqual(retained[phase]["sha256"], expected)
                for phase in set(retained) - set(phases):
                    self.assertIsNone(retained[phase]["sha256"])

    def test_existing_unused_phase_capture_prevents_launch(self):
        capture = Path(str(self.output) + '.lifecycle-changed.png')
        capture.write_bytes(b'retained')
        self.assertNotEqual(self.invoke('inactive-cut-resume'), 0)
        self.assertFalse(self.binary.with_suffix('.launched').exists())
        self.assertEqual(capture.read_bytes(), b'retained')

    def test_existing_base_artifacts_are_never_overwritten(self):
        for index, suffix in enumerate(['', '.manifest.json', '.log', '.png', '.png.warmup.png']):
            with self.subTest(suffix=suffix):
                self.output = self.root / (str(index) + '.json')
                retained = Path(str(self.output) + suffix)
                retained.write_bytes(b'retained')
                self.assertNotEqual(self.invoke(), 0)
                self.assertFalse(self.binary.with_suffix('.launched').exists())
                self.assertEqual(retained.read_bytes(), b'retained')

    def test_losing_log_reservation_cannot_write_the_winners_manifest(self):
        log = Path(str(self.output) + '.log')
        original_digest = runner.digest
        collided = False

        def race(path):
            nonlocal collided
            if Path(path) == self.binary.resolve() and not collided:
                log.write_bytes(b'competing runner owns this log')
                collided = True
            return original_digest(path)

        with mock.patch.object(runner, 'digest', side_effect=race):
            self.assertNotEqual(self.invoke(), 0)
        self.assertTrue(collided)
        self.assertFalse(self.binary.with_suffix('.launched').exists())
        self.assertFalse(Path(str(self.output) + '.manifest.json').exists())
        self.assertEqual(log.read_bytes(), b'competing runner owns this log')

    def test_dangling_phase_symlink_prevents_launch(self):
        capture = Path(str(self.output) + '.lifecycle-restored.png')
        capture.symlink_to(self.root / 'missing.png')
        self.assertNotEqual(self.invoke('resize'), 0)
        self.assertFalse(self.binary.with_suffix('.launched').exists())

    def test_screenshot_cannot_collide_with_a_phase_capture(self):
        path = str(self.output) + '.lifecycle-initial.png'
        self.assertNotEqual(self.invoke('resize', ['--screenshot', path]), 0)
        self.assertFalse(self.binary.with_suffix('.launched').exists())

    def test_report_cannot_be_an_ancestor_of_a_capture_path(self):
        self.assertNotEqual(self.invoke(extra=['--screenshot', str(self.output / 'shot.png')]), 0)
        self.assertFalse(self.binary.with_suffix('.launched').exists())

    def test_invalid_or_incomplete_lifecycle_report_fails_closed(self):
        for kind in ['missing_phase', 'missing_phase_report', 'duplicate_phase', 'invalid_phase',
                     'invalid_phase_name', 'invalid_captures_type', 'symlink_phase',
                     'wrong_phase_path', 'wrong_exercise', 'invalid_lifecycle', 'missing_lifecycle']:
            with self.subTest(kind=kind):
                self.output = self.root / (kind + '.json')
                self.scenario = {'kind': kind}
                self.assertNotEqual(self.invoke('camera-cut'), 0)
                self.assertIn('wrapper_failure', self.manifest())

    def test_reported_main_capture_cannot_substitute_another_file(self):
        self.scenario = {'kind': 'wrong_main_path'}
        self.assertNotEqual(self.invoke(), 0)

    def test_custom_screenshot_path_is_canonical_and_retained(self):
        screenshot = self.root / 'nested captures' / 'final.png'
        self.assertEqual(self.invoke(extra=['--screenshot', str(screenshot)]), 0)
        self.assertEqual(self.manifest()['captures']['screenshot']['path'], str(screenshot.resolve()))
        self.assertTrue(self.manifest()['captures']['warmup_screenshot']['sha256'])

    def test_missing_invalid_report_and_captures_fail_closed(self):
        for kind in ['missing_report', 'malformed_report', 'array_report', 'invalid_report']:
            with self.subTest(kind=kind):
                self.output = self.root / (kind + '.json')
                self.scenario = {'kind': kind}
                self.assertNotEqual(self.invoke(), 0)
                self.assertIn('wrapper_failure', self.manifest())
        self.output = self.root / 'missing_capture.json'
        self.scenario = {'missing_main': 'warmup_screenshot'}
        self.assertNotEqual(self.invoke(), 0)

    def test_partial_phase_evidence_is_hashed_without_a_report(self):
        self.scenario = {'kind': 'missing_report', 'phases': ['initial']}
        self.assertNotEqual(self.invoke('resize'), 0)
        self.assertTrue(self.manifest()['lifecycle_captures']['initial']['sha256'])

    def test_timeout_retains_partial_captures_and_stops_child(self):
        self.scenario = {'timeout': True, 'phases': ['initial']}
        start = time.monotonic()
        self.assertEqual(self.invoke('resize', timeout=.5), 124)
        self.assertLess(time.monotonic() - start, 4)
        manifest = self.manifest()
        self.assertTrue(manifest['timed_out'])
        self.assertIsNotNone(manifest['child_exit_code'])
        self.assertTrue(manifest['lifecycle_captures']['initial']['sha256'])

    def test_binary_changed_during_run_is_invalid(self):
        self.scenario = {'kind': 'binary_changed'}
        self.assertNotEqual(self.invoke(), 0)
        manifest = self.manifest()
        self.assertNotEqual(manifest['binary_sha256'], manifest['binary_sha256_after'])

    def test_nonzero_child_cannot_be_turned_into_success(self):
        self.scenario = {'exit_code': 7}
        self.assertEqual(self.invoke(), 7)
        self.assertEqual(self.manifest()['child_exit_code'], 7)

    def test_duplicate_or_unknown_lifecycle_is_rejected_before_launch(self):
        self.assertNotEqual(self.invoke('unknown'), 0)
        self.assertFalse(self.binary.with_suffix('.launched').exists())
        self.scenario = {}
        self.assertNotEqual(self.invoke('resize', ['--lifecycle', 'camera-cut']), 0)
        self.assertFalse(self.binary.with_suffix('.launched').exists())

    def test_missing_values_and_invalid_timeouts_are_rejected_before_launch(self):
        for extra in [['--screenshot'], ['--lifecycle'], ['--screenshot', '--lifecycle', 'resize']]:
            with self.subTest(extra=extra):
                self.assertNotEqual(self.invoke(extra=extra), 0)
                self.assertFalse(self.binary.with_suffix('.launched').exists())
        for timeout in [0, -1, float('nan'), float('inf')]:
            with self.subTest(timeout=timeout):
                self.assertNotEqual(self.invoke(timeout=timeout), 0)
                self.assertFalse(self.binary.with_suffix('.launched').exists())


if __name__ == '__main__':
    unittest.main()
