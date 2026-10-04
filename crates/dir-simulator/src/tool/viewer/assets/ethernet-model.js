/* Ethernet replay uses committed milestones only; all simulation times remain BigInt. */
(function(root, factory) {
  'use strict';
  const api = factory();
  if (typeof module === 'object' && module.exports) module.exports = api;
  else root.DIREthernetModel = api;
})(typeof globalThis === 'object' ? globalThis : this, function() {
  'use strict';
  const fail = text => { throw new Error(`Ethernet: ${text}`); };
  const name = x => typeof x === 'string' && x.length ? x : fail('文字列が不正です');
  const array = x => Array.isArray(x) ? x : fail('配列が必要です');
  const decimal = x => {
    if (typeof x !== 'string' || !/^(0|[1-9][0-9]*)$/.test(x)) fail('正規十進文字列が必要です');
    const n = BigInt(x); if (n > (1n << 64n) - 1n) fail('u64範囲外です'); return n;
  };
  const fields = (x, keys) => {
    if (!x || typeof x !== 'object' || Array.isArray(x) || Object.keys(x).length !== keys.length || keys.some(k => !Object.hasOwn(x,k))) fail('レコードのキー集合が不正です');
  };
  const compare = (a,b) => a < b ? -1 : a > b ? 1 : 0;
  const owner = port => port.slice(0,port.lastIndexOf('.'));
  function parseResults(raw) {
    if (raw?.schema_version !== 2 || !['ethernet.l2.store-forward.v1','ethernet.l2.qos.v1'].includes(raw.metadata?.model_profile)) fail('未対応のprofileです');
    const qos=raw.metadata.model_profile==='ethernet.l2.qos.v1';
    const sim=raw.simulation;
    if (!sim || typeof sim.partial !== 'boolean' || !['events_exhausted','time_limit','execution_failed'].includes(sim.termination)) fail('終了情報が不正です');
    const start=decimal(sim.start_ps),end=decimal(sim.end_ps);
    if(start!==0n || end<start) fail('観測期間が不正です');
    const actual = x => { if(x===null)return null;const n=decimal(x);if(n>end || (!sim.partial && n===end))fail('実績時刻が観測期間外です');return n; };
    const optional = x => x===null ? null : decimal(x);
    const topology=raw.metadata.ethernet_topology;
    if(!topology)fail('Ethernetトポロジーがありません');
    const devices=new Map(),directions=new Map();
    for(const d of array(topology.devices)) {
      name(d.id);if(devices.has(d.id)||!['endpoint','switch'].includes(d.kind))fail('deviceが不正又は重複しています');
      devices.set(d.id,{...d});
    }
    for(const d of array(topology.directions)) {
      name(d.from_port);name(d.to_port);
      if(directions.has(d.from_port)||!devices.has(owner(d.from_port))||!devices.has(owner(d.to_port)))fail('linkの参照が不正です');
      directions.set(d.from_port,{...d,bitrate:decimal(d.bitrate_bps),delay:decimal(d.delay_ps)});
    }
    const outputs=new Map();
    if(qos)for(const out of array(topology.outputs)){
      if(!directions.has(out.port)||outputs.has(out.port)||!['fifo','strict_priority'].includes(out.scheduler))fail('QoS outputが不正です');
      const priorities=new Set();for(const q of array(out.queues)){const priority=decimal(q.priority);if(priority>7n||priorities.has(q.priority))fail('priorityが不正です');priorities.add(q.priority);decimal(q.capacity_frames);if(q.capacity_bytes!==null)decimal(q.capacity_bytes);}
      if(priorities.size!==8)fail('QoSの8クラスが必要です');outputs.set(out.port,out);
    }
    if(qos&&outputs.size!==directions.size)fail('QoS output設定が不足しています');
    const frames=new Map(),transfers=new Map(),receptions=new Map(),events=[];
    function times(row, names) {const r={...row};for(const k of names)r[k]=actual(row[k]);return r;}
    const schemas={
      'ethernet.frame':['source','src_mac','dst_mac','ether_type','data_hex','pad_bytes','mac_bytes','fcs_hex','mac_hex','generated_ps','ready_ps'],
      'ethernet.transfer':['frame_id','parent_transfer_id','from_port','to_port','queued_ps','sof_ps','eof_ps','release_ps','arrival_ps','planned_eof_ps','planned_release_ps','planned_arrival_ps','status','drop_reason'],
      'ethernet.reception':['frame_id','transfer_id','ingress','observed_ps','ready_ps','planned_ready_ps','status','reason','egress_transfer_ids']
    };
    if(qos){schemas['ethernet.frame'].push('flow_id','priority','deadline_ps');schemas['ethernet.transfer'].push('queue_id','priority');}
    for(const row of array(sim.model_records)) {
      fields(row,['schema_name','schema_version','record_id','subject','request_id','origin_request_id','time_ps','data']);
      if(!schemas[row.schema_name]||row.schema_version!==(qos&&row.schema_name!=='ethernet.reception'?2:1)||row.origin_request_id!==null)fail('未対応のレコードschemaです');
      fields(row.data,schemas[row.schema_name]);name(row.record_id);name(row.subject);name(row.request_id);
      const updated=actual(row.time_ps);if(updated===null)fail('更新時刻がありません');
      let r,map,actualKeys;
      if(row.schema_name==='ethernet.frame') {
        actualKeys=['generated_ps','ready_ps'];r=times(row.data,actualKeys);map=frames;
        if(row.record_id!==row.request_id||r.source!==row.subject||devices.get(r.source)?.kind!=='endpoint'||r.generated_ps===null)fail('frame参照が不正です');
        for(const k of ['ether_type','pad_bytes','mac_bytes'])decimal(r[k]);
        if(qos){name(r.flow_id);if(decimal(r.priority)>7n)fail('frame priorityが不正です');r.deadline_ps=optional(r.deadline_ps);}
        if(!/^(?:[0-9a-f]{2})*$/.test(r.data_hex)||! /^(?:[0-9a-f]{2})+$/.test(r.mac_hex)||! /^[0-9a-f]{8}$/.test(r.fcs_hex))fail('frame bytesが不正です');
        if(r.ready_ps!==null&&r.ready_ps<r.generated_ps)fail('frame時刻順が不正です');
      } else if(row.schema_name==='ethernet.transfer') {
        actualKeys=['queued_ps','sof_ps','eof_ps','release_ps','arrival_ps'];r=times(row.data,actualKeys);map=transfers;
        for(const k of ['planned_eof_ps','planned_release_ps','planned_arrival_ps'])r[k]=optional(r[k]);
        if(row.request_id!==r.frame_id||row.subject!==r.from_port||row.record_id!==`${r.frame_id}@${r.from_port}`||directions.get(r.from_port)?.to_port!==r.to_port||r.queued_ps===null)fail('transfer参照が不正です');
        if(qos&&(decimal(r.priority)>7n||r.queue_id!==`${r.from_port}.queue.${r.priority}`))fail('class queueが不正です');
        if(!['queued','transmitting','serialized','dropped'].includes(r.status))fail('transfer状態が不正です');
        if((r.status==='dropped')!==(r.drop_reason==='queue_full') || (r.status!=='dropped'&&r.drop_reason!==null))fail('drop reasonが不正です');
        if((r.status==='serialized')!==(r.eof_ps!==null)||(['serialized','transmitting'].includes(r.status))!==(r.sof_ps!==null))fail('transfer状態と時刻が一致しません');
        if(r.sof_ps===null) {if(['eof_ps','release_ps','arrival_ps','planned_eof_ps','planned_release_ps','planned_arrival_ps'].some(k=>r[k]!==null))fail('SOF前の送信時刻です');}
        else {
          if(r.sof_ps<r.queued_ps||r.planned_eof_ps===null||r.planned_release_ps===null||r.planned_arrival_ps===null||r.planned_eof_ps<=r.sof_ps||r.planned_release_ps<=r.planned_eof_ps||r.planned_arrival_ps<r.planned_eof_ps)fail('link時刻順が不正です');
          for(const k of ['eof','release','arrival'])if(r[`${k}_ps`]!==null&&r[`${k}_ps`]!==r[`planned_${k}_ps`])fail('実績と予定が不一致です');
          if(r.eof_ps===null&&(r.arrival_ps!==null||r.release_ps!==null))fail('EOF前に到達しています');
        }
      } else {
        actualKeys=['observed_ps','ready_ps'];r=times(row.data,actualKeys);map=receptions;r.planned_ready_ps=optional(r.planned_ready_ps);
        if(row.request_id!==r.frame_id||row.record_id!==`${r.transfer_id}@rx`||row.subject!==owner(r.ingress)||!devices.has(row.subject)||r.observed_ps===null)fail('reception参照が不正です');
        if(!['processing','received','filtered','forwarded'].includes(r.status)||(r.status==='processing')!==(r.ready_ps===null)||r.ready_ps!==null&&r.ready_ps<r.observed_ps)fail('reception状態が不正です');
        array(r.egress_transfer_ids);if(new Set(r.egress_transfer_ids).size!==r.egress_transfer_ids.length)fail('重複copyです');
        if(r.status==='filtered'?!['destination_mismatch','same_ingress'].includes(r.reason):r.reason!==null)fail('filter reasonが不正です');
        if(r.status!=='forwarded'&&r.egress_transfer_ids.length)fail('未転送の子copyです');
        if(r.planned_ready_ps!==null&&(r.planned_ready_ps<r.observed_ps||r.ready_ps!==null&&r.ready_ps!==r.planned_ready_ps))fail('受信処理時刻が不正です');
      }
      if(map.has(row.record_id))fail('レコードが重複しています');
      r.id=row.record_id;r.subject=row.subject;r.raw=row;r.updated=updated;map.set(r.id,r);
      let last=null;for(const k of actualKeys)if(r[k]!==null){events.push({time:r[k],id:r.id,kind:k,frame:row.request_id});if(last===null||r[k]>last)last=r[k];}
      if(updated!==last)fail('更新時刻が最後の実績と一致しません');
    }
    for(const t of transfers.values()) {
      const f=frames.get(t.frame_id);if(!f||f.generated_ps>t.queued_ps)fail('transferのframe参照が不正です');
      if(qos&&t.priority!==f.priority)fail('frameとcopyのpriorityが不一致です');
      if(t.parent_transfer_id===null){if(t.from_port!==`${f.source}.tx`||f.ready_ps!==t.queued_ps)fail('source copyが不正です');}
      else {const parent=transfers.get(t.parent_transfer_id),rx=receptions.get(`${t.parent_transfer_id}@rx`);if(!parent||parent.frame_id!==t.frame_id||!rx||rx.status!=='forwarded'||!rx.egress_transfer_ids.includes(t.id)||rx.ready_ps!==t.queued_ps||owner(parent.to_port)!==owner(t.from_port))fail('parent copyが不正です');}
      const rx=receptions.get(`${t.id}@rx`);if((t.arrival_ps!==null)!==Boolean(rx))fail('arrivalとreceptionが一致しません');
      if(rx&&(rx.observed_ps!==t.arrival_ps||rx.ingress!==t.to_port||rx.frame_id!==t.frame_id))fail('reception到達参照が不正です');
    }
    for(const r of receptions.values()) {
      if(!transfers.has(r.transfer_id))fail('受信元copyがありません');
      const device=devices.get(r.subject);
      if(r.status==='received'&&device.kind!=='endpoint'||r.status==='forwarded'&&device.kind!=='switch')fail('deviceと受信状態が不一致です');
      for(const id of r.egress_transfer_ids)if(transfers.get(id)?.parent_transfer_id!==r.transfer_id)fail('egress copyがありません');
    }
    for(const f of frames.values())if((f.ready_ps!==null)!==transfers.has(`${f.id}@${f.source}.tx`))fail('source readyとcopyが一致しません');
    // Following parents also rejects cycles independently of timestamp equality.
    for(const t of transfers.values()){const seen=new Set();let p=t;while(p){if(seen.has(p.id))fail('parent循環です');seen.add(p.id);p=p.parent_transfer_id===null?null:transfers.get(p.parent_transfer_id);}}
    events.sort((a,b)=>compare(a.time,b.time)||compare(a.id,b.id)||compare(a.kind,b.kind));
    return {raw,start,end,qos,outputs,devices,directions,frames,transfers,receptions,events,eventTimes:[...new Set([start,...events.map(e=>e.time),end])].sort(compare)};
  }
  function transferStateAt(t,time){if(time<t.queued_ps)return 'not_created';if(t.status==='dropped')return 'dropped';if(t.sof_ps===null||time<t.sof_ps)return 'queued';if(t.eof_ps===null||time<t.eof_ps)return 'transmitting';return 'serialized';}
  function receptionStateAt(r,time){if(time<r.observed_ps)return 'not_created';if(r.ready_ps===null||time<r.ready_ps)return 'processing';return r.status;}
  function stateAt(m,time){
    const counts={generated:0,queued:0,transmitting:0,serialized:0,dropped:0,received:0,processing:0,filtered:0,forwarded:0};
    const queues=new Map([...m.directions.keys()].map(p=>[p,[]]));
    const classes=new Map();if(m.qos)for(const out of m.outputs.values())for(const q of out.queues)classes.set(`${out.port}.queue.${q.priority}`,{...q,port:out.port,scheduler:out.scheduler,ids:[],bytes:0n});
    for(const f of m.frames.values())if(f.generated_ps<=time)counts.generated++;
    for(const t of m.transfers.values()){const s=transferStateAt(t,time);if(s!=='not_created')counts[s]++;if(s==='queued'){queues.get(t.from_port).push(t.id);if(m.qos){const q=classes.get(t.queue_id);q.ids.push(t.id);q.bytes+=BigInt(m.frames.get(t.frame_id).mac_bytes);}}}
    for(const r of m.receptions.values()){const s=receptionStateAt(r,time);if(s!=='not_created')counts[s]++;}
    return {counts,queues,classes};
  }
  function stepTransfers(m,destination){const i=m.eventTimes.findIndex(t=>t===destination);if(i<1)return [];const from=m.eventTimes[i-1];return [...m.transfers.values()].filter(t=>t.sof_ps!==null&&t.sof_ps<=destination&&((t.arrival_ps??m.end)>from)).map(t=>({id:t.id,frame:t.frame_id,from:t.from_port,to:t.to_port,start:t.sof_ps<from?from:t.sof_ps,end:t.arrival_ps===null?null:t.arrival_ps,arrived:t.arrival_ps!==null&&t.arrival_ps<=destination}));}
  return {parseResults,stateAt,transferStateAt,receptionStateAt,stepTransfers,owner};
});
