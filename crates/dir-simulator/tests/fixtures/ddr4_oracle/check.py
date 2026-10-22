#!/usr/bin/env python3
"""Independent ISSI selected-mode trace monitor; no runtime imports.

This monitor checks initialization and normal open-page data/refresh traces.
Power/training/raw auto-precharge rules have separate Rust boundary tests; this
monitor does not certify those unexercised command classes or silicon behavior.
Literal timings are transcribed from the pinned Rev F ledger, not generated
from the Rust eligibility code. All applicable past commands are checked.
"""
import collections
import json
import sys

CK = 834
REFI = 7_800_000
MR = [0x0834, 0x0001, 0x0018, 0x0200, 0, 0x0400, 0x0810]

def require(condition, rule):
    if not condition:
        raise ValueError(rule)

def check(document):
    require(document['source_sha256'] == '0606a9501dc13de5934856c9d7fbe0310f6a4edacf26b9cf416f4a5bfc2ca727', 'source_identity')
    requests = {r['id']: r for r in document['requests']}
    require(len(requests) == len(document['requests']), 'request_identity')
    banks = [dict(row=None, act=None, pre=None, read=None, write=None) for _ in range(16)]
    history = []
    reservations = []
    expected_beats = []
    actual_beats = []
    expected_bursts = []
    actual_bursts = []
    logical = collections.defaultdict(list)
    commands = collections.Counter()
    release = cke = last_mrs = zq = ready = last_command = None
    mr_step = 0
    completions = {}
    last_time = 0
    ref_count = 0
    refs = []
    for event in document['effects']:
        t = event['at_ps']
        require(t >= last_time, 'effect_order')
        last_time = t
        kind = event['kind']
        if kind == 'ready':
            require(ready is None and mr_step == 7 and zq is not None, 'initialization_ready_ownership')
            require(t == max(zq + 1024*CK, last_mrs + 768*CK), 'ZQ_first_DLL_lock')
            ready = t
        elif kind == 'command':
            c = event['command']
            op = c['kind']
            commands[op] += 1
            require(t % CK == 0, 'command_edge')
            require(last_command is None or t >= last_command+CK, 'command_bus')
            last_command = t
            if op == 'reset':
                require(not c['asserted'] and release is None, 'selected_trace_reset')
                require(t >= 200_000_000, 'RESET_powerup_low')
                release = t
                continue
            if op == 'cke':
                require(c['high'] and release is not None and cke is None, 'selected_trace_CKE')
                require(t >= release+500_000_000, 'RESET_to_CKE')
                cke = t
                continue
            if op == 'mrs':
                require(ready is None and cke is not None, 'selected_trace_MRS')
                require(t >= cke+324*CK, 'RESET_CKE_to_MRS')
                require(last_mrs is None or t >= last_mrs+8*CK, 'MRS_to_MRS')
                index = [3,6,5,4,2,1,0][mr_step]
                require(c['index'] == index and c['value'] == (0x0934 if index == 0 else MR[index]), 'initialization_sequence')
                mr_step += 1
                last_mrs = t
                continue
            if op == 'zq':
                require(ready is None and mr_step == 7 and c['long'], 'ZQ_first')
                require(t >= last_mrs+24*CK, 'MRS_to_nonMRS')
                zq = t
                continue
            require(ready is not None and t >= ready, 'command_before_initialization')
            if refs:
                require(t >= refs[-1]+312*CK, 'REF_busy')
            for b in banks:
                if b['row'] is not None:
                    require(t-b['act'] <= 9*REFI, 'ACT_open_max')
            require((t-ready)//REFI - ref_count <= 8, 'refresh_postpone_bound')
            require(t-(refs[-1] if refs else ready) <= 9*REFI, 'refresh_gap')
            if op == 'act':
                bank = c['bank']; b = banks[bank]
                require(b['row'] is None, 'bank_command_state')
                if b['pre'] is not None: require(t >= b['pre']+16*CK, 'PRE_to_ACT_REF')
                if b['act'] is not None: require(t >= b['act']+55*CK, 'ACT_to_ACT')
                acts = [(at, p) for at, p in history if p['kind'] == 'act']
                for at, p in acts:
                    require(t >= at+(6 if p['bank']//4 == bank//4 else 4)*CK, 'ACT_bank_group')
                if len(acts) >= 4: require(t >= acts[-4][0]+26*CK, 'ACT_fifth_window')
                b.update(row=c['row'], act=t, read=None, write=None)
            elif op == 'pre':
                selected = range(16) if c['bank'] is None else [c['bank']]
                for index in selected:
                    b = banks[index]
                    for name, cycles in [('act',39),('read',9),('write',34)]:
                        if b[name] is not None: require(t >= b[name]+cycles*CK, f'{name}_to_PRE')
                    b.update(row=None, pre=t)
            elif op in ('read','write'):
                require(not c['auto_precharge'], 'monitor_supports_open_page_trace')
                bank = c['bank']; b = banks[bank]
                require(b['row'] is not None, 'bank_command_state')
                require(t >= b['act']+16*CK, 'ACT_to_RD_WR')
                require(0 <= c['column'] <= 1016 and c['column']%8 == 0, 'burst_address')
                for at, prior in history:
                    p = prior['kind']
                    if p not in ('read','write'): continue
                    same = prior['bank']//4 == bank//4
                    cycles = ((6 if same else 4) if p == op else (10 if op == 'write' else (25 if same else 19)))
                    require(t >= at+cycles*CK, f'{p}_to_{op}_all_history')
                first = t+(12 if op == 'write' else 16)*CK
                end = first+4*CK
                for a, z in reservations: require(end <= a or z <= first, 'burst_no_interruption')
                reservations.append((first,end))
                r = requests[c['request']]
                require(r['operation']['kind'] == op, 'request_operation')
                physical = (b['row']<<14) | (bank<<10) | c['column']
                length = len(r['operation']['data']) if op == 'write' else r['operation']['length']
                for i in range(8):
                    address = physical+i
                    offset = address-r['address']
                    offset = offset if 0 <= offset < length else None
                    require(c['logical_offset'][i] == offset, 'logical_byte_mapping')
                    if offset is not None: logical[c['request']].append(offset)
                    item = {'kind':f'{op}_byte','at_ps':first+i*417,'request':c['request'],'burst':c['burst'],'address':address,'logical_offset':offset}
                    if op == 'write':
                        value = 0 if offset is None else r['operation']['data'][offset]
                        enabled = False if offset is None else r['operation']['byte_enables'][offset]
                        require(c['data'][i] == value and c['byte_enables'][i] == enabled, 'write_data_DM')
                        item.update(value=value,byte_enable=enabled)
                    expected_beats.append(item)
                expected_bursts.append({'kind':'burst_complete','at_ps':end,'request':c['request'],'burst':c['burst']})
                b[op] = t
            elif op == 'refresh':
                require(all(b['row'] is None and (b['pre'] is None or t >= b['pre']+16*CK) for b in banks), 'REF_idle')
                require((t-ready)//REFI-ref_count > -8, 'refresh_pullin_bound')
                require(sum(x > t-2*REFI for x in refs) < 16, 'refresh_rolling_bound')
                refs.append(t); ref_count += 1
            else:
                raise ValueError(f'unsupported_trace_command:{op}')
            history.append((t,c))
        elif kind in ('read_byte','write_byte'):
            actual_beats.append(event)
        elif kind == 'burst_complete':
            actual_bursts.append(event)
        elif kind == 'request_complete':
            require(event['request'] not in completions, 'duplicate_completion')
            completions[event['request']] = t
    key = lambda x: json.dumps(x,sort_keys=True,separators=(',',':'))
    require(sorted(map(key,expected_beats)) == sorted(map(key,actual_beats)), 'physical_byte_bijection')
    require(sorted(map(key,expected_bursts)) == sorted(map(key,actual_bursts)), 'burst_completion_bijection')
    require(set(completions) == set(requests), 'request_completion_bijection')
    for name,r in requests.items():
        n = len(r['operation']['data']) if r['operation']['kind']=='write' else r['operation']['length']
        require(sorted(logical[name]) == list(range(n)), 'logical_byte_bijection')
        require(completions[name] == max(e['at_ps'] for e in expected_bursts if e['request']==name), 'early_logical_completion')
    end = document['end_ps']
    require(isinstance(end,int) and end >= last_time, 'trace_end')
    require(ready is not None and len(refs) >= 5, 'trace_exercises_initialization_and_refresh')
    require((end-ready)//REFI-ref_count <= 8, 'refresh_debt_at_trace_end')
    require(end-(refs[-1] if refs else ready) <= 9*REFI, 'refresh_gap_at_trace_end')
    # This fixture finishes all traffic before its first refresh. Check the
    # selected controller's idle-deadline policy in addition to device legality.
    # JEDEC permits bounded postponement; zero idle debt is not a silicon rule.
    require(max(completions.values()) < ready+REFI, 'fixture_is_idle_before_refresh')
    idle_deadlines = max(0,(end-ready-(CK-1))//REFI)
    require(ref_count >= idle_deadlines, 'controller_idle_refresh_deadline')
    return {'commands':dict(commands),'physical_bytes':len(actual_beats),'requests':len(requests),'scope':'initialization/open-page/data/refresh; power/training/AP require separate gates'}

if __name__ == '__main__':
    try:
        print(json.dumps(check(json.load(sys.stdin)),sort_keys=True))
    except (ValueError,KeyError,IndexError,TypeError) as error:
        print(f'DDR independent monitor rejected: {error}',file=sys.stderr)
        raise SystemExit(1)
