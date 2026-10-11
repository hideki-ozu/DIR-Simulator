"""Check the same-time send FIFO boundary with only receive times changed."""
import argparse
import hashlib
import json
from pathlib import Path
import shutil
import subprocess

ROOT = Path(__file__).resolve().parents[1]
SOURCE = ROOT / 'examples/guide/mailbox-capacity'

def main():
    p = argparse.ArgumentParser(description=__doc__)
    p.add_argument('--binary', type=Path, required=True)
    p.add_argument('--output', type=Path, required=True)
    args = p.parse_args()
    output = args.output.resolve()
    output.mkdir(parents=True,exist_ok=False)
    report = {'source_tag':'v1.1.4','single_variable':'receive generator times','cli_operations':6,'conditions':{}}
    workload = json.loads((SOURCE/'workload.json').read_text())
    for name, times in [('original',['400ps','600ps','800ps']),('earlier',['50ps','250ps','450ps']),('zero',['0ps','100ps','200ps'])]:
        inputs = output/name/'inputs'
        shutil.copytree(SOURCE,inputs)
        changed = json.loads(json.dumps(workload))
        receive = next(g for g in changed['generators'] if g['id']=='receive')
        receive['times'] = times
        (inputs/'workload.json').write_text(json.dumps(changed,indent=2)+'\n')
        expected = json.loads(json.dumps(workload))
        next(g for g in expected['generators'] if g['id']=='receive')['times'] = times
        assert changed == expected
        result = output/name/'result'
        for command in [['validate','--config',str(inputs/'capacity1.ini')],['run','--config',str(inputs/'capacity1.ini'),'--output',str(result)]]:
            run = subprocess.run([str(args.binary.resolve()),*command],capture_output=True,text=True)
            assert run.returncode == 0,run.stderr
        document = json.loads((result/'results.json').read_text())
        meta,sim = document['metadata'],document['simulation']
        assert meta['git_commit']=='b45515644dfc65c205216d564d5bf7ef00280622'
        assert meta['git_dirty'] in (False,'false','clean')
        records = [r for r in sim['model_records'] if r['schema_name']=='memory-ipc.request']
        sends = [r['data'] for r in records if r['data']['op']=='send']
        receivers = [r['data'] for r in records if r['data']['op']=='receive']
        assert len(sends)==len(receivers)==3
        assert all(d['generated_ps']=='0' for d in sends)
        for i, time in enumerate(times):
            d=next(r['data'] for r in records if r['request_id']==f'receive:{i}')
            assert d['generated_ps']==time.removesuffix('ps')
        assert sum(d['reason']=='ok' for d in sends)==1
        assert sum(d['reason']=='full' for d in sends)==2
        assert all(d['status']=='completed' for d in sends+receivers)
        assert sum(d['reason']=='ok' for d in receivers)==1
        assert sum(d['reason']=='empty' for d in receivers)==2
        summary={r['metric']:r['value'] for r in sim['summary']}
        assert summary['memory_ipc.rejected']=='0' and summary['memory_ipc.completed']=='6'
        assert not sim['partial'] and sim['termination']=='events_exhausted'
        if name=='zero':
            first=next(r['data'] for r in records if r['request_id']=='receive:0')
            assert first['started_ps']=='0' and first['reason']=='empty'
        report['conditions'][name]={'receive_times':times,'send_ok':1,'send_full':2,'receive_ok':1,'receive_empty':2,'model_records':sim['model_records'],'summary':sim['summary'],'provenance':{k:meta[k] for k in ('git_commit','git_dirty','binary_sha256','build_source_sha256')},'workload_sha256':hashlib.sha256((inputs/'workload.json').read_bytes()).hexdigest()}
    report['status']='passed'
    raw=json.dumps(report,ensure_ascii=False,indent=2)+'\n'
    (output/'receive-timing.json').write_text(raw)
    (ROOT/'docs/verification/results/mailbox-capacity/receive-timing.json').write_text(raw)
    print('PASS: 6 CLI operations; earlier/zero receives do not increase successful sends with same-time send FIFO')

if __name__=='__main__':
    main()
