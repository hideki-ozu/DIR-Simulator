#!/usr/bin/env python3
"""Independent arithmetic checks for static AXI fixtures, not a simulator."""
import json
import re
from pathlib import Path

ROOT = Path(__file__).resolve().parent


def load(name):
    return json.loads((ROOT / name).read_text())


def time_ps(value):
    number, unit = re.fullmatch(r'(\d+)(ps|ns|us)', value).groups()
    return int(number) * {'ps': 1, 'ns': 1000, 'us': 1_000_000}[unit]


def next_ready(earliest, pattern):
    # Modular closed search over one period, independent of an event scheduler.
    return min(earliest + (index - earliest) % len(pattern)
               for index, bit in enumerate(pattern) if bit == '1')


def response(transaction, ram):
    start = transaction['address']
    end = start + 4 * transaction['beats']
    if start < ram['base'] or end > ram['base'] + ram['size']:
        return 'DECERR'
    for interval in ram['error_ranges']:
        if interval['access'] in ('both', transaction['operation']):
            if max(start, interval['start']) < min(end, interval['end']):
                return 'SLVERR'
    return 'OKAY'


def valid_transaction(transaction):
    op = transaction.get('operation')
    if op not in ('read', 'write'):
        return False
    keys = {'operation', 'address', 'beats'}
    if op == 'write':
        keys |= {'write_data', 'write_strobes'}
    if set(transaction) != keys:
        return False
    address, beats = transaction['address'], transaction['beats']
    if type(address) is not int or type(beats) is not int:
        return False
    if not (0 <= address <= 0xFFFFFFFF and address % 4 == 0 and 1 <= beats <= 256):
        return False
    end = address + 4 * beats
    if end > 2**32 or address // 4096 != (end - 1) // 4096:
        return False
    if op == 'write':
        data, strobes = transaction['write_data'], transaction['write_strobes']
        if not isinstance(data, list) or not isinstance(strobes, list):
            return False
        if len(data) != beats or len(strobes) != beats:
            return False
        if not all(isinstance(x, str) and re.fullmatch('[0-9a-fA-F]{8}', x) for x in data):
            return False
        if not all(type(x) is int and 0 <= x <= 15 for x in strobes):
            return False
    return True


def verify_scenario(scenario):
    name = scenario['name']
    ini = (ROOT / scenario['config']).read_text()
    assert 'model-profile = "axi4.transaction.v1"' in ini
    model_name = re.search(r'model-config = "([^"]+)"', ini).group(1)
    workload_name = re.search(r'workload = "([^"]+)"', ini).group(1)
    config, workload = load(model_name), load(workload_name)
    period = time_ps(config['clock_period'])
    limit = time_ps(re.search(r'sim-time-limit = (\S+)', ini).group(1))
    assert workload['schema_version'] == 2
    ram = config['ram']
    managers = {m['node']: m for m in config['managers']}
    generators = {g['id']: g for g in workload['generators']}
    expected = scenario['expected']
    memory = bytearray(ram['size'])
    for initial in ram['initial']:
        data = bytes.fromhex(initial['data'])
        memory[initial['offset']:initial['offset']+len(data)] = data
    states = {t['request_id']: t for t in expected['transactions']}
    assert all(valid_transaction(g['transaction']) for g in generators.values())
    calculated_rows = []
    for state in expected['transactions']:
        request = state['request_id']
        generator = generators[request.rsplit(':', 1)[0]]
        transaction = generator['transaction']
        assert state['status'] in {'pending', 'active', 'completed', 'dropped'}
        if state['grant_ps'] is None:
            assert state['status'] in {'pending', 'dropped'}
            continue
        grant = state['grant_ps'] // period
        assert state['grant_ps'] % period == 0
        manager = managers[generator['node']]
        address_channel = 'AW' if transaction['operation'] == 'write' else 'AR'
        address_edge = next_ready(grant + 1, ram[address_channel.lower() + '_ready'])
        plan = [(address_channel, None, address_edge, grant + 1)]
        if transaction['operation'] == 'write':
            edge = address_edge
            for beat in range(transaction['beats']):
                earliest = edge + 1
                edge = next_ready(earliest, ram['w_ready'])
                plan.append(('W', beat, edge, grant + 1 if beat == 0 else earliest))
            earliest = edge + ram['write_response_cycles']
            edge = next_ready(earliest, manager['b_ready'])
            plan.append(('B', None, edge, earliest))
        else:
            edge = address_edge
            for beat in range(transaction['beats']):
                earliest = edge + (ram['read_latency_cycles'] if beat == 0 else 1)
                edge = next_ready(earliest, manager['r_ready'])
                plan.append(('R', beat, edge, earliest))
        assert state['completed_ps'] == (edge * period if edge * period < limit else None)
        assert state['response'] == response(transaction, ram)
        for channel, beat, edge, earliest in plan:
            if edge * period < limit:
                calculated_rows.append(dict(request_id=request, channel=channel, beat=beat,
                                            time_ps=edge*period, valid_since_ps=earliest*period))
    calculated_rows.sort(key=lambda row: row['time_ps'])
    assert calculated_rows == expected['handshakes'], name
    memory_time = 0
    reads = {}
    for row in expected['handshakes']:
        request = row['request_id']
        transaction = generators[request.rsplit(':', 1)[0]]['transaction']
        answer = response(transaction, ram)
        if row['channel'] not in ('R', 'W'):
            continue
        offset = transaction['address'] + 4*row['beat'] - ram['base']
        if row['channel'] == 'W' and answer == 'OKAY':
            strobe = transaction['write_strobes'][row['beat']]
            # Integer mask oracle; the design updates individual bytes.
            mask = sum(0xFF << (8*lane) for lane in range(4) if strobe & (1 << lane))
            old = int.from_bytes(memory[offset:offset+4], 'little')
            new = int.from_bytes(bytes.fromhex(transaction['write_data'][row['beat']]), 'little')
            value = (old & (~mask & 0xFFFFFFFF)) | (new & mask)
            memory[offset:offset+4] = value.to_bytes(4, 'little')
            if strobe:
                memory_time = row['time_ps']
        elif row['channel'] == 'R':
            data = memory[offset:offset+4].hex() if answer == 'OKAY' else '00000000'
            reads.setdefault(request, []).append(data)
    assert memory.hex() == expected['memory_hex'], name
    assert memory_time == expected['memory_time_ps'], name
    for request, state in states.items():
        assert state['read_data'] == reads.get(request, []), (name, request)
    assert all(row['time_ps'] < limit for row in expected['handshakes'])
    verify_metrics(config, workload, expected, period, limit, ini)


