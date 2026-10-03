#!/usr/bin/env python3
"""Check static GW fixtures with integer arithmetic; does not execute a simulator."""
import json
from pathlib import Path

ROOT = Path(__file__).resolve().parent

def unique_pairs(pairs):
    result = {}
    for key, value in pairs:
        assert key not in result, f'duplicate key: {key}'
        result[key] = value
    return result

def read(name):
    return json.loads((ROOT / name).read_text(), object_pairs_hook=unique_pairs)

def wire_bits(identifier):
    # Independent GF(2) integer long division on the 19-bit empty standard frame.
    message = identifier << 7
    remainder = message << 15
    polynomial = (1 << 15) | 0x4599
    while remainder.bit_length() >= polynomial.bit_length():
        remainder ^= polynomial << (remainder.bit_length() - polynomial.bit_length())
    bits = f'{message:019b}{remainder:015b}'
    wire = ''
    for bit in bits:
        wire += bit
        if wire.endswith(bit * 5):
            wire += '1' if bit == '0' else '0'
    return len(wire) + 10

def duration(bits, bitrate):
    return (bits * 10**12 + bitrate - 1) // bitrate

def topology(network):
    if network == 'Main':
        return {'Main.src':'A','Main.gw.a':'A','Main.gw.b':'B','Main.sink':'B'}
    if network == 'Multi':
        return {'Multi.src':'A','Multi.gw.a':'A','Multi.gw.b':'B','Multi.sinkB':'B','Multi.gw.c':'C','Multi.sinkC':'C'}
    result = {'Chain.src':'A','Chain.sink':'D'}
    for index in range(1, 4):
        result[f'Chain.g{index}.a'] = chr(64 + index)
        result[f'Chain.g{index}.b'] = chr(65 + index)
    return result

def prepare_projection(routing, workload, buses):
    routes = [(g['node'], r) for g in routing['gateways'] for r in g['routes']]
    for index, (gateway, a) in enumerate(routes):
        for other, b in routes[index + 1:]:
            if (gateway,a['ingress'],a['format']) == (other,b['ingress'],b['format']):
                if max(a['id_min'],b['id_min']) <= min(a['id_max'],b['id_max']):
                    return 'route_overlap'
    owners = []
    for g in workload['generators']:
        f = g['frame']
        owners.append((buses[g['node']],f['format'],f['id'],f['id'],g['node']))
    for _, r in routes:
        for port in r['egress']:
            owners.append((buses[port],r['format'],r['id_min'],r['id_max'],port))
    for index, a in enumerate(owners):
        for b in owners[index + 1:]:
            if a[:2] == b[:2] and a[4] != b[4] and max(a[2],b[2]) <= min(a[3],b[3]):
                return 'owner_overlap'
    # Independently explore paths while intersecting ID intervals, rather than partitioning ID space.
    edges = [(buses[r['ingress']],buses[e],r['format'],r['id_min'],r['id_max']) for _,r in routes for e in r['egress']]
    def visit(node, fmt, lo, hi, stack):
        if node in stack:
            return True
        for source, dest, form, low, high in edges:
            left,right = max(lo,low),min(hi,high)
            if source == node and form == fmt and left <= right:
                if visit(dest,fmt,left,right,stack + (node,)):
                    return True
        return False
    for source, _, fmt, lo, hi in edges:
        if visit(source,fmt,lo,hi,()):
            return 'route_cycle'
    return None

