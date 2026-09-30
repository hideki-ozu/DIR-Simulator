#!/usr/bin/env python3
"""Independent integer projections for media fixtures; no simulator is executed."""
import hashlib
import json
import re
from pathlib import Path

P = Path(__file__).resolve().parent

def unique_pairs(pairs):
    value = {}
    for key, item in pairs:
        assert key not in value, ('duplicate JSON key', key)
        value[key] = item
    return value

def load(path):
    return json.loads((P/path).read_text(), object_pairs_hook=unique_pairs)

def scaled(value):
    m = re.fullmatch(r'(\d+)(ps|ns|us|ms|bps|kbps|Mbps|Gbps)', value)
    return int(m[1]) * {'ps':1,'ns':1000,'us':10**6,'ms':10**9,'bps':1,'kbps':1000,'Mbps':10**6,'Gbps':10**9}[m[2]]

def options(path):
    sections = {}
    current = None
    for line in (P/path).read_text().splitlines():
        if not line.strip():
            continue
        if line.startswith('['):
            current = line[1:-1]
            assert current not in sections
            sections[current] = {}
        else:
            k,v = (x.strip() for x in line.split('=',1))
            assert k not in sections[current]
            sections[current][k] = v
    assert sections['General']['model-profile'] == '"ethernet.l2.store-forward.v2"'
    return sections

def rate_delay(cfg, port):
    root, suffix = port.split('.',1)
    channel = cfg.get(f'Channel {root}::{suffix}', {})
    return scaled(channel.get('bitrate','100Mbps')), scaled(channel.get('delay','100ns'))

