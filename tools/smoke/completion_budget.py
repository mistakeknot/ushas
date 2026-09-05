#!/usr/bin/env python3
"""Read-only 60 FPS budget report for a finished serial-completion campaign.

Print JSON by default; --out creates a fresh file and never replaces evidence.
Run --self-test for the focused CPU-only tests. No renderer is launched.
"""
import argparse
import contextlib
import hashlib
import io
import json
import math
from pathlib import Path
import statistics
import sys
import unittest
from unittest import mock

# Even imports should not create cache files beside the recorded evidence/tools.
sys.dont_write_bytecode = True
import campaign

TARGET_FPS = 60.0
BUDGET_MS = 1000.0 / TARGET_FPS


def interval_stats(frames):
    """Derive statistics only after campaign.completion_metric accepted the ledger."""
    if not frames:
        raise ValueError('no validated completed-frame intervals')
    previous = frames[0]['admitted_ms']
    intervals = []
    for frame in frames:
        callback = frame['callback_observed_ms']
        intervals.append(callback - previous)
        previous = callback
    elapsed_ms = math.fsum(intervals)
    if not math.isfinite(elapsed_ms) or elapsed_ms <= 0:
        raise ValueError('completed-frame interval duration must be positive and finite')
    ordered = sorted(intervals)
    return {'frame_intervals': len(intervals), 'elapsed_seconds': elapsed_ms / 1000,
            'mean_interval_ms': elapsed_ms / len(intervals),
            'p95_interval_ms': ordered[math.ceil(.95 * len(ordered)) - 1],
            'p99_interval_ms': ordered[math.ceil(.99 * len(ordered)) - 1],
            'budget_miss_fraction': sum(interval > BUDGET_MS for interval in intervals) / len(intervals)}


def summarize_runs(runs):
    valid = (len(runs) == campaign.REPETITIONS
             and {run['repetition'] for run in runs} == set(range(campaign.REPETITIONS))
             and all(not run['errors'] and run['intervals'] is not None for run in runs))
    summary = None
    if valid:
        fields = {'mean_interval_ms': 'mean_interval_ms',
                  'mean_of_run_p95_interval_ms': 'p95_interval_ms',
                  'mean_of_run_p99_interval_ms': 'p99_interval_ms',
                  'mean_of_run_budget_miss_fractions': 'budget_miss_fraction'}
        summary = {name: statistics.mean(run['intervals'][field] for run in runs)
                   for name, field in fields.items()}
    return {'valid': valid, 'required_runs': campaign.REPETITIONS,
            'valid_runs': sum(not run['errors'] and run['intervals'] is not None for run in runs),
            'run_ids': [run['id'] for run in runs], 'summary': summary}


