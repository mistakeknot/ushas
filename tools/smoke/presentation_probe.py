#!/usr/bin/env python3
"""Bounded visible presentation diagnostics; never a net-benefit benchmark."""
import sys
sys.dont_write_bytecode = True
import argparse
import ctypes
import contextlib
import hashlib
import io
import json
import math
import os
import plistlib
import signal
import subprocess
import tempfile
import time
import unittest
from datetime import datetime, timezone
from pathlib import Path
from unittest.mock import patch
import run as smoke_run

HERE = Path(__file__).resolve().parent
ARMS = [('temporal-only', 'temporal', 'default'), ('interpolation-single', 'interpolate', 'single'),
        ('interpolation-dual', 'interpolate', 'dual')]
SCOPE = ('Diagnostic reproduction only. Aggregate positive presentedTime values do not establish '
         'per-frame kind/order/content, panel pixels, latency, or net benefit. CPU loop cadence is '
         'not GPU throughput. Display samples cover the main display, not window residency; '
         'conditions between samples remain unobserved.')


def object_hash(value):
    return hashlib.sha256(json.dumps(value, sort_keys=True, separators=(',', ':')).encode()).hexdigest()


def display_probe():
    """Read CoreGraphics display state and best-effort boolean session metadata only."""
    cg = ctypes.CDLL('/System/Library/Frameworks/CoreGraphics.framework/CoreGraphics')
    cf = ctypes.CDLL('/System/Library/Frameworks/CoreFoundation.framework/CoreFoundation')
    cg.CGMainDisplayID.restype = ctypes.c_uint32
    cg.CGDisplayIsAsleep.argtypes = [ctypes.c_uint32]
    cg.CGDisplayIsAsleep.restype = ctypes.c_uint32
    cg.CGSessionCopyCurrentDictionary.restype = ctypes.c_void_p
    cf.CFStringCreateWithCString.argtypes = [ctypes.c_void_p, ctypes.c_char_p, ctypes.c_uint32]
    cf.CFStringCreateWithCString.restype = ctypes.c_void_p
    cf.CFDictionaryGetValue.argtypes = [ctypes.c_void_p, ctypes.c_void_p]
    cf.CFDictionaryGetValue.restype = ctypes.c_void_p
    cf.CFGetTypeID.argtypes = [ctypes.c_void_p]
    cf.CFGetTypeID.restype = ctypes.c_ulong
    cf.CFBooleanGetTypeID.restype = ctypes.c_ulong
    cf.CFBooleanGetValue.argtypes = [ctypes.c_void_p]
    cf.CFBooleanGetValue.restype = ctypes.c_bool
    cf.CFRelease.argtypes = [ctypes.c_void_p]
    dictionary = cg.CGSessionCopyCurrentDictionary()
    def boolean(key):
        if not dictionary:
            return None
        name = cf.CFStringCreateWithCString(None, key.encode(), 0x08000100)
        try:
            value = cf.CFDictionaryGetValue(dictionary, name)
            return bool(cf.CFBooleanGetValue(value)) if value and cf.CFGetTypeID(value) == cf.CFBooleanGetTypeID() else None
        finally:
            cf.CFRelease(name)
    try:
        locked = boolean('CGSSessionScreenIsLocked')
        on_console = boolean('kCGSSessionOnConsoleKey')
    finally:
        if dictionary:
            cf.CFRelease(dictionary)
    lock_source, lock_error = 'CGSessionCopyCurrentDictionary', None
    if locked is None:
        # The lock key is not a documented API contract. Missing/non-boolean
        # keys must remain unknown, never inferred from login or caffeinate.
        lock_source = 'IOConsoleUsers best-effort boolean; undocumented lock key'
        try:
            raw = subprocess.run(['/usr/sbin/ioreg', '-a', '-n', 'Root', '-d', '1'],
                                 capture_output=True, timeout=1, check=True)
            sessions = plistlib.loads(raw.stdout)[0].get('IOConsoleUsers', [])
            selected = [s for s in sessions if s.get('kCGSSessionOnConsoleKey') is True
                        and s.get('kCGSSessionUserIDKey') == os.getuid()]
            if len(selected) == 1:
                value = selected[0].get('CGSSessionScreenIsLocked')
                locked = value if type(value) is bool else None
                on_console = True
        except (OSError, ValueError, IndexError, TypeError, subprocess.SubprocessError) as error:
            lock_error = type(error).__name__
    display_id = cg.CGMainDisplayID()
    return {'display_id': display_id, 'awake': not bool(cg.CGDisplayIsAsleep(display_id)) if display_id else None,
            'locked': locked, 'on_console': on_console, 'lock_source': lock_source,
            'lock_error': lock_error, 'error': None}


