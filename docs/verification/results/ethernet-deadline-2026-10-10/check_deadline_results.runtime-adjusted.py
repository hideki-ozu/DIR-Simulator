"""Read-only checks for the three deadline runs; never simulate or fabricate results.

Only --report writes a new checker report. Viewer capture and visual checks remain
separate, and this checker cannot establish them. JSON numbers use exact Decimal.
"""
import argparse
import copy
from decimal import Decimal
import hashlib
import json
from pathlib import Path
import re

SOURCE = 'b45515644dfc65c205216d564d5bf7ef00280622'
DEADLINES = {'short': 1155999, 'equal': 1156000, 'long': 1156001}
PROFILE = 'ethernet.l2.qos.v1'


def require(condition, message):
    if not condition:
        raise ValueError(message)


def wire_integer(value):
    require(isinstance(value, str) and re.fullmatch(r'0|[1-9][0-9]*', value),
            'expected canonical nonnegative decimal string: ' + repr(value))
    return int(value)


def digest(data):
    return hashlib.sha256(data).hexdigest()


def safe_file(root, relative):
    require(isinstance(relative, str), 'file path must be a string')
    path = (root / relative).resolve()
    require(path.is_relative_to(root.resolve()), 'file escapes evidence root: ' + relative)
    require(path.is_file(), 'missing file: ' + str(path))
    return path


def load_json(path):
    return json.loads(path.read_text(encoding='utf-8'), parse_float=Decimal)


def check_inputs(root):
    files = {str(p.relative_to(root)).replace('\\', '/'): p.read_bytes()
             for p in root.rglob('*') if p.is_file()}
    require(set(files) == {'short.ini', 'equal.ini', 'long.ini', 'short.json',
                         'equal.json', 'long.json', 'model-qos.json', 'models/ethdemo/Main.ned'},
            'expected exactly eight prepared input files')
    normalized = []
    for name, deadline in DEADLINES.items():
        value = json.loads(files[name + '.json'].decode('utf-8'))
        require(value['schema_version'] == 2 and len(value['generators']) == 1, 'single generator/schema')
        g = value['generators'][0]
        require(g == dict(id='packet', node='Main.a', kind='ethernet.explicit.v1', times_ps=['0'],
                         frame=dict(dst_mac='02:00:00:00:00:02', ether_type=2048, data=''),
                         flow_id='flow_test', priority=0, deadline_ps=str(deadline)), 'generator differs: ' + name)
        value['generators'][0]['deadline_ps'] = '__condition__'
        normalized.append(value)
        ini = files[name + '.ini'].decode('utf-8')
        require('sim-time-limit = 3000000ps' in ini and PROFILE in ini, 'profile/time limit')
        require(ini.replace('workload = "' + name + '.json"', 'workload = "equal.json"')
                == files['equal.ini'].decode('utf-8'), 'INI differs beyond workload name')
    require(normalized[0] == normalized[1] == normalized[2], 'one-variable comparison')
    return files


def one(rows, **fields):
    matches = [r for r in rows if all(r.get(k) == v for k, v in fields.items())]
    require(len(matches) == 1, 'expected one row: ' + repr(fields))
    return matches[0]


def metric(rows, target, name):
    return one(rows, target=target, metric=name)