def verify_metrics(config, workload, expected, period, limit, ini):
    from fractions import Fraction

    def exact(value):
        if value is None or isinstance(value, int):
            return value
        return Fraction(value['numerator'], value['denominator'])

    def average(values):
        return Fraction(sum(values), len(values)) if values else None

    managers = sorted(m['node'] for m in config['managers'])
    bus = config['interconnect']
    transactions = {}
    for g in workload['generators']:
        for ordinal, text in enumerate(g['times']):
            when = time_ps(text)
            if when < limit:
                transactions[g['id'] + ':' + str(ordinal)] = (g['node'], when, g['transaction'])
    states = expected['transactions']
    observed = {(row['target'], row['metric']): row for row in expected['metrics']['summary']}
    checked = set()

    def check(target, metric, value, samples=None, reason=None):
        key = (target, 'axi.' + metric)
        checked.add(key)
        row = observed[key]
        assert exact(row['value']) == value, (key, row['value'], value)
        assert row['sample_count'] == samples and row['reason'] == reason, key

    for target in managers + [bus, '$all']:
        own = [s for s in states if target in (bus, '$all') or
               transactions[s['request_id']][0] == target]
        check(target, 'generated', len(own))
        for state in ('completed', 'dropped', 'pending', 'active'):
            check(target, state, sum(s['status'] == state for s in own),
                  reason='outstanding_full' if state == 'dropped' else None)
        for response in ('OKAY', 'SLVERR', 'DECERR'):
            check(target, response.lower(), sum(s['status'] == 'completed' and
                                               s['response'] == response for s in own))
        for metric, field in [('wait_mean_ps', 'grant_ps'), ('latency_mean_ps', 'completed_ps')]:
            samples = [s[field] - transactions[s['request_id']][1]
                       for s in own if s[field] is not None]
            check(target, metric, average(samples), len(samples))
    for manager in managers:
        own = [s for s in states if transactions[s['request_id']][0] == manager and
               s['status'] != 'dropped']
        # Sum per-request rectangles instead of sweeping queue transitions.
        for prefix, end_field in [('queue', 'grant_ps'), ('outstanding', 'completed_ps')]:
            intervals = [(transactions[s['request_id']][1], s[end_field]) for s in own]
            area = sum((limit if end is None else end) - start for start, end in intervals)
            check(manager, prefix + '_mean', Fraction(area, limit) if limit else None)
            starts = [start for start, end in intervals]
            # Generation phase precedes grant, but completion precedes generation.
            peak = max([0] + [sum(start <= when and (end is None or
                        (end >= when if prefix == 'queue' else end > when))
                        for start, end in intervals) for when in starts])
            check(manager, prefix + '_max', peak)
    bit_points = []
    responses = {s['request_id']: s['response'] for s in states}
    for hs in expected['handshakes']:
        t = transactions[hs['request_id']][2]
        if responses[hs['request_id']] != 'OKAY':
            continue
        if hs['channel'] == 'R':
            bit_points.append((hs['time_ps'], 'read', 32))
        if hs['channel'] == 'W':
            strobe = t['write_strobes'][hs['beat']]
            bit_points.append((hs['time_ps'], 'written', 8*sum(bool(strobe & (1 << n)) for n in range(4))))

    def interval_values(start, end):
        elapsed = end - start
        busy = sum(max(0, min(end, s['completed_ps'] if s['completed_ps'] is not None else limit)
                       - max(start, s['grant_ps'])) for s in states if s['grant_ps'] is not None)
        result = {'bus_utilization': Fraction(busy, elapsed) if elapsed else None}
        for kind in ('read', 'written'):
            amount = sum(bits for time, channel, bits in bit_points if channel == kind and start <= time < end)
            result[kind + '_bits'] = amount
            result[kind + '_throughput_bps'] = Fraction(amount*10**12, elapsed) if elapsed else None
        return result

    for metric, value in interval_values(0, limit).items():
        check(bus, metric, value)
    assert checked == set(observed), 'missing or extra AXI summary metric rows'
    found = re.search(r'metrics-window = (\S+)', ini)
    width = time_ps(found.group(1)) if found else 1_000_000_000
    windows = expected['metrics']['windows']
    assert len(windows) == (limit + width - 1) // width
    for i, window in enumerate(windows):
        start, end = i*width, min((i+1)*width, limit)
        assert (window['start_ps'], window['end_ps']) == (start, end)
        assert {key: exact(value) for key, value in window['values'].items()} == {
            'axi.'+key: value for key, value in interval_values(start, end).items()}


