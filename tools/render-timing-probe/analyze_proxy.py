#!/usr/bin/env python3
"""Read-only MetalFX proxy artifact checks. Never certifies a governor producer."""
import argparse
import copy
import hashlib
import json
import math
from pathlib import Path
import struct
import sys
import tempfile
import unittest
import zlib
from analyze import union_ns

SCOPE = ('Synthetic MetalFX supplied-buffer observations only; trace inventory of all '
         'target-process GPU work, calibrated overhead, Bevy coverage and governor validation remain pending.')


def require(condition, message):
    if not condition:
        raise ValueError(message)


def integer(value, minimum=0, maximum=2**64-1):
    return type(value) is int and minimum <= value <= maximum


def sha(value):
    return isinstance(value,str) and len(value)==64 and all(c in '0123456789abcdef' for c in value)


def same_identity(left, right):
    # Python's dict equality treats True == 1; provenance does not.
    return json.dumps(left,sort_keys=True,allow_nan=False)==json.dumps(right,sort_keys=True,allow_nan=False)


def header_valid(header):
    require(header['kind']=='header' and type(header['schema']) is int and header['schema']==1,'unknown header/schema')
    require(header['mode'] in ('spatial','temporal') and header['observe'] in ('off','calls','counters'),'unknown arm')
    for key,value in dict(target_frames=16,ring_size=2,input_width=160,input_height=90,output_width=320,output_height=180,
                          maximum_encoders_per_frame=32,maximum_samples_per_frame=128,maximum_selector_records_per_frame=256,
                          maximum_delivery_age_ms=250).items():
        require(type(header[key]) is int and header[key]==value,'unexpected fixed header '+key)
    require(integer(header['pid'],1),'missing process identity')
    for key in ('device','os','input_pattern','temporal_history','scope'):
        require(isinstance(header[key],str) and bool(header[key]),'missing header '+key)
    require(header['input_output_format']=='RGBA16Float' and header['capture_format']=='RGBA8Unorm','unexpected formats')
    require(header['scaler_supported'] is True and header['validated_for_governor'] is False,'support/governor claim')
    require(type(header['stage_counters_supported']) is bool and type(header['timestamp_counter_set']) is bool,'untyped capability')
    if header['observe']=='counters':
        require(header['stage_counters_supported'] and header['timestamp_counter_set'],'counter capability missing')


