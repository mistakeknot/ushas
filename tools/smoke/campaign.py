#!/usr/bin/env python3
"""Serial fixed-scale smoke campaign and paired run-level CPU-loop analysis.

Default: Claude, offscreen 1280x720, five arms, three loads, four repetitions,
4s warmup + 6s measurement. No experimental timestamps unless explicitly requested.
Use --dry-run to inspect commands or --analyze-existing DIR for read-only analysis.
"""
import argparse
from datetime import datetime, timezone
import hashlib
import json
import math
import os
from pathlib import Path
import random
import signal
import statistics
import subprocess
import sys
import time
import unittest
import uuid


ARMS = (
    ("native-disabled-msaa4", "disabled", "1", True),
    ("temporal-native", "temporal", "1", False),
    ("temporal-half", "temporal", "0.5", False),
    ("temporal-third", "temporal", "0.3333333333333333", False),
    ("bilinear-half", "disabled", "0.5", False),
)
REPETITIONS = 4
BOOTSTRAP_SEED = 21434
BOOTSTRAP_DRAWS = 10000
PRACTICAL_THRESHOLD = 0.08
ROOT = Path(__file__).resolve().parents[2]
RUNNER = Path(__file__).with_name("run.py").resolve()


def digest(path):
    try:
        with Path(path).open("rb") as stream:
            return hashlib.file_digest(stream, "sha256").hexdigest()
    except OSError:
        return None


def make_plan(binary, directory, loads=(0, 8000, 20000), warmup=4, seconds=6,
              timeout=90, experimental=False):
    binary, directory = binary.resolve(), directory.resolve()
    if (not 1 <= len(loads) <= 4 or len(set(loads)) != len(loads)
            or any(type(load) is not int or not 0 <= load <= 100000 for load in loads)):
        raise ValueError('loads must be one to four unique integers in 0..100000')
    if (not all(math.isfinite(x) for x in (warmup, seconds, timeout))
            or warmup < 0 or seconds <= 0 or timeout <= warmup + seconds):
        raise ValueError('finite nonnegative warmup, positive seconds, and a longer timeout are required')
    jobs = []
    for repetition in range(REPETITIONS):
        pair = repetition // 2
        offset = pair % len(loads)
        load_order = (*loads[offset:], *loads[:offset])
        if repetition % 2:
            load_order = tuple(reversed(load_order))
        for load in load_order:
            arm_offset = (pair + loads.index(load)) % len(ARMS)
            arms = (*ARMS[arm_offset:], *ARMS[:arm_offset])
            if repetition % 2:
                arms = tuple(reversed(arms))
            for position, (arm, mode, scale, native_aa) in enumerate(arms):
                identity = f'{len(jobs):03d}-r{repetition}-load{load}-{arm}'
                output = directory / f'{identity}.json'
                arguments = ['--subject', 'claude', '--offscreen', '--mode', mode, '--scale', scale,
                             '--width', '1280', '--height', '720', '--pixel-iterations', str(load),
                             '--warmup', str(warmup), '--seconds', str(seconds), '--out', str(output)]
                if native_aa:
                    arguments.append('--native-aa')
                if experimental:
                    arguments.append('--experimental-timing')
                expected = {'subject': 'claude', 'offscreen': True, 'render_target': 'image',
                            'mode': mode, 'initial_scale': float(scale), 'final_scale': float(scale),
                            'native_aa': native_aa, 'width': 1280, 'height': 720,
                            'pixel_iterations': load, 'warmup_s': warmup, 'measurement_s': seconds,
                            'cpu_delay_ms': 0, 'moving': False, 'hdr': False,
                            'adaptive_requested': False, 'lifecycle': None, 'target_fps': 60.0,
                            'minimum_scale': .5, 'presentation_requested': 'unavailable_offscreen'}
                jobs.append({'id': identity, 'repetition': repetition, 'load': load, 'arm': arm,
                             'position': position, 'output': str(output), 'expected': expected,
                             'arguments': arguments,
                             'argv': [sys.executable, str(RUNNER), '--timeout', str(timeout),
                                      '--binary', str(binary), '--', *arguments]})
    return {'schema': 1, 'binary': str(binary), 'binary_sha256': digest(binary),
            'campaign_script_sha256': digest(Path(__file__)),
            'runner_sha256': digest(RUNNER), 'run_dir': str(directory), 'loads': list(loads),
            'repetitions': REPETITIONS, 'warmup_s': warmup, 'measurement_s': seconds,
            'timeout_seconds': timeout, 'experimental_timing': experimental,
            'bootstrap_seed': BOOTSTRAP_SEED, 'bootstrap_draws': BOOTSTRAP_DRAWS,
            'practical_threshold': PRACTICAL_THRESHOLD,
            'order': 'Two forward/reverse pairs for arm and load order; the second pair rotates the base order. '
                     'Every arm/load combination has the same mean global position over four independent runs.',
            'jobs': jobs}


