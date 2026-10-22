#!/usr/bin/env python3
"""Independent bitwise CRC, source-byte and rational-time oracle on real outputs."""
import json,sys
from pathlib import Path

def crc(data):
    value=0xffffffff
    for byte in data:
        value ^= byte
        for _ in range(8):value=(value>>1) ^ (0xedb88320 if value&1 else 0)
    return value ^ 0xffffffff

count={'outputs':0,'packets':0,'wire':0,'fragments':0,'raw':0,'late_pcs':0}
for arg in sys.argv[1:]:
    path=Path(arg);v=json.loads(path.read_text())
    assert v['simulation']['termination']=='time_limit',path
    rows=v['simulation']['model_records'];transfers={r['record_id']:r['data'] for r in rows if r['schema_name'] in ('ethernet.dynamic.transfer','dir.preemption.transfer')}
    wire_events=[r['data'] for r in rows if r['schema_name']=='ethernet.control.wire.event']
    for r in rows:
        if r['schema_name']!='dir.mac_merge.packet':continue
        p=r['data'];b=bytes.fromhex(p['wire_hex']);rate=int(p['bitrate_bps']);sof=int(p['sof_ps']);count['packets']+=1
        assert b[:6]==bytes([0x55])*6
        assert int(p['planned_eof_ps'])-sof==(len(b)*8*10**12+rate-1)//rate
        assert int(p['planned_release_ps'])-int(p['planned_eof_ps'])==(96*10**12+rate-1)//rate
        if p['kind']=='Express':
            assert b[:8]==bytes([0x55])*7+bytes([0xd5])
            assert b[-4:]==crc(b[8:-4]).to_bytes(4,'little'),(path,p['packet_id'])
        elif p['kind'] in ('Verify','Respond'):
            assert b[-4:]==(crc(b[8:-4])^0xffff).to_bytes(4,'little')
        else:
            source=bytes.fromhex(transfers[p['transfer']]['wire']['mac_hex']);a=int(p['source_start']);z=int(p['source_end'])
            assert b[8:-4]==source[a:z]
            expected=source[-4:] if p['final_packet'] else (crc(source[:z])^0xffff).to_bytes(4,'little')
            assert b[-4:]==expected,(path,p['packet_id'])
            count['fragments']+=1
        if p.get('wire_control_id'):
            count['wire']+=1
            events=[e for e in wire_events if e['packet_id']==p['wire_control_id'] and e['event_kind']=='tx_started']
            assert len(events)==1
            assert bytes.fromhex(events[0]['bytes_hex'])==b[8:]
        if p.get('raw_phy'):
            count['raw']+=1;sample=p['raw_phy']['start_sample'];den=12_000_000_000;n=sample*10**12+12345*den
            assert sof==(n+den-1)//den and 0<=sof*den-n<den
            if p['arrival_ps'] is not None and p['release_ps'] is not None and int(p['arrival_ps'])>int(p['release_ps']):count['late_pcs']+=1
        if p['arrival_ps'] is not None:assert p['received_hex']==p['wire_hex']
    count['outputs']+=1
assert count['wire'] and count['fragments'] and count['raw'] and count['late_pcs'],count
print(json.dumps(count,sort_keys=True))
