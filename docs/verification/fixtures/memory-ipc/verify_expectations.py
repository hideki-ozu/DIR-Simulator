#!/usr/bin/env python3
"""Check fixed analytic projections; does not execute a simulator or scheduler."""
import copy
import json
import re
from pathlib import Path

BASE = Path(__file__).resolve().parent


def unique(pairs):
    result = {}
    for key, value in pairs:
        if key in result:
            raise ValueError(f'duplicate key: {key}')
        result[key] = value
    return result


def load(name):
    return json.loads((BASE / name).read_text(), object_pairs_hook=unique)


def eq(actual, expected):
    assert actual == expected, (actual, expected)


def integer(value):
    assert isinstance(value, str) and re.fullmatch(r'0|[1-9][0-9]*', value)
    number = int(value)
    assert number < 2**64
    return number


def config_rules(c):
    """Selected independently checkable geometry/capacity preparation rules."""
    errors = set()
    for d in c['ddr']:
        if d['size'] != d['banks'] * d['rows_per_bank'] * d['row_bytes']:
            errors.add('geometry_size')
        if not 0 < integer(d['refresh_ps']) < integer(d['refresh_interval_ps']):
            errors.add('refresh_interval')
    for s in c['sram']:
        if not 1 <= s['ports'] <= 16:
            errors.add('port_count')
    for s in c['shared']:
        if not 1 <= s['slots'] <= 1024:
            errors.add('slot_count')
    for d in c['dma']:
        if not 1 <= d['chunk_bytes'] <= 65536:
            errors.add('chunk_size')
    for m in c['mailboxes']:
        if not 0 <= m['capacity'] <= 1024:
            errors.add('mailbox_capacity')
    return errors


def duration(d, length, row_state):
    setup = {'closed': integer(d['open_ps']), 'hit': 0,
             'miss': integer(d['open_ps']) + integer(d['close_ps'])}[row_state]
    beats = (length + d['width_bytes'] - 1) // d['width_bytes']
    return setup + integer(d['column_ps']) + beats * integer(d['beat_ps'])


def input_checks(case, c, workload):
    eq(c['profile'], 'memory.ipc.transaction.v1')
    eq(c['schema_version'], 1)
    eq(workload['schema_version'], 2)
    eq(config_rules(c), set())
    ini = (BASE / case['ini']).read_text()
    for value in [case['config'], case['workload'], c['profile'], 'memoryipc.Main']:
        assert value in ini
    assert 'sim-time-limit = ' + case['stop_ps'] + 'ps' in ini
    nodes = [item['node'] for key in ['ddr', 'sram', 'shared', 'dma', 'mailboxes'] for item in c[key]]
    eq(len(set(nodes)), len(nodes))
    ids = set()
    for gen in workload['generators']:
        assert gen['id'] not in ids
        ids.add(gen['id'])
        assert gen['node'] in nodes
        eq(gen['kind'], 'memory-ipc.explicit.v1')
        times = [integer(t.removesuffix('ps')) for t in gen['times']]
        eq(times, sorted(times))
        r = gen['request']
        if 'hex' in r:
            assert re.fullmatch(r'(?:[0-9a-f]{2})+', r['hex'])
            if 'length' in r:
                eq(len(bytes.fromhex(r['hex'])), r['length'])
        if r['op'] == 'copy':
            assert r['src'] in nodes and r['dst'] in nodes


