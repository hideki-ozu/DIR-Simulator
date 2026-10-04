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
  const wireKeys=['src_mac','dst_mac','ether_type','data_hex','pad_bytes','mac_bytes','tag','fcs_hex','mac_hex'];
  const filterReasons=['ingress_frame_type','ingress_vlan_membership','destination_mismatch','multicast_not_subscribed','same_ingress','no_vlan_egress','multicast_no_egress','unknown_multicast'];
  const vid=x=>{const n=decimal(x);if(n<1n||n>4094n)fail('VIDが不正です');return n;};
  const priority=x=>{const n=decimal(x);if(n>7n)fail('priorityが不正です');return n;};
  const mac=x=>{if(typeof x!=='string'||!/^([0-9a-f]{2}:){5}[0-9a-f]{2}$/.test(x))fail('MACが不正です');return x;};
  const group=x=>(parseInt(mac(x).slice(0,2),16)&1)!==0;
  const reserved=x=>/^01:80:c2:00:00:0[0-9a-f]$/.test(x);
  function tag(x){if(x===null)return null;fields(x,['vid','pcp','dei']);vid(x.vid);priority(x.pcp);if(decimal(x.dei)>1n)fail('DEIが不正です');return x;}
  const sameTag=(a,b)=>a===null||b===null?a===b:['vid','pcp','dei'].every(k=>a[k]===b[k]);
  function crc32(hex){let crc=0xffffffff;for(let i=0;i<hex.length;i+=2){crc^=parseInt(hex.slice(i,i+2),16);for(let bit=0;bit<8;bit++)crc=(crc>>>1)^((crc&1)?0xedb88320:0);}crc=(crc^0xffffffff)>>>0;return [0,8,16,24].map(shift=>((crc>>>shift)&255).toString(16).padStart(2,'0')).join('');}
  function validateWire(w){
    mac(w.src_mac);mac(w.dst_mac);if(group(w.src_mac)||w.src_mac==='00:00:00:00:00:00'||reserved(w.dst_mac))fail('wire MACが不正です');
    const et=decimal(w.ether_type);if(et<1536n||et>65535n||et===33024n||et===34984n)fail('inner EtherTypeが不正です');tag(w.tag);
    if(typeof w.data_hex!=='string'||! /^(?:[0-9a-f]{2})*$/.test(w.data_hex)||w.data_hex.length>3000||typeof w.mac_hex!=='string'||! /^(?:[0-9a-f]{2})+$/.test(w.mac_hex)||! /^[0-9a-f]{8}$/.test(w.fcs_hex))fail('wire bytesが不正です');
    const padding=Math.max(0,46-w.data_hex.length/2);if(decimal(w.pad_bytes)!==BigInt(padding))fail('wire paddingが不正です');
    const header=w.dst_mac.replaceAll(':','')+w.src_mac.replaceAll(':','')+(w.tag?'8100'+((Number(w.tag.pcp)<<13)|(Number(w.tag.dei)<<12)|Number(w.tag.vid)).toString(16).padStart(4,'0'):'')+Number(et).toString(16).padStart(4,'0');
    const body=header+w.data_hex+'00'.repeat(padding);if(crc32(body)!==w.fcs_hex||body+w.fcs_hex!==w.mac_hex||decimal(w.mac_bytes)!==BigInt(w.mac_hex.length/2))fail('wire header/TCI/length/FCSが不一致です');
  }
  function parseResults(raw) {
    if (raw?.schema_version !== 2 || !['ethernet.l2.store-forward.v1','ethernet.l2.qos.v1','ethernet.l2.vlan.v1'].includes(raw.metadata?.model_profile)) fail('未対応のprofileです');
    const vlan=raw.metadata.model_profile==='ethernet.l2.vlan.v1',qos=vlan||raw.metadata.model_profile==='ethernet.l2.qos.v1';
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
    const policies=new Map(),ingressPolicies=new Map(),flows=new Map();
    if(vlan){
      const ingressPorts=new Set([...directions.values()].map(d=>d.to_port));
      for(const p of array(topology.ports)){
        fields(p,['port','ingress','pvid','admit','default_priority','vlans']);
        if(!directions.has(p.port)||policies.has(p.port)||!ingressPorts.has(p.ingress)||ingressPolicies.has(p.ingress)||owner(p.port)!==owner(p.ingress)||!['all','tagged_only','untagged_only'].includes(p.admit))fail('Port policy参照が不正です');
        const incoming=[...directions.values()].find(d=>d.to_port===p.ingress),outgoing=directions.get(p.port);
        if(owner(incoming.from_port)!==owner(outgoing.to_port))fail('Port policyのtx/rx対が不正です');
        vid(p.pvid);priority(p.default_priority);const members=new Map();let untagged=0;
        for(const v of array(p.vlans)){fields(v,['vid','tagged']);vid(v.vid);if(typeof v.tagged!=='boolean'||members.has(v.vid))fail('VLAN membershipが不正です');if(!v.tagged){untagged++;if(v.vid!==p.pvid)fail('untagged VLANがPVIDと不一致です');}members.set(v.vid,v.tagged);}
        if(!members.has(p.pvid)||untagged>1)fail('PVID membershipが不正です');
        const policy={...p,members};policies.set(p.port,policy);ingressPolicies.set(p.ingress,policy);
      }
      if(policies.size!==directions.size||ingressPolicies.size!==ingressPorts.size)fail('Port policyが不足しています');
      for(const d of devices.values()){
        const keys=new Set();d.groups=new Map();
        for(const entry of array(d.multicast)){
          fields(entry,d.kind==='switch'?['vid','dst_mac','egresses']:['vid','dst_mac']);vid(entry.vid);mac(entry.dst_mac);
          const key=`${entry.vid}/${entry.dst_mac}`;if(!group(entry.dst_mac)||entry.dst_mac==='ff:ff:ff:ff:ff:ff'||reserved(entry.dst_mac)||keys.has(key))fail('multicast表/購読が不正です');keys.add(key);
          if(d.kind==='switch'){const egresses=array(entry.egresses);if(new Set(egresses).size!==egresses.length)fail('multicast egress重複です');for(const port of egresses)if(owner(port)!==d.id||!policies.get(port)?.members.has(entry.vid))fail('multicast egress参照が不正です');d.groups.set(key,new Set(egresses));}
          else{if(![...policies.values()].some(p=>owner(p.port)===d.id&&p.members.has(entry.vid)))fail('購読VLANがmemberではありません');d.groups.set(key,true);}
        }
        if(d.kind==='switch'){
          if(!['flood','drop'].includes(d.unknown_multicast))fail('unknown_multicastが不正です');d.vlanFdb=new Map();
          for(const entry of array(d.vlan_fdb)){fields(entry,['vid','dst_mac','egress']);vid(entry.vid);mac(entry.dst_mac);const key=`${entry.vid}/${entry.dst_mac}`;if(group(entry.dst_mac)||entry.dst_mac==='00:00:00:00:00:00'||d.vlanFdb.has(key)||owner(entry.egress)!==d.id||!policies.get(entry.egress)?.members.has(entry.vid))fail('VLAN FDB参照が不正です');d.vlanFdb.set(key,entry.egress);}
        }
      }
      if(raw.metadata.model_schemas!==undefined){const expected=new Map([['ethernet.frame',3],['ethernet.transfer',3],['ethernet.reception',2]]),seen=new Set();for(const s of array(raw.metadata.model_schemas)){fields(s,['schema_name','schema_version']);if(expected.get(s.schema_name)!==s.schema_version||seen.has(s.schema_name))fail('metadata schemaが不正です');seen.add(s.schema_name);}if(seen.size!==3)fail('metadata schemaが不足しています');}
      for(const f of array(raw.metadata.flows)){fields(f,['flow_id','priority','deadline_ps','dst_mac','tag','source_vlan_id']);name(f.flow_id);priority(f.priority);optional(f.deadline_ps);mac(f.dst_mac);tag(f.tag);vid(f.source_vlan_id);if(reserved(f.dst_mac)||flows.has(f.flow_id)||f.tag&&(f.tag.vid!==f.source_vlan_id||f.tag.pcp!==f.priority))fail('source flow契約が不正です');flows.set(f.flow_id,f);}
    }
    const frames=new Map(),transfers=new Map(),receptions=new Map(),events=[];
    function times(row, names) {const r={...row};for(const k of names)r[k]=actual(row[k]);return r;}
    const schemas={
      'ethernet.frame':['source','src_mac','dst_mac','ether_type','data_hex','pad_bytes','mac_bytes','fcs_hex','mac_hex','generated_ps','ready_ps'],
      'ethernet.transfer':['frame_id','parent_transfer_id','from_port','to_port','queued_ps','sof_ps','eof_ps','release_ps','arrival_ps','planned_eof_ps','planned_release_ps','planned_arrival_ps','status','drop_reason'],
      'ethernet.reception':['frame_id','transfer_id','ingress','observed_ps','ready_ps','planned_ready_ps','status','reason','egress_transfer_ids']
    };
    if(qos){schemas['ethernet.frame'].push('flow_id','priority','deadline_ps');schemas['ethernet.transfer'].push('queue_id','priority');}
    if(vlan){schemas['ethernet.frame'].push('tag','source_vlan_id');schemas['ethernet.transfer'].push('vlan_id','wire');schemas['ethernet.reception'].push('vlan_id','priority');}
    for(const row of array(sim.model_records)) {
      fields(row,['schema_name','schema_version','record_id','subject','request_id','origin_request_id','time_ps','data']);
      const version=vlan?(row.schema_name==='ethernet.reception'?2:3):(qos&&row.schema_name!=='ethernet.reception'?2:1);
      if(!schemas[row.schema_name]||row.schema_version!==version||row.origin_request_id!==null)fail('未対応のレコードschemaです');
      fields(row.data,schemas[row.schema_name]);name(row.record_id);name(row.subject);name(row.request_id);
      const updated=actual(row.time_ps);if(updated===null)fail('更新時刻がありません');
      let r,map,actualKeys;
      if(row.schema_name==='ethernet.frame') {
        actualKeys=['generated_ps','ready_ps'];r=times(row.data,actualKeys);map=frames;
        if(row.record_id!==row.request_id||r.source!==row.subject||devices.get(r.source)?.kind!=='endpoint'||r.generated_ps===null)fail('frame参照が不正です');
        for(const k of ['ether_type','pad_bytes','mac_bytes'])decimal(r[k]);
        if(qos){name(r.flow_id);if(decimal(r.priority)>7n)fail('frame priorityが不正です');r.deadline_ps=optional(r.deadline_ps);}
        if(vlan){vid(r.source_vlan_id);validateWire(r);if(r.tag&&(r.tag.vid!==r.source_vlan_id||r.tag.pcp!==r.priority))fail('source tag/classが不一致です');}
        if(!/^(?:[0-9a-f]{2})*$/.test(r.data_hex)||! /^(?:[0-9a-f]{2})+$/.test(r.mac_hex)||! /^[0-9a-f]{8}$/.test(r.fcs_hex))fail('frame bytesが不正です');
        if(r.ready_ps!==null&&r.ready_ps<r.generated_ps)fail('frame時刻順が不正です');
      } else if(row.schema_name==='ethernet.transfer') {
        actualKeys=['queued_ps','sof_ps','eof_ps','release_ps','arrival_ps'];r=times(row.data,actualKeys);map=transfers;
        for(const k of ['planned_eof_ps','planned_release_ps','planned_arrival_ps'])r[k]=optional(r[k]);
        if(row.request_id!==r.frame_id||row.subject!==r.from_port||row.record_id!==`${r.frame_id}@${r.from_port}`||directions.get(r.from_port)?.to_port!==r.to_port||r.queued_ps===null)fail('transfer参照が不正です');
        if(qos&&(decimal(r.priority)>7n||r.queue_id!==`${r.from_port}.queue.${r.priority}`))fail('class queueが不正です');
        if(vlan){vid(r.vlan_id);fields(r.wire,wireKeys);validateWire(r.wire);if(r.wire.tag&&(r.wire.tag.vid!==r.vlan_id||r.wire.tag.pcp!==r.priority))fail('copy tag/classが不一致です');}
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
        if(r.status==='filtered'?!(vlan?filterReasons:['destination_mismatch','same_ingress']).includes(r.reason):r.reason!==null)fail('filter reasonが不正です');
        if(vlan){vid(r.vlan_id);priority(r.priority);if(['ingress_frame_type','ingress_vlan_membership','destination_mismatch','multicast_not_subscribed'].includes(r.reason)&&(r.ready_ps!==r.observed_ps||r.planned_ready_ps!==null))fail('即filter時刻が不正です');}
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
      if(qos&&!vlan&&t.priority!==f.priority)fail('frameとcopyのpriorityが不一致です');
      if(vlan)for(const k of ['src_mac','dst_mac','ether_type','data_hex','pad_bytes'])if(t.wire[k]!==f[k])fail('copyの不変wire情報が不一致です');
      if(vlan&&t.sof_ps!==null){const d=directions.get(t.from_port),wireBits=8n*(BigInt(t.wire.mac_bytes)+8n),ceilDiv=(n,b)=>(n+b-1n)/b;if(d.bitrate===0n||t.planned_eof_ps!==t.sof_ps+ceilDiv(wireBits*1000000000000n,d.bitrate)||t.planned_release_ps!==t.sof_ps+ceilDiv((wireBits+96n)*1000000000000n,d.bitrate)||t.planned_arrival_ps!==t.planned_eof_ps+d.delay)fail('hop wire長と予定時刻が不一致です');}
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
    if(vlan){
      for(const f of frames.values()){
        const contract=flows.get(f.flow_id);if(!contract||contract.priority!==f.priority||optional(contract.deadline_ps)!==f.deadline_ps||contract.dst_mac!==f.dst_mac||contract.source_vlan_id!==f.source_vlan_id||!sameTag(contract.tag,f.tag)||devices.get(f.source).mac!==f.src_mac)fail('frameとsource flow契約が不一致です');
        const p=policies.get(`${f.source}.tx`);if(!p||!p.members.has(f.source_vlan_id)||p.members.get(f.source_vlan_id)!==Boolean(f.tag)||(f.tag?f.tag.vid:p.pvid)!==f.source_vlan_id)fail('source VLAN policyが不一致です');
        const source=transfers.get(`${f.id}@${f.source}.tx`);if(source&&(source.vlan_id!==f.source_vlan_id||source.priority!==f.priority||wireKeys.some(k=>k==='tag'?!sameTag(source.wire[k],f[k]):source.wire[k]!==f[k])) )fail('source copy wire/classが不一致です');
      }
      for(const r of receptions.values()){
        const t=transfers.get(r.transfer_id),w=t.wire,p=ingressPolicies.get(r.ingress),d=devices.get(r.subject),classified=w.tag?.vid??p.pvid,classPriority=w.tag?.pcp??p.default_priority;
        if(r.vlan_id!==classified||r.priority!==classPriority)fail('ingress分類が不一致です');
        let reason=null,candidates=[];
        if((p.admit==='tagged_only'&&!w.tag)||(p.admit==='untagged_only'&&w.tag))reason='ingress_frame_type';
        else if(!p.members.has(classified))reason='ingress_vlan_membership';
        else if(d.kind==='endpoint'){
          if(w.dst_mac!=='ff:ff:ff:ff:ff:ff'&&w.dst_mac!==d.mac)reason=group(w.dst_mac)?(d.groups.has(`${classified}/${w.dst_mac}`)?null:'multicast_not_subscribed'):'destination_mismatch';
        }else if(r.status!=='processing'){
          const eligible=[...policies.values()].filter(out=>owner(out.port)===d.id&&out.port!==p.port&&out.members.has(classified)).map(out=>out.port).sort(),key=`${classified}/${w.dst_mac}`;
          if(d.vlanFdb.has(key)){const port=d.vlanFdb.get(key);if(port===p.port)reason='same_ingress';else candidates=[port];}
          else if(w.dst_mac!=='ff:ff:ff:ff:ff:ff'&&group(w.dst_mac)){
            if(d.groups.has(key)){candidates=eligible.filter(port=>d.groups.get(key).has(port));if(!candidates.length)reason='multicast_no_egress';}
            else if(d.unknown_multicast==='drop')reason='unknown_multicast';else{candidates=eligible;if(!candidates.length)reason='no_vlan_egress';}
          }else{candidates=eligible;if(!candidates.length)reason='no_vlan_egress';}
        }
        if(reason){if(r.status!=='filtered'||r.reason!==reason)fail('filter判断がpolicyと不一致です');}
        else if(r.status==='filtered')fail('filter理由がpolicyと不一致です');
        if(r.status==='forwarded'){
          const children=r.egress_transfer_ids.map(id=>transfers.get(id));if(JSON.stringify(children.map(child=>child.from_port).sort())!==JSON.stringify(candidates))fail('VLAN転送候補が不一致です');
          for(const child of children){const tagged=policies.get(child.from_port).members.get(classified),expectedTag=tagged?{vid:classified,pcp:classPriority,dei:w.tag?.dei??'0'}:null;if(child.vlan_id!==classified||child.priority!==classPriority||!sameTag(child.wire.tag,expectedTag))fail('egress wire/classがpolicyと不一致です');}
        }
      }
    }
    // Following parents also rejects cycles independently of timestamp equality.
    for(const t of transfers.values()){const seen=new Set();let p=t;while(p){if(seen.has(p.id))fail('parent循環です');seen.add(p.id);p=p.parent_transfer_id===null?null:transfers.get(p.parent_transfer_id);}}
    events.sort((a,b)=>compare(a.time,b.time)||compare(a.id,b.id)||compare(a.kind,b.kind));
    return {raw,start,end,qos,vlan,outputs,policies,ingressPolicies,devices,directions,frames,transfers,receptions,events,eventTimes:[...new Set([start,...events.map(e=>e.time),end])].sort(compare)};
  }
  function transferStateAt(t,time){if(time<t.queued_ps)return 'not_created';if(t.status==='dropped')return 'dropped';if(t.sof_ps===null||time<t.sof_ps)return 'queued';if(t.eof_ps===null||time<t.eof_ps)return 'transmitting';return 'serialized';}
  function receptionStateAt(r,time){if(time<r.observed_ps)return 'not_created';if(r.ready_ps===null||time<r.ready_ps)return 'processing';return r.status;}
  function stateAt(m,time){
    const counts={generated:0,queued:0,transmitting:0,serialized:0,dropped:0,received:0,processing:0,filtered:0,forwarded:0};
    const queues=new Map([...m.directions.keys()].map(p=>[p,[]]));
    const classes=new Map();if(m.qos)for(const out of m.outputs.values())for(const q of out.queues)classes.set(`${out.port}.queue.${q.priority}`,{...q,port:out.port,scheduler:out.scheduler,ids:[],bytes:0n});
    for(const f of m.frames.values())if(f.generated_ps<=time)counts.generated++;
    for(const t of m.transfers.values()){const s=transferStateAt(t,time);if(s!=='not_created')counts[s]++;if(s==='queued'){queues.get(t.from_port).push(t.id);if(m.qos){const q=classes.get(t.queue_id);q.ids.push(t.id);q.bytes+=BigInt((m.vlan?t.wire:m.frames.get(t.frame_id)).mac_bytes);}}}
    for(const r of m.receptions.values()){const s=receptionStateAt(r,time);if(s!=='not_created')counts[s]++;}
    return {counts,queues,classes};
  }
  function stepTransfers(m,destination){const i=m.eventTimes.findIndex(t=>t===destination);if(i<1)return [];const from=m.eventTimes[i-1];return [...m.transfers.values()].filter(t=>t.sof_ps!==null&&t.sof_ps<=destination&&((t.arrival_ps??m.end)>from)).map(t=>({id:t.id,frame:t.frame_id,from:t.from_port,to:t.to_port,start:t.sof_ps<from?from:t.sof_ps,end:t.arrival_ps===null?null:t.arrival_ps,arrived:t.arrival_ps!==null&&t.arrival_ps<=destination}));}
  function visibleTransfers(m,vlanId=null){return [...m.transfers.values()].filter(t=>vlanId===null||t.vlan_id===vlanId);}
  return {parseResults,stateAt,transferStateAt,receptionStateAt,stepTransfers,visibleTransfers,owner};
});