def build_report(directory):
    directory = Path(directory).resolve()
    journal = directory / 'campaign.json'
    journal_bytes = journal.read_bytes()
    state = json.loads(journal_bytes)
    if not isinstance(state, dict) or not isinstance(state.get('finished_utc'), str) or not state['finished_utc']:
        raise ValueError('campaign is unfinished; wait for its final journal')
    plan = state['plan']
    if plan.get('completion') is not True:
        raise ValueError('budget report requires a serial-completion campaign')
    if any(type(job['expected'].get('target_fps')) not in (int, float)
           or job['expected']['target_fps'] != TARGET_FPS for job in plan['jobs']):
        raise ValueError('every planned run must declare the fixed 60 FPS analysis budget')

    # The existing validator owns ledger, manifest, camera, image, configuration,
    # binary, and cross-run identity checks. These hashes only reject evidence
    # changing while that validator and this reader consume it.
    watched = {journal: hashlib.sha256(journal_bytes).hexdigest()}
    for job in plan['jobs']:
        for suffix in ('', '.manifest.json', '.png', '.png.warmup.png'):
            path = Path(job['output'] + suffix)
            watched[path] = campaign.digest(path)
    validation = campaign.analyze(state)
    runs = []
    for validated in validation['runs']:
        row = {key: validated[key] for key in ('id', 'arm', 'load', 'repetition', 'output')}
        row.update(errors=list(validated['errors']), intervals=None)
        if not row['errors']:
            path = Path(row['output'])
            payload = path.read_bytes()
            if hashlib.sha256(payload).hexdigest() != watched[path]:
                row['errors'].append('report changed after campaign validation')
            else:
                report = json.loads(payload)
                measured = [frame for frame in report['serial_completion']['frames'] if frame['epoch'] == 2]
                stats = interval_stats(measured)
                accepted = validated['serial_completion']
                if (stats['frame_intervals'] != accepted['qualified_render_frames']
                        or not math.isclose(stats['elapsed_seconds'], accepted['elapsed_seconds'], rel_tol=1e-8)
                        or not math.isclose(stats['mean_interval_ms'], accepted['mean_completed_render_ms'], rel_tol=1e-8)):
                    row['errors'].append('derived intervals disagree with accepted completion metric')
                else:
                    row['intervals'] = stats
        runs.append(row)
    changed = [str(path) for path, digest in watched.items() if campaign.digest(path) != digest]
    if changed:
        for row in runs:
            row['errors'].append('campaign evidence changed during validation')
            row['intervals'] = None
    cells = []
    declared_arms = list(dict.fromkeys(run['arm'] for run in validation['runs']))
    for load in plan['loads']:
        for arm in declared_arms:
            cell = summarize_runs([run for run in runs if run['load'] == load and run['arm'] == arm])
            cells.append({'load': load, 'arm': arm, **cell})
    return {'schema': 1, 'valid': validation['valid'] and all(cell['valid'] for cell in cells),
            'scope': 'Serial callback-observed completed-render intervals including CPU preparation, '
                     'scheduling, instrumentation and polling; not normal pipelined FPS, GPU busy time, '
                     'hardware latency, or presentation. No adaptive-governor input.',
            'target_fps': TARGET_FPS, 'budget_ms': BUDGET_MS,
            'interval_definition': 'First callback minus first admission; then callback minus previous callback. '
                                   'Intervals partition the measured epoch duration, including CPU gaps.',
            'quantile_method': 'Nearest rank within each run: sorted[ceil(q*N)-1].',
            'aggregation': 'Each of four independent process runs has equal weight per arm/load. '
                           'Cell p95/p99 fields average per-run quantiles; they are not pooled quantiles. '
                           'No frame-level confidence interval or independence assumption is used.',
            'campaign': str(journal), 'campaign_sha256': watched[journal],
            'helper_sha256': campaign.digest(Path(__file__)),
            'validator_sha256': campaign.digest(Path(campaign.__file__)),
            'binary_sha256': plan['binary_sha256'], 'provenance': validation['provenance'],
            'validation_errors': validation['errors'], 'changed_evidence': changed,
            'runs': runs, 'cells': cells}


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('campaign', type=Path, help='directory containing the finished campaign.json')
    parser.add_argument('--out', type=Path, help='create this fresh JSON file; default prints to stdout')
    args = parser.parse_args(argv)
    try:
        report = build_report(args.campaign)
        encoded = json.dumps(report, indent=2, allow_nan=False) + '\n'
        if args.out is None:
            print(encoded, end='')
        else:
            with args.out.open('x') as output:
                output.write(encoded)
        return 0 if report['valid'] else 1
    except (OSError, ValueError, KeyError, TypeError, OverflowError) as error:
        print(f'completion budget: {error}', file=sys.stderr)
        return 1