def paired_summary(ratios):
    if len(ratios) != REPETITIONS or any(not math.isfinite(x) or x <= 0 for x in ratios):
        raise ValueError('four positive finite paired run ratios are required')
    rng = random.Random(BOOTSTRAP_SEED)
    draws = sorted(statistics.mean(rng.choices(ratios, k=len(ratios))) for _ in range(BOOTSTRAP_DRAWS))
    interval = [draws[round((len(draws) - 1) * q)] for q in (.025, .975)]
    decision = 'uncertain_or_below_practical_threshold'
    if interval[1] < 1 - PRACTICAL_THRESHOLD:
        decision = 'clear_practical_improvement'
    elif interval[0] > 1 + PRACTICAL_THRESHOLD:
        decision = 'clear_practical_regression'
    return {'paired_runs': len(ratios), 'ratios': ratios, 'mean_ratio': statistics.mean(ratios),
            'ratio_ci95': interval, 'mean_loop_time_reduction': 1 - statistics.mean(ratios),
            'decision': decision}


def load_object(path):
    value = json.loads(Path(path).read_text())
    if not isinstance(value, dict):
        raise ValueError(f'{path}: expected a JSON object')
    return value


def matches(actual, expected):
    if type(expected) in (int, float):
        return type(actual) in (int, float) and math.isfinite(actual) and math.isclose(actual, expected, abs_tol=1e-6)
    return type(actual) is type(expected) and actual == expected


def marker_summary(report, requested):
    markers = report.get('experimental_timing')
    observations = markers.get('observations', []) if isinstance(markers, dict) else []
    observations = observations if isinstance(observations, list) else []
    values = [row.get('gpu_elapsed_ms') for row in observations if isinstance(row, dict)]
    usable = [value for value in values if type(value) in (int, float) and math.isfinite(value) and value > 0]
    return {'requested': requested, 'status': markers.get('status') if isinstance(markers, dict) else None,
            'retained_measured_observations': len(observations),
            'completed_total': markers.get('completed_total') if isinstance(markers, dict) else None,
            'retained_unfiltered_count': markers.get('retained_unfiltered_count') if isinstance(markers, dict) else None,
            'mean_elapsed_ms': statistics.mean(usable) if usable and len(usable) == len(observations) else None,
            'availability': 'retained measured-frame observations' if usable else
                            'no retained measured-frame observations; this does not imply zero completed queries',
            'validated_for_governor': False,
            'scope': 'Experimental elapsed marker envelope; not GPU busy cost or panel delivery.'}