def validate_projection(model,cfg):
    assert model['schema_version'] == 2
    ports = set()
    for link in model['physical_links']:
        assert link['a'] < link['b']
        assert not ({link['a'],link['b']} & ports)
        ports |= {link['a'],link['b']}
        r,p = rate_delay(cfg,link['a'])
        assert (r,p) == rate_delay(cfg,link['b'])
        mode,duplex = link['phy_mode'],link['duplex']
        roles = (link['a_phy']['role'],link['b_phy']['role'])
        if mode == '1000base-t1':
            if duplex != 'full' or r != 10**9:
                return 'phy_duplex'
            if set(roles) != {'master','slave'}:
                return 'phy_roles'
        else:
            assert mode in ('10base-t','100base-tx') and duplex in ('half','full')
            assert r == (10**7 if mode == '10base-t' else 10**8)
            assert roles == ('none','none')
            assert all(end['tx_latency_ps'] == end['rx_latency_ps'] == '0' for end in (link['a_phy'],link['b_phy']))
            if duplex == 'half' and 2*p + 32*(10**12//r) >= 512*(10**12//r):
                return 'half_slot_bound'
    return None

def draw(seed,tid,n):
    # NUL-separated ASCII is constructed in one expression; fixed digest vectors
    # below validate the entire byte-level algorithm, not merely the slot range.
    raw = f'dir.ethernet.beb.v1\x00{seed}\x00{tid}\x00{n}'.encode('utf-8')
    digest = hashlib.sha256(raw).hexdigest()
    return digest, int(digest[:16],16) % (2 ** min(n,10))

def main():
    cases = load('scenarios.json')['cases']
    expected = {c['name']:c['expected'] for c in cases}
    for path in P.glob('*.json'):
        load(path.name)
    for c in cases:
        cfg = options(c['config'])
        general = cfg['General']
        model = load(general['model-config'].strip('"'))
        work = load(general['workload'].strip('"'))
        assert work['schema_version'] == 2
        assert (P/'models/media'/f"{general['network'].split('.')[1]}.ned").exists()
        assert validate_projection(model,cfg) == c['expected'].get('prepare_rule'), c['name']
    for v in load('backoff-vectors.json')['vectors']:
        digest,slots = draw(v['seed'],v['transfer_id'],v['collision_number'])
        assert (digest,slots) == (v['sha256'],v['slots'])
        assert v['k'] == min(v['collision_number'],10)
    B = 10000
    prop = 100000
    wire, ifg, slot, preamble, jam = (n*B for n in (576,96,512,64,32))
    collision = prop
    jam_start = max(collision,preamble)
    jam_end = jam_start + jam
    peer_end = jam_end + prop
    slots = [draw(1,f'{x}:0@Main.{x}.tx',1)[1] for x in 'ab']
    deadlines = [jam_end+n*slot for n in slots]
    first_sof = max(peer_end+ifg, deadlines[0])
    first_eof = first_sof+wire
    second_sof = max(first_eof+prop+ifg,deadlines[1])
    base = dict(collision_ps=[collision]*2,planned_jam_start_ps=[jam_start]*2,jam_end_ps=[jam_end]*2,backoff_slots=slots,backoff_until_ps=deadlines,retry_sof_ps=[first_sof,second_sof],retry_eof_ps=[first_eof,second_sof+wire],received_ps=[first_eof+prop,second_sof+wire+prop])
    assert expected['collision'] == base
    rep = expected['collision-repeat']
    second_start = jam_end + slot
    second_jam_end = second_start+preamble+jam
    nextslots = [draw(0,f'{x}:0@Main.{x}.tx',2)[1] for x in 'ab']
    assert rep == dict(attempt1_slots=[1,1],attempt2_sof_ps=second_start,attempt2_collision_ps=second_start+prop,attempt2_jam_end_ps=second_jam_end,attempt2_slots=nextslots,attempt3_sof_ps={'a':second_jam_end+nextslots[0]*slot,'b':second_jam_end+prop+ifg})
    for name in ('carrier','arrival-tie'):
        assert expected[name] == dict(sof_ps={'a':0,'b':wire+prop+ifg},collision_count=0)
    assert expected['late-start'] == dict(sof_ps=[0,50000],collision_ps=[150000,100000],jam_end_ps=[preamble+jam,50000+preamble+jam])
    first0 = preamble+jam+ifg
    assert expected['zero-propagation'] == dict(collision_ps=[0,0],jam_end_ps=[preamble+jam]*2,retry_sof_ps=[first0,first0+wire+ifg])
    assert expected['half-10'] == {k:[10*x for x in base[k]] for k in ('collision_ps','planned_jam_start_ps','jam_end_ps','retry_sof_ps')}
    assert expected['queue-retry'] == dict(at_ps=300000,current='a:0@Main.a.tx',waiting=['a:1@Main.a.tx'],dropped='a:2@Main.a.tx',reason='queue_full')
    assert expected['stop-jam'] == dict(status='jamming',jam_end_ps=None,jam_ps_per_output=max(0,800000-jam_start),tx_ps_per_output=800000,reception_count=0)
    assert deadlines[0] < 1200000 < deadlines[1] and 1200000 < first_sof
    assert expected['stop-backoff'] == dict(a_status='deferred',b_status='backoff',jam_ps_per_output=jam,reception_count=0)
    t1wire,t1occupied = 576*1000,672*1000
    assert expected['t1-duplex'] == dict(sof_ps=[0,0],eof_ps=[t1wire]*2,release_ps=[t1occupied]*2,arrival_ps=[t1wire+100000+1000+200000,t1wire+300000+1000+400000],collisions=0)
    assert expected['t1-pipeline'] == dict(sof_ps=[0,t1occupied],release_ps=[t1occupied,2*t1occupied],arrival_ps=[t1wire+2001000,t1occupied+t1wire+2001000],received_count=2)
    assert expected['t1-boundary'] == dict(arrival_ps=None,planned_arrival_ps=877000,reception_count=0)
    assert expected['t1-after-boundary'] == dict(arrival_ps=877000,received_count=1)
    assert expected['t1-max-frame'] == dict(eof_ps=(1518+8)*8*1000,release_ps=(1518+20)*8*1000,arrival_ps=(1518+8)*8*1000+301000)
    mix = expected['mixed']
    b_arrival = t1wire+1000
    a_arrival = wire+prop
    half_output_start = a_arrival+ifg
    c_next = b_arrival+wire+ifg
    assert mix == dict(sof_ps={'a:0@Mix.a.tx':0,'b:0@Mix.b.tx':0,'b:0@Mix.sw.tx_c':b_arrival,'a:0@Mix.sw.tx_b':a_arrival,'b:0@Mix.sw.tx_a':half_output_start,'a:0@Mix.sw.tx_c':c_next},arrival_ps={'a:0@Mix.sw.tx_b':a_arrival+t1wire+1000,'b:0@Mix.sw.tx_a':half_output_start+wire+prop},collision_count=0)
    unit = load('attempt-limit-unit.json')
    assert unit['injected_slots'] == [0]*15
    assert unit['expected_attempts'] == unit['expected_collision_count'] == 16
    assert unit['expected_last_sof_ps'] == 15*(preamble+jam+prop+ifg)
    assert unit['expected_last_jam_end_ps'] == unit['expected_last_sof_ps']+preamble+jam
    assert unit['expected_drop_reason'] == 'attempt_limit' and unit['expected_next_attempt'] is None
    rows = load('record-examples.json')
    envelope_keys = {'schema_name','schema_version','record_id','subject','request_id','origin_request_id','time_ps','data'}
    assert all(set(row) == envelope_keys for row in rows)
    assert [(row['schema_name'],row['schema_version']) for row in rows] == [('ethernet.attempt',1),('ethernet.frame',1),('ethernet.phy_link',1),('ethernet.transfer',2)]
    attempt,frame,phy,transfer = [row['data'] for row in rows]
    assert attempt['transfer_id'] == rows[3]['record_id']
    assert transfer['last_attempt_id'] == rows[0]['record_id']
    assert int(attempt['planned_jam_start_ps']) == jam_start
    assert attempt['jam_end_ps'] is None and int(attempt['planned_jam_end_ps']) == jam_end
    assert int(rows[0]['time_ps']) == collision
    assert transfer['planned_eof_ps'] is None and attempt['planned_eof_ps'] == str(wire)
    assert len(bytes.fromhex(frame['mac_hex'])) == 64
    assert phy['deference_policy'] == 'continuous-idle-96.v1'
    descriptors = load('metrics.json')
    assert len(descriptors) == 9 and len({m['metric_id'] for m in descriptors}) == 9
    assert all(set(m) == {'metric_id','version','unit','value_kind','sampling','aggregation'} and m['version'] == '1' for m in descriptors)
    assert descriptors == sorted(descriptors,key=lambda m:m['metric_id'])
    print(f'PASS: {len(cases)} media fixture references/config projections; 15 BEB digest vectors; collision/deferral/T1/mixed arithmetic; injected 16-attempt boundary; 4 complete records; 9 metric descriptors. Simulator NOT RUN.')

if __name__ == '__main__':
    main()
