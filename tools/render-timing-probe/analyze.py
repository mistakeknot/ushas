#!/usr/bin/env python3
"""Validate original-frame Metal stage samples; this never certifies governor input."""
import copy
import argparse
import json
from pathlib import Path
import statistics
import sys
import unittest


def union_ns(intervals):
    total, start, end = 0, None, None
    for left,right in sorted(intervals):
        if type(left) is not int or type(right) is not int or left < 0 or right <= left:
            raise ValueError('invalid integer GPU interval')
        if start is None:
            start,end=left,right
        elif left <= end:
            end=max(end,right)
        else:
            total += end-start
            start,end=left,right
    return total+(end-start if start is not None else 0)


def frame_metrics(admitted, completed):
    result={'valid':False,'errors':[],'validated_for_governor':False}
    try:
        identity=admitted['identity']
        expected={'frame','view','epoch','slot','generation','width','height','iterations','cpu_gap_ms'}
        if set(identity)!=expected or any(type(v) is not int or v<0 for v in identity.values()):
            raise ValueError('invalid original identity')
        if completed['identity']!=identity or completed['status']!='completed':
            raise ValueError('mismatched identity or failed command buffer')
        host=[admitted['host_ns'],completed['callback_host_ns'],completed['resolved_host_ns'],completed['delivered_host_ns']]
        if any(type(t) is not int or t<0 for t in host) or host!=sorted(host):
            raise ValueError('invalid CPU-stage ordering')
        if host[-1]-host[0]>250_000_000:
            raise ValueError('sample delivery older than declared 250ms limit')
        first,second=completed['first_submit_host_ns'],completed['second_submit_host_ns']
        if (type(first) is not int or type(second) is not int or not host[0]<=first<=second<=host[1]
                or second-first<identity['cpu_gap_ms']*1_000_000):
            raise ValueError('CPU submission ordering or configured delay was not observed')
        passes=completed['passes']
        expected_passes={'scene':4,'compute':2,'compose':4,'readback':2}
        if set(passes)!=set(expected_passes):
            raise ValueError('incomplete or unexpected encoder-family scope')
        intervals={}
        for name,count in expected_passes.items():
            ticks=passes[name]
            if len(ticks)!=count or any(type(t) is not int or t<=0 or t>=2**64-1 for t in ticks):
                raise ValueError('missing, zero, or error timestamp: '+name)
            intervals[name]=list(zip(ticks[::2],ticks[1::2]))
            union_ns(intervals[name])
            if count==4 and (ticks[0]>ticks[2] or ticks[1]>ticks[3]):
                raise ValueError('impossible stage order for the fullscreen triangle: '+name)
        if passes['compute'][0]<passes['scene'][3] or passes['compose'][2]<passes['compute'][1] or passes['readback'][0]<passes['compose'][3]:
            raise ValueError('timestamps include an unresolved texture dependency')
        pixels=completed['pixels']
        if any(type(pixels[key]) is not int for key in ('sentinel','count','alpha_errors','sampled_colors')):
            raise ValueError('pixel proof fields must be integers')
        if (pixels['sentinel']!=identity['frame'] or pixels['count']!=identity['width']*identity['height']
                or pixels['alpha_errors']!=0 or not 16<=pixels['sampled_colors']<=pixels['count']):
            raise ValueError('frame sentinel or rendered pixel proof failed')
        owned=intervals['scene']+intervals['compute']+intervals['compose']
        result.update(valid=True,identity=identity,render_stage_union_ns=union_ns(owned),
            outer_render_envelope_ns=max(b for a,b in owned)-min(a for a,b in owned),
            diagnostic_readback_ns=union_ns(intervals['readback']),owned_intervals=owned,
            cpu_submission_gap_ms=(second-first)/1e6,
            delivery_age_ms=(host[-1]-host[0])/1e6,callback_resolve_ms=(host[2]-host[1])/1e6,
            callback_dispatch_ms=(host[3]-host[2])/1e6)
    except (KeyError,TypeError,ValueError) as error:
        result['errors'].append(str(error))
    return result