def frame_valid(header, admitted, completed):
    identity=admitted['identity'];prefix=admitted['command_buffer_prefix'];arm=header['observe']
    require(same_identity(completed['identity'],identity),'completion identity mismatch')
    for key in ('setup_succeeded','metalfx_succeeded','readback_succeeded','owner_committed_metalfx','png_saved','raw_saved','within_delivery_age_limit'):
        require(completed[key] is True,'failed frame '+key)
    require(completed['validated_for_governor'] is False,'unvalidated frame promoted')
    host=[admitted['host_ns']]+[completed[k] for k in ('encode_start_host_ns','encode_end_host_ns','metalfx_callback_host_ns','counter_resolved_host_ns','readback_callback_host_ns','delivered_host_ns')]
    require(all(integer(t,1) for t in host),'invalid CPU clock')
    a,start,end,callback,resolved,readback,delivered=host
    require(a<=start<=end<=callback<=resolved<=delivered and end<=readback<=delivered,'invalid CPU-stage order')
    require(delivered-a<=250_000_000,'stale delivery')
    require(set(completed['command_buffers'])=={'setup','metalfx','finish'},'missing owned command buffer')
    for name,state in completed['command_buffers'].items():
        require(type(state['status']) is int and state['status']==4 and state['error'] is None and state['label']==prefix+'/'+name,'failed/misidentified command buffer')
    families=['scene-input','output-clear']+(['depth-input','motion-input','exposure-input'] if header['mode']=='temporal' else [])
    require(completed['setup_encoder_families']==families,'missing setup inventory')
    pixels=completed['pixels'];expected=dict(count=57600,sentinel=identity['frame'],alpha_errors=0,raw_nonfinite_rgb_values=0)
    for key,value in expected.items():
        require(type(pixels[key]) is int and pixels[key]==value,'invalid pixel proof '+key)
    require(integer(pixels['sampled_colors'],16,3600),'invalid sampled colors')
    require(all(type(pixels[k]) in (int,float) and math.isfinite(pixels[k]) for k in ('raw_min_rgb','raw_max_rgb')) and pixels['raw_min_rgb']<pixels['raw_max_rgb'],'invalid raw color extent')
    require(sha(pixels['metalfx_rgba16_sha256']) and sha(pixels['composed_rgba8_sha256']),'invalid raw/pixel hash')
    observation=completed['observation'];require(observation['validated_for_governor'] is False,'observation promoted')
    pairs=[]
    if arm=='off':
        require(observation==dict(observation_mode='off',available=False,not_requested=True,validated_for_governor=False),'off must explicitly remain unobserved')
    else:
        require(observation['available'] is True and observation['sealed'] is True and observation['completed'] is True and observation['errors']==[],'unavailable or unsealed observation')
        require(same_identity(observation['identity'],identity) and observation['observation_mode']==arm,'observation identity mismatch')
        require(observation['expected_command_buffer_label']==prefix+'/metalfx' and observation['sealed_command_buffer_label']==prefix+'/metalfx','observation buffer mismatch')
        require(type(observation['sealed_command_buffer_status']) is int and observation['sealed_command_buffer_status']==0 and type(observation['completed_command_buffer_status']) is int and observation['completed_command_buffer_status']==4,'submission lifecycle violation')
        encoders=observation['encoders'];selectors=observation['selectors']
        require(isinstance(encoders,list) and 1<=len(encoders)<=32 and isinstance(selectors,list) and len(selectors)<=256,'invalid inventory sizes')
        for key,value in dict(total_encoder_factories=len(encoders),total_selector_calls=len(selectors),dropped_selector_records=0).items():
            require(type(observation[key]) is int and observation[key]==value,'lost/extra inventory '+key)
        require(all(isinstance(s,str) and s for s in selectors),'invalid selector')
        allowed={'device','commandQueue','label','setLabel:','retainedReferences','errorOptions','kernelStartTime','kernelEndTime','GPUStartTime','GPUEndTime','status','error','logs','addCompletedHandler:','addScheduledHandler:','pushDebugGroup:','popDebugGroup'}
        factories={'render':{'renderCommandEncoderWithDescriptor:'},'compute':{'computeCommandEncoder','computeCommandEncoderWithDispatchType:','computeCommandEncoderWithDescriptor:'},'blit':{'blitCommandEncoder','blitCommandEncoderWithDescriptor:'}}
        all_factories=set().union(*factories.values())
        require(all(s in allowed|all_factories for s in selectors),'unobserved selector path')
        require([s for s in selectors if s in all_factories]==[e['factory_selector'] for e in encoders],'selector/factory order mismatch')
        labels=set();samples=0
        for ordinal,encoder in enumerate(encoders,1):
            family=encoder['family'];label=encoder['label']
            require(type(encoder['ordinal']) is int and encoder['ordinal']==ordinal and family in factories and encoder['factory_selector'] in factories[family],'encoder source identity')
            require(isinstance(label,str) and bool(label) and label not in labels,'ambiguous framework label');labels.add(label)
            count=(4 if family=='render' else 2) if arm=='counters' else 0
            require(type(encoder['sample_count']) is int and encoder['sample_count']==count,'missing stage boundaries');samples+=count
            ticks=encoder['ticks']
            if count:
                require(isinstance(ticks,list) and len(ticks)==count and all(integer(t,1,2**64-2) for t in ticks),'invalid ticks')
                interval=list(zip(ticks[::2],ticks[1::2]));union_ns(interval);pairs.extend(interval)
            else:
                require(ticks is None,'calls arm unexpectedly sampled')
        require(type(observation['requested_samples']) is int and observation['requested_samples']==samples<=128,'sample count mismatch')
    return dict(identity=identity,metalfx_rgba16_sha256=pixels['metalfx_rgba16_sha256'],composed_rgba8_sha256=pixels['composed_rgba8_sha256'],
                sampled_stage_union_ns=union_ns(pairs) if pairs else None,sampled_intervals_ns=pairs,
                cpu_encode_ns=end-start,delivery_age_ns=delivered-a)


