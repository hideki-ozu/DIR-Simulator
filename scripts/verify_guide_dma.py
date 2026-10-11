"""Measure DMA data completion versus notification, including isolated ZIP inputs."""
import argparse
import hashlib
import json
from pathlib import Path
import subprocess
import zipfile

ROOT = Path(__file__).resolve().parents[1]
SOURCE = ROOT / 'examples/guide/dma-notification'
CASES = (50,200,600)

def measure(binary,source,output):
    output.mkdir(parents=True)
    values,provenance={},{}
    for delay in CASES:
        name=f'notify{delay}'
        result=output/name
        for command in [
            ['validate','--config',str(source/(name+'.ini'))],
            ['run','--config',str(source/(name+'.ini')),'--output',str(result)],
            ['view','--input',str(result/'results.json'),'--output',str(output/(name+'-viewer.html'))],
        ]:
            run=subprocess.run([str(binary),*command],capture_output=True,text=True)
            assert run.returncode==0,run.stderr
            parsed=json.loads(run.stdout)
            assert parsed.get('status') in ('valid','complete') or parsed.get('exit_code')==0
        manifest=json.loads((result/'manifest.json').read_text())
        assert manifest['status']=='complete' and not manifest['partial']
        for entry in manifest['files']:
            raw=(result/entry['name']).read_bytes()
            assert str(len(raw))==entry['bytes']
            assert hashlib.sha256(raw).hexdigest()==entry['sha256']
        document=json.loads((result/'results.json').read_text())
        meta,sim=document['metadata'],document['simulation']
        assert meta['git_commit']=='b45515644dfc65c205216d564d5bf7ef00280622'
        assert meta['git_dirty'] in (False,'false','clean')
        assert not sim['partial'] and sim['end_ps']=='1000'
        assert sim['last_event_time_ps']=='800'
        assert sim['termination']==('time_limit' if delay==600 else 'events_exhausted')
        assert sim['pending_events']==('1' if delay==600 else '0')
        records={r['request_id']:r for r in sim['model_records'] if r['schema_name']=='memory-ipc.request'}
        assert len(records)==6
        parent=records['copy:0']['data']
        assert parent['model']=='dma' and parent['op']=='copy'
        assert parent['generated_ps']==parent['started_ps']=='0'
        assert parent['committed_bytes']=='8' and parent['data_done_ps']=='600'
        assert parent['planned_notify_ps']==str(600+delay)
        assert parent['status']==('notifying' if delay==600 else 'completed')
        assert parent['reason']==(None if delay==600 else 'ok')
        assert parent['notified_ps']==parent['completed_ps']==(None if delay==600 else str(600+delay))
        for suffix,target,op,address,start,end,hexbytes in [
            ('r/0','Main.src','read',0,100,200,'01020304'),
            ('w/0','Main.dst','write',0,200,350,'01020304'),
            ('r/1','Main.src','read',4,350,450,'05060708'),
            ('w/1','Main.dst','write',4,450,600,'05060708'),
        ]:
            r=records['copy:0/'+suffix];d=r['data']
            assert r['subject']==target and r['origin_request_id']=='copy:0'
            assert d['op']==op and d['address']==str(address) and d['length']=='4'
            assert d['generated_ps']==d['started_ps']==str(start)
            assert d['completed_ps']==str(end) and d['status']=='completed' and d['reason']=='ok'
            assert d['output_hex' if op=='read' else 'input_hex']==hexbytes
        observer=records['observe:0']['data']
        assert observer['generated_ps']==observer['started_ps']=='700'
        assert observer['completed_ps']=='800' and observer['status']=='completed' and observer['reason']=='ok'
        assert observer['output_hex']=='0102030405060708'
        memory={r['subject']:r['data'] for r in sim['model_records'] if r['schema_name']=='memory-ipc.memory'}
        assert memory['Main.src']['hex']=='0102030405060708090a0b0c0d0e0f10'
        assert memory['Main.dst']['hex']=='01020304050607080000000000000000'
        assert all(d['ports']==[None] and d['queue']==[] for d in memory.values())
        engine=next(r['data'] for r in sim['model_records'] if r['schema_name']=='memory-ipc.dma')
        assert engine['queue']==[] and engine['active']==('copy:0' if delay==600 else None)
        summary={(r['target'],r['metric']):r['value'] for r in sim['summary']}
        for target,count in [('Main.src',2),('Main.dst',3),('Main.dma',1)]:
            assert summary[target,'memory_ipc.offered']==str(count)
            assert summary[target,'memory_ipc.rejected']=='0'
            expected=0 if target=='Main.dma' and delay==600 else count
            assert summary[target,'memory_ipc.completed']==str(expected)
        assert summary['Main.dma','memory_ipc.notifications']==('0' if delay==600 else '1')
        assert summary['Main.dma','memory_ipc.committed_bytes']==summary['Main.dst','memory_ipc.committed_bytes']=='8'
        assert abs(summary['Main.dma','memory_ipc.busy_ratio']-(1.0 if delay==600 else (600+delay)/1000))<1e-12
        values[name]=sim
        provenance[name]={k:meta[k] for k in ('git_commit','git_dirty','binary_sha256','build_source_sha256','cargo_lock_sha256')}
        provenance[name]['input_sha256']={s['logical_path']:s['sha256'] for s in meta['sources']}
    # Changing notification must leave every SRAM request and memory snapshot identical.
    def memory_projection(sim):
        return [r for r in sim['model_records'] if r['schema_name']=='memory-ipc.memory' or (r['schema_name']=='memory-ipc.request' and r['data']['model']=='sram')]
    assert all(memory_projection(sim)==memory_projection(values['notify50']) for sim in values.values())
    return values,provenance