def display_errors(samples):
    errors = []
    if len(samples) < 2:
        return ['insufficient display samples']
    if any(s.get('awake') is not True or not s.get('display_id') for s in samples):
        errors.append('display asleep or unknown')
    if any(s.get('locked') is not False or s.get('on_console') is not True for s in samples):
        errors.append('session locked, off-console, or unknown')
    if any(s.get('error') or s.get('lock_error') for s in samples):
        errors.append('display/session probe failure')
    if len({s.get('display_id') for s in samples}) != 1:
        errors.append('main display changed')
    if any(b['elapsed_s'] - a['elapsed_s'] > 2 for a,b in zip(samples, samples[1:])):
        errors.append('display sampling gap exceeded two seconds')
    return errors


def stop_tree(process):
    """Keep cleanup scoped to this wrapper and its descendants, including caffeinate's new group."""
    groups = {process.pid}
    try:
        result = subprocess.run(['/bin/ps', '-axo', 'pid=,ppid=,pgid='], capture_output=True, text=True, timeout=2, check=True)
        rows = [tuple(map(int, line.split())) for line in result.stdout.splitlines()]
        descendants = {process.pid}
        while True:
            found = {pid for pid, ppid, _ in rows if ppid in descendants}
            if found <= descendants:
                break
            descendants |= found
        groups |= {pgid for pid, _, pgid in rows if pid in descendants}
    except (OSError, ValueError, subprocess.SubprocessError):
        pass  # run.py's SIGTERM handler still cleans its owned renderer.
    groups.discard(os.getpgrp())
    for group in groups:
        try:
            os.killpg(group, signal.SIGTERM)
        except ProcessLookupError:
            pass
    try:
        process.wait(timeout=20)
    except subprocess.TimeoutExpired:
        pass
    finally:
        # A wrapper can exit before a resistant child in its separate group.
        for group in groups:
            try:
                os.killpg(group, signal.SIGKILL)
            except ProcessLookupError:
                pass
        process.wait(timeout=5)


def watch(argv, log, samples_file, probe, timeout=100, interval=.5):
    started = time.monotonic()
    result = {'exit_code': 1, 'timed_out': False, 'samples': [], 'child_exit_code': None}
    process = None
    with log.open('x') as output, samples_file.open('x') as samples:
        def sample():
            try:
                observation = probe()
            except Exception as error:
                observation = {'display_id': None, 'awake': None, 'locked': None,
                               'on_console': None, 'error': type(error).__name__ + ': ' + str(error)}
            observation = {**observation, 'elapsed_s': time.monotonic() - started,
                           'utc': datetime.now(timezone.utc).isoformat()}
            result['samples'].append(observation)
            samples.write(json.dumps(observation) + '\n')
            samples.flush()
        try:
            sample()
            initial = result['samples'][0]
            if (initial.get('awake') is not True or initial.get('locked') is not False
                    or initial.get('on_console') is not True or not initial.get('display_id')
                    or initial.get('error') or initial.get('lock_error')):
                result.update(exit_code=3,status='environment_unavailable')
                return result
            result['status'] = 'launched'
            process = subprocess.Popen(argv, stdout=output, stderr=subprocess.STDOUT, start_new_session=True)
            while process.poll() is None:
                if time.monotonic() - started >= timeout:
                    result.update(exit_code=124, timed_out=True)
                    stop_tree(process)
                    break
                try:
                    process.wait(timeout=interval)
                except subprocess.TimeoutExpired:
                    sample()
            if not result['timed_out']:
                result['exit_code'] = process.returncode
        except (OSError, KeyboardInterrupt, SystemExit) as error:
            result.update(exit_code=int(error.code) if isinstance(error,SystemExit) else 130 if isinstance(error, KeyboardInterrupt) else 1,
                          error=type(error).__name__ + ': ' + str(error))
        finally:
            if process is not None and process.poll() is None:
                stop_tree(process)
            result['child_exit_code'] = process.returncode if process else None
            sample()
    return result