def inspect_run(plan, job, result):
    row = {key: job[key] for key in ('id', 'repetition', 'load', 'arm', 'position', 'output')}
    errors = []
    row.update({'errors': errors, 'mean_loop_ms': None, 'loop_samples': None,
                'experimental_markers': marker_summary({}, plan['experimental_timing'])})
    if result is None or result.get('wrapper_exit_code') != 0:
        errors.append('run was not attempted successfully; see campaign execution record')
    output = Path(job['output'])
    directory = Path(plan['run_dir']).resolve()
    if output.resolve().parent != directory or output.name != job['id'] + '.json':
        errors.append('output path is outside its planned campaign directory')
        return row, None
    try:
        report = load_object(output)
        manifest = load_object(str(output) + '.manifest.json')
    except (OSError, ValueError) as error:
        errors.append(str(error))
        return row, None
    if (manifest.get('exit_code') != 0 or manifest.get('child_exit_code') != 0
            or manifest.get('smoke_valid') is not True or manifest.get('timed_out')
            or manifest.get('evidence_errors')):
        errors.append('runner manifest did not validate a successful child')
    if report.get('valid') is not True or report.get('timed_out') is not False:
        errors.append('smoke report did not validate a complete run')
    if manifest.get('report_sha256') != digest(output):
        errors.append('report hash differs from retained manifest')
    binary_hash = plan.get('binary_sha256')
    if (not binary_hash or manifest.get('binary_sha256') != binary_hash
            or manifest.get('binary_sha256_after') != binary_hash
            or manifest.get('binary') != plan['binary']):
        errors.append('binary identity differs from campaign plan')
    if manifest.get('argv') != [plan['binary'], *job['arguments']]:
        errors.append('runner arguments differ from the predeclared arm')
    for key, expected in job['expected'].items():
        if not matches(report.get(key), expected):
            errors.append(f'configuration mismatch: {key}')
    environment = report.get('environment')
    if not isinstance(environment, dict):
        environment = {}
        errors.append('runtime environment is missing')
    runtime_hash = environment.get('binary_sha256')
    if not isinstance(runtime_hash, str) or runtime_hash.split()[:1] != [binary_hash]:
        errors.append('runtime binary hash differs from campaign plan')
    if environment.get('arguments') != job['arguments']:
        errors.append('runtime arguments differ from the predeclared arm')
    camera = report.get('camera')
    if not isinstance(camera, dict) or camera.get('active') is not True or camera.get('target_size') != [1280, 720]:
        errors.append('active camera or target geometry is inconsistent')
    presentation = report.get('presentation')
    if not isinstance(presentation, dict) or presentation.get('available') is not False:
        errors.append('offscreen presentation must remain unavailable')
    captures = manifest.get('captures', {})
    if not isinstance(captures, dict):
        captures = {}
    for name, suffix in [('screenshot', '.png'), ('warmup_screenshot', '.png.warmup.png')]:
        path = Path(str(output) + suffix)
        capture = captures.get(name, {})
        proof = report.get(name, {})
        actual_hash = None if path.is_symlink() else digest(path)
        if (not isinstance(capture, dict) or not isinstance(proof, dict)
                or capture.get('path') != str(path) or proof.get('path') != str(path)
                or proof.get('nonuniform') is not True or not actual_hash
                or capture.get('sha256') != actual_hash):
            errors.append(f'missing, changed, or mismatched {name}')
    loop = report.get('frame_loop', {})
    loop = loop if isinstance(loop, dict) else {}
    mean, count = loop.get('mean_ms'), loop.get('count')
    if (type(mean) not in (int, float) or not math.isfinite(mean) or mean <= 0
            or type(count) is not int or count < 20):
        errors.append('missing or invalid measured CPU-loop summary')
    else:
        row.update({'mean_loop_ms': mean, 'loop_samples': count})
    row['experimental_markers'] = marker_summary(report, plan['experimental_timing'])
    if not plan['experimental_timing'] and report.get('experimental_timing') is not None:
        errors.append('unexpected timestamp instrumentation in the throughput campaign')
    fingerprint = {key: report.get(key) for key in ('source_revision', 'source_dirty_at_build',
                   'subject', 'scene_version', 'adapter')}
    fingerprint['runtime'] = {key: environment.get(key) for key in ('rustc', 'features', 'os', 'metal_debug_layer')}
    fingerprint['environment'] = manifest.get('environment')
    # A separately frozen binary can be invoked from a checkout where tools keep changing.
    # These snapshots describe the runner checkout, not the binary's compiled source.
    row['runner_checkout'] = {key: manifest.get(key) for key in ('source_head', 'source_status',
                            'lock_sha256', 'toolchain_sha256')}
    if any(not fingerprint[key] for key in ('source_revision', 'scene_version', 'adapter')):
        errors.append('source, scene, or adapter identity is missing')
    for key in ('source_head', 'source_status'):
        value = manifest.get(key)
        if not isinstance(value, dict) or value.get('exit_code') != 0 or not isinstance(value.get('stdout'), str):
            errors.append(f'failed provenance command: {key}')
    if any(not manifest.get(key) for key in ('lock_sha256', 'toolchain_sha256')):
        errors.append('build-input hashes are missing')
    return row, fingerprint


