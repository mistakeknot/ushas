#!/usr/bin/env python3
"""Run one deterministic quality arm; never a performance comparison or video gate."""
import argparse
import hashlib
import json
import math
import os
from pathlib import Path
import signal
import subprocess
import struct
import time

CAPTURES = ((31, 'settled'), (63, 'motion32'), (93, 'motion62'), (94, 'motion63'),
            (95, 'motion64'), (127, 'before-cut'), (128, 'cut0'), (129, 'cut1'),
            (130, 'cut2'), (132, 'cut4'), (136, 'cut8'), (144, 'cut16'))
MOVING_CAPTURES = tuple((tick, 'moving-' + name) for tick, name in CAPTURES[:6]) + tuple(
    (128 + index, f'moving-cut{index}') for index in range(17))


def expected_camera_matrix(tick, moving_reset):
    """Independent look-at basis for the moving protocol's world-from-view proof."""
    if tick < 128:
        eye = [.75 * math.sin(max(0, tick - 32) / 60 * .8), 2.4, 8.]
        target = [0., .4, 0.]
    else:
        pan = .75 * math.sin((tick - 128) / 60 * .8) if moving_reset else 0.
        eye = [-1.4 + pan, 2.1, 7.5]
        target = [.2, .5, 0.]
    def normalize(vector):
        length = math.sqrt(sum(v * v for v in vector))
        return [v / length for v in vector]
    def cross(a, b):
        return [a[1]*b[2]-a[2]*b[1], a[2]*b[0]-a[0]*b[2], a[0]*b[1]-a[1]*b[0]]
    back = normalize([a - b for a, b in zip(eye, target)])
    right = normalize(cross([0., 1., 0.], back))
    up = cross(back, right)
    return right + [0.] + up + [0.] + back + [0.] + eye + [1.]


def digest(path):
    if path.is_symlink() or not path.is_file():
        return None
    with path.open('rb') as stream:
        return hashlib.file_digest(stream, 'sha256').hexdigest()


