#!/usr/bin/env python3
"""Join the synthetic probe's actual Metal stages; never certifies governor input."""
import argparse
import collections
import copy
import hashlib
import json
from pathlib import Path
import re
import sys
import tempfile
import unittest
import xml.etree.ElementTree as ET

import analyze as records
from analyze import union_ns


def read_table(path, schema):
    root=ET.parse(path).getroot()
    ids={}
    for element in root.iter():
        if element.get('id'):
            if element.get('id') in ids:raise ValueError('duplicate XML reference identity')
            ids[element.get('id')]=element

    def value(element):
        seen=set()
        while element.get('ref'):
            ref=element.get('ref')
            if ref in seen or ref not in ids:raise ValueError('cyclic or missing XML reference')
            seen.add(ref);element=ids[ref]
        return element.get('fmt','') if len(element) else (element.text or element.get('fmt',''))

    nodes=[node for node in root.findall('node') if node.find('schema') is not None and node.find('schema').get('name')==schema]
    if len(nodes)!=1:raise ValueError('missing or duplicate table schema: '+schema)
    names=[col.findtext('mnemonic') for col in nodes[0].find('schema').findall('col')]
    if not names or None in names or len(set(names))!=len(names):raise ValueError('invalid column schema')
    return [dict(zip(names,map(value,row),strict=True)) for row in nodes[0].findall('row')]


LABEL=re.compile(r'stage-probe/frame=(\d+)/view=(\d+)/epoch=(\d+)/slot=(\d+)/gen=(\d+)/(scene|compute|compose|diagnostic-readback)')


def integer(value):
    if type(value) is not str or not re.fullmatch(r'\d+',value):raise ValueError('invalid exported integer')
    return int(value)


def interval(row):
    start,duration=integer(row['start']),integer(row['duration'])
    if duration==0:raise ValueError('zero-length exported interval')
    return start,start+duration


def merged(intervals):
    result=[]
    for start,end in sorted(intervals):
        if result and start<=result[-1][1]:result[-1]=(result[-1][0],max(end,result[-1][1]))
        else:result.append((start,end))
    return result


def intersect_ns(left,right):
    return union_ns([(max(a,c),min(b,d)) for a,b in merged(left) for c,d in merged(right) if max(a,c)<min(b,d)])