def analyze(records):
    result=dict(valid=False,errors=[],scope=SCOPE,validated_for_governor=False,frames=[])
    try:
        require(isinstance(records,list) and len(records)>=2,'incomplete records')
        header=records[0];header_valid(header);result['header']=header
        admitted={};active={};generations={0:0,1:0};previous=0;completed_ids=set()
        for record in records[1:-1]:
            kind=record['kind'];require(kind in ('admitted','completed'),'unknown event '+str(kind))
            identity=record['identity'];frame=identity['frame'];slot=identity['slot']
            require(set(identity)=={'frame','view','epoch','slot','generation','mode','observe','input_width','input_height','output_width','output_height','reset'},'unexpected identity shape')
            require(all(integer(identity[k],0 if k=='slot' else 1) for k in ('frame','view','epoch','slot','generation','input_width','input_height','output_width','output_height')),'identity integer types')
            require(slot in (0,1) and identity['view']==1 and identity['epoch']==1,'unsupported view/epoch/slot')
            for key in ('mode','observe','input_width','input_height','output_width','output_height'):
                require(identity[key]==header[key],'arm/content mismatch')
            require(identity['reset'] is (header['mode']=='temporal' and frame==1),'invalid history reset')
            when=record['host_ns'] if kind=='admitted' else record['delivered_host_ns']
            require(integer(when,1) and when>=previous,'event clock went backwards');previous=when
            if kind=='admitted':
                require(frame==len(admitted)+1 and frame<=16 and slot not in active,'noncontiguous admission or reused live slot')
                require(identity['generation']==generations[slot]+1,'slot generation reused/skipped')
                prefix=f'proxy/frame={frame}/view=1/epoch=1/slot={slot}/gen={identity["generation"]}'
                require(record['command_buffer_prefix']==prefix,'invalid source prefix')
                generations[slot]+=1;active[slot]=frame;admitted[frame]=record
            else:
                require(frame in admitted and frame not in completed_ids and active.get(slot)==frame,'missing/duplicate completion')
                result['frames'].append(frame_valid(header,admitted[frame],record));completed_ids.add(frame);del active[slot]
        summary=records[-1];require(summary['kind']=='summary','summary missing')
        expected=dict(exit_code=0,admitted_frames=16,completed_frames=16,unresolved_frames=0,gpu_failed_frames=0,pixel_failed_frames=0,observation_unavailable_frames=0)
        for key,value in expected.items():
            require(type(summary[key]) is int and summary[key]==value,'failed/missing summary '+key)
        require(summary['validated_for_governor'] is False and integer(summary['skipped_admission_ticks']),'invalid summary')
        require(len(admitted)==16 and len(completed_ids)==16 and not active,'incomplete frames')
        require(len({f['metalfx_rgba16_sha256'] for f in result['frames']})==16,'repeated raw output despite changing frame input')
        result['frames'].sort(key=lambda f:f['identity']['frame'])
        intervals=[pair for f in result['frames'] for pair in f['sampled_intervals_ns']]
        result.update(valid=True,complete_frames=16,global_sampled_stage_union_ns=union_ns(intervals) if intervals else None)
    except (KeyError,TypeError,ValueError,IndexError,OverflowError) as error:
        result['errors'].append(str(error))
    return result


def compare(reference, candidate):
    result=dict(valid=False,errors=[],scope='Exact per-frame raw MetalFX output and composed pixel parity only; not GPU coverage or overhead.')
    try:
        require(reference['valid'] is True and candidate['valid'] is True,'invalid run cannot establish parity')
        require(reference['header']['observe']=='off' and candidate['header']['observe'] in ('calls','counters'),'parity requires real-buffer off reference')
        ignore={'observe','pid'}
        require({k:v for k,v in reference['header'].items() if k not in ignore}=={k:v for k,v in candidate['header'].items() if k not in ignore},'reference configuration mismatch')
        require(len(reference['frames'])==len(candidate['frames'])==16,'missing parity frames')
        for left,right in zip(reference['frames'],candidate['frames']):
            # Admission slots may differ with timing; the deterministic input/history may not.
            require({k:v for k,v in left['identity'].items() if k not in {'observe','slot','generation'}}=={k:v for k,v in right['identity'].items() if k not in {'observe','slot','generation'}},'parity source identity mismatch')
            for key in ('metalfx_rgba16_sha256','composed_rgba8_sha256'):
                require(left[key]==right[key],f'exact output mismatch frame {left["identity"]["frame"]} {key}')
        result['valid']=True
    except (KeyError,TypeError,ValueError) as error:
        result['errors'].append(str(error))
    return result