def validate(report, output, mode, scale, size, hdr, msaa, moving_reset=False):
    from PIL import Image
    scale = struct.unpack("f", struct.pack("f", scale))[0]
    captures_expected = MOVING_CAPTURES if moving_reset else CAPTURES
    protocol = 'claude-60hz-moving-cut-v2' if moving_reset else 'claude-60hz-sampled-v1'
    errors = []
    def require(condition, reason):
        if not condition:
            errors.append(reason)
    def integer(value):
        return type(value) is int and value >= 0
    def number(value):
        return type(value) in (int, float) and math.isfinite(value)
    def close(value, expected):
        return number(value) and abs(value - expected) <= 0.00001
    if not isinstance(report, dict):
        return ['report must be an object']
    require(report.get('kind') == 'quality_sequence' and report.get('protocol') == protocol, 'wrong quality protocol')
    require(report.get('valid') is True and report.get('errors') == [] and report.get('offscreen') is True, 'run is not valid offscreen evidence')
    require(report.get('mode') == mode and close(report.get('scale'), scale) and report.get('output_size') == size and report.get('hdr') is hdr and type(report.get('msaa_samples')) is int and report.get('msaa_samples') == msaa, 'arm mismatch')
    require(report.get('scene_version') == 'claude-toy-v1', 'unknown scene version')
    require(type(report.get('expected_capture_count')) is int and report.get('expected_capture_count') == len(captures_expected), 'capture count mismatch')
    frames = report.get('scripted_render_frames')
    captures = report.get('captures')
    if not isinstance(frames, list) or len(frames) != 145 or not isinstance(captures, list) or len(captures) != len(captures_expected):
        return errors + [f'expected exactly 145 scripted frames and {len(captures_expected)} captures']
    content = [max(1, math.floor(v * scale + 0.5)) for v in size]
    state = 'Disabled' if mode == 'Disabled' else 'OutputWritten'
    by_tick = {}
    first_frame = None
    expected_scope = None
    shot_ids = set()
    for tick, proof in enumerate(frames):
        if not isinstance(proof, dict):
            errors.append(f'invalid frame {tick}'); continue
        f = proof.get('render_frame')
        if tick == 0:
            first_frame = f
            expected_scope = {'view_id': proof.get('view_id'), 'image_target': proof.get('image_target'),
                'mode': mode, 'scale': scale, 'content_size': content, 'output_size': size}
        require(integer(f) and integer(first_frame) and f == first_frame + tick, f'noncontiguous frame {tick}')
        require(type(proof.get('tick')) is int and proof.get('tick') == tick, f'wrong tick {tick}')
        require(all(integer(proof.get(k)) and proof[k] == f for k in ('request_frame','extraction_frame')), f'request/extraction mismatch {tick}')
        require(integer(proof.get('view_id')) and proof.get('view_id') == expected_scope['view_id'] and isinstance(proof.get('image_target'),str) and bool(proof['image_target']) and proof['image_target'] == expected_scope['image_target'], f'view/target changed {tick}')
        require(proof.get('valid') is True and proof.get('target_matches') is True and proof.get('camera_pose_matches') is True and proof.get('format_matches') is True, f'unproved frame {tick}')
        require(type(proof.get('msaa_samples')) is int and proof.get('msaa_samples') == msaa, f'MSAA mismatch {tick}')
        require((proof.get('main_texture_format') == 'Rgba16Float') is hdr, f'HDR format mismatch {tick}')
        pose_tick = tick if moving_reset else min(tick,127)
        require(close(proof.get('simulation_seconds'), max(0,pose_tick-32)/60) and type(proof.get('jitter_index')) is int and proof.get('jitter_index') == tick%32, f'pose clock mismatch {tick}')
        matrix=proof.get('world_from_view'); expected_matrix=proof.get('expected_world_from_view')
        require(isinstance(matrix,list) and len(matrix)==16 and isinstance(expected_matrix,list) and len(expected_matrix)==16 and all(number(v) and close(v,e) for v,e in zip(matrix,expected_matrix)), f'camera matrix mismatch {tick}')
        if moving_reset:
            camera = expected_camera_matrix(tick, True)
            require(isinstance(matrix,list) and len(matrix)==16 and all(close(v,e) for v,e in zip(matrix,camera)), f'moving camera trajectory mismatch {tick}')
        if mode == 'Temporal':
            def radical(n,base):
                total=0.; denominator=1.
                while n:
                    denominator*=base; total+=(n%base)/denominator; n//=base
                return total-.5
            jitter=proof.get('jitter')
            require(isinstance(jitter,list) and len(jitter)==2 and all(close(v,radical(tick%32+1,b)) for v,b in zip(jitter,(2,3))), f'jitter mismatch {tick}')
        else:
            require(proof.get('jitter') is None, f'unexpected jitter {tick}')
        require(proof.get('reset_before_encode') is (mode == 'Temporal' and tick in (0,128)) and proof.get('reset_after_encode') is False, f'reset acknowledgement mismatch {tick}')
        require(type(proof.get('reset_ordinal')) is int and proof.get('reset_ordinal') == ((1 if tick<128 else 2) if mode=='Temporal' else 0), f'reset ordinal mismatch {tick}')
        effect=proof.get('effect',{})
        require(isinstance(effect,dict) and integer(effect.get('frame_id')) and effect.get('frame_id')==f and integer(effect.get('view_id')) and effect.get('view_id')==expected_scope['view_id'] and effect.get('requested_mode')==mode and effect.get('effective_mode')==mode and close(effect.get('scale'),scale) and effect.get('content_size')==content and effect.get('output_size')==size and effect.get('state')==state, f'effect mismatch {tick}')
        shot=proof.get('shot_entity')
        if tick in dict(captures_expected):
            require(integer(shot) and shot not in shot_ids and type(proof.get('extracted_shot_entity')) is int and proof.get('extracted_shot_entity')==shot, f'screenshot extraction mismatch {tick}')
            if integer(shot): shot_ids.add(shot)
        else:
            require(shot is None and proof.get('extracted_shot_entity') is None, f'unexpected screenshot {tick}')
        by_tick[tick]=proof
    directory=Path(str(output)+'.quality')
    last_callback = -1.0
    for capture,(tick,name) in zip(captures,captures_expected):
        if not isinstance(capture,dict):
            errors.append(f'invalid capture {name}');continue
        path=directory/(name+'.png'); proof=by_tick.get(tick,{})
        require(capture.get('valid') is True and type(capture.get('tick')) is int and capture.get('tick')==tick and capture.get('name')==name, f'capture timeline mismatch {name}')
        require(capture.get('path')==str(path) and not path.is_symlink() and not directory.is_symlink() and path.is_file(), f'capture path mismatch {name}')
        require(type(capture.get('shot_entity')) is int and capture.get('shot_entity')==proof.get('shot_entity') and integer(capture.get('request_frame')) and capture.get('request_frame')==proof.get('request_frame') and capture.get('render_proof')==proof, f'capture render identity mismatch {name}')
        arrival=capture.get('readback_arrived_main_frame'); f=proof.get('render_frame')
        require(integer(arrival) and integer(f) and arrival>=f, f'readback predates render {name}')
        fence=capture.get('completion_proof',{}); effect=fence.get('effect',{}) if isinstance(fence,dict) else {}
        require(isinstance(fence,dict) and integer(fence.get('frame_id')) and fence.get('frame_id')==f and fence.get('qualified') is True and number(fence.get('admitted_ms')) and number(fence.get('callback_observed_ms')) and 0<=fence['admitted_ms']<=fence['callback_observed_ms'] and fence.get('scope')==expected_scope and isinstance(effect,dict) and integer(effect.get('frame_id')) and effect.get('frame_id')==f and effect.get('ready') is True and effect.get('state')==state and effect.get('scope')==expected_scope, f'completion scope mismatch {name}')
        scopes = (fence.get('scope'), effect.get('scope')) if isinstance(fence,dict) and isinstance(effect,dict) else (None,None)
        require(all(isinstance(scope,dict) and integer(scope.get('view_id')) for scope in scopes), f'untyped completion scope {name}')
        require(isinstance(fence,dict) and fence.get('failure') is None, f'failed completion {name}')
        if isinstance(fence,dict) and number(fence.get('admitted_ms')) and number(fence.get('callback_observed_ms')):
            require(fence['admitted_ms'] >= last_callback, f'overlapping completion intervals {name}')
            last_callback = fence['callback_observed_ms']
        require(capture.get('width')==size[0] and capture.get('height')==size[1] and isinstance(capture.get('image_proof'),dict) and capture['image_proof'].get('nonuniform') is True and capture['image_proof'].get('opaque_fraction')==1.0, f'capture proof mismatch {name}')
        try:
            if path.is_symlink() or directory.is_symlink(): raise ValueError('symlink')
            with Image.open(path) as image:
                image.load()
                require(image.format=='PNG' and image.mode=='RGBA' and list(image.size)==size and image.getchannel('A').getextrema()==(255,255), f'PNG opacity or dimensions mismatch {name}')
                scene=image.crop((size[0]//10,size[1]//5,size[0]*9//10,size[1]*9//10)).convert('RGB')
                colors=scene.getcolors(33)
                require(colors is None, f'flat scene capture {name}')
        except (OSError,ValueError) as error:
            errors.append(f'cannot inspect PNG {name}: {error}')
    return errors


def bounded(command, log, timeout):
    stopped=[]
    handlers={sig:signal.getsignal(sig) for sig in (signal.SIGTERM,signal.SIGINT)}
    child=None
    try:
        for sig in handlers: signal.signal(sig,lambda received,_frame:stopped.append(received))
        child=subprocess.Popen(command,stdout=log,stderr=subprocess.STDOUT,start_new_session=True)
        deadline=time.monotonic()+timeout
        while True:
            if stopped:return 128+stopped[0]
            remaining=deadline-time.monotonic()
            if remaining<=0:return 124
            try:return child.wait(timeout=min(.25,remaining))
            except subprocess.TimeoutExpired:pass
    finally:
        if child is not None:
            try:
                os.killpg(child.pid,signal.SIGTERM);child.wait(timeout=3)
            except (ProcessLookupError,subprocess.TimeoutExpired):pass
            try:os.killpg(child.pid,signal.SIGKILL)
            except ProcessLookupError:pass
            child.wait()
        for sig,handler in handlers.items():signal.signal(sig,handler)


def main(argv=None):
    parser=argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--binary',type=Path,required=True)
    parser.add_argument('--out',type=Path,required=True)
    parser.add_argument('--mode',choices=('disabled','spatial','temporal'),required=True)
    parser.add_argument('--scale',type=float,required=True)
    parser.add_argument('--width',type=int,default=1280)
    parser.add_argument('--height',type=int,default=720)
    parser.add_argument('--hdr',action='store_true')
    parser.add_argument('--native-aa',action='store_true')
    parser.add_argument('--moving-reset',action='store_true',help='continue model/camera motion after the cut and retain every cut0..16 image')
    parser.add_argument('--timeout',type=float,default=90)
    args=parser.parse_args(argv)
    if not math.isfinite(args.scale) or not .1<=args.scale<=1 or not 1<=args.width<=8192 or not 1<=args.height<=8192 or not math.isfinite(args.timeout) or not 1<=args.timeout<=120:
        parser.error('scale, dimensions or timeout out of bounds')
    if args.native_aa != (args.mode=='disabled' and args.scale==1):
        parser.error('native scale Disabled requires native-aa; all reconstruction arms use MSAA off')
    output=args.out.parent.resolve()/args.out.name;binary=args.binary.resolve()
    if not output.parent.is_dir():
        parser.error('output needs an existing parent directory')
    paths=[output,Path(str(output)+'.quality'),Path(str(output)+'.quality.log'),Path(str(output)+'.quality-manifest.json')]
    if any(p.exists() or p.is_symlink() for p in paths):
        parser.error('output artifacts must not already exist')
    binary_hash=digest(binary)
    if not binary_hash or not os.access(binary,os.X_OK):parser.error('binary must be an executable regular file')
    protocol = 'claude-60hz-moving-cut-v2' if args.moving_reset else 'claude-60hz-sampled-v1'
    captures_expected = MOVING_CAPTURES if args.moving_reset else CAPTURES
    quality_flag = '--quality-moving-reset' if args.moving_reset else '--quality-sequence'
    command=[str(binary),quality_flag,'--offscreen','--subject','claude','--mode',args.mode,'--scale',str(args.scale),
             '--width',str(args.width),'--height',str(args.height),'--warmup','4','--out',str(output)]
    if args.hdr:command.append('--hdr')
    if args.native_aa:command.append('--native-aa')
    # Inherit Metal validation settings; all descendants share a cleanup group.
    if os.uname().sysname=='Darwin':command=['/usr/bin/caffeinate','-di',*command]
    receipt={'kind':'quality_sequence_run','protocol':protocol,'command':command,'binary':str(binary),'binary_sha256':binary_hash,
             'runner_sha256':digest(Path(__file__).resolve()),'valid':False,'child_exit':None}
    code=1;errors=[];validated=False
    try:
        # This reservation owns the evidence set. A loser must never finalize it.
        log=paths[2].open('x')
    except OSError as error:
        parser.error(f'could not reserve run: {error}')
    with log:
        try:
            code=bounded(command,log,args.timeout);receipt['child_exit']=code
            report=json.loads(output.read_text())
            errors=validate(report,output,args.mode.title(),args.scale,[args.width,args.height],args.hdr,4 if args.native_aa else 1,args.moving_reset)
            validated=True
        except Exception as error:  # Retain unexpected validator failures as invalid evidence.
            errors.append(f'failed run or invalid report: {error}')
        finally:
            receipt['binary_sha256_after']=digest(binary)
            receipt['report_sha256']=digest(output)
            receipt['captures']={name:{'path':str(paths[1]/(name+'.png')),'sha256':digest(paths[1]/(name+'.png')) if not paths[1].is_symlink() else None} for _,name in captures_expected}
            if receipt['binary_sha256_after']!=binary_hash:errors.append('binary changed during run')
            log.flush();receipt['log_sha256']=digest(paths[2])
            if not receipt['report_sha256'] or not receipt['log_sha256'] or not all(c['sha256'] for c in receipt['captures'].values()):errors.append('retained evidence hash inventory incomplete')
            receipt['validation_completed']=validated
            receipt['errors']=errors;receipt['valid']=code==0 and validated and not errors
            with paths[3].open('x') as manifest:json.dump(receipt,manifest,indent=2);manifest.write('\n')
    print(json.dumps({'valid':receipt['valid'],'manifest':str(paths[3]),'errors':errors}))
    return code if code else (0 if receipt['valid'] else 1)


if __name__=='__main__':
    raise SystemExit(main())