def check(case):
    name, e, T = case['id'], case['expected'], integer(case['stop_ps'])
    c, w = load(case['config']), load(case['workload'])
    input_checks(case, c, w)
    d, s, sh, dm, m = (c[key][0] for key in ['ddr', 'sram', 'shared', 'dma', 'mailboxes'])
    requests = {g['id']: g['request'] for g in w['generators']}
    if name.startswith('ddr-'):
        first = duration(d, requests['a']['length'], 'closed')
        if name == 'ddr-row-refresh':
            second = first + duration(d, 4, 'hit')
            third = second + duration(d, 4, 'miss')
            start = max(integer(d['refresh_interval_ps']), third)
            end = start + integer(d['refresh_ps'])
            fourth = end + duration(d, 4, 'closed')
            eq(e['grants'], list(map(str, [0, first, second, end])))
            eq(e['completions'], list(map(str, [first, second, third, fourth])))
            eq(e['row_hits'], [False, True, False, False])
            eq(e['refresh_start'], str(start)); eq(e['refresh_end'], str(end))
            assert fourth < T < start + integer(d['refresh_interval_ps'])
            memory = bytearray(d['size'])
            for key in ['a', 'c']:
                r = requests[key]
                memory[r['address']:r['address'] + r['length']] = bytes.fromhex(r['hex'])
            eq(e['final_hex'], memory.hex())
            eq(e['read_hex'], [memory[:4].hex()] * 2)
            eq(e['committed_bytes'], str(requests['a']['length'] + requests['c']['length']))
            eq(e['busy_ps'], str(third + fourth - end))
        elif name == 'ddr-stop':
            eq(first, T); eq(e['first_completion'], None); eq(e['first_status'], 'active')
            eq(e['final_hex'], '00' * d['size']); eq(e['committed_bytes'], '0')
            eq(e['busy_ps'], str(T))
        else:
            eq(d['queue_capacity'], 2)
            eq(e['first_completion'], str(first)); eq(e['committed_bytes'], '0')
            assert requests['d']['address'] % d['row_bytes'] + requests['d']['length'] > d['row_bytes']
            assert requests['e']['address'] + requests['e']['length'] > d['size']
            eq(d['fault_ranges'], [{'offset': requests['b']['address'], 'length': requests['b']['length']}])
            eq(e['reasons'], {'b:0': 'memory_fault', 'c:0': 'queue_full', 'd:0': 'address_error', 'e:0': 'address_error'})
    elif name.startswith('sram-'):
        write, read = integer(s['write_ps']), integer(s['read_ps'])
        eq(s['ports'], 2); eq(write, read)
        memory = bytearray(s['size']); memory[:2] = bytes.fromhex(requests['a']['hex'])
        if name == 'sram-ports':
            eq(e['ports'], [0, 1, 0]); eq(e['grants'], ['0', '0', str(write)])
            eq(e['completions'], [str(write), str(read), str(2 * write)])
            eq(e['read_hex'], memory[:2].hex())
            memory[1:2] = bytes.fromhex(requests['c']['hex'])
            eq(e['committed_bytes'], '3'); assert 2 * write < T
        else:
            eq(2 * write, T); eq(e['last_completion'], None); eq(e['last_status'], 'active')
            eq(e['committed_bytes'], '2')
        eq(e['final_hex'], memory.hex())
        eq(e['busy_ps'], str(3 * write)); eq(e['busy_denominator_ps'], str(s['ports'] * T))
    elif name.startswith('shared-'):
        publish = integer(sh['publish_ps'])
        if name == 'shared-ownership':
            eq(e['publish_completion'], str(publish))
            eq(e['consume_completion'], str(5 + integer(sh['consume_ps'])))
            assert publish < 4 < 5 < 10 < 11 < T
            eq(e['read_hex'], requests['a']['hex']); eq(e['slot_state'], 'free')
            eq(e['slot_hex'], ''); eq(e['published'], '1'); eq(e['consumed'], '1')
            eq(e['reasons'], {'b:0': 'full', 'd:0': 'empty', 'e:0': 'access_denied'})
            assert requests['e']['actor'] not in sh['producers']
        else:
            eq(T, publish); eq(e['completion'], None); eq(e['slot_state'], 'publishing')
            eq(e['owner'], requests['a']['actor']); eq(e['slot_hex'], '')
            eq(e['published'], '0'); eq(e['consumed'], '0')
    elif name == 'dma-offer-order':
        eq(s['queue_capacity'], 1)
        eq(integer(dm['setup_ps']), 2)
        eq(w['generators'][0]['times'], ['2ps'])
        keys = [('a', 0, 0, 0, 0), ('z', 0, 1, 0, 0)]
        eq(e['offer_keys'], [list(k) for k in sorted(reversed(keys))])
        eq(e['external_completion'], str(2 + integer(s['read_ps'])))
        eq(e['child_reason'], 'queue_full'); eq(e['parent_status'], 'failed')
        eq(e['parent_reason'], 'child_queue_full'); eq(e['parent_completed_ps'], '2')
        eq(e['committed_bytes'], '0'); eq(e['final_hex'], '00' * d['size'])
    elif name.startswith('dma-'):
        chunk, length = dm['chunk_bytes'], requests['a']['length']
        eq(chunk, 4); eq(length, 2 * chunk)
        first_read = integer(dm['setup_ps']) + integer(s['read_ps'])
        first_write = first_read + duration(d, chunk, 'closed')
        second_read = first_write + integer(s['read_ps'])
        second_write = second_read + duration(d, chunk, 'closed')
        # Direct source write completes before the second read and changes its data.
        assert 4 + integer(s['write_ps']) < first_write
        source = bytearray.fromhex(s['initial'][0]['hex'])
        source[4:8] = bytes.fromhex(requests['b']['hex'])
        target = bytearray(d['size']); target[:chunk] = source[:chunk]
        if name == 'dma-memory-content':
            target[chunk:length] = source[chunk:length]
            eq(e['read_completion'], list(map(str, [first_read, second_read])))
            eq(e['write_completion'], list(map(str, [first_write, second_write])))
            eq(e['data_done'], str(second_write)); eq(e['notified'], str(second_write + integer(dm['notify_ps'])))
            eq(e['committed_bytes'], str(length)); eq(e['status'], 'completed'); eq(e['reason'], 'ok')
            assert integer(e['notified']) < T
        elif name == 'dma-stop':
            eq(T, second_write); eq(e['write_completion'], [str(first_write), None])
            eq(e['data_done'], None); eq(e['notified'], None)
            eq(e['committed_bytes'], str(chunk)); eq(e['status'], 'writing')
        else:
            eq(d['fault_ranges'], [{'offset': chunk, 'length': chunk}])
            eq(e['failed_ps'], str(second_read)); eq(e['data_done'], None); eq(e['notified'], None)
            eq(e['committed_bytes'], str(chunk)); eq(e['status'], 'failed'); eq(e['reason'], 'child_memory_fault')
        eq(e['final_hex'], target.hex())
    elif name.startswith('mailbox-'):
        service = integer(m['service_ps']); notify = service + integer(m['notify_ps'])
        eq(m['capacity'], 1); eq(e['notify_planned'], str(notify))
        eq(e['queue_length'], '0'); eq(e['sent'], '1'); eq(e['received'], '1')
        eq(e['received_hex'], requests['a']['hex'])
        if name == 'mailbox-fifo':
            eq(e['completions'], list(map(str, [service, 2 * service, 3 * service, 7 + service])))
            eq(e['reasons'], ['ok', 'full', 'ok', 'empty'])
            eq(e['received_message_id'], 'a:0'); eq(e['notify_delivered'], str(notify))
            assert 3 * service < notify < T
        else:
            eq(T, notify); eq(e['notify_delivered'], None); assert 3 * service < T
    else:
        raise AssertionError('unrecognized analytic case ' + name)


def main():
    cases = load('scenarios.json')['cases']
    eq(len({c['id'] for c in cases}), len(cases))
    eq({c['test'] for c in cases}, {f'DIR-TEST-{i:04}' for i in range(50, 60)})
    for case in cases:
        check(case)
    negatives = load('invalid-configs.json')
    for mutation in negatives['mutations']:
        config = copy.deepcopy(load(negatives['base_config']))
        owner = config
        for part in mutation['path'][:-1]:
            owner = owner[part]
        owner[mutation['path'][-1]] = mutation['value']
        eq(config_rules(config), {mutation['rule']})
    try:
        json.loads('{"a":1,"a":2}', object_pairs_hook=unique)
    except ValueError:
        pass
    else:
        raise AssertionError('duplicate key accepted')
    for name in ['Types.ned', 'Main.ned']:
        assert (BASE / 'models/memoryipc' / name).is_file()
    print(f'PASS: {len(cases)} analytic projections, {len(negatives["mutations"])} config boundaries, 10 test IDs; product execution not performed')


if __name__ == '__main__':
    main()