def analyze(events):
    errors,admissions,completions=[],{},{}
    failed=lambda message: {'valid':False,'validated_for_governor':False,'errors':[message]}
    if not isinstance(events,list) or any(not isinstance(e,dict) for e in events):
        return failed('expected a list of object records')
    headers=[e for e in events if e.get('kind')=='header']
    summaries=[e for e in events if e.get('kind')=='summary']
    if len(headers)!=1 or len(summaries)!=1 or events[0] is not headers[0] or events[-1] is not summaries[0]:
        return failed('missing or misplaced header/final summary')
    header,summary=headers[0],summaries[0]
    expected_header={'schema':1,'target_frames':32,'ring_size':4,'width':320,'height':180,'maximum_delivery_age_ms':250}
    if any(type(header.get(k)) is not int or header[k]!=v for k,v in expected_header.items()):
        return failed('unexpected probe schema or declared configuration')
    if header.get('contains_metalfx') is not False or header.get('validated_for_governor') is not False:
        return failed('invalid scope assertion')
    live,generations,previous_gpu_end={},{},{}
    max_live,last_host=0,0
    for event in events[1:-1]:
        kind=event.get('kind')
        if kind not in ('admitted','completed'):
            errors.append('unknown or misplaced event kind')
            continue
        try:
            identity=event['identity']
            if not isinstance(identity,dict) or any(type(v) is not int for v in identity.values()):
                raise ValueError('noninteger identity')
            frame,slot,generation=identity['frame'],identity['slot'],identity['generation']
            if not 1<=frame<=32 or not 0<=slot<4:
                raise ValueError('frame or slot outside declared bounds')
            arm=(frame-1)%4
            expected={'frame':frame,'view':1,'epoch':arm+1,'slot':slot,'generation':generation,
                      'width':320,'height':180,'iterations':1000 if arm%2 else 0,'cpu_gap_ms':20 if arm>=2 else 0}
            if identity!=expected:
                raise ValueError('identity does not match declared original arm')
            host=event['host_ns' if kind=='admitted' else 'delivered_host_ns']
            if type(host) is not int or host<last_host:
                raise ValueError('control-queue delivery order is not monotonic')
            last_host=host
            target=admissions if kind=='admitted' else completions
            if frame in target:
                raise ValueError('duplicate '+kind+' frame')
            if kind=='admitted':
                if frame!=len(admissions)+1:
                    raise ValueError('noncontiguous frame admission')
                if slot in live or generation!=generations.get(slot,0)+1:
                    raise ValueError('live slot reuse or invalid slot generation')
                live[slot]=frame
                generations[slot]=generation
                max_live=max(max_live,len(live))
            else:
                if live.get(slot)!=frame or admissions[frame]['identity']!=identity:
                    raise ValueError('completion has no matching live admission')
                if event.get('selected_png_saved') is not True:
                    raise ValueError('missing or failed PNG-save outcome')
                value=frame_metrics(admissions[frame],event)
                if value['valid']:
                    if min(a for a,b in value['owned_intervals'])<previous_gpu_end.get(slot,0):
                        raise ValueError('reused slot contains older or overlapping GPU samples')
                    previous_gpu_end[slot]=event['passes']['readback'][1]
                del live[slot]
            target[frame]=event
        except (KeyError,TypeError,ValueError) as error:
            errors.append(str(error))
    if live or set(admissions)!=set(range(1,33)) or set(admissions)!=set(completions):
        errors.append('missing or unexpected admitted/completed frames')
    expected_summary={'admitted_frames':len(admissions),'completed_frames':len(completions),
                      'unresolved_frames':0,'exit_code':0,'command_errors':0}
    if (any(type(summary.get(k)) is not int or summary[k]!=v for k,v in expected_summary.items())
            or type(summary.get('skipped_admission_ticks')) is not int or summary['skipped_admission_ticks']<0
            or summary.get('validated_for_governor') is not False):
        errors.append('probe did not finish all admitted GPU work')
    frames=[]
    for frame in sorted(admissions.keys() & completions.keys()):
        value=frame_metrics(admissions[frame],completions[frame])
        value['frame']=frame
        frames.append(value)
        if not value['valid']:
            errors.append('invalid frame '+str(frame))
    arms={}
    for iterations,gap in ((0,0),(1000,0),(0,20),(1000,20)):
        group=[f for f in frames if f['valid'] and f['identity']['iterations']==iterations and f['identity']['cpu_gap_ms']==gap]
        arms[f'iterations{iterations}-gap{gap}']={'valid_frames':len(group),
            'median_stage_union_ms':statistics.median(f['render_stage_union_ns']/1e6 for f in group) if group else None,
            'median_outer_envelope_ms':statistics.median(f['outer_render_envelope_ns']/1e6 for f in group) if group else None,
            'median_cpu_submission_gap_ms':statistics.median(f['cpu_submission_gap_ms'] for f in group) if group else None,
            'max_delivery_age_ms':max((f['delivery_age_ms'] for f in group),default=None)}
        if len(group)!=8:errors.append('incomplete declared arm')
    good=[frame for frame in frames if frame['valid']]
    overlaps=[]
    for index,left in enumerate(good):
        for right in good[index+1:]:
            common=[(max(a,c),min(b,d)) for a,b in left['owned_intervals'] for c,d in right['owned_intervals'] if max(a,c)<min(b,d)]
            if common:overlaps.append({'frames':[left['frame'],right['frame']],'stage_overlap_ns':union_ns(common)})
    return {'valid':not errors,'validated_for_governor':False,'errors':errors,'frames':frames,'arms':arms,
            'valid_means':'structurally complete records; not successful control response or a validated timing producer',
            'maximum_in_flight':max_live,'cross_frame_overlap_pairs':len(overlaps),'cross_frame_overlaps':overlaps,
            'global_stage_union_ns':union_ns([interval for f in good for interval in f['owned_intervals']]),
            'sum_frame_stage_union_ns':sum(f['render_stage_union_ns'] for f in good),
            'scope':'Synthetic render/compute/composition stage union only; diagnostic readback excluded; no MetalFX/Bevy scope, trace validation, perturbation or live-governor acceptance.',
            'header':headers[0],'summary':summaries[0]}