def decode_png(path):
    data=path.read_bytes();require(data[:8]==b'\x89PNG\r\n\x1a\n','PNG signature');offset=8;compressed=b'';header=False;ended=False
    while offset<len(data):
        require(offset+12<=len(data),'truncated PNG')
        length=struct.unpack_from('>I',data,offset)[0];kind=data[offset+4:offset+8];end=offset+12+length
        require(end<=len(data),'truncated PNG chunk');payload=data[offset+8:end-4]
        require(zlib.crc32(kind+payload)==struct.unpack_from('>I',data,end-4)[0],'PNG CRC')
        if kind==b'IHDR':
            require(not header and offset==8 and payload==struct.pack('>IIBBBBB',320,180,8,6,0,0,0),'unsupported PNG layout');header=True
        elif kind==b'IDAT':compressed+=payload
        elif kind==b'IEND':require(length==0 and end==len(data),'PNG trailing bytes');ended=True
        elif not kind[0]&32:raise ValueError('unknown critical PNG chunk')
        offset=end
    require(header and ended,'incomplete PNG');raw=zlib.decompress(compressed);stride=1280
    require(len(raw)==180*(stride+1),'PNG decoded length');output=bytearray();prior=bytes(stride)
    for y in range(180):
        filt=raw[y*(stride+1)];row=bytearray(raw[y*(stride+1)+1:(y+1)*(stride+1)]);require(filt<=4,'PNG filter')
        for i in range(stride):
            a=row[i-4] if i>=4 else 0;b=prior[i];c=prior[i-4] if i>=4 else 0
            if filt==1:predict=a
            elif filt==2:predict=b
            elif filt==3:predict=(a+b)//2
            elif filt==4:
                p=a+b-c;pa,pb,pc=abs(p-a),abs(p-b),abs(p-c);predict=a if pa<=pb and pa<=pc else b if pb<=pc else c
            else:predict=0
            row[i]=(row[i]+predict)&255
        output.extend(row);prior=row
    return bytes(output)


def inspect(directory):
    try:
        source=directory/'samples.jsonl';require(source.is_file() and not source.is_symlink(),'missing/linked protocol')
        records=[json.loads(line) for line in source.read_text().splitlines()];result=analyze(records)
        result['samples_sha256']=hashlib.sha256(source.read_bytes()).hexdigest();result['artifacts']=[]
        if not result['valid']:return result
        for record in (r for r in records if r['kind']=='completed'):
            number=record['identity']['frame'];png=directory/f'frame-{number:04d}.png';raw=directory/f'frame-{number:04d}.rgba16'
            require(png.is_file() and raw.is_file() and not png.is_symlink() and not raw.is_symlink(),'missing/linked frame artifacts')
            rgba=decode_png(png);data=raw.read_bytes();proof=record['pixels']
            require(len(data)==57600*8,'raw output size')
            require(hashlib.sha256(data).hexdigest()==proof['metalfx_rgba16_sha256'] and hashlib.sha256(rgba).hexdigest()==proof['composed_rgba8_sha256'],'retained pixel hash mismatch')
            require(rgba[0]+256*rgba[1]==number and all(a==255 for a in rgba[3::4]),'decoded PNG identity/alpha')
            require(len({rgba[i:i+3] for i in range(0,len(rgba),64)})==proof['sampled_colors'],'decoded PNG color proof')
            values=[value for pixel in struct.iter_unpack('<eeee',data) for value in pixel[:3]]
            require(all(math.isfinite(v) for v in values) and min(values)==proof['raw_min_rgb'] and max(values)==proof['raw_max_rgb'],'retained raw finite/range proof')
            result['artifacts'].append(dict(frame=number,png_sha256=hashlib.sha256(png.read_bytes()).hexdigest(),raw_sha256=hashlib.sha256(data).hexdigest()))
        return result
    except (OSError,ValueError,KeyError,TypeError,struct.error,zlib.error) as error:
        return dict(valid=False,errors=[str(error)],scope=SCOPE,validated_for_governor=False)