def check_case(root, name, deadline, inputs, execution):
    folder = root / ('deadline-' + name)
    manifest_path = safe_file(folder, 'manifest.json')
    manifest = load_json(manifest_path)
    require(manifest['metadata_ref'] == 'results.json#/metadata', 'manifest metadata reference')
    entries = []
    require(len({e['name'] for e in manifest['files']}) == len(manifest['files']), 'duplicate manifest members')
    for entry in manifest['files']:
        data = safe_file(folder, entry['name']).read_bytes()
        require(len(data) == wire_integer(entry['bytes']) and digest(data) == entry['sha256'],
                'manifest mismatch: ' + entry['name'])
        entries.append(dict(name=entry['name'], bytes=len(data), sha256=digest(data)))
    result_path = safe_file(folder, 'results.json')
    result = load_json(result_path)
    require(result['schema_version'] == 2, 'results schema')
    meta = result['metadata']
    require(meta['model_profile'] == PROFILE and meta['runtime_version'] == '0.1.0', 'runtime/profile')
    require(meta['git_commit'] == SOURCE, 'measured source is not requested b455156')
    require(meta['binary_sha256'] == execution['binary']['sha256'], 'binary identity mismatch')
    require(meta['compiler'].startswith('rustc 1.85.0 '), 'compiler differs')
    expected = {name + '.ini', name + '.json', 'model-qos.json', 'models/ethdemo/Main.ned'}
    sources = meta['sources']
    require(len(sources) == 4, 'expected four captured source snapshots')
    seen = set()
    for source in sources:
        data = source['content_utf8'].encode('utf-8')
        require(digest(data) == source['sha256'], 'snapshot self hash')
        matches = [p for p in expected if inputs[p] == data]
        require(len(matches) == 1, 'snapshot does not exactly match prepared condition input')
        seen.add(matches[0])
    require(seen == expected, 'snapshot coverage')
    # These canonical-hash inputs contain strings only; no floating JSON values.
    canonical = lambda v: json.dumps(v, ensure_ascii=False, sort_keys=True, separators=(',', ':')).encode('utf-8')
    source_hashes = [dict(logical_path=s['logical_path'], sha256=s['sha256']) for s in sources]
    require(digest(canonical(source_hashes)) == meta['input_sha256'], 'aggregate input hash')
    require(digest(canonical(meta['config'])) == meta['config_sha256'], 'effective config hash')
    sim = result['simulation']
    require(sim['termination'] == 'events_exhausted' and sim['partial'] is False,
            'simulation must complete without partial state')
    require(wire_integer(sim['pending_events']) == 0 and wire_integer(sim['end_ps']) == 3000000, 'end/pending')
    rows = sim['model_records']
    require(len(rows) == 5, 'frame1/transfer2/reception2')
    frame = one(rows, schema_name='ethernet.frame', record_id='packet:0')
    require(frame['schema_version'] == 2 and frame['subject'] == 'Main.a', 'frame envelope')
    fd = frame['data']
    require(fd['flow_id'] == 'flow_test' and fd['priority'] == '0' and fd['deadline_ps'] == str(deadline), 'frame QoS')
    require(fd['generated_ps'] == '0' and fd['ready_ps'] == '0' and fd['mac_bytes'] == '64'
            and fd['data_hex'] == '' and fd['dst_mac'] == '02:00:00:00:00:02', 'frame fixed input')
    transfers = [r for r in rows if r['schema_name'] == 'ethernet.transfer']
    require(len(transfers) == 2, 'transfer count')
    a = one(transfers, subject='Main.a.tx')
    sw = one(transfers, subject='Main.sw.tx_b')
    for row, milestones, peer in [(a, [0, 0, 576000, 672000, 577000], 'Main.sw.rx_a'),
                                   (sw, [579000, 579000, 1155000, 1251000, 1156000], 'Main.b.rx')]:
        d = row['data']
        require(row['schema_version'] == 2 and d['frame_id'] == 'packet:0' and d['priority'] == '0', 'transfer QoS')
        require(isinstance(d['queue_id'], str) and d['queue_id'], 'queue id missing')
        require(d['status'] == 'serialized' and d['drop_reason'] is None and d['to_port'] == peer, 'transfer state/path')
        require([wire_integer(d[k]) for k in ['queued_ps', 'sof_ps', 'eof_ps', 'release_ps', 'arrival_ps']]
                == milestones, 'actual transfer milestones')
    require(a['data']['parent_transfer_id'] is None and sw['data']['parent_transfer_id'] == a['record_id'], 'transfer ancestry')
    receptions = [r for r in rows if r['schema_name'] == 'ethernet.reception']
    require(len(receptions) == 2, 'reception count')
    for subject, status, observed, ready, transfer in [('Main.sw', 'forwarded', 577000, 579000, a),
                                                     ('Main.b', 'received', 1156000, 1156000, sw)]:
        row = one(receptions, subject=subject)
        d = row['data']
        require(row['schema_version'] == 1 and d['frame_id'] == 'packet:0' and d['transfer_id'] == transfer['record_id'], 'reception envelope/ancestry')
        require(d['status'] == status and d['reason'] is None, 'completed reception status')
        require(wire_integer(d['observed_ps']) == observed and wire_integer(d['ready_ps']) == ready, 'actual received/processed time')
    summary = sim['summary']
    for key, value in dict(generated=1, transfer_offered=2, serialized=2, forwarded=1, received=1, dropped=0, filtered=0).items():
        require(wire_integer(metric(summary, '$all', 'ethernet.' + key)['value']) == value, 'global count ' + key)
    missed = int(name == 'short')
    for target in ['@flow:flow_test', '@flow:flow_test:Main.b']:
        for key, value in dict(received=1, deadline_sample_count=1, deadline_missed=missed).items():
            require(wire_integer(metric(summary, target, 'ethernet.flow.' + key)['value']) == value, 'flow count ' + key)
        for key, value in dict(deadline_miss_ratio=missed, delivery_mean_ps=1156000,
                               queue_wait_mean_ps=0, serialization_mean_ps=1152000,
                               propagation_mean_ps=2000, processing_mean_ps=2000).items():
            row = metric(summary, target, 'ethernet.flow.' + key)
            require(Decimal(str(row['value'])) == Decimal(value) and row['sample_count'] == '1', 'flow numerical metric ' + key)
    for key, value in dict(delivery_ps=1156000, queue_wait_ps=0, serialization_ps=1152000,
                           propagation_ps=2000, processing_ps=2000).items():
        row = one(sim['records'], metric='ethernet.' + key, target='Main.b', request_id='packet:0', receiver='Main.b')
        require(wire_integer(row['value']) == value and row['time_ps'] == '1156000', 'delivery component ' + key)
    normalized = copy.deepcopy(sim)
    one(normalized['model_records'], schema_name='ethernet.frame')['data']['deadline_ps'] = '__deadline__'
    for row in normalized['summary'] + normalized['records']:
        if row['metric'] in ['ethernet.flow.deadline_missed', 'ethernet.flow.deadline_miss_ratio']:
            row['value'] = '__deadline_evaluation__'
    viewer = safe_file(root, 'deadline-' + name + '-viewer.html').read_bytes()
    require(len(viewer) > 0, 'empty Viewer HTML')
    return normalized, dict(condition=name, results_sha256=digest(result_path.read_bytes()),
                            manifest_sha256=digest(manifest_path.read_bytes()), manifest_files=entries,
                            all_model_records=rows, all_metric_records=sim['records'], all_summary_metrics=summary,
                            viewer_html_sha256=digest(viewer), viewer_image_and_visual_check='not_run')


