#!/usr/bin/env python3
"""One fresh-process validation-layer diagnostic arm, never a governor source."""
import argparse
from datetime import datetime, timezone
import hashlib
import json
import os
from pathlib import Path
import re
import signal
import subprocess
import time


def digest(path):
    return hashlib.sha256(path.read_bytes()).hexdigest()


def controlled_environment(inherited, validation, stacks):
    if inherited.get('DYLD_INSERT_LIBRARIES'):
        raise ValueError('inherited library injection prevents a controlled arm')
    removed = sorted(key for key in inherited if key.startswith('MTL_'))
    env = {key: value for key, value in inherited.items() if key not in removed}
    controls = {'MTL_DEBUG_LAYER': validation, 'MTL_SHADER_VALIDATION': '0',
                'USHAS_OBSERVATION_CAPTURE_UNKNOWN_STACK': stacks}
    env.update(controls)
    return env, controls, removed


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--binary', type=Path, required=True)
    parser.add_argument('--build-receipt', type=Path, required=True)
    parser.add_argument('--out', type=Path, required=True)
    parser.add_argument('--validation', choices=['0', '1'], required=True)
    parser.add_argument('--stacks', choices=['0', '1'], required=True)
    parser.add_argument('--mode', choices=['spatial', 'temporal'], required=True)
    parser.add_argument('--observe', choices=['off', 'calls', 'counters'], required=True)
    args = parser.parse_args()
    binary, out, build = args.binary.resolve(), args.out.resolve(), args.build_receipt.resolve()
    log, receipt = Path(str(out) + '.log'), Path(str(out) + '.run.json')
    if any(p.exists() for p in (out, log, receipt)):
        parser.error('output directory, log and receipt must all be new')
    before = digest(binary)
    recorded = json.loads(build.read_text())
    if (recorded.get('binary_sha256') != before
            or type(recorded.get('build_exit_code')) is not int or recorded['build_exit_code'] != 0
            or recorded.get('source_status') != ''
            or not isinstance(recorded.get('source_revision'), str)
            or not re.fullmatch(r'[0-9a-f]{40}', recorded['source_revision'])):
        parser.error('binary must match a successful clean-source build receipt')
    try:
        env, controls, removed = controlled_environment(os.environ, args.validation, args.stacks)
    except ValueError as error:
        parser.error(str(error))
    argv = [str(binary), '--mode', args.mode, '--observe', args.observe, '--out', str(out)]
    def interrupted(signum, _frame):
        raise InterruptedError(f'signal {signum}')
    signal.signal(signal.SIGTERM, interrupted)
    start, start_utc = time.monotonic(), datetime.now(timezone.utc).isoformat()
    timed_out, interruption, code = False, None, None
    with log.open('xb') as stream:
        child = subprocess.Popen(argv, stdout=stream, stderr=subprocess.STDOUT, env=env, start_new_session=True)
        try:
            code = child.wait(timeout=65)
        except (subprocess.TimeoutExpired, KeyboardInterrupt, InterruptedError) as error:
            timed_out = isinstance(error, subprocess.TimeoutExpired)
            interruption = None if timed_out else str(error) or 'keyboard interruption'
            signal.signal(signal.SIGINT, signal.SIG_IGN)
            signal.signal(signal.SIGTERM, signal.SIG_IGN)
            try:
                try:
                    os.killpg(child.pid, signal.SIGTERM)
                except ProcessLookupError:
                    pass
                try:
                    child.wait(timeout=2)
                except subprocess.TimeoutExpired:
                    pass
            finally:
                # Sweep helpers even if the group leader has already exited.
                try:
                    os.killpg(child.pid, signal.SIGKILL)
                except ProcessLookupError:
                    pass
            code = child.wait(timeout=2)
    result = dict(argv=argv, child_exit_code=code, timed_out=timed_out, interruption=interruption,
                  elapsed_seconds=time.monotonic() - start, started_utc=start_utc,
                  finished_utc=datetime.now(timezone.utc).isoformat(), binary_sha256=before,
                  binary_sha256_after=digest(binary), build_receipt=str(build), build_receipt_sha256=digest(build),
                  runner_sha256=digest(Path(__file__)), log_sha256=digest(log), environment_controls=controls,
                  removed_inherited_mtl_keys=removed, root_gpu_runs_serialized=True,
                  other_application_gpu_work='not controlled; trace inventory required',
                  structurally_valid=None, validated_for_governor=False)
    with receipt.open('x') as stream:
        json.dump(result, stream, indent=2)
        stream.write('\n')
    print(json.dumps(result))
    return 0 if code == 0 and not timed_out and not interruption and digest(binary) == before else 1


if __name__ == '__main__':
    raise SystemExit(main())
