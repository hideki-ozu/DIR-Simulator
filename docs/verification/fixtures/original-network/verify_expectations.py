#!/usr/bin/env python3
"""Independent integer oracle; no simulator or CAN FD codec is exercised."""
import configparser
import hashlib
import json
from pathlib import Path

DLC_LENGTHS = list(range(9)) + [12, 16, 20, 24, 32, 48, 64]

def ceildiv(n, d):
    return n // d + bool(n % d)

def binding(v):
    fields = [v['format'], str(v['id']), v['data'].lower(), str(int(v['brs'])),
              str(v['nominal_bits']), str(v['data_bits']), str(v['nominal_rate']), str(v['data_rate'])]
    return hashlib.sha256('|'.join(fields).encode('ascii')).hexdigest()

def fd(v):
    assert type(v['id']) is int and v['format'] in ('standard', 'extended')
    assert 0 <= v['id'] < 2 ** (11 if v['format'] == 'standard' else 29)
    assert isinstance(v['data'], str) and len(v['data']) % 2 == 0
    assert all(c in '0123456789abcdefABCDEF' for c in v['data'])
    size = len(v['data']) // 2
    assert size in DLC_LENGTHS and type(v['brs']) is bool
    n, d, rn, rd = (v[k] for k in ('nominal_bits', 'data_bits', 'nominal_rate', 'data_rate'))
    assert all(type(x) is int for x in (n, d, rn, rd))
    assert 1 <= rn <= 1000000 and rn <= rd <= 8000000
    assert 1 <= n <= 1000000 and 0 <= d <= 1000000
    assert (d >= max(1, size * 8)) if v['brs'] else (d == 0 and n >= size * 8)
    assert isinstance(v['evidence'], str) and 1 <= len(v['evidence']) <= 512
    assert v['binding_sha256'] == binding(v)
    duration = ceildiv(10**12 * (n * rd + d * rn), rn * rd)
    return [duration, ceildiv(10**12 * ((n+3) * rd + d * rn), rn * rd), DLC_LENGTHS.index(size)]

def t1(v):
    assert v['rate'] == 100000000 and v['duplex'] == 'full'
    assert sorted(v['roles']) == ['master', 'slave']
    assert 64 <= v['mac_bytes'] <= 1518
    assert all(type(v[k]) is int and 0 <= v[k] < 2**64 for k in ('tx', 'rx', 'propagation'))
    eof = (v['mac_bytes'] + 8) * 80000
    return [eof, (v['mac_bytes'] + 20) * 80000, eof + v['tx'] + v['propagation'] + v['rx']]

def pairs_unique(items):
    result = {}
    for key, value in items:
        if key in result:
            raise ValueError('duplicate JSON key: ' + key)
        result[key] = value
    return result

def check_inputs():
    base = Path(__file__).parent
    for name, profile in [('fd', 'can.fd.precomputed.v1'), ('t1', 'ethernet.l2.100base-t1.v1')]:
        ini = configparser.ConfigParser()
        ini.read(base / (name + '.ini'))
        general = ini['General']
        assert general['model-profile'].strip('"') == profile
        model = json.loads((base / general['model-config'].strip('"')).read_text(), object_pairs_hook=pairs_unique)
        workload = json.loads((base / general['workload'].strip('"')).read_text(), object_pairs_hook=pairs_unique)
        ned = base / general['ned-path'].strip('"')
        assert (ned / ('fd/Main.ned' if name == 'fd' else 'media/Main.ned')).is_file()
        assert set(workload) == {'schema_version', 'generators'} and workload['schema_version'] == 2
        for generator in workload['generators']:
            assert set(generator) == {'id', 'node', 'kind', 'times_ps', 'frame'}
            assert generator['times_ps'] == ['0']
            assert generator['node'] in ('Main.a', 'Main.b')
        if name == 'fd':
            assert model == {'schema_version': 1, 'profile': profile}
            assert general['Main.bus.nominalBitrate'] == '500kbps'
            assert general['Main.bus.dataBitrate'] == '2Mbps'
            g = workload['generators'][0]
            assert g['kind'] == 'can.fd.explicit.v1'
            frame = dict(g['frame'])
            assert set(frame) == {'format', 'id', 'data', 'brs', 'wire'}
            wire = frame.pop('wire')
            assert set(wire) == {'nominal_bits', 'data_bits', 'evidence', 'binding_sha256'}
            frame.update(wire)
            frame.update(nominal_rate=500000, data_rate=2000000)
            assert fd(frame) == [380000000, 386000000, 15]
            assert len(frame['data']) == 128
        else:
            assert set(model) == {'schema_version', 'endpoints', 'switches', 'seed', 'physical_links'}
            assert model['schema_version'] == 2 and len(model['physical_links']) == 1
            link = model['physical_links'][0]
            assert set(link) == {'id', 'a', 'b', 'phy_mode', 'duplex', 'a_phy', 'b_phy'}
            assert link['phy_mode'] == '100base-t1'
            for end in ['a', 'b']:
                assert set(link[end + '_phy']) == {'role', 'tx_latency_ps', 'rx_latency_ps'}
                assert ini['Channel Main::' + end + '.tx']['bitrate'] == '100Mbps'
                assert ini['Channel Main::' + end + '.tx']['delay'] == '1ns'
            for a, b, expected in [('a', 'b', 6061000), ('b', 'a', 6461000)]:
                row = dict(rate=100000000, duplex=link['duplex'], roles=[link['a_phy']['role'],link['b_phy']['role']], mac_bytes=64,
                           tx=int(link[a+'_phy']['tx_latency_ps']), rx=int(link[b+'_phy']['rx_latency_ps']), propagation=1000)
                assert t1(row)[2] == expected
            assert all(g['kind'] == 'ethernet.explicit.v1' and g['frame']['data'] == '' for g in workload['generators'])
    return 2

def main():
    doc = json.loads(Path(__file__).with_name('vectors.json').read_text(), object_pairs_hook=pairs_unique)
    assert doc['schema_version'] == 1
    input_count = check_inputs()
    count = 0
    for kind, function in [('fd', fd), ('t1', t1)]:
        for case in doc[kind]:
            assert function(case['input']) == case['expected'], case['name']
            count += 1
        for case in doc[kind + '_invalid']:
            v = dict(doc[kind][0]['input'])
            v.update(case['patch'])
            # Preserve original binding deliberately: altered wire/frame must fail.
            try:
                function(v)
            except (AssertionError, ValueError, TypeError):
                count += 1
            else:
                raise AssertionError('accepted invalid case: ' + case['name'])
    first = doc['fd'][0]['expected']
    assert [0, first[1], first[0], first[1] + first[0]] == doc['arbitration_expected']
    for c in doc['stop']:
        actual = 'serialized' if c['eof'] < c['T'] else 'transmitting'
        received = c['arrival'] < c['T']
        assert [actual, received] == c['expected']
        count += 1
    print(f'PASS: {count} analytic cases, {input_count} configured input sets; NED file existence only; product runtime and FD bitstream conformance untested')

if __name__ == '__main__':
    main()
