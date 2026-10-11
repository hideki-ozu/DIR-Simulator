"""Measure the public mailbox-capacity comparison and isolated ZIP reproduction."""
import argparse
import hashlib
import json
from pathlib import Path
import subprocess
import zipfile

ROOT = Path(__file__).resolve().parents[1]
SOURCE = ROOT / 'examples/guide/mailbox-capacity'

def measure(binary, source, output):
    output.mkdir(parents=True)
    values, provenance, counts = {}, {}, {}
    for capacity in (1, 2, 3):
        name = f'capacity{capacity}'
        result = output / name
        for args in [
            ['validate', '--config', str(source / (name + '.ini'))],
            ['run', '--config', str(source / (name + '.ini')), '--output', str(result)],
            ['view', '--input', str(result / 'results.json'), '--output', str(output / (name + '-viewer.html'))],
        ]:
            run = subprocess.run([str(binary), *args], capture_output=True, text=True)
            assert run.returncode == 0, run.stderr
            parsed = json.loads(run.stdout)
            assert parsed.get('status') in ('valid', 'complete') or parsed.get('exit_code') == 0
        manifest = json.loads((result / 'manifest.json').read_text())
        assert manifest['status'] == 'complete' and not manifest['partial']
        for entry in manifest['files']:
            raw = (result / entry['name']).read_bytes()
            assert str(len(raw)) == entry['bytes']
            assert hashlib.sha256(raw).hexdigest() == entry['sha256']
        document = json.loads((result / 'results.json').read_text())
        meta, sim = document['metadata'], document['simulation']
        assert meta['git_commit'] == 'b45515644dfc65c205216d564d5bf7ef00280622'
        assert meta['git_dirty'] in (False, 'false', 'clean')
        assert not sim['partial'] and sim['termination'] == 'events_exhausted'
        assert sim['end_ps'] == '1000' and sim['last_event_time_ps'] == '900'
        requests = {r['request_id']: r['data'] for r in sim['model_records'] if r['schema_name'] == 'memory-ipc.request'}
        assert len(requests) == 6
        for i, hexbytes in enumerate(['01020304','11121314','21222324']):
            send_id = 'send_' + 'abc'[i] + ':0'
            send, receive = requests[send_id], requests[f'receive:{i}']
            assert send['status'] == receive['status'] == 'completed'
            assert send['generated_ps'] == '0'
            assert send['started_ps'] == str(i * 100)
            assert send['completed_ps'] == str((i + 1) * 100)
            assert send['input_hex'] == hexbytes
            assert send['reason'] == ('ok' if i < capacity else 'full')
            assert send['message_id'] == (send_id if i < capacity else None)
            assert receive['generated_ps'] == receive['started_ps'] == str(400 + i * 200)
            assert receive['completed_ps'] == str(500 + i * 200)
            assert receive['reason'] == ('ok' if i < capacity else 'empty')
            assert receive['message_id'] == (send_id if i < capacity else None)
            assert receive['output_hex'] == (hexbytes if i < capacity else None)
        notifications = [r['data'] for r in sim['model_records'] if r['schema_name'] == 'memory-ipc.notification']
        assert len(notifications) == capacity
        for i, n in enumerate(sorted(notifications, key=lambda x:x['message_id'])):
            assert n['message_id'] == 'send_' + 'abc'[i] + ':0'
            assert n['enqueued_ps'] == str((i + 1) * 100)
            assert n['planned_ps'] == n['delivered_ps'] == str((i + 1) * 100 + 50)
            assert n['receivers'] == ['consumer']
        mailbox = [r for r in sim['model_records'] if r['schema_name'] == 'memory-ipc.mailbox']
        assert len(mailbox) == 1 and mailbox[0]['data'] == {'active':None,'messages':[],'queue':[]}
        summary = {r['metric']: r['value'] for r in sim['summary']}
        assert summary['memory_ipc.offered'] == summary['memory_ipc.completed'] == '6'
        assert summary['memory_ipc.rejected'] == '0'
        assert summary['memory_ipc.notifications'] == str(capacity)
        assert summary['memory_ipc.committed_bytes'] == str(capacity * 4)
        assert summary['memory_ipc.busy_ratio'] == 0.6 and summary['memory_ipc.queue_mean'] == 0.3
        counts[name] = {'send_ok':capacity,'send_full':3-capacity,'receive_ok':capacity,'receive_empty':3-capacity,'notifications':capacity}
        values[name] = sim
        provenance[name] = {k: meta[k] for k in ('git_commit','git_dirty','binary_sha256','build_source_sha256','cargo_lock_sha256')}
        provenance[name]['input_sha256'] = {s['logical_path']: s['sha256'] for s in meta['sources']}
    return values, provenance, counts

def main():
    p = argparse.ArgumentParser(description=__doc__)
    p.add_argument('--binary', type=Path, required=True)
    p.add_argument('--output', type=Path, required=True)
    args = p.parse_args()
    output = args.output.resolve()
    output.mkdir(parents=True, exist_ok=False)
    base = json.loads((SOURCE / 'capacity1.json').read_text())
    base['mailboxes'][0].pop('capacity')
    for capacity in (1,2,3):
        name = f'capacity{capacity}'
        value = json.loads((SOURCE / (name + '.json')).read_text())
        assert value['mailboxes'][0].pop('capacity') == capacity
        assert value == base
        assert (SOURCE / (name + '.ini')).read_text().replace(name+'.json','MODEL.json') == (SOURCE/'capacity1.ini').read_text().replace('capacity1.json','MODEL.json')
    public, provenance, counts = measure(args.binary.resolve(), SOURCE, output/'repository')
    with zipfile.ZipFile(ROOT/'docs/guide/downloads/mailbox-capacity-inputs.zip') as z:
        expected = {p.relative_to(ROOT).as_posix():p.read_bytes() for p in SOURCE.rglob('*') if p.is_file()}
        assert set(z.namelist()) == set(expected)
        for name, raw in expected.items():
            assert z.read(name) == raw
        z.extractall(output/'isolated')
    zipped, zip_provenance, zip_counts = measure(args.binary.resolve(), output/'isolated/examples/guide/mailbox-capacity',output/'zip')
    assert public == zipped and counts == zip_counts
    evidence = {'status':'passed','source_tag':'v1.1.4','source_commit':'b45515644dfc65c205216d564d5bf7ef00280622','cli_operations':18,'zip_input_count':len(expected),'single_variable':'mailboxes[0].capacity','zip_reproduction':'all simulation objects identical','derived_counts':counts,'conditions':public,'provenance':provenance,'zip_provenance':zip_provenance}
    destination = ROOT/'docs/verification/results/mailbox-capacity'
    destination.mkdir(parents=True,exist_ok=True)
    raw = json.dumps(evidence,ensure_ascii=False,indent=2)+'\n'
    (destination/'measurements.json').write_text(raw)
    (output/'measurements.json').write_text(raw)
    print('PASS: 18 CLI operations, three capacities, full/empty/FIFO/notification, hashes and isolated ZIP reproduction')

if __name__ == '__main__':
    main()
