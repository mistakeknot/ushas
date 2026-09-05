import copy
import importlib.util
import json
from pathlib import Path
import subprocess
import sys
import tempfile
import time
import unittest
from PIL import Image

import quality_runner as q


class QualityContract(unittest.TestCase):
    @staticmethod
    def radical(n,base):
        total=0.;denominator=1.
        while n:denominator*=base;total+=(n%base)/denominator;n//=base
        return total-.5

    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.output = Path(self.temp.name) / 'quality.json'
        self.directory = Path(str(self.output) + '.quality')
        self.directory.mkdir()
        self.report = {'kind': 'quality_sequence', 'protocol': 'claude-60hz-sampled-v1',
            'valid': True, 'offscreen': True, 'mode': 'Temporal', 'scale': 0.5,
            'output_size': [64,64], 'hdr': False, 'msaa_samples': 1,
            'scene_version': 'claude-toy-v1', 'expected_capture_count': 12, 'errors': [],
            'scripted_render_frames': [], 'captures': []}
        for tick in range(145):
            f = 1000 + tick
            shot = 2000 + tick if tick in dict(q.CAPTURES) else None
            proof = {'tick': tick, 'request_frame': f, 'extraction_frame': f, 'render_frame': f,
                'shot_entity': shot, 'extracted_shot_entity': shot, 'valid': True,
                'view_id': 9, 'image_target': 'Image(1)', 'target_matches': True,
                'msaa_samples': 1, 'main_texture_format': 'Rgba8UnormSrgb',
                'simulation_seconds': max(0,min(tick,127)-32)/60, 'jitter_index': tick%32,
                'jitter': [self.radical(tick%32+1,2),self.radical(tick%32+1,3)], 'world_from_view': list(range(16)),
                'expected_world_from_view':list(range(16)), 'camera_pose_matches':True,'format_matches':True,
                'reset_ordinal': 1 if tick<128 else 2,
                'reset_before_encode': tick in (0,128), 'reset_after_encode': False,
                'effect': {'frame_id': f, 'view_id': 9, 'requested_mode': 'Temporal',
                    'effective_mode': 'Temporal', 'scale': 0.5, 'content_size': [32,32],
                    'output_size': [64,64], 'state': 'OutputWritten'}}
            self.report['scripted_render_frames'].append(proof)
            if shot is not None:
                name = dict(q.CAPTURES)[tick]
                path = self.directory / (name+'.png')
                im = Image.new('RGBA',(64,64)); im.putdata([(n%256,n%251,n%241,255) for n in range(4096)]); im.save(path)
                scope = {'view_id':9,'image_target':'Image(1)','mode':'Temporal','scale':.5,'content_size':[32,32],'output_size':[64,64]}
                fence = {'frame_id':f,'qualified':True,'admitted_ms':float(tick),'callback_observed_ms':float(tick)+.5,
                    'scope':scope,'effect':{'frame_id':f,'scope':scope,'ready':True,'state':'OutputWritten'}}
                self.report['captures'].append({'tick':tick,'name':name,'path':str(path),'shot_entity':shot,
                    'request_frame':f,'readback_arrived_main_frame':f+1,'width':64,'height':64,'valid':True,
                    'image_proof':{'nonuniform':True,'opaque_fraction':1.0},'render_proof':copy.deepcopy(proof),'completion_proof':fence})

    def validate(self, report=None):
        return q.validate(self.report if report is None else report,self.output,'Temporal',.5,[64,64],False,1)

    def test_real_f32_nonbinary_scales_remain_valid(self):
        import struct,math
        for scale in (.58,2/3,1/3):
            actual=struct.unpack('f',struct.pack('f',scale))[0]
            report=copy.deepcopy(self.report);report['scale']=actual
            content=[math.floor(64*actual+.5)]*2
            for proof in report['scripted_render_frames']:
                proof['effect']['scale']=actual;proof['effect']['content_size']=content
            for capture in report['captures']:
                capture['render_proof']=copy.deepcopy(report['scripted_render_frames'][capture['tick']])
                fence=capture['completion_proof']
                for scope in (fence['scope'],fence['effect']['scope']):scope['scale']=actual;scope['content_size']=content
            self.assertEqual(q.validate(report,self.output,'Temporal',scale,[64,64],False,1),[],scale)

    def test_fences_must_be_ordered_and_have_typed_view_ids(self):
        for mutate in [lambda r:r['captures'][1]['completion_proof'].update(admitted_ms=0),lambda r:r['captures'][0]['completion_proof'].update(failure='failed'),lambda r:r['captures'][0]['completion_proof']['scope'].update(view_id=9.0)]:
            report=copy.deepcopy(self.report);mutate(report)
            self.assertTrue(self.validate(report))

    def test_complete_sequence_is_accepted(self):
        self.assertEqual(self.validate(), [])

    def test_missing_duplicate_or_wrong_timeline_never_passes(self):
        for mutate in [lambda r:r['captures'].pop(),lambda r:r['captures'].__setitem__(1,r['captures'][0]),lambda r:r['scripted_render_frames'].pop(5)]:
            report = copy.deepcopy(self.report); mutate(report)
            self.assertTrue(self.validate(report))

    def test_request_render_readback_identity_and_reset_must_match(self):
        for key,value in [('request_frame',0),('shot_entity',999),('render_proof',{}),('completion_proof',{}),('readback_arrived_main_frame',0)]:
            report=copy.deepcopy(self.report); report['captures'][0][key]=value
            self.assertTrue(self.validate(report), key)
        for key,value in [('effect',{}),('extraction_frame',0),('msaa_samples',4),('reset_before_encode',False),('reset_after_encode',True)]:
            report=copy.deepcopy(self.report); report['scripted_render_frames'][128][key]=value
            self.assertTrue(self.validate(report),key)

    def test_typed_ids_and_incorrect_arm_are_rejected(self):
        for key,value in [('mode','Disabled'),('scale',.75),('hdr',True),('expected_capture_count',12.0)]:
            report=copy.deepcopy(self.report); report[key]=value
            self.assertTrue(self.validate(report),key)
        report=copy.deepcopy(self.report); report['scripted_render_frames'][1]['render_frame']=1001.0
        self.assertTrue(self.validate(report))

    def test_actual_zero_alpha_png_fails_even_if_report_claims_opaque(self):
        path=Path(self.report['captures'][0]['path']); im=Image.open(path).convert('RGBA');im.putalpha(0);im.save(path)
        self.assertTrue(self.validate())

    def test_paths_cannot_escape_the_predeclared_capture_inventory(self):
        report=copy.deepcopy(self.report);report['captures'][0]['path']=str(self.output)
        self.assertTrue(self.validate(report))

    def test_normal_runner_cannot_accept_quality_report_as_two_image_smoke(self):
        spec=importlib.util.spec_from_file_location('normal_runner',Path(__file__).with_name('run.py'))
        normal=importlib.util.module_from_spec(spec);spec.loader.exec_module(normal)
        self.output.write_text(json.dumps(self.report))
        errors=normal.check_report(self.output,self.output.with_suffix('.png'),None,{}, {})
        self.assertIn('missing or mismatched screenshot',errors)
        self.assertIn('missing or mismatched warmup_screenshot',errors)

    def test_bounded_timeout_terminates_child(self):
        pidfile=Path(self.temp.name)/'pid'
        command=[sys.executable,'-c',f'import os,time;open({str(pidfile)!r},"w").write(str(os.getpid()));time.sleep(30)']
        with (Path(self.temp.name)/'log').open('w') as log:
            self.assertEqual(q.bounded(command,log,.3),124)
        import os
        with self.assertRaises(ProcessLookupError):os.kill(int(pidfile.read_text()),0)

    def test_sigterm_reaps_the_owned_process_group(self):
        import os,signal
        pidfile=Path(self.temp.name)/'term-pid'
        wrapper=Path(self.temp.name)/'wrapper.py'
        wrapper.write_text('import sys\nsys.path.insert(0,'+repr(str(Path(q.__file__).parent))+')\nimport quality_runner as q\nwith open('+repr(str(Path(self.temp.name)/'term-log'))+',"w") as log:\n raise SystemExit(q.bounded([sys.executable,"-c",'+repr('import os,time;open('+repr(str(pidfile))+',"w").write(str(os.getpid()));time.sleep(30)')+'],log,20))\n')
        child=subprocess.Popen([sys.executable,str(wrapper)])
        self.addCleanup(lambda: child.kill() if child.poll() is None else None)
        deadline=time.monotonic()+3
        while not pidfile.exists() and time.monotonic()<deadline:time.sleep(.01)
        self.assertTrue(pidfile.exists())
        child.send_signal(signal.SIGTERM)
        self.assertEqual(child.wait(timeout=5),143)
        with self.assertRaises(ProcessLookupError):os.kill(int(pidfile.read_text()),0)

    def test_malformed_successful_child_cannot_leave_a_valid_manifest(self):
        report=copy.deepcopy(self.report);report['captures'][0]['image_proof']=None
        fixture=Path(self.temp.name)/'malformed.json';fixture.write_text(json.dumps(report))
        binary=Path(self.temp.name)/'malformed-child'
        binary.write_text('#!'+sys.executable+'\nimport pathlib,sys\npathlib.Path(sys.argv[sys.argv.index("--out")+1]).write_bytes(pathlib.Path('+repr(str(fixture))+').read_bytes())\n')
        binary.chmod(0o700)
        output=Path(self.temp.name)/'malformed-result.json'
        child=subprocess.run([sys.executable,str(Path(q.__file__)), '--binary',str(binary),'--out',str(output),'--mode','temporal','--scale','.5','--width','64','--height','64'],capture_output=True,text=True)
        self.assertNotEqual(child.returncode,0)
        receipt=json.loads(Path(str(output)+'.quality-manifest.json').read_text())
        self.assertEqual(receipt['child_exit'],0, receipt)
        self.assertIs(receipt['valid'],False)

    def test_cli_retains_failed_run_and_refuses_existing_artifacts(self):
        binary=Path(self.temp.name)/'fake';binary.write_text('#!/bin/sh\nexit 0\n');binary.chmod(0o700)
        output=Path(self.temp.name)/'new.json'
        command=[sys.executable,str(Path(q.__file__)), '--binary',str(binary),'--out',str(output),'--mode','temporal','--scale','.5']
        child=subprocess.run(command,capture_output=True,text=True)
        self.assertNotEqual(child.returncode,0)
        manifest=Path(str(output)+'.quality-manifest.json')
        self.assertTrue(manifest.is_file())
        receipt=json.loads(manifest.read_text())
        self.assertFalse(receipt['valid']);self.assertEqual(receipt['child_exit'],0)
        original=manifest.read_bytes()
        child=subprocess.run(command,capture_output=True,text=True)
        self.assertNotEqual(child.returncode,0);self.assertEqual(manifest.read_bytes(),original)


if __name__=='__main__':unittest.main()