def analyze(state):
    plan, results = state['plan'], state.get('results', [])
    jobs = plan['jobs']
    global_errors = []
    expected_keys = {(load, repetition, arm[0]) for load in plan['loads']
                     for repetition in range(REPETITIONS) for arm in ARMS}
    actual_keys = [(j['load'], j['repetition'], j['arm']) for j in jobs]
    if set(actual_keys) != expected_keys or len(actual_keys) != len(expected_keys):
        raise ValueError('campaign plan must contain every arm/load/repetition exactly once')
    ids = [job['id'] for job in jobs]
    if len(set(ids)) != len(ids):
        raise ValueError('duplicate campaign job identities')
    result_ids = [result['id'] for result in results]
    if len(set(result_ids)) != len(result_ids) or not set(result_ids).issubset(ids):
        global_errors.append('duplicate or unplanned execution results')
    indexed = {result['id']: result for result in results}
    inspected = [inspect_run(plan, job, indexed.get(job['id'])) for job in jobs]
    rows = [row for row, _ in inspected]
    identities = {json.dumps(fingerprint, sort_keys=True) for _, fingerprint in inspected if fingerprint is not None}
    if len(identities) > 1:
        global_errors.append('compiled source, scene, adapter, toolchain, or environment changed across runs')
    if global_errors:
        for row in rows:
            row['errors'].extend(global_errors)
    by_pair = {(row['load'], row['repetition'], row['arm']): row for row in rows}
    comparisons = []
    for load in plan['loads']:
        for arm, *_ in ARMS[1:]:
            pairs = [(by_pair[(load, repetition, ARMS[0][0])], by_pair[(load, repetition, arm)])
                     for repetition in range(REPETITIONS)]
            failures = [row['id'] for pair in pairs for row in pair if row['errors']]
            comparison = {'load': load, 'baseline': ARMS[0][0], 'candidate': arm,
                          'failed_or_missing_runs': failures}
            if failures:
                comparison.update({'paired_runs': sum(not a['errors'] and not b['errors'] for a, b in pairs),
                                   'mean_ratio': None, 'ratio_ci95': None, 'decision': 'incomplete_or_invalid'})
            else:
                comparison.update(paired_summary([b['mean_loop_ms'] / a['mean_loop_ms'] for a, b in pairs]))
            comparisons.append(comparison)
    return {'schema': 1, 'valid': all(not row['errors'] for row in rows),
            'scope': 'Paired run-level mean CPU-loop time ratios, candidate/native-MSAA4; lower is faster. '
                     'These are not GPU cost, GPU completion rate, or panel delivery measurements.',
            'limitations': ['Four repetitions provide limited uncertainty resolution.',
                            'Each run is one observation; frame samples are not independent repetitions.',
                            'Pipeline depth, synchronization, and readback can affect CPU-loop throughput.',
                            'The unpaced image queue can retain GPU work beyond CPU observations; CPU cadence is not frame latency or GPU completion.',
                            'Intervals are pointwise bootstrap estimates, not family-wise guarantees across arms and loads.'],
            'bootstrap_seed': BOOTSTRAP_SEED, 'bootstrap_draws': BOOTSTRAP_DRAWS,
            'practical_threshold': PRACTICAL_THRESHOLD,
            'provenance': json.loads(next(iter(identities))) if len(identities) == 1 else None,
            'runner_checkout_varied': len({json.dumps(row.get('runner_checkout'), sort_keys=True)
                                          for row in rows if 'runner_checkout' in row}) > 1,
            'errors': global_errors, 'planned_runs': len(jobs), 'execution_records': len(results),
            'runs': rows, 'comparisons': comparisons}


def run_job(job, deadline):
    process = None
    result = {'wrapper_exit_code': 1, 'runner_log': job['output'] + '.runner.log'}

    def stop():
        if process is None or process.poll() is not None:
            return
        try:
            process.terminate()  # run.py handles SIGTERM and terminates its child group.
        except ProcessLookupError:
            pass
        try:
            process.wait(timeout=15)
        except subprocess.TimeoutExpired:
            process.kill()
            process.wait()
            result['forced_wrapper_termination'] = True

    try:
        with Path(result['runner_log']).open('x') as log:
            process = subprocess.Popen(job['argv'], cwd=ROOT, stdout=log,
                                       stderr=subprocess.STDOUT, start_new_session=True)
            try:
                result['wrapper_exit_code'] = process.wait(timeout=deadline)
            except subprocess.TimeoutExpired:
                result.update({'wrapper_exit_code': 124, 'campaign_timeout': True})
                stop()
            except (KeyboardInterrupt, InterruptedError):
                stop()
                raise
    except (KeyboardInterrupt, InterruptedError):
        raise
    except OSError as error:
        result['launch_error'] = str(error)
    finally:
        stop()
    return result


def save_state(path, value):
    temporary = Path(str(path) + '.tmp')
    temporary.write_text(json.dumps(value, indent=2) + '\n')
    temporary.replace(path)


