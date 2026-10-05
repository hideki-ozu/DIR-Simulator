#!/usr/bin/env python3
"""Check independent analytic expectations and complete fixture references, not a simulator."""
import configparser
import json
from pathlib import Path

ROOT = Path(__file__).resolve().parent

def read(name):
    return json.loads((ROOT / name).read_text())

def main():
    cases = read('scenarios.json')['cases']
    for c in cases:
        name = c['name']
        ini = configparser.ConfigParser()
        ini.read(ROOT / c['config'])
        cfg = {k:v.strip(chr(34)) for k,v in ini['General'].items()}
        model = read(cfg['model-config'])
        workload = read(cfg['workload'])
        assert (ROOT / cfg['ned-path'] / 'demo' / 'Main.ned').is_file()
        assert model['profile'] == cfg['model-profile']
        assert cfg['sim-time-limit'] == f"{c['T']}ps"
        assert model['clock_period'] == '10ps'
        e = c['expected']
        tx = {r['id']: r for r in e['transactions']}
        requests = {f"{g['id']}:{i}": (int(t[:-2]), g['transaction'])
                    for g in workload['generators'] for i, t in enumerate(g['times'])
                    if int(t[:-2]) < c['T']}
        assert set(tx) == set(requests)
        assert all(r['status'] in {'pending', 'active', 'completed', 'dropped'} for r in tx.values())
        bits = sum(8 * requests[i][1]['bytes'] for i,r in tx.items()
                   if r['status'] == 'completed' and r['response'] == 'OKAY')
        assert bits == e['delivered_bits']
        for i, r in tx.items():
            assert r['start'] is None or r['start'] >= requests[i][0]
            assert r['end'] is None or r['start'] < r['end'] < c['T']
            plan = r['active_plan']
            if plan is not None:
                assert r['status'] == 'active'
                assert plan['start_ps'] < c['T'] <= plan['planned_end_ps']
                assert r['end'] is None
        if name == 'soc-round-robin':
            duration = ((8 + model['bytes_per_cycle'] - 1) // model['bytes_per_cycle'] + model['targets'][0]['service_cycles']) * 10
            assert [r['id'] for r in e['transactions']] == ['a:0', 'b:0', 'a:1']
            assert [r['start'] for r in e['transactions']] == [0,duration,2*duration]
            assert [r['end'] for r in e['transactions']] == [duration,2*duration,3*duration]
            assert e['busy_ps'] == 3*duration
        elif name == 'soc-capacity-stop':
            assert model['sources'][0]['capacity'] == 1
            assert tx['a:1']['status'] == 'dropped'
            duration = (1+model['targets'][0]['service_cycles'])*10
            assert tx['a:0']['end'] == tx['a:2']['start'] == duration
            assert tx['a:0']['response'] == 'ERROR'
            assert 2*duration == c['T'] and tx['a:2']['status'] == 'active'
            assert e['busy_ps'] == c['T']
            assert tx['a:2']['active_plan'] == dict(resource='Main.bus',hop=0,start_ps=20,planned_end_ps=2*duration)
        elif name == 'ahb-wait-error':
            normal = (2+model['targets'][0]['wait_cycles'])*10
            error = (3+model['targets'][0]['wait_cycles'])*10
            assert tx['a:0']['end'] == tx['b:0']['start'] == normal
            assert tx['b:0']['end'] == e['busy_ps'] == normal+error
            assert tx['b:0']['response'] == 'ERROR'
        elif name == 'ahb-boundary':
            assert requests['a:0'][1]['address'] >= model['targets'][0]['size']
            assert c['T'] == 3*10 == e['busy_ps']
            assert tx['a:0']['status'] == 'active' and tx['a:1']['status'] == 'dropped'
            assert tx['a:0']['active_plan'] == dict(resource='Main.bus',hop=0,start_ps=0,planned_end_ps=3*10)
        else:
            routers = {r['node']:(r['x'],r['y']) for r in model['routers']}
            endpoint = {r['node']:r['router'] for r in model['endpoints']}
            generators = {g['id']:g for g in workload['generators']}
            for ident in requests:
                completed_hops = [h for h in e['hops'] if h[0] == ident]
                g = generators[ident.split(':')[0]]
                x,y = routers[endpoint[g['node']]]
                dx,dy = routers[endpoint[g['transaction']['destination']]]
                route=[]
                while x != dx:
                    direction = 'east' if x < dx else 'west'
                    route.append(f'Main.r{x}{y}:out_{direction}')
                    x += 1 if x < dx else -1
                while y != dy:
                    direction = 'north' if y < dy else 'south'
                    route.append(f'Main.r{x}{y}:out_{direction}')
                    y += 1 if y < dy else -1
                route.append(f'Main.r{x}{y}:local_out')
                assert [h[1] for h in completed_hops] == route[:len(completed_hops)]
                duration = ((g['transaction']['bytes']+model['bytes_per_cycle']-1)//model['bytes_per_cycle']+model['link_cycles'])*10
                assert all(h[3]-h[2] == duration and h[3] < c['T'] for h in completed_hops)
                if tx[ident]['status']=='completed':
                    assert len(completed_hops)==len(route)
                    assert tx[ident]['start']==completed_hops[0][2]
                    assert tx[ident]['end']==completed_hops[-1][3]
            for resource in {h[1] for h in e['hops']}:
                intervals=sorted((h[2],h[3]) for h in e['hops'] if h[1]==resource)
                assert all(a[1]<=b[0] for a,b in zip(intervals,intervals[1:]))
            if name=='noc-xy':
                assert tx['a:0']['end']==3*((8+3)//4)*10
            else:
                # e10 local output first serves 16 bytes. a0 occupies west input
                # until that local output becomes free, blocking a1's east grant.
                release=((16+3)//4)*10
                assert tx['a:1']['start']==release
                assert tx['a:0']['end']==release+10
                assert tx['a:1']['end']==tx['a:2']['start']+10==release+20
                assert tx['a:2']['status']=='active' and release+30>c['T']
                assert tx['a:3']['status']=='dropped' and model['source_capacity']==1
                assert tx['a:2']['active_plan'] == dict(resource='Main.r10:local_out',hop=1,start_ps=release+20,planned_end_ps=release+30)
        print(f'{name}: analytic input/timing/order/conservation PASS')
    assert len(cases)==6
    print('6 analytic fixtures PASS; product CLI and event-loop tests NOT RUN')

if __name__ == '__main__':
    main()