def audit(events, encoders, gpu, states):
    result={'valid':False,'validated_for_governor':False,'errors':[],
            'scope':'Synthetic actual-stage counter/trace agreement. Stage boundaries and trace Active rows are not exclusive GPU busy counters. No Bevy, MetalFX, overhead or live-governor validation.'}
    errors=result['errors']
    native=records.analyze(events)
    if not native['valid']:
        errors.append('native record protocol failed');result['native_errors']=native['errors'];return result
    pid=native['header'].get('pid')
    if type(pid) is not int or pid<=0:
        errors.append('missing native process identity');return result
    process=re.compile(rf'\({pid}\)$')
    complete={event['identity']['frame']:event for event in events if event['kind']=='completed'}
    try:
        target_cpu=[row for row in encoders if process.search(row['process'])]
        target_gpu=[row for row in gpu if process.search(row['process'])]
        devices={row['gpu'] for row in target_cpu+target_gpu}
        if len(devices)!=1 or '' in devices:raise ValueError('owned encoders must identify exactly one GPU')
        device=next(iter(devices))
        foreign=[row for row in gpu if row['gpu']==device and row['process'] and not process.search(row['process'])]
        unattributed=[row for row in gpu if row['gpu']==device and not row['process']]
        if len(target_cpu)!=128:errors.append('expected128 owned CPU encoders')
        cpu_by_id={};owned={};commands={}
        for row in target_cpu:
            encoder_id=integer(row['encoder-id'])
            command_id=integer(row['cmdbuffer-id'])
            if encoder_id in cpu_by_id:raise ValueError('duplicate CPU encoder identity')
            cpu_by_id[encoder_id]=row
            match=LABEL.fullmatch(row['encoder-label'])
            if not match:raise ValueError('unexpected target encoder label')
            frame,view,epoch,slot,generation=map(int,match.groups()[:5]);family=match.group(6)
            if frame not in complete:raise ValueError('encoder names an unobserved frame')
            identity=complete[frame]['identity']
            if (view,epoch,slot,generation)!=tuple(identity[key] for key in ('view','epoch','slot','generation')):
                raise ValueError('encoder frame identity does not match original record')
            key=(frame,family)
            if key in owned:raise ValueError('duplicate owned encoder family')
            owned[key]=encoder_id
            if command_id in commands and commands[command_id]!=frame:raise ValueError('command buffer assigned to multiple frames')
            commands[command_id]=frame
        stages=collections.defaultdict(list)
        for row in target_gpu:
            encoder_id=integer(row['encoder-id'])
            if encoder_id not in cpu_by_id:raise ValueError('target GPU interval has no CPU encoder identity')
            if row['cmdbuffer-id']!=cpu_by_id[encoder_id]['cmdbuffer-id']:raise ValueError('CPU/GPU command-buffer identity differs')
            if row['state']!='Active':raise ValueError('unexpected GPU interval state; scope requires inspection')
            interval(row)
            stages[encoder_id].append(row)
        pairs=[];frames=[];counter_all=[];trace_all=[];stage_gap=0
        for frame,record in sorted(complete.items()):
            frame_counter=[];frame_trace=[];frame_pairs=[];cb={}
            for family in ('scene','compute','compose','diagnostic-readback'):
                key=(frame,family)
                if key not in owned:raise ValueError('missing owned encoder family')
                encoder_id=owned[key];cb[family]=integer(cpu_by_id[encoder_id]['cmdbuffer-id'])
                rows=stages[encoder_id]
                native_family='readback' if family=='diagnostic-readback' else family
                ticks=record['passes'][native_family]
                channel_names={row['channel-name'] for row in rows}
                expected={'Vertex','Fragment'} if len(ticks)==4 else ({'Compute','Blit'} if family=='diagnostic-readback' else {'Compute'})
                if not channel_names or not channel_names<=expected or (len(ticks)==4 and channel_names!=expected):
                    raise ValueError('missing or unexpected encoder stage channel')
                groups=[('Vertex',rows if len(ticks)==2 else [r for r in rows if r['channel-name']=='Vertex'])]
                if len(ticks)==4:groups.append(('Fragment',[r for r in rows if r['channel-name']=='Fragment']))
                else:groups=[('Encoder',rows)]
                for index,(stage,stage_rows) in enumerate(groups):
                    parts=[interval(row) for row in stage_rows]
                    left,right=min(a for a,b in parts),max(b for a,b in parts)
                    cstart,cend=ticks[index*2:index*2+2]
                    gap=right-left-union_ns(parts);stage_gap+=gap
                    pair={'frame':frame,'family':family,'stage':stage,'encoder_id':encoder_id,
                          'counter':[cstart,cend],'trace_bounds':[left,right],'trace_rows':parts,
                          'trace_channels':sorted({r['channel-name'] for r in stage_rows}),
                          'trace_row_gap_ns':gap,'counter_minus_trace_duration_ns':cend-cstart-(right-left)}
                    pairs.append(pair);frame_pairs.append(pair)
                    if family!='diagnostic-readback':frame_counter.append((cstart,cend));frame_trace.extend(parts)
            if (cb['scene']==cb['compute'] or len({cb['compute'],cb['compose'],cb['diagnostic-readback']})!=1):
                raise ValueError('expected one scene and one dependent final command buffer per frame')
            counter_all.extend(frame_counter);trace_all.extend(frame_trace)
            frames.append({'frame':frame,'identity':record['identity'],'counter_stage_union_ns':union_ns(frame_counter),
                           'trace_stage_union_ns':union_ns(frame_trace),'counter_intervals':frame_counter,
                           'trace_intervals':merged(frame_trace)})
        # Fix the clock origin with only the first scene vertex boundary, then test every endpoint.
        anchor=next(pair for pair in pairs if pair['frame']==1 and pair['family']=='scene' and pair['stage']=='Vertex')
        offset=anchor['counter'][0]-anchor['trace_bounds'][0]
        residuals=[]
        for pair in pairs:
            pair['endpoint_residual_ns']=[counter-trace-offset for counter,trace in zip(pair['counter'],pair['trace_bounds'])]
            residuals.extend(pair['endpoint_residual_ns'])
        maximum=max(abs(value) for value in residuals)
        if maximum>1:errors.append('endpoint mismatch beyond declared1ns export-rounding tolerance')
        state_active=[];state_idle=[]
        for row in states:
            if row['gpu']!=device:continue
            if row['state']=='Active':state_active.append(interval(row))
            elif row['state']=='Idle':state_idle.append(interval(row))
            else:raise ValueError('unexpected global GPU state')
        global_trace=union_ns(trace_all)
        if intersect_ns(trace_all,state_active+state_idle)!=global_trace:
            errors.append('global state table does not cover all owned stage intervals')
        overlap_pairs=0
        for index,left in enumerate(frames):
            for right in frames[index+1:]:
                if intersect_ns(left['trace_intervals'],right['trace_intervals']):overlap_pairs+=1
        result.update(pid=pid,matched_encoders=len(owned),matched_stage_pairs=len(pairs),
                      gpu=device,target_gpu_rows=len(target_gpu),foreign_gpu_rows_same_device=len(foreign),
                      unattributed_gpu_rows_same_device=len(unattributed),
                      foreign_gpu_processes=dict(collections.Counter(row['process'] for row in foreign)),
                      foreign_gpu_overlap_inside_owned_stage_union_ns=intersect_ns(trace_all,[interval(row) for row in foreign]),
                      unattributed_gpu_overlap_inside_owned_stage_union_ns=intersect_ns(trace_all,[interval(row) for row in unattributed]),
                      counter_minus_trace_offset_ns=offset,maximum_endpoint_residual_ns=maximum,
                      global_counter_stage_union_ns=union_ns(counter_all),global_trace_stage_union_ns=global_trace,
                      sum_frame_counter_stage_union_ns=sum(f['counter_stage_union_ns'] for f in frames),
                      sum_frame_trace_stage_union_ns=sum(f['trace_stage_union_ns'] for f in frames),
                      cross_frame_trace_overlap_pairs=overlap_pairs,stage_row_gap_ns=stage_gap,
                      idle_inside_trace_stage_union_ns=intersect_ns(trace_all,state_idle),
                      active_inside_trace_stage_union_ns=intersect_ns(trace_all,state_active),
                      active_idle_overlap_inside_trace_stage_union_ns=intersect_ns(
                          trace_all,[(max(a,c),min(b,d)) for a,b in merged(state_active) for c,d in merged(state_idle) if max(a,c)<min(b,d)]),
                      frames=frames,stage_pairs=pairs)
    except (KeyError,TypeError,ValueError,StopIteration) as error:errors.append(str(error))
    result['valid']=not errors
    return result