def inspect_child(output, expected, binary_hash, process_exit, expected_argv=None):
    errors = []
    try:
        if output.is_symlink():
            raise ValueError('symlink report')
        output = output.resolve()
        manifest_path = Path(str(output) + '.manifest.json')
        if manifest_path.is_symlink():
            raise ValueError('symlink report or manifest')
        report, manifest = json.loads(output.read_text()), json.loads(manifest_path.read_text())
        for name, document in [('report',report),('manifest',manifest)]:
            if not isinstance(document,dict) or type(document.get('schema')) is not int or document['schema'] != 1:
                raise ValueError('unsupported or missing ' + name + ' schema')
        if process_exit != 0 or manifest.get('exit_code') != 0 or manifest.get('child_exit_code') != 0:
            errors.append('child or wrapper failed')
        errors.extend(smoke_run.check_report(output, Path(str(output) + '.png'), None, {}, {}))
        if manifest.get('evidence_errors') != [] or manifest.get('report_sha256') != smoke_run.digest(output):
            errors.append('wrapper evidence or report hash mismatch')
        log = Path(str(output)+'.log')
        if log.is_symlink() or not manifest.get('log_sha256') or manifest['log_sha256'] != smoke_run.digest(log):
            errors.append('child log hash mismatch')
        if manifest.get('binary_sha256') != binary_hash or manifest.get('binary_sha256_after') != binary_hash:
            errors.append('binary identity mismatch')
        if expected_argv is not None and manifest.get('argv') != expected_argv:
            errors.append('child argv mismatch')
        for key, value in expected.items():
            if report.get(key) != value:
                errors.append('report identity mismatch: ' + key)
        for name in ('screenshot', 'warmup_screenshot'):
            proof = report.get(name, {})
            if (proof.get('width') != 1280 or proof.get('height') != 720
                    or proof.get('opaque_fraction') != 1.0 or proof.get('alpha_zero_pixels') != 0
                    or proof.get('all_zero_rgba') is not False or proof.get('nonuniform') is not True
                    or proof.get('capture_error') is not None):
                errors.append('invalid opaque pixel proof: ' + name)
            retained = manifest.get('captures', {}).get(name, {})
            if not retained.get('sha256') or retained.get('sha256') != smoke_run.digest(proof.get('path', '')):
                errors.append('capture hash mismatch: ' + name)
    except (OSError, ValueError, TypeError, AttributeError) as error:
        errors.append('invalid child evidence: ' + str(error))
    return errors


def presentation_disposition(mode, evidence_errors, sample_errors, observed):
    if evidence_errors:
        return 'unavailable_invalid_child_evidence'
    if sample_errors:
        return 'unavailable_display_or_session_conditions'
    if mode == 'temporal':
        return 'not_applicable_temporal_baseline'
    if not isinstance(observed, dict):
        return 'unavailable_no_positive_timestamps'
    fps = observed.get('timestamp_fps')
    count = observed.get('presented_time_count')
    if type(count) is not int or count < 2 or type(fps) not in (float,int) or not math.isfinite(fps) or fps <= 0:
        return 'unavailable_no_positive_timestamps'
    return 'aggregate_timestamps_only_unvalidated'


