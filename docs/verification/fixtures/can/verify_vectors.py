#!/usr/bin/env python3
"""Check project analytic fixtures only; this is not a CAN simulator test."""
import json
from pathlib import Path

ROOT = Path(__file__).resolve().parent


def crc_register(bits):
    register = 0
    for digit in bits:
        feedback = int(digit) ^ (register >> 14)
        register = (register << 1) & 0x7FFF
        if feedback:
            register ^= 0x4599
    return register


def crc_division(bits):
    dividend = int(bits, 2) << 15
    polynomial = sum(1 << x for x in (15, 14, 10, 8, 7, 4, 3, 0))
    while dividend.bit_length() >= polynomial.bit_length():
        dividend ^= polynomial << (dividend.bit_length() - polynomial.bit_length())
    return dividend


def fields(vector):
    # Build with named field widths, independently from the fixture generator.
    identifier = vector['id']
    if vector['format'] == 'standard':
        header = [(0, 1), (identifier, 11), (0, 1), (0, 1), (0, 1)]
    else:
        header = [(0, 1), (identifier // 262144, 11), (1, 1), (1, 1),
                  (identifier % 262144, 18), (0, 1), (0, 1), (0, 1)]
    data = bytes.fromhex(vector['data'])
    header += [(len(data), 4)] + [(byte, 8) for byte in data]
    return ''.join(format(value, '0%db' % width) for value, width in header)


def stuff(raw):
    result, positions, previous, count = [], [], None, 0
    for digit in raw:
        count = count + 1 if digit == previous else 1
        result.append(digit)
        previous = digit
        if count == 5:
            previous = '1' if digit == '0' else '0'
            positions.append(len(result))
            result.append(previous)
            count = 1
    return ''.join(result), positions


def destuff(encoded):
    raw, positions, i = [], [], 0
    while i < len(encoded):
        raw.append(encoded[i])
        i += 1
        # Decode by inspecting transmitted prefix, including previous stuff bits.
        if i >= 5 and len(set(encoded[i-5:i])) == 1:
            assert i < len(encoded), 'missing terminal stuff bit'
            assert encoded[i] != encoded[i-1], 'incorrect stuff bit'
            positions.append(i)
            i += 1
    return ''.join(raw), positions


def main():
    vectors = json.loads((ROOT / 'vectors.json').read_text())['vectors']
    for v in vectors:
        m = fields(v)
        assert m == v['crc_input'], v['name']
        assert crc_register(m) == crc_division(m) == v['crc15'], v['name']
        assert format(v['crc15'], '04X') == v['crc_hex']
        raw = m + format(v['crc15'], '015b')
        encoded, positions = stuff(raw)
        assert encoded == v['stuffed_region']
        assert positions == v['stuff_positions']
        assert destuff(encoded) == (raw, positions)
        assert encoded + '1011111111' == v['frame']
        assert len(positions) == v['stuff_bits']
        assert len(v['frame']) == v['frame_bits']
        assert v['occupied_bits'] == v['frame_bits'] + 3
        assert len(m) == (19 if v['format'] == 'standard' else 39) + len(v['data']) * 4
        assert crc_division(raw) == 0
    assert any(v['stuff_positions'][-1:] == [len(v['stuffed_region'])-1] for v in vectors)
    scenarios = json.loads((ROOT / 'scenarios.json').read_text())['scenarios']
    by_name = {s['name']: s['expected'] for s in scenarios}
    for s in scenarios:
        ini = ROOT / s['config']
        assert ini.is_file()
        workload = json.loads(ini.with_suffix('.json').read_text())
        assert workload['schema_version'] == 1
        for g in workload['generators']:
            assert g['node'] in {'Main.a', 'Main.b', 'Main.c'}
    # Independent timeline checks: flat arithmetic for three named scenarios.
    a_duration = vectors[0]['frame_bits'] * 2_000_000
    a_busy = vectors[0]['occupied_bits'] * 2_000_000
    b_duration = vectors[1]['frame_bits'] * 2_000_000
    b_busy = vectors[1]['occupied_bits'] * 2_000_000
    assert [r['sof_ps'] for r in by_name['competition']['requests']] == [0, a_busy]
    assert [r['eof_ps'] for r in by_name['competition']['requests']] == [a_duration, a_busy + b_duration]
    assert [r['release_ps'] for r in by_name['competition']['requests']] == [a_busy, a_busy + b_busy]
    assert by_name['release-arrival']['sof_times_ps'] == [0, b_busy, b_busy + a_busy]
    d = by_name['delay-filter']
    assert d['sof_ps'] == 3_000_000
    assert d['eof_ps'] == 3_000_000 + a_duration
    assert d['release_ps'] == 3_000_000 + a_busy
    assert d['b_observed_ps'] == d['eof_ps'] + 2_000_000 + 3_000_000
    assert d['b_received_ps'] == d['b_observed_ps'] + 7_000_000
    assert d['c_observed_ps'] == d['eof_ps'] + 2_000_000
    # Input timing, queue/status and implementation outputs require the future runner.
    print(f'PASS: {len(vectors)} analytic bit vectors, {len(scenarios)} fixture references, 3 timeline projections')
    print('Simulator implementation: NOT RUN; no conformance or integration claim')


if __name__ == '__main__':
    main()