def main():
    p=argparse.ArgumentParser(description=__doc__)
    p.add_argument('--binary',type=Path,required=True)
    p.add_argument('--output',type=Path,required=True)
    args=p.parse_args()
    output=args.output.resolve();output.mkdir(parents=True,exist_ok=False)
    base=json.loads((SOURCE/'notify50.json').read_text());base['dma'][0].pop('notify_ps')
    for delay in CASES:
        name=f'notify{delay}';value=json.loads((SOURCE/(name+'.json')).read_text())
        assert value['dma'][0].pop('notify_ps')==str(delay) and value==base
        assert (SOURCE/(name+'.ini')).read_text().replace(name+'.json','MODEL.json')==(SOURCE/'notify50.ini').read_text().replace('notify50.json','MODEL.json')
    public,provenance=measure(args.binary.resolve(),SOURCE,output/'repository')
    with zipfile.ZipFile(ROOT/'docs/guide/downloads/dma-notification-inputs.zip') as z:
        expected={p.relative_to(ROOT).as_posix():p.read_bytes() for p in SOURCE.rglob('*') if p.is_file()}
        assert set(z.namelist())==set(expected)
        for name,raw in expected.items():assert z.read(name)==raw
        z.extractall(output/'isolated')
    zipped,zip_provenance=measure(args.binary.resolve(),output/'isolated/examples/guide/dma-notification',output/'zip')
    assert public==zipped
    evidence={'status':'passed','source_tag':'v1.1.4','source_commit':'b45515644dfc65c205216d564d5bf7ef00280622','cli_operations':18,'zip_input_count':len(expected),'single_variable':'dma[0].notify_ps','zip_reproduction':'all simulation objects identical','memory_projection':'all SRAM requests and snapshots invariant','conditions':public,'provenance':provenance,'zip_provenance':zip_provenance}
    destination=ROOT/'docs/verification/results/dma-notification';destination.mkdir(parents=True,exist_ok=True)
    raw=json.dumps(evidence,ensure_ascii=False,indent=2)+'\n'
    (destination/'measurements.json').write_text(raw);(output/'measurements.json').write_text(raw)
    print('PASS: 18 CLI operations; DMA child bytes/times, observer read, notified versus pending, manifest and isolated ZIP')

if __name__=='__main__':main()