def main():
    parser = argparse.ArgumentParser(description=__doc__ + ' ' + SCOPE)
    parser.add_argument('--binary', type=Path, required=True)
    parser.add_argument('--source-revision', required=True, help='expected full compiled clean source revision')
    parser.add_argument('--out', type=Path, required=True, help='new directory; never reused')
    parser.add_argument('--refresh-hz', type=float, default=120, help='sink assumption, not measured refresh')
    parser.add_argument('--parent-manifest', type=Path)
    args = parser.parse_args()
    if len(args.source_revision) != 40 or any(c not in '0123456789abcdef' for c in args.source_revision):
        parser.error('--source-revision must be a full lowercase git SHA')
    if not 1 <= args.refresh_hz <= 1000:
        parser.error('--refresh-hz must be finite and between 1 and 1000')
    binary = args.binary.resolve()
    binary_hash = smoke_run.digest(binary)
    if not binary_hash:
        parser.error('binary must be a regular file')
    if args.parent_manifest is not None and not smoke_run.digest(args.parent_manifest):
        parser.error('--parent-manifest must be an existing regular file')
    directory = args.out.absolute()
    directory.mkdir(parents=True, exist_ok=False)
    directory = directory.resolve()
    tool_hashes = {name:smoke_run.digest(HERE/name) for name in ('run.py','presentation_probe.py')}
    state = {'schema':1,'started_utc':datetime.now(timezone.utc).isoformat(),'scope':SCOPE,
        'net_benefit_verdict':'not_established','binary':str(binary),'binary_sha256':binary_hash,
        'expected_compiled_source_revision':args.source_revision,'tool_sha256':tool_hashes,
        'argv':sys.argv,'parent':{'pid':os.getpid(),'ppid':os.getppid(),'cwd':os.getcwd(),
            'python':sys.executable,'manifest':str(args.parent_manifest) if args.parent_manifest else None,
            'manifest_sha256':smoke_run.digest(args.parent_manifest) if args.parent_manifest else None},
        'invoker_source':smoke_run.command(['git','rev-parse','HEAD'],HERE.parents[1]),
        'invoker_status':smoke_run.command(['git','status','--porcelain'],HERE.parents[1]),
        'environment':{k:os.environ[k] for k in ('MTL_DEBUG_LAYER','MTL_SHADER_VALIDATION','WGPU_BACKEND','WGPU_POWER_PREF') if k in os.environ},
        'environment_sha256':object_hash(dict(os.environ)),
        'environment_hash_scope':'Hash of complete inherited environment; only render-related values disclosed.',
        'display_sampling':{'interval_seconds':.5,'maximum_gap_seconds':2,'lock_status':'best effort; unknown is not unlocked'},
        'child_timeout_seconds':65,'parent_watchdog_seconds':100,'cleanup_allowance_seconds':25,
        'refresh_hz_assumption':args.refresh_hz,'runs':[]}
    path = directory/'manifest.json'
    def retain():
        path.write_text(json.dumps(state,indent=2)+'\n')
    exit_code, aborted = 0, None
    def interrupted(signum, _frame):
        raise SystemExit(128 + signum)
    previous = signal.signal(signal.SIGTERM, interrupted)
    try:
        retain()
        for name,mode,presentation in ARMS:
            arm = {'arm':name,'mode':mode,'presentation':presentation}
            state['runs'].append(arm)
            if aborted:
                arm['status'] = 'skipped'
                arm['skipped'] = aborted
                continue
            out = directory/(name+'.json')
            argv = [sys.executable,str(HERE/'run.py'),'--timeout','65','--binary',str(binary),'--',
                    '--subject','claude','--mode',mode,'--presentation',presentation,'--refresh-hz',str(args.refresh_hz),
                    '--scale','0.5','--width','1280','--height','720','--pixel-iterations','20000',
                    '--warmup','2','--seconds','6','--out',str(out)]
            arm['argv'] = argv
            arm['argv_sha256'] = object_hash(argv)
            retain()
            if smoke_run.digest(binary) != binary_hash or any(smoke_run.digest(HERE/n)!=h for n,h in tool_hashes.items()):
                aborted = 'binary or runner changed'
                arm.update(skipped=aborted,valid=False)
                exit_code = exit_code or 1
                continue
            watched = watch(argv, directory/(name+'.parent.log'), directory/(name+'.display.jsonl'), display_probe)
            arm['process'] = {k:v for k,v in watched.items() if k!='samples'}
            arm['status'] = watched.get('status','launch_failed')
            arm['display_errors'] = display_errors(watched['samples'])
            expected = {'source_revision':args.source_revision,'source_dirty_at_build':'false',
                        'subject':'claude','mode':mode,'initial_scale':.5,'final_scale':.5,
                        'width':1280,'height':720,'pixel_iterations':20000,'warmup_s':2,'measurement_s':6,
                        'hdr':False,'moving':False,'native_aa':False,'adaptive_requested':False,
                        'completion_requested':False,'offscreen':False,'render_target':'window',
                        'presentation_requested':presentation}
            if watched.get('status') == 'environment_unavailable':
                arm['evidence_errors'] = ['not launched: environment_unavailable']
                aborted = 'environment unavailable at preceding arm preflight'
            else:
                arm['evidence_errors'] = inspect_child(out, expected, binary_hash, watched['exit_code'],
                                                      [str(binary),*argv[argv.index('--')+1:]])
            if smoke_run.digest(binary)!=binary_hash or any(smoke_run.digest(HERE/n)!=h for n,h in tool_hashes.items()):
                arm['evidence_errors'].append('binary or runner changed during arm')
                aborted = 'binary or runner changed'
            arm['render_evidence_valid'] = not arm['evidence_errors']
            arm['valid'] = arm['render_evidence_valid'] and not arm['display_errors'] and watched['exit_code'] == 0
            try:
                arm['observed_presentation'] = json.loads(out.read_text()).get('presentation')
            except (OSError,ValueError,AttributeError):
                arm['observed_presentation'] = None
            arm['presentation_disposition'] = presentation_disposition(mode, arm['evidence_errors'],
                arm['display_errors'], arm['observed_presentation'])
            arm['net_benefit_verdict'] = 'not_established'
            if watched['timed_out'] or watched.get('error'):
                aborted = 'watchdog or interrupted/failed wrapper'
            if not arm['valid'] or arm['display_errors']:
                code = watched['exit_code']
                exit_code = exit_code or (code if code > 0 else 128-code if code < 0 else 1)
            retain()
    except (OSError, KeyboardInterrupt, SystemExit) as error:
        exit_code = int(error.code) if isinstance(error,SystemExit) else 130 if isinstance(error,KeyboardInterrupt) else 1
        state['error'] = type(error).__name__ + ': ' + str(error)
    finally:
        signal.signal(signal.SIGTERM, previous)
        state.update(finished_utc=datetime.now(timezone.utc).isoformat(),exit_code=exit_code,
            valid=exit_code==0 and len(state['runs'])==3 and all(r.get('valid') for r in state['runs']))
        state['artifact_sha256'] = {p.name:smoke_run.digest(p) for p in directory.iterdir() if p.is_file() and p!=path}
        retain()
    print(json.dumps({'manifest':str(path),'exit_code':exit_code,'net_benefit_verdict':'not_established'}))
    return exit_code