class Tests(unittest.TestCase):
    def fixture(self):
        identity = {'frame':7,'view':1,'epoch':2,'slot':0,'generation':3,
                    'width':320,'height':180,'iterations':1000,'cpu_gap_ms':20}
        admitted = {'identity':identity,'host_ns':1000}
        completed = {'identity':copy.deepcopy(identity),'first_submit_host_ns':1000,
            'second_submit_host_ns':20_001_000,'callback_host_ns':22_000_000,
            'resolved_host_ns':24_000_000,'delivered_host_ns':25_000_000,'status':'completed',
            'passes':{'scene':[100,120,110,160], 'compute':[170,190],
                      'compose':[180,200,200,240], 'readback':[250,270]},
            'pixels':{'sentinel':7,'count':320*180,'alpha_errors':0,'sampled_colors':40}}
        return admitted,completed

    def test_union_does_not_double_count_overlapping_stages(self):
        self.assertEqual(union_ns([(100,120),(110,160),(170,190),(180,200),(200,240)]),130)
        self.assertEqual(union_ns([(1,10),(2,3),(10,12)]),11)

    def test_complete_original_identity_and_scope(self):
        a,c = self.fixture()
        result = frame_metrics(a,c)
        self.assertTrue(result['valid'])
        self.assertEqual(result.get('render_stage_union_ns'),130)
        self.assertEqual(result.get('outer_render_envelope_ns'),140)
        self.assertEqual(result.get('diagnostic_readback_ns'),20)
        self.assertEqual(result.get('validated_for_governor'),False)

    def test_missing_error_and_inverted_timestamps_fail(self):
        for values in ([0,120,130,160],[100,120,130,2**64-1],[120,100,130,160],[100,120]):
            a,c = self.fixture();c['passes']['scene']=values
            self.assertFalse(frame_metrics(a,c)['valid'])
        a,c=self.fixture();del c['passes']['compute']
        self.assertFalse(frame_metrics(a,c)['valid'])

    def test_identity_sentinel_and_stale_callback_fail(self):
        for key in ('frame','view','epoch','slot','generation','width','iterations','cpu_gap_ms'):
            a,c=self.fixture();c['identity'][key]+=1
            self.assertFalse(frame_metrics(a,c)['valid'],key)
        a,c=self.fixture();c['pixels']['sentinel']=6
        self.assertFalse(frame_metrics(a,c)['valid'])
        a,c=self.fixture();c['delivered_host_ns']=300_000_001
        self.assertFalse(frame_metrics(a,c)['valid'])

    def test_dependency_overlap_and_failed_command_buffer_fail(self):
        a,c=self.fixture();c['passes']['compute']=[150,180]
        self.assertFalse(frame_metrics(a,c)['valid'])
        a,c=self.fixture();c['status']='error'
        self.assertFalse(frame_metrics(a,c)['valid'])

    def test_cpu_submission_gap_is_observed_not_assumed(self):
        a,c=self.fixture()
        self.assertEqual(frame_metrics(a,c).get('cpu_submission_gap_ms'),20)
        c['second_submit_host_ns']=c['first_submit_host_ns']
        self.assertFalse(frame_metrics(a,c)['valid'])
        a,c=self.fixture();del c['second_submit_host_ns']
        self.assertFalse(frame_metrics(a,c)['valid'])

    def test_render_stage_order_and_typed_pixel_proof_fail_closed(self):
        for family in ('scene','compose'):
            a,c=self.fixture();c['passes'][family]=[150,160,100,120]
            self.assertFalse(frame_metrics(a,c)['valid'],family)
        for key,value in [('sentinel',7.0),('count',57600.0),('alpha_errors',False),
                          ('sampled_colors',float('nan')),('sampled_colors',57601)]:
            a,c=self.fixture();c['pixels'][key]=value
            self.assertFalse(frame_metrics(a,c)['valid'],key)

    def protocol(self):
        events=[{'kind':'header','schema':1,'target_frames':32,'ring_size':4,
                 'width':320,'height':180,'maximum_delivery_age_ms':250,
                 'contains_metalfx':False,'validated_for_governor':False}]
        for frame in range(1,33):
            a,c=self.fixture()
            arm=(frame-1)%4
            identity={'frame':frame,'view':1,'epoch':arm+1,'slot':arm,
                      'generation':(frame-1)//4+1,'width':320,'height':180,
                      'iterations':1000 if arm%2 else 0,'cpu_gap_ms':20 if arm>=2 else 0}
            a.update(kind='admitted',identity=identity,host_ns=frame*30_000_000)
            second=a['host_ns']+identity['cpu_gap_ms']*1_000_000
            c.update(kind='completed',identity=copy.deepcopy(identity),
                     first_submit_host_ns=a['host_ns'],second_submit_host_ns=second,
                     callback_host_ns=second+22000,resolved_host_ns=second+24000,
                     delivered_host_ns=second+25000,selected_png_saved=True)
            c['pixels']['sentinel']=frame
            c['passes']={name:[tick+frame*1000 for tick in ticks] for name,ticks in c['passes'].items()}
            events.extend((a,c))
        events.append({'kind':'summary','exit_code':0,'admitted_frames':32,'completed_frames':32,
                       'unresolved_frames':0,'command_errors':0,'skipped_admission_ticks':0,
                       'validated_for_governor':False})
        return events

    def test_complete_protocol_retains_cross_frame_overlap_scope(self):
        result=analyze(self.protocol())
        self.assertTrue(result['valid'],result['errors'])
        self.assertEqual(result.get('maximum_in_flight'),1)
        self.assertEqual(result.get('cross_frame_overlap_pairs'),0)
        self.assertEqual(result.get('global_stage_union_ns'),32*130)
        self.assertFalse(result['validated_for_governor'])
        self.assertTrue(all(arm['valid_frames']==8 for arm in result['arms'].values()))

    def test_protocol_rejects_header_summary_and_png_mutations(self):
        mutations=[(0,'schema',True),(0,'schema',2),(0,'target_frames',0),
                   (0,'ring_size',8),(0,'width',640),(0,'maximum_delivery_age_ms',500),
                   (-1,'admitted_frames',31),(-1,'completed_frames',True),
                   (-1,'command_errors',1),(-1,'unresolved_frames',1),
                   (-1,'exit_code',True),(-1,'skipped_admission_ticks',-1),
                   (2,'selected_png_saved',False),(2,'selected_png_saved',None)]
        for index,key,value in mutations:
            events=self.protocol();events[index][key]=value
            self.assertFalse(analyze(events)['valid'],(index,key,value))
        events=self.protocol();del events[2]['selected_png_saved']
        self.assertFalse(analyze(events)['valid'])

    def test_protocol_rejects_identity_laundering_and_stale_slot_samples(self):
        for key,value in [('frame',50),('view',2),('epoch',4),('slot',5),('generation',5),
                          ('width',321),('height',181),('iterations',999),('cpu_gap_ms',1)]:
            events=self.protocol()
            events[1]['identity'][key]=value;events[2]['identity'][key]=value
            if key=='frame':events[2]['pixels']['sentinel']=value
            self.assertFalse(analyze(events)['valid'],key)
        events=self.protocol();events[10]['passes']=copy.deepcopy(events[2]['passes'])
        self.assertFalse(analyze(events)['valid'],'slot reused with old query samples')

    def test_protocol_rejects_order_reuse_unknown_and_incomplete_records(self):
        cases=[]
        events=self.protocol();events[1],events[2]=events[2],events[1];cases.append(events)
        events=self.protocol();events.insert(2,copy.deepcopy(events[1]));cases.append(events)
        events=self.protocol();events.pop(2);cases.append(events)
        events=self.protocol();events.insert(2,{'kind':'fatal'});cases.append(events)
        events=self.protocol();events.append(events.pop(0));cases.append(events)
        events=self.protocol();events.insert(2,events.pop());cases.append(events)
        events=self.protocol()
        # Admit frame2 into frame1's still-live slot, preserving exact per-record identity.
        events[3]['identity']['slot']=0;events[4]['identity']['slot']=0
        events[3]['identity']['generation']=2;events[4]['identity']['generation']=2
        events.insert(2,events.pop(3));cases.append(events)
        for events in cases:self.assertFalse(analyze(events)['valid'])


if __name__=='__main__':
    if sys.argv[1:]==['--self-test']:
        unittest.main(argv=[sys.argv[0]])
    else:
        parser=argparse.ArgumentParser(description=__doc__)
        parser.add_argument('input',type=Path)
        parser.add_argument('--out',type=Path)
        args=parser.parse_args()
        result=analyze([json.loads(line) for line in args.input.read_text().splitlines() if line])
        payload=json.dumps(result,indent=2)+'\n'
        if args.out:
            with args.out.open('x') as stream:stream.write(payload)
        else:print(payload,end='')
        raise SystemExit(0 if result['valid'] else 1)