class Tests(unittest.TestCase):
    def fixture(self):
        events=records.Tests().protocol()
        events[0]['pid']=1234
        cpu,gpu=[] ,[]
        for record in events:
            if record['kind']!='completed':continue
            identity=record['identity'];frame=identity['frame']
            prefix=f"stage-probe/frame={frame}/view=1/epoch={identity['epoch']}/slot={identity['slot']}/gen={identity['generation']}"
            for index,(family,ticks) in enumerate(record['passes'].items()):
                encoder=frame*10+index+1;command=frame*10+(0 if family=='scene' else 5)
                cpu.append({'process':'stage-probe (1234)','gpu':'M5 Max','encoder-id':str(encoder),
                            'cmdbuffer-id':str(command),'encoder-label':prefix+'/'+('diagnostic-readback' if family=='readback' else family)})
                channels=['Vertex','Fragment'] if len(ticks)==4 else ['Compute']
                for channel,(start,end) in zip(channels,zip(ticks[::2],ticks[1::2])):
                    gpu.append({'process':'stage-probe (1234)','gpu':'M5 Max','encoder-id':str(encoder),
                                'cmdbuffer-id':str(command),'channel-name':channel,
                                'start':str(start-1000),'duration':str(end-start),'state':'Active'})
        states=[{'start':'0','duration':'100000','state':'Active','gpu':'M5 Max'}]
        return events,cpu,gpu,states

    def test_complete_identity_scope_and_fixed_clock_offset(self):
        result=audit(*self.fixture())
        self.assertTrue(result['valid'],result['errors'])
        self.assertFalse(result['validated_for_governor'])
        self.assertEqual(result['matched_encoders'],128)
        self.assertEqual(result['matched_stage_pairs'],192)
        self.assertEqual(result['counter_minus_trace_offset_ns'],1000)
        self.assertEqual(result['maximum_endpoint_residual_ns'],0)
        self.assertEqual(result['global_counter_stage_union_ns'],32*130)
        self.assertEqual(result['global_trace_stage_union_ns'],32*130)
        self.assertEqual(result['idle_inside_trace_stage_union_ns'],0)

    def test_laundered_identity_missing_and_extra_encoders_fail(self):
        for mutation in ['epoch','duplicate','missing','foreign','command','unjoined']:
            e,c,g,s=self.fixture()
            if mutation=='epoch':c[0]['encoder-label']=c[0]['encoder-label'].replace('/epoch=1/','/epoch=9/')
            if mutation=='duplicate':c.append(copy.deepcopy(c[0]))
            if mutation=='missing':g.pop(0)
            if mutation=='foreign':c[0]['process']='stage-probe (9999)'
            if mutation=='command':g[0]['cmdbuffer-id']='9999'
            if mutation=='unjoined':g.append({**g[0],'encoder-id':'99999'})
            self.assertFalse(audit(e,c,g,s)['valid'],mutation)

    def test_endpoint_drift_and_inverted_intervals_fail(self):
        e,c,g,s=self.fixture();g[10]['start']=str(int(g[10]['start'])+100)
        self.assertFalse(audit(e,c,g,s)['valid'])
        e,c,g,s=self.fixture();g[0]['duration']='-1'
        self.assertFalse(audit(e,c,g,s)['valid'])

    def test_split_stage_gaps_and_idle_are_not_hidden_as_busy(self):
        e,c,g,s=self.fixture()
        row=g[0];start=int(row['start']);duration=int(row['duration'])
        row['duration']=str(duration//2-1)
        g.append({**row,'start':str(start+duration//2+1),'duration':str(duration-duration//2-1)})
        s.append({'start':str(start+2),'duration':'3','state':'Idle','gpu':'M5 Max'})
        result=audit(e,c,g,s)
        self.assertTrue(result['valid'],result['errors'])
        self.assertEqual(result['stage_row_gap_ns'],2)
        # The fragment interval covers one nanosecond of the vertex-stage gap.
        self.assertEqual(result['global_counter_stage_union_ns']-result['global_trace_stage_union_ns'],1)
        self.assertEqual(result['idle_inside_trace_stage_union_ns'],3)
        self.assertFalse(result['validated_for_governor'])

    def test_invalid_native_records_and_missing_state_coverage_fail(self):
        e,c,g,s=self.fixture();e[-1]['command_errors']=1
        self.assertFalse(audit(e,c,g,s)['valid'])
        e,c,g,s=self.fixture()
        self.assertFalse(audit(e,c,g,[])['valid'])

    def test_other_process_overlap_and_device_state_are_separate(self):
        e,c,g,s=self.fixture()
        extra={**g[0],'process':'another-app (888)','duration':'5'}
        g.extend([extra,copy.deepcopy(extra)])
        result=audit(e,c,g,s)
        self.assertTrue(result['valid'],result['errors'])
        self.assertEqual(result.get('foreign_gpu_rows_same_device'),2)
        self.assertEqual(result.get('foreign_gpu_overlap_inside_owned_stage_union_ns'),5)
        s[0]['gpu']='Different GPU'
        self.assertFalse(audit(e,c,g,s)['valid'])

    def test_xml_references_and_malformed_exports(self):
        xml='<trace-query-result><node><schema name="table"><col><mnemonic>key</mnemonic></col></schema><row><int id="1">42</int></row><row><int ref="1"/></row></node></trace-query-result>'
        with tempfile.TemporaryDirectory() as directory:
            path=Path(directory)/'table.xml';path.write_text(xml)
            self.assertEqual(read_table(path,'table'),[{'key':'42'},{'key':'42'}])
            for bad in [xml.replace('ref="1"','ref="9"'),xml.replace('<int id="1">42</int>','<int id="1" ref="1"/>'),xml.replace('<int ref="1"/>','<int ref="1"/><int>9</int>')]:
                path.write_text(bad)
                with self.assertRaises(ValueError):read_table(path,'table')


if __name__=='__main__':
    if sys.argv[1:]==['--self-test']:
        unittest.main(argv=[sys.argv[0]])
    else:
        parser=argparse.ArgumentParser(description=__doc__)
        for name in ('samples','encoders','gpu','states','out'):parser.add_argument('--'+name,required=True,type=Path)
        args=parser.parse_args()
        paths={key:getattr(args,key) for key in ('samples','encoders','gpu','states')}
        try:
            events=[json.loads(line) for line in args.samples.read_text().splitlines() if line]
            result=audit(events,read_table(args.encoders,'metal-application-encoders-list'),
                         read_table(args.gpu,'metal-gpu-intervals'),read_table(args.states,'metal-gpu-state-intervals'))
        except (ET.ParseError,KeyError,TypeError,ValueError,OSError) as error:
            result={'valid':False,'validated_for_governor':False,'errors':[str(error)]}
        result['inputs']={key:{'path':str(path),'sha256':hashlib.sha256(path.read_bytes()).hexdigest()} for key,path in paths.items() if path.is_file()}
        with args.out.open('x') as stream:json.dump(result,stream,indent=2);stream.write('\n')
        print(json.dumps({key:value for key,value in result.items() if key not in ('frames','stage_pairs','inputs')},indent=2))
        raise SystemExit(0 if result['valid'] else 1)