def analyze(root, inputs_root):
    inputs = check_inputs(inputs_root)
    execution = load_json(safe_file(root, 'execution.json'))
    require(execution['status'] == 'cli_operations_passed_analysis_pending', 'CLI execution is absent/failed')
    require(execution['source_commit'] == SOURCE and execution['release'] == 'v1.1.4', 'execution source/version')
    expected = {name + '-' + command for name in DEADLINES for command in ['validate', 'run', 'view']}
    cli = [c for c in execution['commands'] if c['name'] in expected]
    require(len(cli) == 9 and {c['name'] for c in cli} == expected, 'all nine CLI operations required')
    for command in execution['commands']:
        require(command['exit_code'] == 0, 'failed command ' + command['name'])
        for stream in ['stdout', 'stderr']:
            entry = command[stream]
            data = safe_file(root, entry['path']).read_bytes()
            require(len(data) == entry['bytes'] and digest(data) == entry['sha256'], 'command log hash')
    require({x['path']: x['sha256'] for x in execution['inputs']} ==
            {p: digest(data) for p, data in inputs.items()}, 'execution input inventory')
    normalized, reports = [], []
    for name, deadline in DEADLINES.items():
        value, report = check_case(root, name, deadline, inputs, execution)
        normalized.append(value)
        reports.append(report)
    require(normalized[0] == normalized[1] == normalized[2], 'non-deadline records/metrics differ between conditions')
    return dict(schema_version=1, status='records_metrics_hashes_passed_limited', measured_source=SOURCE,
                measured_release='v1.1.4', analysis_basis_release='v1.1.3', conditions=reports,
                metric_number_encoding='Exact Decimal values serialized as decimal strings here; original result JSON retained',
                binary=execution['binary'], all_non_deadline_simulation_fields_equal=True,
                actual_Viewer_images='not_run', ZIP_separate_execution='not established by one result set',
                guide_site_390px_search_nav_PR_Project='not_run')


def main():
    p = argparse.ArgumentParser(description=__doc__)
    p.add_argument('--execution-root', type=Path, required=True)
    p.add_argument('--inputs-root', type=Path, default=Path(__file__).parent / 'examples/guide/ethernet-deadline')
    p.add_argument('--report', type=Path, help='New report file; existing files are never overwritten')
    a = p.parse_args()
    report = analyze(a.execution_root.resolve(), a.inputs_root.resolve())
    if a.report:
        with a.report.open('x', encoding='utf-8', newline='\n') as out:
            json.dump(report, out, ensure_ascii=False, indent=2, default=str)
            out.write('\n')
    print(json.dumps(dict(status=report['status'], source=report['measured_source'], conditions=3)))


if __name__ == '__main__':
    main()