class BudgetTests(unittest.TestCase):
    def fixture(self):
        source = campaign.CampaignTests()
        self.addCleanup(source.doCleanups)
        state = source.completion_fixture()
        state['finished_utc'] = '2026-09-04T00:00:00+00:00'
        directory = Path(state['plan']['run_dir'])
        (directory / 'campaign.json').write_text(json.dumps(state))
        return source, state, directory

    @staticmethod
    def frames(intervals):
        previous = 100.0
        result = []
        for interval in intervals:
            result.append({'admitted_ms': previous + (0 if not result else interval / 2),
                           'callback_observed_ms': previous + interval})
            previous += interval
        return result

    def test_intervals_include_cpu_gaps_and_quantiles_use_nearest_rank(self):
        result = interval_stats(self.frames([10, 20, 40]))
        self.assertEqual(result['frame_intervals'], 3)
        self.assertAlmostEqual(result['elapsed_seconds'], .07)
        self.assertAlmostEqual(result['mean_interval_ms'], 70 / 3)
        self.assertEqual(result['p95_interval_ms'], 40)
        self.assertEqual(result['p99_interval_ms'], 40)
        self.assertAlmostEqual(result['budget_miss_fraction'], 2 / 3)
        self.assertEqual(interval_stats([{'admitted_ms': 0, 'callback_observed_ms': BUDGET_MS}])['budget_miss_fraction'], 0)
        hundred = interval_stats(self.frames([1] * 94 + [20] * 5 + [100]))
        self.assertEqual(hundred['p95_interval_ms'], 20)
        self.assertEqual(hundred['p99_interval_ms'], 20)

    def test_cells_weight_each_of_four_runs_equally_not_each_frame(self):
        runs = []
        for repetition, (count, duration) in enumerate([(20, 10), (200, 20), (40, 30), (400, 40)]):
            runs.append({'id': str(repetition), 'repetition': repetition, 'errors': [],
                         'intervals': interval_stats(self.frames([duration] * count))})
        summary = summarize_runs(runs)
        self.assertTrue(summary['valid'])
        self.assertEqual(summary['summary']['mean_interval_ms'], 25)
        self.assertEqual(summary['summary']['mean_of_run_p95_interval_ms'], 25)
        self.assertEqual(summary['summary']['mean_of_run_p99_interval_ms'], 25)
        self.assertEqual(summary['summary']['mean_of_run_budget_miss_fractions'], .75)
        runs[-1]['errors'].append('failed completion evidence')
        self.assertIsNone(summarize_runs(runs)['summary'])
        self.assertIsNone(summarize_runs(runs[:3])['summary'])
        runs[-1]['errors'].clear()
        runs[-1]['repetition'] = 0
        self.assertIsNone(summarize_runs(runs)['summary'])

    def test_invalid_ledger_is_rejected_by_existing_campaign_validation(self):
        source, state, directory = self.fixture()
        bad = state['plan']['jobs'][0]
        source.rewrite_report(bad, lambda r: r['serial_completion']['epochs'].pop())
        report = build_report(directory)
        self.assertFalse(report['valid'])
        row = next(r for r in report['runs'] if r['id'] == bad['id'])
        self.assertTrue(row['errors'])
        self.assertIsNone(row['intervals'])
        cell = next(c for c in report['cells'] if c['arm'] == bad['arm'])
        self.assertIsNone(cell['summary'])

    def test_unfinished_or_cpu_campaign_cannot_supply_a_completion_budget(self):
        _, state, directory = self.fixture()
        state.pop('finished_utc')
        (directory / 'campaign.json').write_text(json.dumps(state))
        with self.assertRaises(ValueError):
            build_report(directory)
        state['finished_utc'] = 'finished'
        state['plan']['completion'] = False
        (directory / 'campaign.json').write_text(json.dumps(state))
        with self.assertRaises(ValueError):
            build_report(directory)

    def test_default_cli_is_read_only_and_never_starts_a_process(self):
        _, _, directory = self.fixture()
        before = {p.name: (p.stat().st_mtime_ns, campaign.digest(p)) for p in directory.iterdir()}
        output = io.StringIO()
        with mock.patch.object(campaign.subprocess, 'Popen', side_effect=AssertionError('no process')):
            with contextlib.redirect_stdout(output):
                self.assertEqual(main([str(directory)]), 0)
        self.assertTrue(json.loads(output.getvalue())['valid'])
        self.assertEqual(before, {p.name: (p.stat().st_mtime_ns, campaign.digest(p)) for p in directory.iterdir()})

    def test_output_must_be_explicit_and_fresh(self):
        _, _, directory = self.fixture()
        output = directory.parent / 'budget.json'
        with contextlib.redirect_stdout(io.StringIO()):
            self.assertEqual(main([str(directory), '--out', str(output)]), 0)
        original = output.read_bytes()
        with contextlib.redirect_stderr(io.StringIO()):
            self.assertEqual(main([str(directory), '--out', str(output)]), 1)
        self.assertEqual(output.read_bytes(), original)

    def test_evidence_mutation_after_validation_withholds_every_cell(self):
        _, state, directory = self.fixture()
        analyze = campaign.analyze

        def mutate_after_validation(snapshot):
            accepted = analyze(snapshot)
            path = Path(state['plan']['jobs'][0]['output'])
            path.write_bytes(path.read_bytes() + b'\n')
            return accepted

        with mock.patch.object(campaign, 'analyze', side_effect=mutate_after_validation):
            report = build_report(directory)
        self.assertFalse(report['valid'])
        self.assertTrue(report['changed_evidence'])
        self.assertTrue(all(cell['summary'] is None for cell in report['cells']))

    def test_fixed_budget_rejects_a_different_declared_target(self):
        _, state, directory = self.fixture()
        state['plan']['jobs'][0]['expected']['target_fps'] = 30
        (directory / 'campaign.json').write_text(json.dumps(state))
        with self.assertRaises(ValueError):
            build_report(directory)

    def test_cell_names_follow_validated_plan_rows_not_a_global_arm_default(self):
        _, state, directory = self.fixture()
        validated = campaign.analyze(state)
        declared = {job['arm'] for job in state['plan']['jobs']}
        with mock.patch.object(campaign, 'analyze', return_value=validated):
            with mock.patch.object(campaign, 'ARMS', (('future-default', 'disabled', '1', True),)):
                report = build_report(directory)
        self.assertEqual({cell['arm'] for cell in report['cells']}, declared)
        self.assertTrue(report['valid'])


if __name__ == '__main__':
    if '--self-test' in sys.argv:
        unittest.main(argv=[sys.argv[0]])
    else:
        raise SystemExit(main())