def verify_metric_registry():
    metrics = load('metrics.json')['metrics']
    assert len(metrics) == 24
    assert metrics == sorted(metrics, key=lambda x: x['metric_id'])
    groups = [
        ('generated completed dropped pending active okay slverr decerr', 'count', 'integer', 'summary', 'sum'),
        ('queue_length outstanding', 'count', 'integer', 'point', 'identity'),
        ('queue_mean outstanding_mean', 'count', 'number', 'summary', 'time_mean'),
        ('queue_max outstanding_max', 'count', 'integer', 'summary', 'max'),
        ('bus_utilization', '1', 'number', 'window_summary', 'occupancy_ratio'),
        ('wait_ps latency_ps', 'ps', 'integer', 'point', 'identity'),
        ('wait_mean_ps latency_mean_ps', 'ps', 'number', 'summary', 'sample_mean'),
        ('read_bits written_bits', 'bit', 'integer', 'window_summary', 'sum'),
        ('read_throughput_bps written_throughput_bps', 'bit/s', 'number', 'window_summary', 'rate'),
        ('channel_stall_cycles', 'count', 'integer', 'point', 'identity')]
    registered = {m['metric_id']: m for m in metrics}
    for names, unit, kind, sampling, aggregation in groups:
        for name in names.split():
            assert registered['axi.'+name] == dict(metric_id='axi.'+name, version='1', unit=unit,
                   value_kind=kind, sampling=sampling, aggregation=aggregation)


def main():
    verify_metric_registry()
    scenarios = load('scenarios.json')['scenarios']
    for scenario in scenarios:
        verify_scenario(scenario)
    by_name = {s['name']: s['expected'] for s in scenarios}
    assert [t['request_id'] for t in by_name['round-robin']['transactions']] == ['a:0','b:0','a:1']
    assert [t['status'] for t in by_name['capacity']['transactions']] == ['completed','dropped']
    base = load('read-write.workload.json')['generators'][0]['transaction']
    mutations = load('invalid-transactions.json')['mutations']
    for mutation in mutations:
        transaction = dict(base)
        transaction[mutation['field']] = mutation['value']
        assert not valid_transaction(transaction), mutation['name']
    # Legal limits, independent from the 8 nominal fixture scenarios.
    assert valid_transaction(dict(operation='read', address=0, beats=256))
    assert valid_transaction(dict(operation='read', address=0xFFFFFFFC, beats=1))
    assert (ROOT / 'models/demo/Main.ned').is_file()
    print(f'PASS: {len(scenarios)} AXI analytic scenarios; {len(mutations)} invalid mutations; 2 legal boundary inputs')
    print('PASS: 24 metric descriptors, 424 summary projections, 11 window groups')
    print('Simulator/RTL/AXI conformance: NOT RUN')


if __name__ == '__main__':
    main()