def fixture(observe='counters', mode='spatial'):
    header = dict(kind='header', schema=1, mode=mode, observe=observe, pid=100, device='fake device', os='fake os',
                  target_frames=16, ring_size=2, input_width=160, input_height=90, output_width=320, output_height=180,
                  scaler_supported=True, stage_counters_supported=True, timestamp_counter_set=True,
                  maximum_encoders_per_frame=32, maximum_samples_per_frame=128, maximum_selector_records_per_frame=256,
                  maximum_delivery_age_ms=250, input_output_format='RGBA16Float', capture_format='RGBA8Unorm',
                  input_pattern='red=x/160;green=y/90;blue=.2+frame*.02;alpha=1',
                  temporal_history='one scaler; reset frame1 only; all frame inputs retained; zero jitter/motion; reversed depth0.5; exposure1',
                  scope='supplied MetalFX command-buffer observation only; all-process encoder trace inventory still required',
                  validated_for_governor=False, metal_debug_layer='1')
    records = [header]
    for frame in range(1,17):
        identity = dict(frame=frame,view=1,epoch=1,slot=0,generation=frame,mode=mode,observe=observe,
                        input_width=160,input_height=90,output_width=320,output_height=180,reset=mode=='temporal' and frame==1)
        prefix=f'proxy/frame={frame}/view=1/epoch=1/slot=0/gen={frame}'
        now=frame*1_000_000
        records.append(dict(kind='admitted',identity=identity,host_ns=now,command_buffer_prefix=prefix))
        encoders=[dict(ordinal=1,family='compute',factory_selector='computeCommandEncoder',label='MetalFX real encoder',
                       sample_count=2 if observe=='counters' else 0,ticks=[now+20,now+30] if observe=='counters' else None)]
        observation=dict(available=True,validated_for_governor=False,identity=identity,observation_mode=observe,
                         expected_command_buffer_label=prefix+'/metalfx',sealed_command_buffer_label=prefix+'/metalfx',
                         sealed_command_buffer_status=0,completed_command_buffer_status=4,sealed=True,completed=True,
                         errors=[],selectors=['computeCommandEncoder'],total_selector_calls=1,dropped_selector_records=0,
                         total_encoder_factories=1,requested_samples=2 if observe=='counters' else 0,encoders=encoders)
        if observe=='off': observation=dict(observation_mode='off',available=False,not_requested=True,validated_for_governor=False)
        records.append(dict(kind='completed',identity=identity,setup_succeeded=True,metalfx_succeeded=True,
            readback_succeeded=True,owner_committed_metalfx=True,png_saved=True,raw_saved=True,
            command_buffers={family:dict(status=4,label=prefix+'/'+family,error=None) for family in ['setup','metalfx','finish']},
            pixels=dict(count=57600,sentinel=frame,alpha_errors=0,sampled_colors=100,raw_nonfinite_rgb_values=0,
                        raw_min_rgb=0.0,raw_max_rgb=1.0,metalfx_rgba16_sha256=f'{frame:064x}',composed_rgba8_sha256=f'{frame+100:064x}'),
            observation=observation,setup_encoder_families=['scene-input','output-clear']+(['depth-input','motion-input','exposure-input'] if mode=='temporal' else []),
            encode_start_host_ns=now+1,encode_end_host_ns=now+10,metalfx_callback_host_ns=now+40,
            counter_resolved_host_ns=now+50,readback_callback_host_ns=now+60,delivered_host_ns=now+80,
            within_delivery_age_limit=True,validated_for_governor=False))
    records.append(dict(kind='summary',exit_code=0,reason='all frame callbacks and readbacks retained',admitted_frames=16,
        completed_frames=16,unresolved_frames=0,skipped_admission_ticks=0,gpu_failed_frames=0,pixel_failed_frames=0,
        observation_unavailable_frames=0,validated_for_governor=False))
    return records