def run_campaign(plan, executor=run_job):
    if not plan['binary_sha256'] or not os.access(plan['binary'], os.X_OK):
        raise ValueError('campaign binary must exist and be executable')
    directory = Path(plan['run_dir'])
    directory.mkdir(parents=True, exist_ok=False)
    state = {'plan': plan, 'started_utc': datetime.now(timezone.utc).isoformat(), 'results': []}
    journal = directory / 'campaign.json'
    save_state(journal, state)
    abort_reason = None
    for index, job in enumerate(plan['jobs']):
        if digest(plan['binary']) != plan['binary_sha256'] or digest(RUNNER) != plan['runner_sha256']:
            abort_reason = 'binary or runner identity changed; campaign remains invalid'
        record = {'id': job['id']}
        state['results'].append(record)
        if abort_reason:
            record.update({'wrapper_exit_code': None, 'not_attempted': abort_reason})
        else:
            record['started_utc'] = datetime.now(timezone.utc).isoformat()
            save_state(journal, state)
            print(f'{index + 1}/{len(plan["jobs"])} {job["id"]}', flush=True)
            try:
                # run.py separately bounds child time; allow its bounded metadata and cleanup overhead.
                record.update(executor(job, plan['timeout_seconds'] + 80))
                if record.get('campaign_timeout'):
                    abort_reason = 'outer campaign watchdog expired; no further workloads launched'
            except (KeyboardInterrupt, InterruptedError):
                record.update({'wrapper_exit_code': 130, 'interrupted': True})
                abort_reason = 'campaign interrupted; no further workloads launched'
            record['finished_utc'] = datetime.now(timezone.utc).isoformat()
        save_state(journal, state)
    state['finished_utc'] = datetime.now(timezone.utc).isoformat()
    save_state(journal, state)
    result = analyze(state)
    save_state(directory / 'analysis.json', result)
    return result


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__)
    action = parser.add_mutually_exclusive_group()
    action.add_argument('--dry-run', action='store_true')
    action.add_argument('--analyze-existing', type=Path)
    parser.add_argument('--run-dir', type=Path)
    parser.add_argument('--binary', type=Path, default=RUNNER.parent / 'target/release/ushas-smoke')
    parser.add_argument('--loads', default='0,8000,20000')
    parser.add_argument('--warmup', type=float, default=4)
    parser.add_argument('--seconds', type=float, default=6)
    parser.add_argument('--timeout', type=float, default=90)
    parser.add_argument('--experimental-timing', action='store_true',
                        help='separate instrumented control; markers remain unvalidated')
    args = parser.parse_args(argv)
    previous_sigterm = signal.getsignal(signal.SIGTERM)

    def interrupted(_signum, _frame):
        raise InterruptedError('campaign interrupted')

    signal.signal(signal.SIGTERM, interrupted)
    try:
        if args.analyze_existing:
            result = analyze(load_object(args.analyze_existing / 'campaign.json'))
            print(json.dumps(result, indent=2))
            return 0 if result['valid'] else 1
        directory = args.run_dir or Path('/tmp') / (
            'ushas-campaign-' + datetime.now(timezone.utc).strftime('%Y%m%dT%H%M%SZ-') + uuid.uuid4().hex[:8])
        loads = tuple(int(value) for value in args.loads.split(','))
        plan = make_plan(args.binary, directory, loads, args.warmup, args.seconds,
                         args.timeout, args.experimental_timing)
        if args.dry_run:
            print(json.dumps(plan, indent=2))
            return 0
        result = run_campaign(plan)
        print(f'valid={result["valid"]} analysis={Path(plan["run_dir"]) / "analysis.json"}')
        return 0 if result['valid'] else 1
    except (OSError, ValueError, KeyError, TypeError, AttributeError) as error:
        print(json.dumps({'valid': False, 'error': str(error)}), file=sys.stderr)
        return 1
    finally:
        signal.signal(signal.SIGTERM, previous_sigterm)