def main():
    scenarios = read('scenarios.json')['cases']
    expected = {c['name']:c['expected'] for c in scenarios}
    for path in ROOT.rglob('*.json'):
        read(str(path.relative_to(ROOT)))
    for case in scenarios:
        ini = (ROOT/case['config']).read_text()
        options = {}
        for line in ini.splitlines():
            if '=' in line:
                key,value = line.split('=',1)
                options[key.strip()] = value.strip().strip('"')
        assert options['model-profile'] == 'can.cc.multibus.v1'
        network = options['network'].split('.')[1]
        assert (ROOT/f'models/gw/{network}.ned').is_file()
        routing, workload = read(options['model-config']), read(options['workload'])
        assert routing['schema_version'] == 1 and workload['schema_version'] == 2
        failure = prepare_projection(routing,workload,topology(network))
        assert failure == case['expected'].get('prepare_failure'), (case['name'],failure)
    records = read('model-record-examples.json')
    fields = {'schema_name','schema_version','record_id','subject','request_id','origin_request_id','time_ps','data'}
    assert all(set(row) == fields and row['schema_version'] == 1 for row in records)
    assert [row['schema_name'] for row in records] == ['can.request','can.receiver','gw.forward','gw.rx_buffer']
    request, receiver, forward, rx_buffer = [row['data'] for row in records]
    assert forward['child_request_id'] == request['request_id'] == receiver['request_id']
    assert request['model_fields']['origin_request_id'] == forward['origin_request_id'] == 'source:0'
    assert int(request['eof_ps']) - int(request['sof_ps']) == duration(50,500000)
    assert int(records[0]['time_ps']) == int(request['model_fields']['release_ps'])
    assert int(receiver['received_ps']) == int(request['eof_ps'])
    assert rx_buffer['parent_request_id'] == forward['parent_request_id']
    assert rx_buffer['released_ps'] == request['model_fields']['tx_enqueued_ps'] == request['ready_ps']
    assert rx_buffer['egress'] == [forward['egress']] and rx_buffer['status'] == 'released'
    assert wire_bits(0) == 50 and wire_bits(1) == 47
    eof_a = duration(wire_bits(0),500000)
    eof_1 = duration(wire_bits(1),500000)
    child = 'gw:source:0/Main.gw/ab/Main.gw.b'
    assert expected['independent']['sof_ps'] == {'source:0':0,'other:0':0,child:eof_a}
    assert expected['independent']['eof_ps'] == {'source:0':eof_a,'other:0':eof_1,child:2*eof_a}
    # Delay case: distinct processing, propagation, and destination contention terms.
    micro = 10**6
    source_sof = 3*micro
    source_eof = source_sof + eof_a
    observed = source_eof + (2+3)*micro
    received = observed + 7*micro
    generated = received + 11*micro
    ready = generated + 13*micro
    busy_release = duration(wire_bits(1)+3,250000)
    sof = max(ready,busy_release)
    eof = sof + duration(wire_bits(0),250000)
    sink_observed = eof + (2+3)*micro
    sink_received = sink_observed + 7*micro
    d = expected['delay']
    assert d['sof_ps'] == {'source:0':source_sof,'other:0':0,child:sof}
    assert d['eof_ps'] == {'source:0':source_eof,'other:0':duration(47,250000),child:eof}
    for key,value in [('ingress_observed_ps',observed),('ingress_received_ps',received),('copy_generated_ps',generated),('copy_ready_ps',ready),('sink_observed_ps',sink_observed),('sink_received_ps',sink_received),('path_delay_ps',sink_received)]:
        assert d[key] == value, key
    # Queue occupancy: first copy is in-flight, second fills the one waiting slot,
    # third stays in RX until the second starts and frees the TX waiting slot.
    q = expected['queue']
    first_release = eof_a + duration(53,125000)
    arrivals = [eof_a + t*micro for t in (0,110,220)]
    assert arrivals[1] < arrivals[2] < first_release
    assert q['source_eof_ps'] == arrivals
    second_release = first_release + duration(53,125000)
    assert q['copy_sof_ps'] == [eof_a,first_release,second_release]
    assert q['copy_eof_ps'] == [eof_a+duration(50,125000),first_release+duration(50,125000),None]
    assert second_release < 1000*micro < second_release + duration(50,125000)
    assert q['copy_status'] == ['success','success','in_flight']
    assert q['copy_drop_reason'] == [None,None,None]
    assert expected['capacity-zero'] == dict(copy_status='dropped',copy_drop_reason='queue_full',attempts=0)
    assert expected['forward-boundary'] == dict(forward_status='processing',planned_forward_ps=eof_a+20*micro,child_request_count=0)
    assert expected['eof-boundary'] == dict(copy_status='in_flight',copy_eof_ps=None,copy_planned_eof_ps=2*eof_a)
    assert expected['no-route'] == dict(forward_status='filtered',reason='no_route',child_request_count=0)
    assert expected['rx-filter'] == dict(receiver_status='filtered',forward_count=0)
    assert expected['multicast'] == dict(child_eof_ps={'Multi.gw.b':eof_a+duration(50,250000),'Multi.gw.c':eof_a+duration(50,1000000)},origin='source:0',copy_count=2)
    assert expected['multicast-drop'] == dict(copy_status={'Multi.gw.b':'success','Multi.gw.c':'dropped'},reason_c='queue_full')
    assert expected['hop'] == dict(hops=[1,2,3],forwarded_ps=[eof_a*n for n in (1,2,3)],statuses=['submitted','submitted','dropped'],reason='dropped_hop_limit',child_request_count=2)
    assert expected['disjoint-cycle'] == dict(prepare_valid=True,request_count=0)
    assert expected['independent-only'] == dict(sof_ps={'source:0':0,'other:0':0},forward_count=0)
    print(f'GW static fixtures: {len(scenarios)} cases; JSON, file references, intervals and integer analytic expectations PASS (simulator not executed)')

if __name__ == '__main__':
    main()