class Tests(unittest.TestCase):
    def test_good_arms_have_explicit_limited_scope(self):
        for mode in ('spatial','temporal'):
            for arm in ('off','calls','counters'):
                result=analyze(fixture(arm,mode))
                self.assertTrue(result['valid'],result)
                self.assertFalse(result['validated_for_governor'])
                self.assertEqual(result.get('complete_frames'),16)

    def test_ledger_mutations_rejected(self):
        mutations=[lambda r:r[0].update(schema=True),lambda r:r[0].update(target_frames=15),
          lambda r:r[-1].update(exit_code=1),lambda r:r[-1].update(completed_frames=15),
          lambda r:r[2].update(png_saved=False),lambda r:r[2].update(raw_saved=False),
          lambda r:r[2]['pixels'].update(sampled_colors=float('nan')),lambda r:r[2]['pixels'].update(sentinel=True),
          lambda r:r[2]['pixels'].update(raw_nonfinite_rgb_values=1),lambda r:r[2].update(validated_for_governor=True),
          lambda r:r[2]['observation'].update(errors=['unsupported_selector']),lambda r:r[2]['observation'].update(available=False),
          lambda r:r[2]['observation'].update(total_encoder_factories=2),lambda r:r[2]['observation']['encoders'][0].update(ticks=[30,20]),
          lambda r:r[2]['command_buffers']['metalfx'].update(status=3),lambda r:r[2].update(delivered_host_ns=999_000_000),
          lambda r:r[2]['observation'].update(sealed_command_buffer_label='wrong'),lambda r:r[2]['pixels'].update(metalfx_rgba16_sha256='x')]
        for mutate in mutations:
            r=copy.deepcopy(fixture());mutate(r)
            with self.subTest(mutation=mutate):self.assertFalse(analyze(r)['valid'])

    def test_slot_reuse_and_identity_are_not_laundered(self):
        r=fixture();r[2],r[3]=r[3],r[2]
        self.assertFalse(analyze(r)['valid'])

    def test_boolean_completion_identity_is_not_an_integer_identity(self):
        for key in ('identity','observation'):
            r=copy.deepcopy(fixture());r[2]['identity']=dict(r[2]['identity']);r[2]['observation']['identity']=dict(r[2]['observation']['identity'])
            target=r[2]['identity'] if key=='identity' else r[2]['observation']['identity']
            target['frame']=True
            self.assertFalse(analyze(r)['valid'])
        for field,value in [('frame',5),('generation',1),('epoch',2),('mode','temporal'),('reset',True)]:
            r=copy.deepcopy(fixture());r[3]['identity'][field]=value
            self.assertFalse(analyze(r)['valid'])
        r=fixture();r[4]['delivered_host_ns']=r[2]['delivered_host_ns']
        self.assertFalse(analyze(r)['valid'])

    def test_missing_duplicate_or_unknown_events(self):
        for change in (lambda r:r.pop(2),lambda r:r.insert(3,copy.deepcopy(r[2])),lambda r:r.insert(1,dict(kind='unknown'))):
            r=fixture();change(r);self.assertFalse(analyze(r)['valid'])

    def test_overlap_union_not_sum(self):
        r=fixture();e=r[2]['observation']['encoders'][0];e.update(family='render',factory_selector='renderCommandEncoderWithDescriptor:',sample_count=4,ticks=[100,200,150,250])
        r[2]['observation'].update(selectors=['renderCommandEncoderWithDescriptor:'],requested_samples=4)
        result=analyze(r);self.assertTrue(result['valid'],result)
        self.assertEqual(result['frames'][0]['sampled_stage_union_ns'],150)

    def test_exact_parity_requires_matching_arms_and_all_frames(self):
        off=analyze(fixture('off'));calls=analyze(fixture('calls'))
        self.assertTrue(compare(off,calls)['valid'])
        calls=analyze(fixture('calls','temporal'));self.assertFalse(compare(off,calls)['valid'])
        calls=analyze(fixture('calls'));calls['frames'][0]['metalfx_rgba16_sha256']='f'*64
        self.assertFalse(compare(off,calls)['valid'])
        self.assertFalse(compare(analyze(fixture('calls')),analyze(fixture('counters')))['valid'])

    def test_png_roundtrip_and_corruption(self):
        def chunk(name,payload):return struct.pack('>I',len(payload))+name+payload+struct.pack('>I',zlib.crc32(name+payload))
        pixels=bytes([1,2,3,255])*320*180
        data=b'\x89PNG\r\n\x1a\n'+chunk(b'IHDR',struct.pack('>IIBBBBB',320,180,8,6,0,0,0))+chunk(b'IDAT',zlib.compress(b''.join(b'\0'+pixels[y*1280:(y+1)*1280] for y in range(180))))+chunk(b'IEND',b'')
        with tempfile.TemporaryDirectory() as tmp:
            path=Path(tmp)/'test.png';path.write_bytes(data);self.assertEqual(decode_png(path),pixels)
            path.write_bytes(data[:-1]+b'x')
            with self.assertRaises(ValueError):decode_png(path)

    def test_missing_files_fail_without_mutating_records(self):
        with tempfile.TemporaryDirectory() as tmp:
            path=Path(tmp);source='\n'.join(json.dumps(r) for r in fixture())+'\n';(path/'samples.jsonl').write_text(source)
            self.assertFalse(inspect(path)['valid'])
            self.assertEqual((path/'samples.jsonl').read_text(),source)
            self.assertEqual(len(list(path.iterdir())),1)

    def test_actual_artifact_decode_and_raw_corruption(self):
        def chunk(name,payload):return struct.pack('>I',len(payload))+name+payload+struct.pack('>I',zlib.crc32(name+payload))
        records=fixture('off')
        with tempfile.TemporaryDirectory() as tmp:
            path=Path(tmp)
            for frame in range(1,17):
                rgba=bytearray(bytes(v for i in range(320*180) for v in (i%251,(i//320)%180,frame,255)))
                rgba[:4]=bytes((frame,0,128,255))
                raw=b''.join(struct.pack('<eeee',i%16/16,0,frame/32,1) for i in range(320*180))
                compressed=zlib.compress(b''.join(b'\0'+rgba[y*1280:(y+1)*1280] for y in range(180)))
                png=b'\x89PNG\r\n\x1a\n'+chunk(b'IHDR',struct.pack('>IIBBBBB',320,180,8,6,0,0,0))+chunk(b'IDAT',compressed)+chunk(b'IEND',b'')
                (path/f'frame-{frame:04d}.png').write_bytes(png);(path/f'frame-{frame:04d}.rgba16').write_bytes(raw)
                records[2*frame]['pixels'].update(sampled_colors=len({bytes(rgba[i:i+3]) for i in range(0,len(rgba),64)}),raw_max_rgb=15/16,
                    metalfx_rgba16_sha256=hashlib.sha256(raw).hexdigest(),composed_rgba8_sha256=hashlib.sha256(rgba).hexdigest())
            (path/'samples.jsonl').write_text('\n'.join(json.dumps(r) for r in records)+'\n')
            result=inspect(path);self.assertTrue(result['valid'],result);self.assertEqual(len(result['artifacts']),16)
            (path/'frame-0001.rgba16').write_bytes(b'wrong')
            self.assertFalse(inspect(path)['valid'])


if __name__=='__main__':
    if sys.argv[1:]==['--self-test']:
        unittest.main(argv=[sys.argv[0]])
    else:
        parser=argparse.ArgumentParser(description=__doc__)
        parser.add_argument('--run',type=Path,required=True)
        parser.add_argument('--reference',type=Path)
        parser.add_argument('--out',type=Path,help='Optional new JSON file; never overwrites existing evidence')
        args=parser.parse_args();result=inspect(args.run)
        if args.reference:
            result['parity']=compare(inspect(args.reference),result)
            result['valid']=result['valid'] and result['parity']['valid']
        payload=json.dumps(result,sort_keys=True,indent=2,allow_nan=False)+'\n'
        if args.out:
            with args.out.open('x') as stream:stream.write(payload)
        else:print(payload,end='')
        raise SystemExit(0 if result['valid'] else 1)