class CampaignTests(unittest.TestCase):
    def fixture(self):
        import tempfile
        directory = tempfile.TemporaryDirectory(prefix='ushas campaign CPU ')
        self.addCleanup(directory.cleanup)
        root = Path(directory.name)
        binary = root / 'never-executed-binary'
        binary.write_bytes(b'CPU analysis fixture; not executable')
        plan = make_plan(binary, root / 'runs', loads=(0,))
        Path(plan['run_dir']).mkdir()
        results = []
        for job in plan['jobs']:
            output = Path(job['output'])
            report = {**job['expected'], 'valid': True, 'timed_out': False,
                      'source_revision': 'compiled-source', 'source_dirty_at_build': 'false',
                      'scene_version': 'claude-toy-v1', 'adapter': {'name': 'CPU fixture'},
                      'camera': {'active': True, 'target_size': [1280, 720]},
                      'presentation': {'available': False},
                      'environment': {'binary_sha256': plan['binary_sha256'] + '  ' + str(binary),
                                      'arguments': job['arguments'], 'rustc': 'fixture',
                                      'features': 'fixture', 'os': 'fixture'}}
            ratio = [.7, .8, .9, 1.0][job['repetition']]
            report['frame_loop'] = {'mean_ms': 10 if job['arm'] == ARMS[0][0] else 10 * ratio,
                                    'count': 100 * (10 ** job['repetition'])}
            captures = {}
            for name, suffix in [('screenshot', '.png'), ('warmup_screenshot', '.png.warmup.png')]:
                path = Path(str(output) + suffix)
                path.write_bytes((name + job['id']).encode())
                report[name] = {'path': str(path), 'nonuniform': True}
                captures[name] = {'path': str(path), 'sha256': digest(path)}
            output.write_text(json.dumps(report))
            manifest = {'binary': str(binary.resolve()), 'binary_sha256': plan['binary_sha256'],
                        'binary_sha256_after': plan['binary_sha256'], 'argv': [str(binary.resolve()), *job['arguments']],
                        'smoke_valid': True, 'exit_code': 0, 'child_exit_code': 0,
                        'captures': captures, 'report_sha256': digest(output), 'evidence_errors': [],
                        'source_head': {'exit_code': 0, 'stdout': 'checkout-source'},
                        'source_status': {'exit_code': 0, 'stdout': ''}, 'lock_sha256': 'lock',
                        'toolchain_sha256': 'toolchain', 'os': {'fixture': True},
                        'machine': {'fixture': True}, 'environment': {}}
            Path(str(output) + '.manifest.json').write_text(json.dumps(manifest))
            results.append({'id': job['id'], 'wrapper_exit_code': 0})
        return {'plan': plan, 'results': results}

    @staticmethod
    def rewrite_report(job, change):
        output = Path(job['output'])
        report = json.loads(output.read_text())
        change(report)
        output.write_text(json.dumps(report))
        path = Path(str(output) + '.manifest.json')
        manifest = json.loads(path.read_text())
        manifest['report_sha256'] = digest(output)
        path.write_text(json.dumps(manifest))

    def test_analysis_uses_run_pairs_not_the_unequal_frame_counts(self):
        result = analyze(self.fixture())
        self.assertTrue(result['valid'])
        self.assertEqual(len(result['runs']), 20)
        self.assertEqual(len(result['comparisons']), 4)
        for comparison in result['comparisons']:
            self.assertEqual(comparison['paired_runs'], 4)
            self.assertAlmostEqual(comparison['mean_ratio'], .85)

    def test_failure_is_retained_and_withholds_incomplete_comparisons(self):
        state = self.fixture()
        state['results'][0]['wrapper_exit_code'] = 124
        result = analyze(state)
        self.assertFalse(result['valid'])
        self.assertEqual(len(result['runs']), 20)
        self.assertTrue(result['runs'][0]['errors'])
        self.assertTrue(all(c['ratio_ci95'] is None for c in result['comparisons']))

    def test_different_scene_or_configuration_cannot_be_pooled(self):
        for field, value in [('scene_version', 'other-scene'), ('width', 1920), ('source_revision', 'other-source')]:
            state = self.fixture()
            self.rewrite_report(state['plan']['jobs'][1], lambda r: r.update({field: value}))
            with self.subTest(field=field):
                result = analyze(state)
                self.assertFalse(result['valid'])
                self.assertTrue(any(run['errors'] for run in result['runs']))

    def test_missing_measured_marker_observations_are_not_zero_gpu_cost(self):
        state = self.fixture()
        job = state['plan']['jobs'][1]
        self.rewrite_report(job, lambda r: r.update({'experimental_timing': {
            'status': 'Pending', 'observations': [], 'validated_for_governor': False}}))
        result = analyze(state)
        marker = result['runs'][1]['experimental_markers']
        self.assertIsNone(marker['mean_elapsed_ms'])
        self.assertEqual(marker['retained_measured_observations'], 0)
        self.assertFalse(marker['validated_for_governor'])

    def test_dry_run_does_not_create_a_directory_or_launch_a_binary(self):
        import contextlib
        import io
        state = self.fixture()
        directory = Path(state['plan']['run_dir']).parent / 'dry-only'
        with contextlib.redirect_stdout(io.StringIO()) as output:
            self.assertEqual(main(['--dry-run', '--run-dir', str(directory),
                                   '--binary', '/does/not/exist']), 0)
        self.assertEqual(len(json.loads(output.getvalue())['jobs']), 60)
        self.assertFalse(directory.exists())

    def test_analyze_existing_is_read_only(self):
        import contextlib
        import io
        state = self.fixture()
        directory = Path(state['plan']['run_dir'])
        path = directory / 'campaign.json'
        path.write_text(json.dumps(state))
        before = {p.name: (p.stat().st_mtime_ns, digest(p)) for p in directory.iterdir()}
        with contextlib.redirect_stdout(io.StringIO()) as output:
            self.assertEqual(main(['--analyze-existing', str(directory)]), 0)
        self.assertTrue(json.loads(output.getvalue())['valid'])
        self.assertEqual(before, {p.name: (p.stat().st_mtime_ns, digest(p)) for p in directory.iterdir()})

    def test_serial_execution_journals_every_failure_without_reusing_directories(self):
        import contextlib
        import io
        state = self.fixture()
        binary = Path(state['plan']['binary'])
        binary.chmod(0o755)
        directory = Path(state['plan']['run_dir']).parent / 'execute-cpu-stub'
        plan = make_plan(binary, directory, loads=(0,))
        called = []

        def execute(job, _deadline):
            called.append(job['id'])
            return {'wrapper_exit_code': 7 if len(called) == 2 else 0}

        with contextlib.redirect_stdout(io.StringIO()):
            result = run_campaign(plan, execute)
        self.assertFalse(result['valid'])  # Stub deliberately writes no smoke evidence.
        self.assertEqual(called, [job['id'] for job in plan['jobs']])
        journal = load_object(directory / 'campaign.json')
        self.assertEqual(len(journal['results']), 20)
        self.assertEqual(journal['results'][1]['wrapper_exit_code'], 7)
        with self.assertRaises(FileExistsError):
            run_campaign(plan, execute)

    def test_cpu_wrapper_timeout_is_bounded(self):
        state = self.fixture()
        output = str(Path(state['plan']['run_dir']) / 'cpu-timeout.json')
        job = {'output': output, 'argv': [sys.executable, '-c', 'import time; time.sleep(10)']}
        started = time.monotonic()
        result = run_job(job, .1)
        self.assertEqual(result['wrapper_exit_code'], 124)
        self.assertTrue(result['campaign_timeout'])
        self.assertLess(time.monotonic() - started, 3)

    def test_binary_change_stops_further_launches_but_retains_the_plan(self):
        import contextlib
        import io
        state = self.fixture()
        binary = Path(state['plan']['binary'])
        binary.chmod(0o755)
        directory = Path(state['plan']['run_dir']).parent / 'changed-binary'
        plan = make_plan(binary, directory, loads=(0,))
        called = []

        def execute(job, _deadline):
            called.append(job['id'])
            binary.write_bytes(b'changed executable')
            return {'wrapper_exit_code': 0}

        with contextlib.redirect_stdout(io.StringIO()):
            run_campaign(plan, execute)
        self.assertEqual(len(called), 1)
        journal = load_object(directory / 'campaign.json')
        self.assertEqual(len(journal['results']), 20)
        self.assertIn('identity changed', journal['results'][1]['not_attempted'])

    def test_interruption_is_not_swallowed_as_a_launch_error(self):
        from unittest import mock
        state = self.fixture()
        job = {'output': str(Path(state['plan']['run_dir']) / 'interrupted.json'), 'argv': ['unused']}
        with mock.patch.object(subprocess, 'Popen', side_effect=InterruptedError('cancelled')):
            with self.assertRaises(InterruptedError):
                run_job(job, 1)

    def test_malformed_nested_report_remains_a_failed_run_in_the_analysis(self):
        state = self.fixture()
        self.rewrite_report(state['plan']['jobs'][1], lambda r: r.update({'presentation': None}))
        result = analyze(state)
        self.assertFalse(result['valid'])
        self.assertEqual(len(result['runs']), 20)
        self.assertTrue(result['runs'][1]['errors'])

    def test_changed_capture_and_binary_manifest_cannot_validate_a_campaign(self):
        for kind in ('capture', 'binary'):
            state = self.fixture()
            output = Path(state['plan']['jobs'][1]['output'])
            if kind == 'capture':
                Path(str(output) + '.png').write_bytes(b'changed after wrapper validation')
            else:
                path = Path(str(output) + '.manifest.json')
                manifest = load_object(path)
                manifest['binary_sha256_after'] = 'changed'
                path.write_text(json.dumps(manifest))
            with self.subTest(kind=kind):
                result = analyze(state)
                self.assertFalse(result['valid'])
                self.assertTrue(result['runs'][1]['errors'])

    def test_invoker_checkout_changes_are_context_for_an_identical_frozen_binary(self):
        state = self.fixture()
        output = Path(state['plan']['jobs'][1]['output'])
        path = Path(str(output) + '.manifest.json')
        manifest = load_object(path)
        manifest['source_head']['stdout'] = 'new-tools-checkout-head'
        path.write_text(json.dumps(manifest))
        result = analyze(state)
        self.assertTrue(result['valid'])
        self.assertTrue(result['runner_checkout_varied'])

    def test_default_plan_is_bounded_and_balances_forward_reverse_pairs(self):
        plan = make_plan(Path('/missing/binary'), Path('/tmp/unused-campaign'))
        self.assertEqual(len(plan['jobs']), 60)
        self.assertEqual(len({job['output'] for job in plan['jobs']}), 60)
        for load in (0, 8000, 20000):
            for repetition in range(4):
                block = [j for j in plan['jobs'] if j['load'] == load and j['repetition'] == repetition]
                self.assertEqual({j['arm'] for j in block}, {a[0] for a in ARMS})
            for arm, *_ in ARMS:
                positions = [j['position'] for j in plan['jobs'] if j['load'] == load and j['arm'] == arm]
                self.assertEqual(sum(positions), 8)
            orders = [[j['arm'] for j in plan['jobs'] if j['load'] == load and j['repetition'] == repetition]
                      for repetition in range(4)]
            self.assertEqual(orders[1], list(reversed(orders[0])))
            self.assertEqual(orders[3], list(reversed(orders[2])))
        load_orders = [[j['load'] for j in plan['jobs'] if j['position'] == 0 and j['repetition'] == repetition]
                       for repetition in range(4)]
        self.assertEqual(load_orders[1], list(reversed(load_orders[0])))
        self.assertEqual(load_orders[3], list(reversed(load_orders[2])))
        for load in plan['loads']:
            for arm, *_ in ARMS:
                self.assertEqual(sum(i for i, job in enumerate(plan['jobs'])
                                     if job['load'] == load and job['arm'] == arm), 118)
        for job in plan['jobs']:
            self.assertIn('--offscreen', job['argv'])
            self.assertNotIn('--experimental-timing', job['argv'])
        self.assertEqual(plan['warmup_s'], 4)
        self.assertEqual(plan['measurement_s'], 6)

    def test_bootstrap_uses_four_paired_runs_and_a_practical_margin(self):
        summary = paired_summary([.8] * 4)
        self.assertEqual(summary['paired_runs'], 4)
        self.assertEqual(summary['ratio_ci95'], [.8, .8])
        self.assertEqual(summary['decision'], 'clear_practical_improvement')
        varied = paired_summary([.7, .8, .9, 1.0])
        self.assertAlmostEqual(varied['mean_ratio'], .85)
        self.assertEqual(varied, paired_summary([.7, .8, .9, 1.0]))
        self.assertEqual(varied['decision'], 'uncertain_or_below_practical_threshold')
        self.assertEqual(paired_summary([1.2] * 4)['decision'], 'clear_practical_regression')

    def test_missing_or_invalid_repetitions_cannot_produce_an_interval(self):
        for ratios in ([.8] * 3, [.8, .8, 0, .8], [.8, .8, float('nan'), .8]):
            with self.subTest(ratios=ratios), self.assertRaises(ValueError):
                paired_summary(ratios)


if __name__ == '__main__':
    if '--self-test' in sys.argv:
        unittest.main(argv=[sys.argv[0]])
    else:
        sys.exit(main())