class Tests(unittest.TestCase):
    def awake(self):
        return {'display_id': 1, 'awake': True, 'locked': False, 'on_console': True,
                'error': None, 'elapsed_s': 0}

    def test_display_loss_unknown_lock_and_probe_failure_are_not_eligible(self):
        good = [self.awake(), {**self.awake(), 'elapsed_s': .5}]
        self.assertEqual(display_errors(good), [])
        for changed in ({'awake': False}, {'locked': None}, {'locked': True},
                        {'on_console': None}, {'error': 'probe failed'}, {'display_id': 2}):
            with self.subTest(changed=changed):
                self.assertTrue(display_errors([good[0], {**good[1], **changed}]))
        self.assertTrue(display_errors([]))
        self.assertTrue(display_errors([good[0], {**good[1], 'elapsed_s': 4}]))

    def test_watchdog_terminates_cpu_child_and_retains_samples(self):
        with tempfile.TemporaryDirectory() as tmp:
            p = Path(tmp)
            result = watch([sys.executable, '-c', 'import time; time.sleep(30)'],
                           p/'log', p/'display.jsonl', self.awake, timeout=.15, interval=.03)
            self.assertTrue(result['timed_out'])
            self.assertEqual(result['exit_code'], 124)
            self.assertIsNotNone(result['child_exit_code'])
            self.assertGreaterEqual(len(result['samples']), 2)
            self.assertEqual(len((p/'display.jsonl').read_text().splitlines()), len(result['samples']))

    def test_nonzero_child_exit_is_retained(self):
        with tempfile.TemporaryDirectory() as tmp:
            p = Path(tmp)
            result = watch([sys.executable, '-c', 'raise SystemExit(7)'],
                           p/'log', p/'display.jsonl', self.awake, timeout=2, interval=.03)
            self.assertEqual(result['exit_code'], 7)
            self.assertFalse(result['timed_out'])

    def test_probe_failure_remains_unknown_and_does_not_launch(self):
        def broken():
            raise OSError('CPU test probe unavailable')
        with tempfile.TemporaryDirectory() as tmp:
            p = Path(tmp)
            with patch('subprocess.Popen') as launch:
                result = watch([sys.executable, '-c', 'pass'], p/'log', p/'display.jsonl',
                               broken, timeout=2, interval=.03)
                launch.assert_not_called()
            self.assertEqual(result['exit_code'], 3)
            self.assertEqual(result['status'],'environment_unavailable')
            self.assertTrue(display_errors(result['samples']))
            self.assertTrue(all(s['locked'] is None for s in result['samples']))

    def test_locked_preflight_retains_blocked_attempt_without_child(self):
        with tempfile.TemporaryDirectory() as tmp:
            p = Path(tmp)
            with patch('subprocess.Popen') as launch:
                result = watch(['must-not-launch'],p/'log',p/'display.jsonl',
                               lambda:{**self.awake(),'locked':True}, timeout=.01)
                launch.assert_not_called()
            self.assertEqual(result['status'],'environment_unavailable')
            self.assertIsNone(result['child_exit_code'])
            self.assertTrue(result['samples'])

    def test_cli_retains_one_blocked_attempt_and_two_skipped_arms(self):
        with tempfile.TemporaryDirectory() as tmp:
            p = Path(tmp)
            binary = p/'frozen-binary'
            binary.write_bytes(b'CPU-only placeholder; must not execute')
            argv = [str(Path(__file__)), '--binary',str(binary),'--source-revision','0'*40,'--out',str(p/'new')]
            with patch.object(sys,'argv',argv), patch(__name__+'.display_probe',return_value={**self.awake(),'locked':True}), \
                    patch.object(smoke_run,'command',return_value={'exit_code':0,'stdout':'test','stderr':''}), \
                    patch('subprocess.Popen') as launch, contextlib.redirect_stdout(io.StringIO()):
                code = main()
                launch.assert_not_called()
            report = json.loads((p/'new/manifest.json').read_text())
            self.assertEqual(code,3)
            self.assertFalse(report['valid'])
            self.assertEqual([r['status'] for r in report['runs']],['environment_unavailable','skipped','skipped'])
            self.assertEqual(report['net_benefit_verdict'],'not_established')
            self.assertFalse(any((p/'new').glob('*.json.manifest.json')))

    def test_midrun_lock_loss_cannot_leave_valid_true(self):
        with tempfile.TemporaryDirectory() as tmp:
            p = Path(tmp)
            binary = p/'binary'
            binary.write_bytes(b'never executed')
            argv = [str(Path(__file__)),'--binary',str(binary),'--source-revision','0'*40,'--out',str(p/'new')]
            observed = {'status':'launched','exit_code':0,'child_exit_code':0,'timed_out':False,
                'samples':[self.awake(),{**self.awake(),'locked':True,'elapsed_s':.5}]}
            with patch.object(sys,'argv',argv), patch(__name__+'.watch',return_value=observed), \
                    patch(__name__+'.inspect_child',return_value=[]), \
                    patch.object(smoke_run,'command',return_value={}), contextlib.redirect_stdout(io.StringIO()):
                self.assertEqual(main(),1)
            report = json.loads((p/'new/manifest.json').read_text())
            self.assertFalse(report['valid'])
            self.assertTrue(all(r['render_evidence_valid'] for r in report['runs']))
            self.assertTrue(all(r['valid'] is False for r in report['runs']))

    def test_interruption_after_all_arm_records_cannot_leave_valid_true(self):
        with tempfile.TemporaryDirectory() as tmp:
            p = Path(tmp)
            binary = p/'binary'
            binary.write_bytes(b'never executed')
            argv = [str(Path(__file__)),'--binary',str(binary),'--source-revision','0'*40,'--out',str(p/'new')]
            observed = {'status':'launched','exit_code':0,'child_exit_code':0,'timed_out':False,
                'samples':[self.awake(),{**self.awake(),'elapsed_s':.5}]}
            original_write, calls = Path.write_text, 0
            def interrupt_last_arm(path, *a, **kw):
                nonlocal calls
                calls += 1
                if calls == 7:
                    raise KeyboardInterrupt('after all three child records')
                return original_write(path,*a,**kw)
            with patch.object(sys,'argv',argv), patch(__name__+'.watch',return_value=observed), \
                    patch(__name__+'.inspect_child',return_value=[]), patch.object(Path,'write_text',interrupt_last_arm), \
                    patch.object(smoke_run,'command',return_value={}), contextlib.redirect_stdout(io.StringIO()):
                self.assertEqual(main(),130)
            report = json.loads((p/'new/manifest.json').read_text())
            self.assertEqual(len(report['runs']),3)
            self.assertTrue(all(r['valid'] for r in report['runs']))
            self.assertFalse(report['valid'])

    def test_missing_or_invalid_child_evidence_never_passes(self):
        with tempfile.TemporaryDirectory() as tmp:
            out = Path(tmp)/'report.json'
            self.assertTrue(inspect_child(out, {'mode':'temporal'}, 'binary', 0))
            out.write_text(json.dumps({'valid':False}))
            self.assertTrue(inspect_child(out, {'mode':'temporal'}, 'binary', 0))
            out.write_text(json.dumps({'valid':True}))
            self.assertTrue(inspect_child(out, {'mode':'temporal'}, 'binary', 7))

    def test_actual_capture_schema_passes_and_mutations_fail(self):
        with tempfile.TemporaryDirectory() as tmp:
            out = Path(tmp)/'report.json'
            report = {'schema':1,'valid':True,'mode':'temporal','lifecycle':None}
            log = Path(str(out)+'.log')
            log.write_text('CPU fixture log')
            for name,suffix in [('screenshot','.png'),('warmup_screenshot','.png.warmup.png')]:
                shot = Path(str(out)+suffix)
                shot.write_bytes(b'fixture capture; decoder proof belongs to frozen smoke')
                report[name] = {'path':str(shot),'width':1280,'height':720,
                                'opaque_fraction':1.0,'alpha_zero_pixels':0,'all_zero_rgba':False,
                                'nonuniform':True,'capture_error':None}
            def retain():
                out.write_text(json.dumps(report))
                manifest = {'schema':1,'exit_code':0,'child_exit_code':0,'evidence_errors':[],
                    'binary_sha256':'binary','binary_sha256_after':'binary',
                    'report_sha256':smoke_run.digest(out),'log_sha256':smoke_run.digest(log),
                    'captures':{n:{'sha256':smoke_run.digest(report[n]['path'])} for n in ('screenshot','warmup_screenshot')}}
                Path(str(out)+'.manifest.json').write_text(json.dumps(manifest))
            retain()
            self.assertEqual(inspect_child(out, {'mode':'temporal'}, 'binary', 0), [])
            for key,value in [('opaque_fraction',0),('alpha_zero_pixels',1),('all_zero_rgba',True),
                              ('nonuniform',False),('capture_error','failed'),('width',1)]:
                old = report['screenshot'][key]
                report['screenshot'][key] = value
                retain()
                self.assertTrue(inspect_child(out, {'mode':'temporal'}, 'binary', 0), key)
                report['screenshot'][key] = old
            retain()
            self.assertTrue(inspect_child(out, {'mode':'interpolate'}, 'binary', 0))
            self.assertTrue(inspect_child(out, {'mode':'temporal'}, 'changed', 0))
            for version in (None,2,True,1.0):
                if version is None:
                    report.pop('schema',None)
                else:
                    report['schema'] = version
                retain()
                self.assertTrue(inspect_child(out, {'mode':'temporal'}, 'binary', 0),'report schema')
                report['schema'] = 1
                retain()
                manifest_path = Path(str(out)+'.manifest.json')
                changed = json.loads(manifest_path.read_text())
                if version is None:
                    changed.pop('schema',None)
                else:
                    changed['schema'] = version
                manifest_path.write_text(json.dumps(changed))
                self.assertTrue(inspect_child(out, {'mode':'temporal'}, 'binary', 0),'manifest schema')
            retain()
            log.write_text('modified after child manifest')
            self.assertTrue(inspect_child(out, {'mode':'temporal'}, 'binary', 0),'log hash')

    def test_disposition_never_promotes_missing_counts_or_unknown_lock(self):
        observed = {'timestamp_fps':120.0,'presented_time_count':500}
        self.assertEqual(presentation_disposition('interpolate',[],[],observed), 'aggregate_timestamps_only_unvalidated')
        self.assertEqual(presentation_disposition('temporal',[],[],None),'not_applicable_temporal_baseline')
        for missing in (None,{}, {'timestamp_fps':float('nan'),'presented_time_count':500},
                        {'timestamp_fps':120.0,'presented_time_count':0}):
            self.assertEqual(presentation_disposition('interpolate',[],[],missing),'unavailable_no_positive_timestamps')
        self.assertEqual(presentation_disposition('interpolate',[],['unknown lock'],observed),
                         'unavailable_display_or_session_conditions')
        self.assertEqual(presentation_disposition('interpolate',['failed'],[],observed),
                         'unavailable_invalid_child_evidence')

    def test_watchdog_cleans_resistant_descendant_in_separate_group(self):
        with tempfile.TemporaryDirectory() as tmp:
            p = Path(tmp)
            pidfile = p/'pids'
            heartbeat = p/'heartbeat'
            child_script = ('import signal,time\nfrom pathlib import Path\n'
                            'signal.signal(signal.SIGTERM,signal.SIG_IGN)\n'
                            f'p=Path({str(heartbeat)!r})\n'
                            'for i in range(3000):\n p.write_text(str(i)); time.sleep(.01)\n')
            script = ('import os,subprocess,sys,time,json; '
                      f'child=subprocess.Popen([sys.executable,"-c",{child_script!r}],start_new_session=True); '
                      f'open({str(pidfile)!r},"w").write(json.dumps([os.getpid(),child.pid])); time.sleep(30)')
            def process_table(*_a,**_k):
                parent,child = json.loads(pidfile.read_text())
                return subprocess.CompletedProcess([],0,f'{parent} {os.getpid()} {parent}\n{child} {parent} {child}\n','')
            try:
                with patch('subprocess.run',side_effect=process_table):
                    result = watch([sys.executable,'-c',script],p/'log',p/'samples',self.awake,timeout=.3,interval=.03)
                self.assertEqual(result['exit_code'],124)
                before = heartbeat.read_text()
                time.sleep(.1)
                self.assertEqual(heartbeat.read_text(),before)
            finally:
                if pidfile.exists():
                    try: os.killpg(json.loads(pidfile.read_text())[1],signal.SIGKILL)
                    except ProcessLookupError: pass


if __name__ == '__main__':
    if sys.argv[1:] == ['--self-test']:
        unittest.main(argv=[sys.argv[0]])
    else:
        raise SystemExit(main())
