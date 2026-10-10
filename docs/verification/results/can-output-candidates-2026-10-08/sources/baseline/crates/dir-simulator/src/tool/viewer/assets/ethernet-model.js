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
  const Q=1000000000000n;
  const wide = x => {
    if(typeof x!=='string'||!/^(0|[1-9][0-9]*)$/.test(x))fail('u128正規十進文字列が必要です');
    const n=BigInt(x);if(n>(1n<<128n)-1n)fail('u128範囲外です');return n;
  };
  function ipBytes(family,value) {
    name(value);
    const ipv4=x=>{const parts=x.split('.');if(parts.length!==4||parts.some(p=>!/^(0|[1-9][0-9]{0,2})$/.test(p)||Number(p)>255))fail('IPv4 addressが不正です');return parts.map(Number);};
    if(family==='ipv4')return ipv4(value);
    if(family!=='ipv6'||value.includes('%')||value.split('::').length>2)fail('IP family/addressが不正です');
    let text=value;if(text.includes('.')){const i=text.lastIndexOf(':'),v4=ipv4(text.slice(i+1));text=text.slice(0,i+1)+((v4[0]<<8)|v4[1]).toString(16)+':'+((v4[2]<<8)|v4[3]).toString(16);}
    const compressed=text.includes('::'),parts=text.split('::'),left=parts[0]?parts[0].split(':'):[],right=parts[1]?parts[1].split(':'):[];
    if([...left,...right].some(p=>!/^[0-9a-fA-F]{1,4}$/.test(p))||(compressed?left.length+right.length>=8:left.length!==8))fail('IPv6 addressが不正です');
    const words=compressed?[...left,...Array(8-left.length-right.length).fill('0'),...right]:left;
    return words.flatMap(p=>{const n=parseInt(p,16);return [n>>8,n&255];});
  }
  function validateIpMulticast(value,destination,etherType) {
    if(value===null)return null;fields(value,['family','source','group']);
    const source=ipBytes(value.family,value.source),groupBytes=ipBytes(value.family,value.group);
    const v4=value.family==='ipv4',isGroup=bytes=>v4?bytes[0]>=224&&bytes[0]<=239:bytes[0]===255;
    if(isGroup(source)||source.every(b=>b===0)||v4&&source.every(b=>b===255)||!isGroup(groupBytes))fail('IP multicast source/groupが不正です');
    const mapped=v4?[1,0,94,groupBytes[1]&127,groupBytes[2],groupBytes[3]]:[51,51,...groupBytes.slice(-4)];
    if(mapped.map(b=>b.toString(16).padStart(2,'0')).join(':')!==destination||etherType!==(v4?'2048':'34525'))fail('IP groupとMAC/EtherTypeが不一致です');
    return {family:value.family,source,group:groupBytes};
  }
  function prepareSchedule(s) {
    if(s===null)return null;
    fields(s,['id','base_time_ps','cycle_time_ps','entries']);name(s.id);
    const base=decimal(s.base_time_ps),cycle=decimal(s.cycle_time_ps);let total=0n;
    const entries=array(s.entries).map(e=>{fields(e,['duration_ps','open_priorities']);const duration=decimal(e.duration_ps),open=array(e.open_priorities);if(duration===0n||new Set(open).size!==open.length||open.some(p=>!Number.isInteger(p)||p<0||p>7))fail('GCL entryが不正です');const entry={start:total,end:total+duration,open};total+=duration;return entry;});
    if(!entries.length||cycle===0n||total!==cycle||total>(1n<<64n)-1n)fail('GCL周期が不正です');
    return {...s,base,cycle,entries};
  }
  function scheduleStateAt(schedule,time) {
    if(schedule===null)return {open:[0,1,2,3,4,5,6,7],next:null};
    const s=schedule.base===undefined?prepareSchedule(schedule):schedule;
    if(time<s.base)return {open:[],next:s.base};
    const phase=(time-s.base)%s.cycle,index=s.entries.findIndex(e=>phase<e.end),entry=s.entries[index],mask=canonical([...entry.open].sort());let next=time+entry.end-phase;
    for(let offset=1;offset<=s.entries.length;offset++){const following=s.entries[(index+offset)%s.entries.length];if(canonical([...following.open].sort())!==mask)return {open:[...entry.open],next};next+=following.end-following.start;}
    return {open:[...entry.open],next:null};
  }
  function openElapsed(schedule,priority,start,end) {
    if(end<start)fail('credit時刻が逆行しています');
    if(schedule===null)return end-start;
    const s=schedule.base===undefined?prepareSchedule(schedule):schedule;
    const prefix=time=>{if(time<=s.base)return 0n;const duration=time-s.base,cycles=duration/s.cycle,phase=duration%s.cycle;let perCycle=0n,remainder=0n;for(const e of s.entries)if(e.open.includes(priority)){perCycle+=e.end-e.start;if(phase>e.start)remainder+=(phase<e.end?phase:e.end)-e.start;}return cycles*perCycle+remainder;};
    return prefix(end)-prefix(start);
  }
  function creditNumeratorAt(value,slope,start,end,{schedule=null,priority=0,sending=false,backlog=true,hi=(1n<<128n)-1n,lo=(1n<<128n)-1n}={}) {
    if(end<start)fail('credit時刻が逆行しています');
    if(!sending&&!backlog&&value>=0n)return 0n;
    const elapsed=sending?end-start:openElapsed(schedule,priority,start,end);
    let result=value+slope*elapsed;
    if(result>hi)result=hi;if(result < -lo)result=-lo;
    if(!sending&&!backlog&&result>0n)result=0n;
    return result;
  }
  function nextGateFit(schedule,priority,time,occupancy,update=null) {
    if(occupancy<=0n)fail('占有時間が不正です');
    if(schedule===null)return update!==null&&time+occupancy>update?null:time;
    const s=schedule.base===undefined?prepareSchedule(schedule):schedule;
    const open=s.entries.filter(e=>e.open.includes(priority));if(!open.length)return null;
    if(open.length===s.entries.length){const candidate=time<s.base?s.base:time;return update!==null&&candidate+occupancy>update?null:candidate;}
    const runs=[];for(const e of open){const last=runs.at(-1);if(last&&last.end===e.start)last.end=e.end;else runs.push({start:e.start,end:e.end});}
    if(runs.length>1&&runs[0].start===0n&&runs.at(-1).end===s.cycle){const last=runs.pop(),first=runs.shift();runs.unshift({start:last.start-s.cycle,end:first.end});}
    const start=time<s.base?s.base:time,phase=(start-s.base)%s.cycle,cycleStart=start-phase;
    let best=null;
    // The merged wrap run starts in the previous cycle; include two following occurrences.
    for(const run of runs){if(run.end-run.start<occupancy)continue;for(const shift of [0n,s.cycle,2n*s.cycle]){const a=cycleStart+run.start+shift,b=cycleStart+run.end+shift,candidate=a>start?a:start;if(candidate<s.base||candidate+occupancy>b||update!==null&&candidate+occupancy>update)continue;if(best===null||candidate<best)best=candidate;}}
    return best;
  }
  function potentialTokens(value,rate,last,time,capacity) {
    if(time<last||value<0n||value>capacity||rate<0n)fail('meter状態が不正です');
    const refill=rate*(time-last),room=capacity-value;return value+(refill<room?refill:room);
  }
  const canonical=value=>JSON.stringify(value,(_key,v)=>v&&typeof v==='object'&&!Array.isArray(v)?Object.fromEntries(Object.keys(v).sort().map(k=>[k,v[k]])):v);
  function preparePolicyHistory(records,{ports,switches,end,links=new Map()}) {
    const histories=[],tables={mac:new Map(),membership:new Map(),router:new Map(),registration:new Map()},generations=new Map();
    let epoch=null,generation=0n,lastTime=0n,snapshot=null,staticVlans=null;
    const port=p=>{if(!ports.has(p))fail('policyのport参照が不正です');};
    const switchId=id=>{if(!switches.has(id))fail('policyのSwitch参照が不正です');};
    function validateSnapshot(value,currentEpoch,currentGeneration) {
      fields(value,['policy_epoch','topology_generation','roles','link_up','effective_vlans']);
      if(decimal(value.policy_epoch)!==currentEpoch||decimal(value.topology_generation)!==currentGeneration)fail('policy snapshotの世代が不一致です');
      for(const [p,role]of Object.entries(value.roles)){port(p);if(!['root','designated','alternate','disabled','converging'].includes(role))fail('STP roleが不正です');}
      for(const [p,up]of Object.entries(value.link_up)){port(p);if(typeof up!=='boolean')fail('link stateが不正です');}
      for(const [p,vlans]of Object.entries(value.effective_vlans)){port(p);if(!vlans||typeof vlans!=='object'||Array.isArray(vlans))fail('VID tableが不正です');for(const [v,tagged]of Object.entries(vlans)){vid(v);if(typeof tagged!=='boolean')fail('VID taggingが不正です');}}
      for(const map of [value.roles,value.link_up,value.effective_vlans])if(Object.keys(map).length!==ports.size)fail('policy snapshotのportが不足しています');
      return value;
    }
    function validateKey(table,key) {
      const keys={mac:['switch','vid','mac'],membership:['switch','vid','family','group','port'],router:['switch','vid','family','port'],registration:['port','vid']}[table];
      if(!keys)fail('未知のpolicy tableです');fields(key,keys);vid(key.vid);
      if(key.switch!==undefined)switchId(key.switch);if(key.port!==undefined){port(key.port);if(key.switch!==undefined&&owner(key.port)!==key.switch)fail('policy portの所属が不一致です');}
      if(table==='mac'){mac(key.mac);if(group(key.mac)||key.mac==='00:00:00:00:00:00')fail('学習MACが不正です');}
      if(key.family!==undefined&&!['ipv4','ipv6'].includes(key.family))fail('policy IP familyが不正です');
      if(key.group!==undefined){const bytes=ipBytes(key.family,key.group);if(key.family==='ipv4'?bytes[0]<224||bytes[0]>239:bytes[0]!==255)fail('membership groupが不正です');}
    }
    function validateLease(table,key,value,time) {
      if(value===null)return;fields(value,['expires_at','generation','value']);
      if(decimal(value.expires_at)<=time||decimal(value.generation)===0n)fail('policy期限/世代が不正です');
      if(table==='mac'){port(value.value);if(owner(value.value)!==key.switch)fail('学習portの所属が不一致です');}
      else if(table==='membership'){fields(value.value,['mode','sources']);if(!['include','exclude'].includes(value.value.mode))fail('source filter modeが不正です');const sources=array(value.value.sources).map(source=>{const bytes=ipBytes(key.family,source);if(bytes.every(b=>b===0)||(key.family==='ipv4'?bytes[0]>=224:bytes[0]===255))fail('filter sourceがunicastではありません');return bytes.join(',');});if(new Set(sources).size!==sources.length)fail('source setが重複しています');}
      else if(value.value!==true)fail('policy lease値が不正です');
    }
    for(const r of records){
      fields(r,['time_ps','policy_epoch','topology_generation','initial','changes']);
      const time=decimal(r.time_ps),next=decimal(r.policy_epoch),topology=decimal(r.topology_generation);
      if(time>end||time<lastTime||typeof r.initial!=='boolean'||(epoch===null?(!r.initial||time!==0n||next!==0n||topology!==0n):(r.initial||next!==epoch+1n)))fail('policy commit順序が不正です');
      if(topology<generation||topology>generation+1n)fail('topology世代が不正です');
      const policyBefore=snapshot;let expectedRoles=snapshot?{...snapshot.roles}:null,expectedLinks=snapshot?{...snapshot.link_up}:null;
      for(const change of array(r.changes)){
        fields(change,['table','key','before','after']);
        if(change.table==='link'){fields(change.key,['link']);const affected=links.get(change.key.link);if(!affected||typeof change.before!=='boolean'||typeof change.after!=='boolean'||affected.some(p=>expectedLinks?.[p]!==change.before))fail('link差分が不正です');for(const p of affected)expectedLinks[p]=change.after;continue;}
        if(change.table==='topology'){if(change.key!==null||decimal(change.before)!==generation||decimal(change.after)!==topology||topology!==generation+1n)fail('topology差分が不正です');expectedRoles={};for(const pair of links.values()){const inter=pair.every(p=>switches.has(owner(p)));for(const p of pair)expectedRoles[p]=!expectedLinks[p]?'disabled':inter?'converging':'designated';}continue;}
        if(change.table==='roles'){if(change.key!==null||canonical(change.before)!==canonical(expectedRoles))fail('roles差分が不正です');for(const [p,role]of Object.entries(change.after)){port(p);if(!['root','designated','alternate','disabled','converging'].includes(role))fail('STP roleが不正です');}expectedRoles=change.after;continue;}
        if(change.table==='policy'){if(change.key!==null||canonical(change.before)!==canonical(policyBefore))fail('policy snapshotのbeforeが不一致です');snapshot=validateSnapshot(change.after,next,topology);if(staticVlans===null)staticVlans=snapshot.effective_vlans;else{const effective=Object.fromEntries(Object.entries(staticVlans).map(([p,v])=>[p,{...v}]));for(const [key,lease]of tables.registration){const k=JSON.parse(key);effective[k.port][k.vid]=true;}if(canonical(snapshot.roles)!==canonical(expectedRoles)||canonical(snapshot.link_up)!==canonical(expectedLinks)||canonical(snapshot.effective_vlans)!==canonical(effective))fail('policy snapshotとtable差分が不一致です');}continue;}
        validateKey(change.table,change.key);const table=tables[change.table],key=canonical(change.key),old=table.get(key)??null;
        if(canonical(change.before)!==canonical(old))fail('table差分のbeforeが不一致です');
        validateLease(change.table,change.key,change.after,time);
        if(change.after!==null){const ledgerKey=change.table+'/'+key,n=decimal(change.after.generation),previous=generations.get(ledgerKey)??0n;if(n<=previous)fail('lease世代が再利用されています');generations.set(ledgerKey,n);table.set(key,change.after);}else table.delete(key);
      }
      if(!snapshot)fail('初期policy snapshotがありません');
      epoch=next;generation=topology;lastTime=time;
      snapshot={...snapshot,policy_epoch:next.toString(),topology_generation:topology.toString()};
      histories.push({time,epoch,generation,snapshot,changes:r.changes,checkpoint:histories.length%64===0?Object.fromEntries(Object.entries(tables).map(([k,v])=>[k,new Map(v)])):null});
    }
    if(!histories.length)fail('policy履歴がありません');return histories;
  }
  function policyStateAt(history,time) {
    let lo=0,hi=history.length;while(lo<hi){const mid=Math.floor((lo+hi)/2);if(history[mid].time<=time)lo=mid+1;else hi=mid;}const index=lo-1;if(index<0)return null;
    let checkpoint=index;while(!history[checkpoint].checkpoint)checkpoint--;
    const tables=Object.fromEntries(Object.entries(history[checkpoint].checkpoint).map(([k,v])=>[k,new Map(v)]));
    for(let i=checkpoint+1;i<=index;i++)for(const change of history[i].changes)if(Object.hasOwn(tables,change.table)){const key=canonical(change.key);if(change.after===null)tables[change.table].delete(key);else tables[change.table].set(key,change.after);}
    for(const table of Object.values(tables))for(const [key,value]of table)table.set(key,JSON.parse(JSON.stringify(value)));const row=history[index];return {epoch:row.epoch,generation:row.generation,snapshot:JSON.parse(JSON.stringify(row.snapshot)),tables};
  }
  function validateDynamicRows(raw,policyRows,controlRows,tsnRows,m) {
    const metadata=raw.metadata,cfg=metadata.ethernet_dynamic;
    fields(cfg,['config','initial_policy']);
    const config=cfg.config;fields(config,['mac_age_ps','convergence_ps','bridges','links','registrable','limits']);
    if(decimal(config.mac_age_ps)===0n)fail('MAC ageが不正です');decimal(config.convergence_ps);
    const ports=new Set(m.directions.keys()),switches=new Set([...m.devices.values()].filter(d=>d.kind==='switch').map(d=>d.id));
    const bridges=new Set(),bridgeIds=new Set();for(const b of array(config.bridges)){fields(b,['instance','bridge_id']);const id=decimal(b.bridge_id);if(!switches.has(b.instance)||bridges.has(b.instance)||bridgeIds.has(id))fail('bridge参照が不正です');bridges.add(b.instance);bridgeIds.add(id);}if(bridges.size!==switches.size)fail('bridge設定が不足しています');
    const links=new Map(),covered=new Set();for(const l of array(config.links)){fields(l,['id','ports','up','cost']);name(l.id);if(links.has(l.id)||typeof l.up!=='boolean'||decimal(l.cost)===0n||array(l.ports).length!==2||l.ports[0]===l.ports[1])fail('dynamic linkが不正です');for(const p of l.ports){if(!ports.has(p)||covered.has(p))fail('dynamic link portが重複又は不正です');covered.add(p);}if(owner(m.directions.get(l.ports[0]).to_port)!==owner(l.ports[1])||owner(m.directions.get(l.ports[1]).to_port)!==owner(l.ports[0]))fail('dynamic link対が不一致です');links.set(l.id,l.ports);}if(covered.size!==ports.size)fail('dynamic linkが不足しています');
    for(const r of array(config.registrable)){fields(r,['port','vid','tagged']);if(!ports.has(r.port)||r.tagged!==true)fail('registrable portが不正です');vid(typeof r.vid==='number'?String(r.vid):r.vid);}
    fields(config.limits,['mac_entries','membership_entries','sources_per_entry','registrations','control_events','pending_timers','visits_per_frame']);for(const limit of Object.values(config.limits))if(typeof limit==='number'? !Number.isSafeInteger(limit)||limit<1:decimal(limit)<1n)fail('dynamic上限が不正です');
    const rows=policyRows.sort((a,b)=>compare(decimal(a.time_ps),decimal(b.time_ps))||compare(a._effect,b._effect));
    const policyIds=new Set();for(const r of rows){if(policyIds.has(name(r._id)))fail('policy IDが重複しています');policyIds.add(r._id);}
    const history=preparePolicyHistory(rows.map(({_id,_effect,...r})=>r),{ports,switches,end:m.end,links});
    if(canonical(history[0].snapshot)!==canonical(cfg.initial_policy))fail('metadata初期policyが不一致です');
    const epochRows=new Map(history.map((r,i)=>[r.epoch,{...r,effect:rows[i]._effect}]));
    const epochAt=(value,time)=>{const n=decimal(value),row=epochRows.get(n),next=epochRows.get(n+1n);if(!row||row.time>time||next&&next.time<time)fail('存在しない又は時刻不一致のpolicy epochです');return n;};
    controlRows.sort((a,b)=>compare(decimal(a.time_ps),decimal(b.time_ps))||compare(decimal(a.data.effect_seq),decimal(b.data.effect_seq)));const controls=new Set();
    for(const row of controlRows){
      const d=row.data;fields(d,['control_id','kind','scheduled_ps','applied_ps','batch_ordinal','epoch_before','epoch_after','generation','outcome','key','before','after','effect_seq']);
      if(row.request_id!==null||row.record_id!==d.control_id||controls.has(name(d.control_id))||d.applied_ps===null||decimal(d.applied_ps)!==decimal(row.time_ps)||decimal(d.scheduled_ps)>decimal(d.applied_ps)||!['changed','no_op','stale'].includes(d.outcome)||!['membership_set','membership_leave','router_set','router_leave','vlan_register','vlan_unregister','link_set','mac_flush','tree_publish','static_shadowed','mac_capacity','mac_learn','mac_refresh','mac_move','mac_expire','membership_expire','router_expire','registration_expire'].includes(d.kind))fail('control実績が不正です');controls.add(d.control_id);
      const before=decimal(d.epoch_before),after=decimal(d.epoch_after);decimal(d.batch_ordinal);
      if(!epochRows.has(before)||!epochRows.has(after)||after<before||after>before+1n||d.outcome==='changed'&&after===before)fail('control commit/ordinalが不正です');if(d.generation!==null)decimal(d.generation);
    }
    const visits=new Map(),copyNumbers=new Set();
    for(const t of m.transfers.values()){
      const visit=decimal(t.visit_id),match=/\/v(0|[1-9][0-9]*)\/c(0|[1-9][0-9]*)$/.exec(t.id);
      if(!match||t.copy_id!==t.id||t.id!==`${t.frame_id}/v${t.visit_id}/c${match[2]}`||decimal(match[1])!==visit||copyNumbers.has(match[2]))fail('visit/copy IDが不正です');copyNumbers.add(match[2]);t.copyNumber=decimal(match[2]);
      epochAt(t.policy_epoch_offer,t.queued_ps);if(t.sof_ps===null?t.policy_epoch_sof!==null:t.policy_epoch_sof===null)fail('start epochが不正です');if(t.policy_epoch_sof!==null)epochAt(t.policy_epoch_sof,t.sof_ps);
      if(t.parent_transfer_id===null){if(visit!==0n)fail('source visitは0です');}
      else{const parent=m.transfers.get(t.parent_transfer_id);const pMatch=parent&&/\/c(0|[1-9][0-9]*)$/.exec(parent.id);if(!pMatch||decimal(pMatch[1])>=t.copyNumber||visit===0n)fail('parent copyが前進していません');const rx=m.receptions.get(`${parent.id}@rx`);if(!rx||rx.visit_id!==t.visit_id||rx.vlan_id!==t.vlan_id||rx.priority!==t.priority)fail('copyのvisit/classと親receptionが不一致です');}
      if(t.status==='dropped'&&t.updated>t.queued_ps)m.events.push({time:t.updated,id:t.id,kind:'policy_drop'});
    }
    for(const r of m.receptions.values()){const p=m.ingressPolicies.get(r.ingress),wire=m.transfers.get(r.transfer_id).wire;if(!p||r.vlan_id!==(wire.tag?.vid??p.pvid)||r.priority!==(wire.tag?.pcp??p.default_priority))fail('dynamic ingress分類が不一致です');const visit=decimal(r.visit_id),key=`${r.frame_id}/${visit}`;if(visit===0n||visits.has(key))fail('受信visitが重複又は0です');visits.set(key,r.id);epochAt(r.policy_epoch_ingress,r.observed_ps);}
    for(const f of m.frames.values()){
      const flow=array(metadata.flows).find(row=>row.flow_id===f.flow_id);if(!flow||flow.priority!==f.priority||flow.dst_mac!==f.dst_mac||flow.source_vlan_id!==f.source_vlan_id||(flow.deadline_ps===null?f.deadline_ps!==null:decimal(flow.deadline_ps)!==f.deadline_ps)||!sameTag(flow.tag,f.tag)||canonical(flow.ip_multicast)!==canonical(f.ip_multicast)||m.devices.get(f.source).mac!==f.src_mac)fail('dynamic source flowが不一致です');
      const copies=[...m.transfers.values()].filter(t=>t.frame_id===f.id&&t.parent_transfer_id===null);if(copies.length>1)fail('source copyが重複しています');
    }
    const stop=metadata.ethernet_dynamic_snapshot??metadata.network_runtime?.dynamic;if(stop!==undefined)validateStopPolicy(stop,policyStateAt(history,m.end));
    if(raw.metadata.model_profile==='ethernet.tsn.v1')validateTsnRows(raw,tsnRows,m,epochRows);
    return history;
  }
  function validateStopPolicy(snapshot,state) {
    fields(snapshot,['policy','mac','membership','router','registration','next_deadline','converge_at']);
    if(canonical(snapshot.policy)!==canonical(state.snapshot))fail('停止policyと実績prefixが不一致です');
    for(const table of ['mac','membership','router','registration']){const values=new Map();for(const row of array(snapshot[table])){fields(row,['key','lease']);const key=canonical(row.key);if(values.has(key))fail('停止tableのkeyが重複しています');values.set(key,row.lease);}if(canonical([...values].sort())!==canonical([...state.tables[table]].sort()))fail('停止tableと実績prefixが不一致です');}
    for(const k of ['next_deadline','converge_at'])if(snapshot[k]!==null)decimal(snapshot[k]);
  }
  function validateTsnRows(raw,rows,m,epochs) {
    const config=raw.metadata.ethernet_tsn;fields(config,['clock','outputs','streams','gcl_updates']);if(config.clock!=='ideal_shared')fail('TSN clockが不正です');
    const outputs=new Map(),schedules=new Map(),streams=new Map(),updates=new Map();
    const classNumber=p=>{if(!Number.isInteger(p)||p<0||p>7)fail('TSN classが不正です');return p;};
    const addSchedule=(value,port)=>{const prepared=prepareSchedule(value);if(prepared){const key=port+'/'+prepared.id;if(schedules.has(key))fail('schedule IDが重複しています');schedules.set(key,prepared);}return prepared;};
    for(const out of array(config.outputs)){
      fields(out,['port','tas','cbs']);const direction=m.directions.get(out.port);if(!direction||outputs.has(out.port))fail('TSN output参照が不正です');
      const cbs=new Map();for(const c of array(out.cbs)){fields(c,['priority','idle_slope_bps','hi_credit_bits','lo_credit_bits']);const p=classNumber(c.priority),idle=decimal(c.idle_slope_bps),hi=decimal(c.hi_credit_bits)*Q,lo=decimal(c.lo_credit_bits)*Q;if(cbs.has(p)||idle===0n||idle>=direction.bitrate||hi>(1n<<128n)-1n||lo>(1n<<128n)-1n)fail('CBS設定が不正です');cbs.set(p,{...c,idle,hi,lo});}
      outputs.set(out.port,{...out,tas:addSchedule(out.tas,out.port),cbs});
    }
    if(outputs.size!==m.directions.size)fail('TSN output設定が不足しています');
    for(const update of array(config.gcl_updates)){fields(update,['id','submitted_at_ps','effective_at_ps','port','schedule']);name(update.id);if(updates.has(update.id)||!outputs.has(update.port)||decimal(update.submitted_at_ps)>decimal(update.effective_at_ps)||update.schedule?.base_time_ps!==update.effective_at_ps)fail('GCL updateが不正です');updates.set(update.id,update);addSchedule(update.schedule,update.port);}
    const streamKeys=new Set();for(const stream of array(config.streams)){
      fields(stream,['id','ingress','dst_mac','vid','priority','max_sdu_bytes','gate','meter']);name(stream.id);mac(stream.dst_mac);const p=classNumber(stream.priority),v=typeof stream.vid==='number'?String(stream.vid):stream.vid;vid(v);decimal(stream.max_sdu_bytes);const key=canonical([stream.ingress,stream.dst_mac,v,p]);if(![...m.directions.values()].some(d=>d.to_port===stream.ingress)||streams.has(stream.id)||streamKeys.has(key))fail('stream参照が不正です');streamKeys.add(key);
      if(stream.gate!==null){fields(stream.gate,['base_time_ps','cycle_time_ps','entries']);prepareSchedule({id:stream.id,...stream.gate,entries:array(stream.gate.entries).map(e=>{fields(e,['duration_ps','open']);if(typeof e.open!=='boolean')fail('stream gateが不正です');return {duration_ps:e.duration_ps,open_priorities:e.open?[0,1,2,3,4,5,6,7]:[]};})});}
      if(stream.meter!==null){const meter=stream.meter;fields(meter,['committed_rate_bps','peak_rate_bps','committed_burst_bytes','peak_burst_bytes','yellow_action']);const committed=decimal(meter.committed_rate_bps),peak=decimal(meter.peak_rate_bps),cb=decimal(meter.committed_burst_bytes),pb=decimal(meter.peak_burst_bytes);if(committed===0n||peak<committed||cb===0n||pb<cb||pb*8n*Q>(1n<<128n)-1n||!['pass','drop'].includes(meter.yellow_action))fail('stream meterが不正です');}
      streams.set(stream.id,{...stream,vid:v});
    }
    const unique=new Set(),policed=new Set(),gateHistory=new Map(),creditHistory=new Map(),meterHistory=new Map();
    for(const row of rows.sort((a,b)=>compare(decimal(a.time_ps),decimal(b.time_ps))||compare(decimal(a.data.effect_seq),decimal(b.data.effect_seq)))){
      const d=row.data,time=decimal(row.time_ps);name(row.record_id);if(unique.has(row.record_id))fail('TSN record IDが重複しています');unique.add(row.record_id);
      if(row.schema_name==='ethernet.tsn.gate'){
        fields(d,['port','schedule_id','generation','open_priorities','next_boundary_ps','cause','effect_seq']);const output=outputs.get(d.port);if(!output||!['initial','boundary','update'].includes(d.cause))fail('gate recordが不正です');
        const schedule=d.schedule_id===null?null:schedules.get(d.port+'/'+d.schedule_id);if(d.schedule_id!==null&&!schedule||d.schedule_id===null&&output.tas!==null)fail('gate schedule参照が不正です');const expected=scheduleStateAt(schedule,time),pending=[...updates.values()].filter(u=>u.port===d.port&&decimal(u.effective_at_ps)>time).map(u=>decimal(u.effective_at_ps)).sort(compare)[0];if(pending!==undefined&&(expected.next===null||pending<expected.next))expected.next=pending;const open=array(d.open_priorities).map(p=>Number(priority(p)));if(new Set(open).size!==open.length||canonical([...open].sort())!==canonical([...expected.open].sort())||(d.next_boundary_ps===null?expected.next!==null:wide(d.next_boundary_ps)!==expected.next))fail('gate状態がscheduleと不一致です');
        const generation=decimal(d.generation),previous=gateHistory.get(d.port)?.at(-1);if(d.cause==='initial'&&(previous||time!==0n||generation!==0n||d.schedule_id!==(output.tas?.id??null))||previous&&(generation<previous.generation||generation>previous.generation+1n||d.cause==='update'&&generation!==previous.generation+1n||d.cause==='boundary'&&(generation!==previous.generation||d.schedule_id!==(previous.schedule?.id??null))))fail('gate世代が不正です');
        if(!gateHistory.has(d.port))gateHistory.set(d.port,[]);gateHistory.get(d.port).push({time,generation,schedule,raw:row});
      }else if(row.schema_name==='ethernet.tsn.credit'){
        fields(d,['port','priority','sign','magnitude','scale','slope_bps','cause','effect_seq']);const p=Number(priority(d.priority)),cbs=outputs.get(d.port)?.cbs.get(p);if(!cbs||!['positive','negative'].includes(d.sign)||d.scale!==Q.toString()||typeof d.slope_bps!=='string'||! /^(0|[1-9][0-9]*|-[1-9][0-9]*)$/.test(d.slope_bps))fail('credit recordが不正です');name(d.cause);const magnitude=wide(d.magnitude),value=d.sign==='negative'?-magnitude:magnitude,slope=BigInt(d.slope_bps);if(d.sign==='negative'&&magnitude===0n||value>cbs.hi||value< -cbs.lo||![0n,cbs.idle,cbs.idle-m.directions.get(d.port).bitrate].includes(slope))fail('credit数値が不正です');const key=d.port+'/'+p;if(!creditHistory.has(key))creditHistory.set(key,[]);creditHistory.get(key).push({time,value,slope,cbs,priority:p,port:d.port,raw:row});
      }else if(row.schema_name==='ethernet.tsn.policing'){
        fields(d,['stream_id','reception_id','ingress','mac_bytes','verdict','color','committed_before','committed_after','peak_before','peak_after','consumed_bits','effect_seq']);const reception=m.receptions.get(d.reception_id),stream=d.stream_id===null?null:streams.get(d.stream_id),bytes=decimal(d.mac_bytes),consumed=wide(d.consumed_bits);if(!reception||policed.has(d.reception_id)||reception.ingress!==d.ingress||reception.observed_ps!==time||bytes!==BigInt(m.transfers.get(reception.transfer_id).wire.mac_bytes)||d.stream_id!==null&&!stream||!['bypass','pass','psfp_max_sdu','psfp_gate_closed','psfp_meter_red','psfp_meter_yellow'].includes(d.verdict)||![null,'green','yellow','red'].includes(d.color))fail('policing参照/判定が不正です');policed.add(d.reception_id);
        if(stream&&(stream.ingress!==d.ingress||stream.dst_mac!==m.frames.get(reception.frame_id).dst_mac||stream.vid!==reception.vlan_id||String(stream.priority)!==reception.priority))fail('policing stream照合が不一致です');
        const buckets=['committed_before','committed_after','peak_before','peak_after'];if(d.color===null){if(buckets.some(k=>d[k]!==null)||consumed!==0n)fail('未評価meterの消費です');}else{if(!stream?.meter||buckets.some(k=>d[k]===null))fail('meter bucketがありません');const cb=wide(d.committed_before),ca=wide(d.committed_after),pb=wide(d.peak_before),pa=wide(d.peak_after),cost=bytes*8n*Q;if(cb>BigInt(stream.meter.committed_burst_bytes)*8n*Q||pb>BigInt(stream.meter.peak_burst_bytes)*8n*Q)fail('tokenが容量を超えています');const last=meterHistory.get(d.stream_id)??{time:0n,committed:BigInt(stream.meter.committed_burst_bytes)*8n*Q,peak:BigInt(stream.meter.peak_burst_bytes)*8n*Q};if(cb!==potentialTokens(last.committed,BigInt(stream.meter.committed_rate_bps),last.time,time,BigInt(stream.meter.committed_burst_bytes)*8n*Q)||pb!==potentialTokens(last.peak,BigInt(stream.meter.peak_rate_bps),last.time,time,BigInt(stream.meter.peak_burst_bytes)*8n*Q))fail('meter refillと前回実績が不一致です');meterHistory.set(d.stream_id,{time,committed:ca,peak:pa});const color=pb<cost?'red':cb<cost?'yellow':'green';if(color!==d.color||ca!==cb-(color==='green'?cost:0n)||pa!==pb-(color!=='red'?cost:0n)||consumed!==bytes*8n*(color==='green'?2n:color==='yellow'?1n:0n))fail('meter色/消費保存則が不一致です');}
        if(d.color==='red'&&d.verdict!=='psfp_meter_red'||d.color==='yellow'&&d.verdict!==(stream.meter.yellow_action==='drop'?'psfp_meter_yellow':'pass')||d.color==='green'&&d.verdict!=='pass')fail('meter色とverdictが不一致です');
        if(d.verdict==='bypass'?(stream!==null||d.color!==null):stream===null)fail('bypass streamが不正です');if(d.verdict.startsWith('psfp_')&&reception.reason!==d.verdict)fail('policing drop理由が不一致です');
      }else{
        fields(d,['port','transfer_id','priority','state','policy_epoch','schedule_generation','next_wake_ps','reason','effect_seq']);const epoch=decimal(d.policy_epoch),generation=decimal(d.schedule_generation);if(!outputs.has(d.port)||!epochs.has(epoch)||epochs.get(epoch).time>time||!['ready','busy','gate_closed','guard_blocked','credit_negative','no_active_schedule','never_eligible'].includes(d.state)||d.transfer_id!==null&&!m.transfers.has(d.transfer_id))fail('decision参照/状態が不正です');if(d.priority!==null)priority(d.priority);if(d.next_wake_ps!==null&&wide(d.next_wake_ps)<time)fail('wake時刻が過去です');if(d.reason!==null)name(d.reason);if(generation!==(gateHistory.get(d.port)?.at(-1)?.generation??0n))fail('decision schedule世代がprefixと不一致です');const transfer=d.transfer_id===null?null:m.transfers.get(d.transfer_id);if(transfer&&(transfer.from_port!==d.port||d.priority!==transfer.priority))fail('decision copy/classが不一致です');
      }
    }
    m.tsnState={outputs,schedules,streams,updates,gateHistory,creditHistory,rows};
    if(raw.metadata.network_runtime?.tsn!==undefined)validateStopTsn(raw.metadata.network_runtime.tsn,tsnStateAt(m,m.end),m);
  }
  function tsnStateAt(m,time) {
    if(!m.tsnState)return null;const state=m.tsnState,gates=new Map(),credits=new Map(),meters=new Map(),decisions=new Map();
    for(const stream of state.streams.values())if(stream.meter)meters.set(stream.id,{verdict:'未到着',color:null,consumed_bits:'0',time:0n,last_evaluated_ps:0n,committed:BigInt(stream.meter.committed_burst_bytes)*8n*Q,peak:BigInt(stream.meter.peak_burst_bytes)*8n*Q});
    const oper=(port,t)=>state.gateHistory.get(port)?.filter(r=>r.time<=t).at(-1)??{schedule:state.outputs.get(port).tas,generation:0n};
    for(const out of state.outputs.values()){for(const p of out.cbs.keys())credits.set(out.port+'/'+p,0n);const gate=oper(out.port,time);gates.set(out.port,{...scheduleStateAt(gate.schedule,time),generation:gate.generation,schedule_id:gate.schedule?.id??null});}
    for(const [key,rows]of state.creditHistory){const last=rows.filter(r=>r.time<=time).at(-1);if(!last){credits.set(key,0n);continue;}const sending=last.slope<0n,schedule=oper(last.port,last.time).schedule,backlog=[...m.transfers.values()].some(t=>t.from_port===last.port&&Number(t.priority)===last.priority&&transferStateAt(t,last.time)==='queued');credits.set(key,creditNumeratorAt(last.value,last.slope,last.time,time,{schedule,priority:last.priority,sending,backlog,hi:last.cbs.hi,lo:last.cbs.lo}));}
    for(const row of state.rows)if(decimal(row.time_ps)<=time){const d=row.data;if(row.schema_name==='ethernet.tsn.decision')decisions.set(d.port,d);if(row.schema_name==='ethernet.tsn.policing'&&d.stream_id!==null){const previous=meters.get(d.stream_id);meters.set(d.stream_id,{...d,time:decimal(row.time_ps),last_evaluated_ps:d.color!==null?decimal(row.time_ps):previous?.last_evaluated_ps??null,committed:d.committed_after===null?previous?.committed??null:wide(d.committed_after),peak:d.peak_after===null?previous?.peak??null:wide(d.peak_after)});}}
    for(const [id,meter]of meters){const config=state.streams.get(id).meter;if(config&&meter.last_evaluated_ps!==null){meter.potential_committed=potentialTokens(meter.committed,BigInt(config.committed_rate_bps),meter.last_evaluated_ps,time,BigInt(config.committed_burst_bytes)*8n*Q);meter.potential_peak=potentialTokens(meter.peak,BigInt(config.peak_rate_bps),meter.last_evaluated_ps,time,BigInt(config.peak_burst_bytes)*8n*Q);}}
    return {gates,credits,meters,decisions};
  }
  function validateStopTsn(snapshot,state,m) {
    fields(snapshot,['ports','meters','update_cursor']);decimal(snapshot.update_cursor);
    const covered=new Set();for(const port of array(snapshot.ports)){fields(port,['port','schedule_id','schedule_generation','wake_generation','next_wake_ps','credits']);const gate=state.gates.get(port.port);if(!gate||covered.has(port.port)||gate.schedule_id!==port.schedule_id||gate.generation!==decimal(port.schedule_generation))fail('TSN停止scheduleがprefixと不一致です');covered.add(port.port);decimal(port.wake_generation);if(port.next_wake_ps!==null&&wide(port.next_wake_ps)<m.end)fail('TSN停止wakeが過去です');const priorities=new Set();for(const credit of array(port.credits)){fields(credit,['priority','credit','last_ps','mode','backlog','sending']);fields(credit.credit,['magnitude','negative']);const p=Number(priority(credit.priority));if(priorities.has(p)||typeof credit.credit.negative!=='boolean'||!['sending','frozen','accumulating','recovering','zero'].includes(credit.mode)||typeof credit.backlog!=='boolean'||typeof credit.sending!=='boolean'||decimal(credit.last_ps)>m.end)fail('TSN停止creditが不正です');priorities.add(p);const magnitude=wide(credit.credit.magnitude),value=credit.credit.negative?-magnitude:magnitude;if(credit.credit.negative&&magnitude===0n||state.credits.get(port.port+'/'+p)!==value)fail('TSN停止creditが実績prefixと不一致です');}if(priorities.size!==m.tsnState.outputs.get(port.port).cbs.size)fail('TSN停止creditが不足しています');}
    if(covered.size!==m.tsnState.outputs.size)fail('TSN停止outputが不足しています');
    const streams=new Set();for(const meter of array(snapshot.meters)){fields(meter,['stream_id','committed','peak','last_evaluated_ps']);const actual=state.meters.get(meter.stream_id);if(!actual||streams.has(meter.stream_id)||wide(meter.committed)!==actual.committed||wide(meter.peak)!==actual.peak||decimal(meter.last_evaluated_ps)!==actual.last_evaluated_ps)fail('TSN停止meterが実績prefixと不一致です');streams.add(meter.stream_id);}if(streams.size!==[...m.tsnState.streams.values()].filter(s=>s.meter).length)fail('TSN停止meterが不足しています');
  }
  function decodeCanPayload(hex) {
    if(typeof hex!=='string'||!/^(?:[0-9a-f]{2})*$/.test(hex)||hex.length<22)fail('DIR CAN codec長が不正です');
    const bytes=hex.match(/../g).map(b=>parseInt(b,16));if(bytes.slice(0,4).map(b=>String.fromCharCode(b)).join('')!=='DIRC'||bytes[4]!==1||bytes[5]>1||bytes[10]>8)fail('DIR CAN codec headerが不正です');
    const format=bytes[5]===0?'standard':'extended',id=bytes.slice(6,10).reduce((n,b)=>n*256+b,0),length=11+bytes[10];if(id>(format==='standard'?2047:536870911)||bytes.length<length||bytes.slice(length).some(b=>b!==0))fail('DIR CAN codec ID/paddingが不正です');
    return {format,id:BigInt(id),length:BigInt(length),data:hex.slice(22,length*2)};
  }
  function parseCompositeResults(raw) {
    const records=array(raw.simulation?.model_records),schemas=new Map([['can.request',1],['can.receiver',1],['ethernet.frame',3],['ethernet.transfer',3],['ethernet.reception',2],['dir.can_ethernet.conversion',1],['dir.can_ethernet.branch',1],['dir.can_ethernet.segment',1]]),declared=new Set();
    for(const schema of array(raw.metadata.model_schemas)){fields(schema,['schema_name','schema_version']);if(schemas.get(schema.schema_name)!==schema.schema_version||declared.has(schema.schema_name))fail('composite schema宣言が不正です');declared.add(schema.schema_name);}if(declared.size!==schemas.size)fail('composite schema宣言が不足しています');
    const ids=new Map(),seen=new Set(),effects=new Set();for(const row of records){fields(row,['schema_name','schema_version','record_id','subject','request_id','origin_request_id','time_ps','data']);if(schemas.get(row.schema_name)!==row.schema_version||seen.has(row.schema_name+'/'+name(row.record_id)))fail('composite record schema/IDが不正です');seen.add(row.schema_name+'/'+row.record_id);if(!row.schema_name.startsWith('dir.can_ethernet.')){if(!ids.has(row.record_id))ids.set(row.record_id,[]);ids.get(row.record_id).push(row);}if(row.schema_name.startsWith('dir.can_ethernet.')){if(row.subject!=='@network')fail('composite subjectが不正です');const seq=decimal(row.data.effect_seq);if(effects.has(seq))fail('composite effect_seqが重複しています');effects.add(seq);}}
    const ethernet=parseResults({...raw,metadata:{...raw.metadata,model_profile:'ethernet.l2.vlan.v1',model_schemas:[{schema_name:'ethernet.frame',schema_version:3},{schema_name:'ethernet.transfer',schema_version:3},{schema_name:'ethernet.reception',schema_version:2}]},simulation:{...raw.simulation,model_records:records.filter(r=>r.schema_name.startsWith('ethernet.'))}});
    const C=typeof module==='object'&&module.exports?require('./model.js'):globalThis.DIRViewerModel;
    const can=C.parseResults({...raw,metadata:{...raw.metadata,model_profile:'can.cc.multibus.v1',models:[],initial_state:[],config:[]},simulation:{...raw.simulation,records:(raw.simulation.records??[]).filter(r=>r.metric!=='queue_length'||![...ethernet.directions.keys()].some(port=>r.target===`${port}.queue`||Array.from({length:8},(_,p)=>`${port}.queue.${p}`).includes(r.target))),model_records:records.filter(r=>r.schema_name.startsWith('can.'))}});
    const cfg=raw.metadata.can_ethernet;fields(cfg,['gateways']);const gateways=new Map(),owned=new Set(),rules=new Map();
    for(const gateway of array(cfg.gateways)){fields(gateway,['instance','can_ports','ethernet_endpoint','rx_capacity','conversion_delay_ps','max_hops','rules']);name(gateway.instance);if(gateways.has(gateway.instance)||!ethernet.devices.has(gateway.ethernet_endpoint)||ethernet.devices.get(gateway.ethernet_endpoint).kind!=='endpoint')fail('Gateway参照が不正です');const ports=array(gateway.can_ports);if(!ports.length)fail('Gateway CAN portがありません');for(const port of [...ports,gateway.ethernet_endpoint]){if(owned.has(port)||port!==gateway.ethernet_endpoint&&!can.controllers.some(c=>c.id===port))fail('Gateway portの重複所有です');owned.add(port);}decimal(typeof gateway.rx_capacity==='number'?String(gateway.rx_capacity):gateway.rx_capacity);decimal(gateway.conversion_delay_ps);const hops=decimal(typeof gateway.max_hops==='number'?String(gateway.max_hops):gateway.max_hops);if(hops<1n||hops>255n)fail('Gateway hop上限が不正です');const ruleIds=new Set();for(const rule of array(gateway.rules)){fields(rule,['id','direction','ingress','match','egresses']);name(rule.id);if(ruleIds.has(rule.id)||!['can_to_ethernet','ethernet_to_can'].includes(rule.direction))fail('Gateway ruleが不正です');ruleIds.add(rule.id);const toEthernet=rule.direction==='can_to_ethernet';fields(rule.match,toEthernet?['format','can_id']:['vid','pcp','format','can_id']);const validId=(format,id)=>['standard','extended'].includes(format)&&Number.isInteger(id)&&id>=0&&id<=(format==='standard'?2047:536870911);if(!validId(rule.match.format,rule.match.can_id)||(toEthernet?!ports.includes(rule.ingress):rule.ingress!==gateway.ethernet_endpoint))fail('Gateway rule matchが不正です');if(!toEthernet){vid(String(rule.match.vid));priority(String(rule.match.pcp));}const egresses=array(rule.egresses),unique=new Set();if(!egresses.length)fail('Gateway egressがありません');for(const e of egresses){fields(e,toEthernet?['port','dst_mac','vid','pcp']:['port','format','can_id']);if(unique.has(e.port))fail('Gateway egressが重複しています');unique.add(e.port);if(toEthernet){if(e.port!==`${gateway.ethernet_endpoint}.tx`||!ethernet.policies.get(e.port)?.members.has(String(e.vid)))fail('Gateway Ethernet egressが不正です');mac(e.dst_mac);vid(String(e.vid));priority(String(e.pcp));}else if(!ports.some(p=>`${p}.tx`===e.port)||!validId(e.format,e.can_id))fail('Gateway CAN egressが不正です');}rules.set(gateway.instance+'/'+rule.id,rule);}gateways.set(gateway.instance,gateway);}
    const actual=x=>{if(x===null)return null;const time=decimal(x);if(time>ethernet.end||raw.simulation.termination==='time_limit'&&time===ethernet.end)fail('composite実績が観測期間外です');return time;};
    const conversions=new Map(),branches=new Map(),segments=new Map(),origins=new Map(),originByRecord=new Map(),originAliases=new Map();
    const recordKey=(media,id)=>media+'/'+id;
    const native=(id,schemas,predicate=()=>true)=>{const candidates=(ids.get(id)??[]).filter(row=>schemas.includes(row.schema_name)&&predicate(row));if(candidates.length!==1)fail('composite媒体参照が欠落又は曖昧です');return candidates[0];};
    const sourceMedia=row=>row.schema_name.startsWith('can.')?'can':'ethernet';
    const registerOrigin=(media,id,time)=>{const qualified=media+':'+id;origins.set(qualified,{id:qualified,media,record_id:id,time});if(!originAliases.has(id))originAliases.set(id,[]);originAliases.get(id).push(qualified);originByRecord.set(recordKey(media,id),qualified);};
    for(const request of can.requests)registerOrigin('can',request.id,request.generated);for(const frame of ethernet.frames.values())registerOrigin('ethernet',frame.id,frame.generated_ps);
    for(const [id,candidates]of originAliases)if(candidates.length===1)originByRecord.set(id,id);
    const resolveOrigin=id=>{if(origins.has(id))return id;const candidates=(originAliases.get(id)??[]).filter(candidate=>origins.has(candidate));if(candidates.length!==1)fail('composite origin参照が欠落又は曖昧です');return candidates[0];};
    for(const row of records.filter(r=>r.schema_name==='dir.can_ethernet.conversion')){
      const d=row.data;fields(d,['conversion_id','origin_id','parent_id','gateway','ingress_record','rule_id','visited_gateways','observed_ps','ready_ps','planned_ready_ps','released_ps','status','reason','branch_ids','effect_seq']);if(row.record_id!==d.conversion_id||!gateways.has(d.gateway)||!['processing','waiting_tx','released','rejected'].includes(d.status))fail('conversion参照/状態が不正です');const origin=resolveOrigin(d.origin_id),gateway=gateways.get(d.gateway),rule=d.rule_id===null?null:rules.get(d.gateway+'/'+d.rule_id),ingress=native(d.ingress_record,rule?(rule.direction==='can_to_ethernet'?['can.receiver']:['ethernet.reception']):['can.receiver','ethernet.reception'],r=>[...gateway.can_ports,gateway.ethernet_endpoint].includes(r.schema_name==='can.receiver'?r.data.receiver:owner(r.data.ingress))),media=sourceMedia(ingress),parent=native(d.parent_id,media==='can'?['can.request','can.receiver']:['ethernet.frame','ethernet.transfer','ethernet.reception']);if((media==='can'?parent.data.request_id:parent.schema_name==='ethernet.frame'?parent.record_id:parent.data.frame_id)!==(media==='can'?ingress.data.request_id:ingress.data.frame_id))fail('conversion parent媒体がingressと不一致です');const observed=actual(d.observed_ps),ready=actual(d.ready_ps),released=actual(d.released_ps),visited=array(d.visited_gateways),branchIds=array(d.branch_ids);if(observed===null||observed<origins.get(origin).time||new Set(visited).size!==visited.length||visited.some(g=>!gateways.has(g))||new Set(branchIds).size!==branchIds.length)fail('conversion lineageが不正です');
      if(d.rule_id!==null&&!rules.has(d.gateway+'/'+d.rule_id))fail('conversion rule参照が不正です');const device=ingress.schema_name==='can.receiver'?ingress.data.receiver:ingress.schema_name==='ethernet.reception'?owner(ingress.data.ingress):null;
      if(![...gateway.can_ports,gateway.ethernet_endpoint].includes(device)||ingress.data.observed_ps!==d.observed_ps)fail('conversion ingressがGateway所有ではありません');
      if(d.status==='rejected'){if(ready!==null||released!==null||branchIds.length||!['no_rule','invalid_codec','loop_prevented','hop_limit','rx_full'].includes(d.reason))fail('conversion拒否が不正です');}
      else if(d.reason!==null||visited.at(-1)!==d.gateway||(d.status==='processing'?ready!==null:ready===null))fail('conversion実績と状態が不一致です');
      if(ready!==null&&(ready<observed||d.planned_ready_ps===null||ready!==decimal(d.planned_ready_ps))||released!==null&&(ready===null||released<ready)||d.status==='released'&&released===null||d.status!=='released'&&released!==null)fail('conversion時刻順が不正です');if(d.planned_ready_ps!==null&&decimal(d.planned_ready_ps)<observed)fail('conversion予定時刻が不正です');
      if(actual(row.time_ps)!==[observed,ready,released].filter(t=>t!==null).reduce((a,b)=>a>b?a:b))fail('conversion包絡時刻が不一致です');conversions.set(d.conversion_id,{...d,origin_id:origin,ingressRow:ingress,sourceKey:recordKey(media,media==='can'?ingress.data.request_id:ingress.data.frame_id),observed,ready,released,raw:row});
    }
    for(const row of records.filter(r=>r.schema_name==='dir.can_ethernet.branch')){
      const d=row.data;fields(d,['branch_id','conversion_id','egress','child_id','planned_child_id','codec_length','pcp','input_format','input_can_id','output_format','output_can_id','offer_ps','planned_offer_ps','admitted_ps','sof_ps','status','reason','effect_seq']);const conversion=conversions.get(d.conversion_id),childMedia=ethernet.directions.has(d.egress)?'ethernet':'can',child=d.child_id===null?null:native(d.child_id,childMedia==='can'?['can.request']:['ethernet.frame']);if(row.record_id!==d.branch_id||!conversion||!conversion.branch_ids.includes(d.branch_id)||!['waiting','admitted','dropped'].includes(d.status)||d.child_id!==null&&!child)fail('branch参照/状態が不正です');name(d.planned_child_id);const length=decimal(d.codec_length);if(length<11n||length>19n)fail('branch codec長が不正です');
      for(const [format,id]of [[d.input_format,d.input_can_id],[d.output_format,d.output_can_id]])if(!['standard','extended'].includes(format)||decimal(id)>(format==='standard'?2047n:536870911n))fail('branch CAN format/IDが不正です');if(d.pcp!==null)priority(d.pcp);const rule=rules.get(conversion.gateway+'/'+conversion.rule_id),egress=rule?.egresses.find(e=>e.port===d.egress);if(!rule||!egress||rule.match.format!==d.input_format||String(rule.match.can_id)!==d.input_can_id)fail('branch rule/egressが不一致です');if(rule.direction==='can_to_ethernet'?(d.pcp!==String(egress.pcp)||d.output_format!==d.input_format||d.output_can_id!==d.input_can_id):(d.output_format!==egress.format||d.output_can_id!==String(egress.can_id)||d.pcp!==null&&d.pcp!==String(rule.match.pcp)))fail('branch CAN/PCP映射が不一致です');
      const ingress=conversion.ingressRow;if(ingress.schema_name==='ethernet.reception'){const frame=native(ingress.data.frame_id,['ethernet.frame']);if(frame.data.ether_type!=='34997')fail('conversion EtherTypeが不正です');const decoded=decodeCanPayload(frame.data.data_hex);if(decoded.format!==d.input_format||decoded.id!==decimal(d.input_can_id)||decoded.length!==length)fail('branch入力codecが不一致です');}
      const offer=actual(d.offer_ps),admitted=actual(d.admitted_ps),sof=actual(d.sof_ps),planned=decimal(d.planned_offer_ps);if(offer!==null&&offer!==planned||admitted!==null&&(offer===null||admitted<offer)||sof!==null&&(admitted===null||sof<admitted)||(d.status==='admitted'?(admitted===null||child===null):(admitted!==null||child!==null)))fail('branch admissionが不正です');
      if(d.status==='dropped'?!['tx_unadmittable'].includes(d.reason):d.reason!==null)fail('branch drop理由が不正です');
      if(child){if(d.child_id!==d.planned_child_id||!['can.request','ethernet.frame'].includes(child.schema_name))fail('branch child型が不正です');if(child.schema_name==='ethernet.frame'){const decoded=decodeCanPayload(child.data.data_hex);if(decoded.length!==length||decoded.format!==d.input_format||decoded.id!==decimal(d.input_can_id)||d.pcp!==child.data.priority||child.data.tag&&child.data.tag.pcp!==d.pcp||d.egress!==`${child.data.source}.tx`)fail('Ethernet branch codec/PCPが不一致です');}else if(d.egress!==`${child.data.source}.tx`||d.sof_ps!==child.data.sof_ps)fail('CAN branch childが不一致です');originByRecord.set(recordKey(childMedia,d.child_id),conversion.origin_id);if((originAliases.get(d.child_id)??[]).length===1)originByRecord.set(d.child_id,conversion.raw.data.origin_id);origins.delete(childMedia+':'+d.child_id);}
      branches.set(d.branch_id,{...d,childMedia,offer,admitted,sof,raw:row});
    }
    const childBranches=new Map();for(const branch of branches.values())if(branch.child_id!==null){const key=recordKey(branch.childMedia,branch.child_id);if(childBranches.has(key))fail('composite childの枝所有が重複しています');childBranches.set(key,branch);}
    const ingressSource=conversion=>conversion.sourceKey;
    for(const conversion of conversions.values())if(!origins.has(conversion.origin_id)||originByRecord.get(ingressSource(conversion))!==conversion.origin_id)fail('conversion originが親媒体と不一致です');
    const sourceLineage=source=>{const lineage=[],seen=new Set();while(childBranches.has(source)){if(seen.has(source))fail('conversion lineageが循環しています');seen.add(source);const branch=childBranches.get(source);lineage.unshift(branch.branch_id);source=ingressSource(conversions.get(branch.conversion_id));}return lineage;};
    for(const conversion of conversions.values()){if(conversion.branch_ids.some(id=>!branches.has(id)))fail('conversion branchがありません');if(conversion.status==='released'&&conversion.branch_ids.some(id=>branches.get(id).status==='waiting'))fail('RX解放前にwaiting枝が残っています');}
    for(const row of records.filter(r=>r.schema_name==='dir.can_ethernet.segment')){
      const d=row.data;fields(d,['segment_id','origin_id','source_record_id','branch_lineage','sof_ps','target_count','targets','effect_seq']);const origin=resolveOrigin(d.origin_id),sof=actual(d.sof_ps),lineage=array(d.branch_lineage);if(row.record_id!==d.segment_id||sof===null||lineage.some(id=>!branches.has(id))||new Set(lineage).size!==lineage.length||decimal(d.target_count)!==BigInt(array(d.targets).length))fail('segment参照が不正です');const media=lineage.length?branches.get(lineage.at(-1)).childMedia:origins.get(origin).media,source=native(d.source_record_id,media==='can'?['can.request']:['ethernet.frame']),sourceKey=recordKey(media,d.source_record_id);if(originByRecord.get(sourceKey)!==origin||!lineage.length&&d.origin_id===origin&&(d.source_record_id!==origins.get(origin).record_id||d.segment_id!==origin))fail('segment originとsource媒体が不一致です');const sourceSof=source.schema_name==='can.request'?source.data.sof_ps:[...ethernet.transfers.values()].find(t=>t.frame_id===source.record_id&&t.parent_transfer_id===null)?.sof_ps?.toString();if(sourceSof!==d.sof_ps||canonical(lineage)!==canonical(sourceLineage(sourceKey)))fail('segment SOF/lineageがsourceと不一致です');const tuples=new Set();
      for(const target of d.targets){fields(target,['origin_id','segment_id','branch_lineage','terminal_id','completed_ps','completion_reception_id']);name(target.terminal_id);if(owned.has(target.terminal_id)||!(can.nodes.includes(target.terminal_id)||ethernet.devices.get(target.terminal_id)?.kind==='endpoint'))fail('segment terminalがapplication受信先ではありません');const key=canonical([origin,target.segment_id,target.branch_lineage,target.terminal_id]),completed=actual(target.completed_ps),rx=target.completion_reception_id===null?null:native(target.completion_reception_id,media==='can'?['can.receiver']:['ethernet.reception']);if(resolveOrigin(target.origin_id)!==origin||target.segment_id!==d.segment_id||canonical(target.branch_lineage)!==canonical(lineage)||tuples.has(key)||(completed===null?rx!==null:(!rx||completed<sof)))fail('segment completion tupleが不正です');tuples.add(key);
        if(rx){const receiver=rx.schema_name==='can.receiver'?rx.data.receiver:rx.schema_name==='ethernet.reception'?owner(rx.data.ingress):null,at=rx.schema_name==='can.receiver'?rx.data.received_ps:rx.data.ready_ps;if(receiver!==target.terminal_id||at!==target.completed_ps||owned.has(receiver)||(media==='can'?rx.data.request_id:rx.data.frame_id)!==d.source_record_id)fail('segment terminal completionが不一致です');}
      }
      segments.set(d.segment_id,{...d,origin_id:origin,sourceMedia:media,sof,raw:row});
    }
    const events=[...ethernet.events,...can.events];for(const map of [conversions,branches,segments])for(const row of map.values())for(const key of ['observed','ready','released','offer','admitted','sof'])if(row[key]!==undefined&&row[key]!==null)events.push({time:row[key],id:row.raw.record_id,kind:key});
    return {...ethernet,raw,composite:true,canModel:can,canApi:C,conversions,branches,segments,origins,originByRecord,originAliases,gateways,events,eventTimes:[...new Set([ethernet.start,ethernet.end,...events.map(e=>e.time)])].sort(compare)};
  }
  function compositeStateAt(m,time) {
    const conversions=new Map(),branches=new Map();let held=0,targetCount=0n,completed=0n;
    for(const c of m.conversions.values()){const state=time<c.observed?'not_created':c.status==='rejected'?'rejected':c.released!==null&&time>=c.released?'released':c.ready!==null&&time>=c.ready?'waiting_tx':'processing';conversions.set(c.conversion_id,state);if(['processing','waiting_tx'].includes(state))held++;}
    for(const b of m.branches.values()){const c=m.conversions.get(b.conversion_id);branches.set(b.branch_id,time<c.observed?'not_created':c.ready===null||time<c.ready?'processing':b.status==='dropped'&&time>=BigInt(b.raw.time_ps)?'dropped':b.admitted!==null&&time>=b.admitted?'admitted':'waiting');}
    for(const s of m.segments.values())if(s.sof<=time){targetCount+=BigInt(s.target_count);for(const target of s.targets)if(target.completed_ps!==null&&BigInt(target.completed_ps)<=time)completed++;}
    return {conversions,branches,held,targetCount,completed,can:m.canApi.stateAt(m.canModel,time)};
  }
  function parseResults(raw) {
    if(raw?.schema_version===2&&raw.metadata?.model_profile==='can.ethernet.gateway.v1')return parseCompositeResults(raw);
    if (raw?.schema_version !== 2 || !['ethernet.l2.store-forward.v1','ethernet.l2.qos.v1','ethernet.l2.vlan.v1','ethernet.l2.store-forward.v2','ethernet.l2.100base-t1.v1','ethernet.l2.dynamic.v1','ethernet.tsn.v1'].includes(raw.metadata?.model_profile)) fail('未対応のprofileです');
    const media=['ethernet.l2.store-forward.v2','ethernet.l2.100base-t1.v1'].includes(raw.metadata.model_profile);
    const dynamic=['ethernet.l2.dynamic.v1','ethernet.tsn.v1'].includes(raw.metadata.model_profile),tsn=raw.metadata.model_profile==='ethernet.tsn.v1';
    const vlan=dynamic||raw.metadata.model_profile==='ethernet.l2.vlan.v1',qos=vlan||raw.metadata.model_profile==='ethernet.l2.qos.v1';
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
    const policies=new Map(),ingressPolicies=new Map(),flows=new Map(),referenceMembers=new Map();
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
      // Decode declared references against capability; retain static membership for replay.
      for(const [port,p]of policies)referenceMembers.set(port,new Map(p.members));
      if(dynamic){
        for(const capability of array(raw.metadata.ethernet_dynamic?.config?.registrable)){
          fields(capability,['port','vid','tagged']);if(!Number.isInteger(capability.vid))fail('registrable VIDは整数でなければなりません');const member=referenceMembers.get(capability.port),v=String(capability.vid);vid(v);
          if(!member||capability.tagged!==true||member.has(v))fail('registrableは既知Portの一意なtagged dynamic VIDでなければなりません');
          member.set(v,true);
        }
      }
      for(const d of devices.values()){
        const keys=new Set();d.groups=new Map();
        for(const entry of array(d.multicast)){
          fields(entry,d.kind==='switch'?['vid','dst_mac','egresses']:['vid','dst_mac']);vid(entry.vid);mac(entry.dst_mac);
          const key=`${entry.vid}/${entry.dst_mac}`;if(!group(entry.dst_mac)||entry.dst_mac==='ff:ff:ff:ff:ff:ff'||reserved(entry.dst_mac)||keys.has(key))fail('multicast表/購読が不正です');keys.add(key);
          if(d.kind==='switch'){const egresses=array(entry.egresses);if(new Set(egresses).size!==egresses.length)fail('multicast egress重複です');for(const port of egresses)if(owner(port)!==d.id||!referenceMembers.get(port)?.has(entry.vid))fail('multicast egress参照が不正です');d.groups.set(key,new Set(egresses));}
          else{if(![...policies.values()].some(p=>owner(p.port)===d.id&&referenceMembers.get(p.port).has(entry.vid)))fail('購読VLANがmemberではありません');d.groups.set(key,true);}
        }
        if(d.kind==='switch'){
          if(!['flood','drop'].includes(d.unknown_multicast))fail('unknown_multicastが不正です');d.vlanFdb=new Map();
          for(const entry of array(d.vlan_fdb)){fields(entry,['vid','dst_mac','egress']);vid(entry.vid);mac(entry.dst_mac);const key=`${entry.vid}/${entry.dst_mac}`;if(group(entry.dst_mac)||entry.dst_mac==='00:00:00:00:00:00'||d.vlanFdb.has(key)||owner(entry.egress)!==d.id||!referenceMembers.get(entry.egress)?.has(entry.vid))fail('VLAN FDB参照が不正です');d.vlanFdb.set(key,entry.egress);}
        }
      }
      if(dynamic&&raw.metadata.model_schemas===undefined)fail('dynamic schema宣言がありません');
      if(raw.metadata.model_schemas!==undefined){const expected=dynamic?new Map(['frame','transfer','reception','control','policy'].map(k=>[`ethernet.dynamic.${k}`,1]).concat(tsn?['gate','credit','policing','decision'].map(k=>[`ethernet.tsn.${k}`,1]):[])):new Map([['ethernet.frame',3],['ethernet.transfer',3],['ethernet.reception',2]]),seen=new Set();for(const s of array(raw.metadata.model_schemas)){fields(s,['schema_name','schema_version']);if(expected.get(s.schema_name)!==s.schema_version||seen.has(s.schema_name))fail('metadata schemaが不正です');seen.add(s.schema_name);}if(seen.size!==expected.size)fail('metadata schemaが不足しています');}
      for(const f of array(raw.metadata.flows)){fields(f,['flow_id','priority','deadline_ps','dst_mac','tag','source_vlan_id',...(dynamic?['ip_multicast']:[])]);name(f.flow_id);priority(f.priority);optional(f.deadline_ps);mac(f.dst_mac);tag(f.tag);vid(f.source_vlan_id);if(reserved(f.dst_mac)||flows.has(f.flow_id)||f.tag&&(f.tag.vid!==f.source_vlan_id||f.tag.pcp!==f.priority))fail('source flow契約が不正です');if(dynamic)validateIpMulticast(f.ip_multicast,f.dst_mac,f.ip_multicast?.family==='ipv6'?'34525':'2048');flows.set(f.flow_id,f);}
    }
    const frames=new Map(),transfers=new Map(),receptions=new Map(),attempts=new Map(),physicalLinks=new Map(),events=[],policyRows=[],controlRows=[],tsnRows=[];
    const effectSequences=new Set();
    function times(row, names) {const r={...row};for(const k of names)r[k]=actual(row[k]);return r;}
    const schemas={
      'ethernet.frame':['source','src_mac','dst_mac','ether_type','data_hex','pad_bytes','mac_bytes','fcs_hex','mac_hex','generated_ps','ready_ps'],
      'ethernet.transfer':['frame_id','parent_transfer_id','from_port','to_port','queued_ps','sof_ps','eof_ps','release_ps','arrival_ps','planned_eof_ps','planned_release_ps','planned_arrival_ps','status','drop_reason'],
      'ethernet.reception':['frame_id','transfer_id','ingress','observed_ps','ready_ps','planned_ready_ps','status','reason','egress_transfer_ids']
    };
    if(qos){schemas['ethernet.frame'].push('flow_id','priority','deadline_ps');schemas['ethernet.transfer'].push('queue_id','priority');}
    if(vlan){schemas['ethernet.frame'].push('tag','source_vlan_id');schemas['ethernet.transfer'].push('vlan_id','wire');schemas['ethernet.reception'].push('vlan_id','priority');}
    if(dynamic){schemas['ethernet.frame'].push('ip_multicast','effect_seq');schemas['ethernet.transfer'].push('visit_id','copy_id','policy_epoch_offer','policy_epoch_sof','effect_seq');schemas['ethernet.reception'].push('visit_id','policy_epoch_ingress','effect_seq');}
    if(media)schemas['ethernet.transfer'].push('physical_link','attempt_count','collision_count','last_attempt_id','backoff_until_ps');
    for(const original of array(sim.model_records)) {
      let row=original;
      if(dynamic){
        fields(row,['schema_name','schema_version','record_id','subject','request_id','origin_request_id','time_ps','data']);
        if(row.schema_version!==1||row.subject!=='@network'||row.origin_request_id!==null)fail('dynamic包絡が不正です');
        const effect=decimal(row.data?.effect_seq);if(effectSequences.has(effect))fail('effect_seqが重複しています');effectSequences.add(effect);
        const initialPolicy=row.schema_name==='ethernet.dynamic.policy'&&row.data.initial===true&&row.time_ps==='0',initialTsn=tsn&&end===0n&&row.time_ps==='0'&&sim.committed_events==='0'&&(row.schema_name==='ethernet.tsn.gate'&&row.data.cause==='initial'||row.schema_name==='ethernet.tsn.decision'&&row.data.transfer_id===null&&row.data.priority===null&&row.data.state==='ready'&&row.data.reason==='ready'&&row.data.next_wake_ps===null);const time=end===0n&&(initialPolicy||initialTsn)?0n:actual(row.time_ps);if(sim.termination==='time_limit'&&time===end&&!(initialPolicy&&time===0n||initialTsn))fail('停止境界の実績です');if(time===null)fail('dynamic確定時刻がありません');
        if(row.schema_name==='ethernet.dynamic.policy'){if(row.request_id!==null||row.data.time_ps!==row.time_ps)fail('policy包絡時刻が不一致です');const {effect_seq,...data}=row.data;policyRows.push({...data,_effect:effect,_id:row.record_id});events.push({time,id:row.record_id,kind:'policy',effect});continue;}
        if(row.schema_name==='ethernet.dynamic.control'){controlRows.push(row);events.push({time,id:row.record_id,kind:'control',effect});continue;}
        if(tsn&&['ethernet.tsn.gate','ethernet.tsn.credit','ethernet.tsn.policing','ethernet.tsn.decision'].includes(row.schema_name)){tsnRows.push(row);events.push({time,id:row.record_id,kind:row.schema_name,effect});continue;}
        const kind={'ethernet.dynamic.frame':'ethernet.frame','ethernet.dynamic.transfer':'ethernet.transfer','ethernet.dynamic.reception':'ethernet.reception'}[row.schema_name];if(!kind)fail('未対応のdynamic schemaです');row={...row,schema_name:kind};
      }
      if(media&&['ethernet.attempt','ethernet.phy_link'].includes(row.schema_name)){
        fields(row,['schema_name','schema_version','record_id','subject','request_id','origin_request_id','time_ps','data']);
        if(row.schema_version!==1||row.origin_request_id!==null)fail('media schemaが不正です');
        const r={...row.data,id:name(row.record_id),subject:name(row.subject),raw:row,updated:decimal(row.time_ps)};
        if(row.schema_name==='ethernet.phy_link'){
          fields(r.raw.data,['a','b','phy_mode','duplex','bitrate_bps','propagation_ps','a_phy','b_phy','link_state','seed','deference_policy','backoff_policy']);
          if(row.request_id!==null||r.updated!==0n||r.subject!==`@media:${r.id}`||r.a>=r.b||!directions.has(r.a)||!directions.has(r.b)||owner(directions.get(r.a).to_port)!==owner(r.b)||owner(directions.get(r.b).to_port)!==owner(r.a)||!['half','full'].includes(r.duplex)||r.link_state!=='up'||physicalLinks.has(r.id))fail('phy_link参照が不正です');
          decimal(r.bitrate_bps);decimal(r.propagation_ps);decimal(r.seed);
          for(const phy of [r.a_phy,r.b_phy]){fields(phy,['role','tx_latency_ps','rx_latency_ps']);decimal(phy.tx_latency_ps);decimal(phy.rx_latency_ps);}
          const rate=BigInt(r.bitrate_bps),t1=r.phy_mode.endsWith('base-t1');
          if(raw.metadata.model_profile==='ethernet.l2.100base-t1.v1'?(r.phy_mode!=='100base-t1'||rate!==100000000n):!['10base-t','100base-tx','1000base-t1'].includes(r.phy_mode))fail('PHY profileが不一致です');
          if(t1?(r.duplex!=='full'||new Set([r.a_phy.role,r.b_phy.role]).size!==2||![r.a_phy.role,r.b_phy.role].every(x=>['master','slave'].includes(x))):[r.a_phy,r.b_phy].some(p=>p.role!=='none'||p.tx_latency_ps!=='0'||p.rx_latency_ps!=='0'))fail('PHY roleが不正です');
          if(!t1&&rate!==(r.phy_mode==='10base-t'?10000000n:100000000n)||r.phy_mode==='1000base-t1'&&rate!==1000000000n)fail('PHY速度が不正です');
          physicalLinks.set(r.id,r);
        }else{
          fields(r.raw.data,['transfer_id','physical_link','from_port','to_port','number','sof_ps','planned_eof_ps','planned_release_ps','planned_arrival_ps','collision_ps','planned_jam_start_ps','planned_jam_end_ps','jam_end_ps','eof_ps','release_ps','arrival_ps','backoff_slots','backoff_until_ps','status','planned_mdi_sof_ps','planned_mdi_eof_ps','planned_peer_mdi_sof_ps','planned_peer_mdi_eof_ps']);
          if(row.request_id===null||r.subject!==r.from_port||r.id!==`${r.transfer_id}#${r.number}`||!['transmitting','jamming','collided','serialized'].includes(r.status)||attempts.has(r.id))fail('attempt参照・状態が不正です');
          for(const k of ['sof_ps','collision_ps','jam_end_ps','eof_ps','release_ps','arrival_ps'])r[k]=actual(r[k]);
          for(const k of ['planned_eof_ps','planned_release_ps','planned_arrival_ps','planned_jam_start_ps','planned_jam_end_ps','backoff_slots','backoff_until_ps','planned_mdi_sof_ps','planned_mdi_eof_ps','planned_peer_mdi_sof_ps','planned_peer_mdi_eof_ps'])r[k]=optional(r[k]);
          r.number=decimal(r.number);if(r.sof_ps===null||r.number<1n||r.number>16n||r.planned_eof_ps<=r.sof_ps||r.planned_release_ps<=r.planned_eof_ps||r.planned_arrival_ps<r.planned_eof_ps||r.updated!==[r.sof_ps,r.collision_ps,r.jam_end_ps,r.eof_ps,r.release_ps,r.arrival_ps].filter(x=>x!==null).reduce((a,b)=>a>b?a:b))fail('attempt時刻が不正です');
          if(r.collision_ps!==null){if(r.eof_ps!==null||r.release_ps!==null||r.arrival_ps!==null||r.planned_mdi_eof_ps!==null||r.planned_peer_mdi_eof_ps!==null||r.planned_jam_start_ps<r.collision_ps||r.planned_jam_end_ps<=r.planned_jam_start_ps||r.status!==(r.jam_end_ps===null?'jamming':'collided'))fail('衝突attemptの実績が不正です');}
          else if(r.status!==(r.eof_ps===null?'transmitting':'serialized')||[r.planned_jam_start_ps,r.planned_jam_end_ps,r.jam_end_ps,r.backoff_slots,r.backoff_until_ps].some(x=>x!==null)||(r.eof_ps===null)!==(r.planned_mdi_eof_ps===null)||(r.eof_ps===null)!==(r.planned_peer_mdi_eof_ps===null))fail('正常attemptの実績が不正です');
          for(const k of ['sof_ps','collision_ps','jam_end_ps','eof_ps','release_ps','arrival_ps'])if(r[k]!==null)events.push({time:r[k],id:r.id,kind:k,frame:row.request_id});
          attempts.set(r.id,r);
        }
        continue;
      }
      fields(row,['schema_name','schema_version','record_id','subject','request_id','origin_request_id','time_ps','data']);
      const version=dynamic?1:media&&row.schema_name==='ethernet.transfer'?2:vlan?(row.schema_name==='ethernet.reception'?2:3):(qos&&row.schema_name!=='ethernet.reception'?2:1);
      if(!schemas[row.schema_name]||row.schema_version!==version||row.origin_request_id!==null)fail('未対応のレコードschemaです');
      fields(row.data,schemas[row.schema_name]);name(row.record_id);name(row.subject);name(row.request_id);
      const updated=actual(row.time_ps);if(updated===null)fail('更新時刻がありません');
      let r,map,actualKeys;
      if(row.schema_name==='ethernet.frame') {
        actualKeys=['generated_ps','ready_ps'];r=times(row.data,actualKeys);map=frames;
        if(row.record_id!==row.request_id||(!dynamic&&r.source!==row.subject)||devices.get(r.source)?.kind!=='endpoint'||r.generated_ps===null)fail('frame参照が不正です');
        for(const k of ['ether_type','pad_bytes','mac_bytes'])decimal(r[k]);
        if(qos){name(r.flow_id);if(decimal(r.priority)>7n)fail('frame priorityが不正です');r.deadline_ps=optional(r.deadline_ps);}
        if(dynamic)validateIpMulticast(r.ip_multicast,r.dst_mac,r.ether_type);
        if(vlan){vid(r.source_vlan_id);validateWire(r);if(r.tag&&(r.tag.vid!==r.source_vlan_id||r.tag.pcp!==r.priority))fail('source tag/classが不一致です');}
        if(!/^(?:[0-9a-f]{2})*$/.test(r.data_hex)||! /^(?:[0-9a-f]{2})+$/.test(r.mac_hex)||! /^[0-9a-f]{8}$/.test(r.fcs_hex))fail('frame bytesが不正です');
        if(r.ready_ps!==null&&r.ready_ps<r.generated_ps)fail('frame時刻順が不正です');
      } else if(row.schema_name==='ethernet.transfer') {
        actualKeys=['queued_ps','sof_ps','eof_ps','release_ps','arrival_ps'];r=times(row.data,actualKeys);map=transfers;
        for(const k of ['planned_eof_ps','planned_release_ps','planned_arrival_ps'])r[k]=optional(r[k]);
        if(row.request_id!==r.frame_id||(!dynamic&&(row.subject!==r.from_port||row.record_id!==`${r.frame_id}@${r.from_port}`))||directions.get(r.from_port)?.to_port!==r.to_port||r.queued_ps===null)fail('transfer参照が不正です');
        if(qos&&(decimal(r.priority)>7n||r.queue_id!==`${r.from_port}.queue.${r.priority}`))fail('class queueが不正です');
        if(vlan){vid(r.vlan_id);fields(r.wire,wireKeys);validateWire(r.wire);if(r.wire.tag&&(r.wire.tag.vid!==r.vlan_id||r.wire.tag.pcp!==r.priority))fail('copy tag/classが不一致です');}
        if(media){
          r.attempt_count=decimal(r.attempt_count);r.collision_count=decimal(r.collision_count);r.backoff_until_ps=optional(r.backoff_until_ps);
          if(!['queued','deferred','transmitting','jamming','backoff','serialized','dropped'].includes(r.status)||!['queue_full','attempt_limit',null].includes(r.drop_reason)||(r.status==='dropped')!==(r.drop_reason!==null)||(r.status==='serialized')!==(r.eof_ps!==null)||r.eof_ps===null&&(r.release_ps!==null||r.arrival_ps!==null))fail('media transfer状態が不正です');
          if(r.sof_ps===null? r.attempt_count!==0n||r.last_attempt_id!==null:r.sof_ps<r.queued_ps||r.attempt_count===0n||r.last_attempt_id===null)fail('media SOFと試行数が不一致です');
          for(const k of ['eof','release','arrival'])if(r[`${k}_ps`]!==null&&r[`${k}_ps`]!==r[`planned_${k}_ps`])fail('media実績と予定が不一致です');
        }else{
        if(!['queued','transmitting','serialized','dropped'].includes(r.status))fail('transfer状態が不正です');
        if((r.status==='dropped')!==(dynamic?['queue_full','link_down','stp_discarding','vlan_unregistered'].includes(r.drop_reason):r.drop_reason==='queue_full') || (r.status!=='dropped'&&r.drop_reason!==null))fail('drop reasonが不正です');
        if((r.status==='serialized')!==(r.eof_ps!==null)||(['serialized','transmitting'].includes(r.status))!==(r.sof_ps!==null))fail('transfer状態と時刻が一致しません');
        if(r.sof_ps===null) {if(['eof_ps','release_ps','arrival_ps','planned_eof_ps','planned_release_ps','planned_arrival_ps'].some(k=>r[k]!==null))fail('SOF前の送信時刻です');}
        else {
          if(r.sof_ps<r.queued_ps||r.planned_eof_ps===null||r.planned_release_ps===null||r.planned_arrival_ps===null||r.planned_eof_ps<=r.sof_ps||r.planned_release_ps<=r.planned_eof_ps||r.planned_arrival_ps<r.planned_eof_ps)fail('link時刻順が不正です');
          for(const k of ['eof','release','arrival'])if(r[`${k}_ps`]!==null&&r[`${k}_ps`]!==r[`planned_${k}_ps`])fail('実績と予定が不一致です');
          if(r.eof_ps===null&&(r.arrival_ps!==null||r.release_ps!==null))fail('EOF前に到達しています');
        }
        }
      } else {
        actualKeys=['observed_ps','ready_ps'];r=times(row.data,actualKeys);map=receptions;r.planned_ready_ps=optional(r.planned_ready_ps);
        if(row.request_id!==r.frame_id||row.record_id!==`${r.transfer_id}@rx`||(!dynamic&&row.subject!==owner(r.ingress))||!devices.has(dynamic?owner(r.ingress):row.subject)||r.observed_ps===null)fail('reception参照が不正です');
        if(!['processing','received','filtered','forwarded'].includes(r.status)||(r.status==='processing')!==(r.ready_ps===null)||r.ready_ps!==null&&r.ready_ps<r.observed_ps)fail('reception状態が不正です');
        array(r.egress_transfer_ids);if(new Set(r.egress_transfer_ids).size!==r.egress_transfer_ids.length)fail('重複copyです');
        if(r.status==='filtered'?!(dynamic?[...filterReasons,'link_down','stp_discarding','vlan_unregistered','known_egress_ineligible','multicast_filtered','visit_limit','psfp_max_sdu','psfp_gate_closed','psfp_meter_red','psfp_meter_yellow']:vlan?filterReasons:['destination_mismatch','same_ingress']).includes(r.reason):r.reason!==null)fail('filter reasonが不正です');
        if(vlan){vid(r.vlan_id);priority(r.priority);if(['ingress_frame_type','ingress_vlan_membership','destination_mismatch','multicast_not_subscribed'].includes(r.reason)&&(r.ready_ps!==r.observed_ps||r.planned_ready_ps!==null))fail('即filter時刻が不正です');}
        if(r.status!=='forwarded'&&r.egress_transfer_ids.length)fail('未転送の子copyです');
        if(r.planned_ready_ps!==null&&(r.planned_ready_ps<r.observed_ps||r.ready_ps!==null&&r.ready_ps!==r.planned_ready_ps))fail('受信処理時刻が不正です');
      }
      if(map.has(row.record_id))fail('レコードが重複しています');
      r.id=row.record_id;r.subject=dynamic?(row.schema_name==='ethernet.reception'?owner(r.ingress):row.schema_name==='ethernet.transfer'?r.from_port:r.source):row.subject;r.raw=original;r.updated=updated;map.set(r.id,r);
      let last=null;for(const k of actualKeys)if(r[k]!==null){events.push({time:r[k],id:r.id,kind:k,frame:row.request_id});if(last===null||r[k]>last)last=r[k];}
      if((media||dynamic)&&row.schema_name==='ethernet.transfer'?(updated<last):updated!==last)fail('更新時刻が最後の実績と一致しません');
    }
    if(media){
      const covered=new Set();for(const p of physicalLinks.values())for(const port of [p.a,p.b]){if(covered.has(port))fail('physical linkのport重複です');covered.add(port);const d=directions.get(port);if(d.bitrate!==BigInt(p.bitrate_bps)||d.delay!==BigInt(p.propagation_ps))fail('physical link速度・遅延が不一致です');}if(covered.size!==directions.size)fail('physical linkが不足しています');
      for(const a of attempts.values()){const t=transfers.get(a.transfer_id),p=physicalLinks.get(a.physical_link);if(!t||!p||a.from_port!==t.from_port||a.to_port!==t.to_port||a.raw.request_id!==t.frame_id||t.physical_link!==p.id)fail('attempt親参照が不正です');}
      for(const t of transfers.values()){
        t.attempts=[...attempts.values()].filter(a=>a.transfer_id===t.id).sort((a,b)=>compare(a.number,b.number));const p=physicalLinks.get(t.physical_link);
        if(!p||![p.a,p.b].includes(t.from_port)||BigInt(t.attempts.length)!==t.attempt_count||BigInt(t.attempts.filter(a=>a.collision_ps!==null).length)!==t.collision_count||t.last_attempt_id!==(t.attempts.at(-1)?.id??null)||t.attempts.some((a,i)=>a.number!==BigInt(i+1)))fail('transfer試行保存則が不正です');
        // Backoff expiration is a committed state transition, not a planned completion.
        for(const attempt of t.attempts)if(attempt.jam_end_ps!==null&&attempt.backoff_until_ps!==null){
          const deadline=attempt.backoff_until_ps;
          if(deadline<end||sim.termination==='execution_failed'&&deadline===end&&t.updated>=deadline&&t.status!=='backoff')events.push({time:deadline,id:attempt.id,kind:'backoff_expired',frame:t.frame_id});
        }
        events.push({time:t.updated,id:t.id,kind:'transfer_updated',frame:t.frame_id});
        t.initialStateAt=time=>{
          if(time===end&&t.sof_ps===null&&['queued','deferred'].includes(t.status))return t.status;
          if(p.duplex!=='half')return 'queued';
          const own=[...transfers.values()].filter(other=>other.from_port===t.from_port&&other.id!==t.id);
          const offerOrder=copy=>{const point=(sim.records||[]).find(row=>row.metric==='ethernet.media.queue_length'&&row.target===`${copy.from_port}.queue`&&row.request_id===copy.frame_id);return point?BigInt(point.seq):null;};
          const before=(a,b)=>{if(a.queued_ps!==b.queued_ps)return compare(a.queued_ps,b.queued_ps);const ao=offerOrder(a),bo=offerOrder(b);return ao!==null&&bo!==null?compare(ao,bo):compare(a.id,b.id);};
          if(own.some(other=>other.queued_ps<=time&&!(other.status==='dropped'&&other.updated<=time)&&
              (other.attempts?.[0]?.sof_ps<=time?(other.release_ps===null||other.release_ps>time):before(other,t)<0)))return 'queued';
          let lastIdle=null;
          for(const signal of attempts.values())if(signal.physical_link===p.id){
            const shift=signal.from_port===t.from_port?0n:BigInt(p.propagation_ps),start=signal.sof_ps+shift;
            const finish=(signal.collision_ps!==null?(signal.jam_end_ps??signal.planned_jam_end_ps??end):(signal.eof_ps??end))+shift;
            if(start<=time&&time<finish)return 'deferred';
            if(finish<=time&&(lastIdle===null||finish>lastIdle))lastIdle=finish;
          }
          return lastIdle!==null&&time<lastIdle+96n*(1000000000000n/BigInt(p.bitrate_bps))?'deferred':'queued';
        };
        const a=t.attempts.at(-1);if(a&&(t.sof_ps!==a.sof_ps||t.eof_ps!==a.eof_ps||t.release_ps!==a.release_ps||t.arrival_ps!==a.arrival_ps)||t.drop_reason==='attempt_limit'&&(t.attempt_count!==16n||a?.jam_end_ps===null||a?.backoff_slots!==null))fail('transfer最終試行が不一致です');
      }
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
    for(const f of frames.values())if((f.ready_ps!==null)!==(dynamic?[...transfers.values()].some(t=>t.frame_id===f.id&&t.parent_transfer_id===null):transfers.has(`${f.id}@${f.source}.tx`)))fail('source readyとcopyが一致しません');
    if(vlan&&dynamic)for(const f of frames.values()){
      const p=policies.get(`${f.source}.tx`),members=referenceMembers.get(`${f.source}.tx`);
      if(!p||!members?.has(f.source_vlan_id)||members.get(f.source_vlan_id)!==Boolean(f.tag)||(f.tag?f.tag.vid:p.pvid)!==f.source_vlan_id)fail('source VLAN capabilityが不一致です');
    }
    if(vlan&&!dynamic){
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
    const dynamicModel={frames,transfers,receptions,directions,devices,policies,ingressPolicies,end,events};
    let policyHistory=null;if(dynamic)policyHistory=validateDynamicRows(raw,policyRows,controlRows,tsnRows,dynamicModel);
    // Following parents also rejects cycles independently of timestamp equality.
    for(const t of transfers.values()){const seen=new Set();let p=t;while(p){if(seen.has(p.id))fail('parent循環です');seen.add(p.id);p=p.parent_transfer_id===null?null:transfers.get(p.parent_transfer_id);}}
    events.sort((a,b)=>compare(a.time,b.time)||(dynamic?compare(a.effect??0n,b.effect??0n):0)||compare(a.id,b.id)||compare(a.kind,b.kind));
    return {raw,start,end,qos,vlan,media,dynamic,tsn,tsnState:dynamicModel.tsnState,policyHistory,controlRows,tsnRows,attempts,physicalLinks,outputs,policies,ingressPolicies,devices,directions,frames,transfers,receptions,events,eventTimes:[...new Set([start,...events.map(e=>e.time),end])].sort(compare)};
  }
  function transferStateAt(t,time){if(t.attempts){
      if(time<t.queued_ps)return 'not_created';if(t.status==='dropped'&&time>=t.updated)return 'dropped';
      const a=t.attempts.filter(a=>a.sof_ps<=time).at(-1);if(!a)return t.initialStateAt?.(time)??'queued';
      if(a.eof_ps!==null&&time>=a.eof_ps)return 'serialized';if(a.collision_ps===null||time<a.collision_ps)return 'transmitting';
      if(a.jam_end_ps===null||time<a.jam_end_ps)return 'jamming';if(a.backoff_until_ps!==null&&time<a.backoff_until_ps)return 'backoff';return 'deferred';
    }if(time<t.queued_ps)return 'not_created';if(t.status==='dropped'&&time>=t.updated)return 'dropped';if(t.sof_ps===null||time<t.sof_ps)return 'queued';if(t.eof_ps===null||time<t.eof_ps)return 'transmitting';return 'serialized';}
  function receptionStateAt(r,time){if(time<r.observed_ps)return 'not_created';if(r.ready_ps===null||time<r.ready_ps)return 'processing';return r.status;}
  function stateAt(m,time){
    const counts={generated:0,queued:0,deferred:0,jamming:0,backoff:0,transmitting:0,serialized:0,dropped:0,received:0,processing:0,filtered:0,forwarded:0};
    const queues=new Map([...m.directions.keys()].map(p=>[p,[]]));
    const classes=new Map();if(m.qos)for(const out of m.outputs.values())for(const q of out.queues)classes.set(`${out.port}.queue.${q.priority}`,{...q,port:out.port,scheduler:out.scheduler,ids:[],bytes:0n});
    for(const f of m.frames.values())if(f.generated_ps<=time)counts.generated++;
    for(const t of m.transfers.values()){const s=transferStateAt(t,time);if(s!=='not_created')counts[s]++;if(s==='queued'||s==='deferred'&&t.attempts?.length===0){queues.get(t.from_port).push(t.id);if(m.qos){const q=classes.get(t.queue_id);q.ids.push(t.id);q.bytes+=BigInt((m.vlan?t.wire:m.frames.get(t.frame_id)).mac_bytes);}}}
    for(const r of m.receptions.values()){const s=receptionStateAt(r,time);if(s!=='not_created')counts[s]++;}
    return {counts,queues,classes,...(m.composite?{composite:compositeStateAt(m,time)}:{}),...(m.dynamic?{policy:policyStateAt(m.policyHistory,time),tsn:tsnStateAt(m,time)}:{})};
  }
  function stepTransfers(m,destination){if(m.media){const i=m.eventTimes.findIndex(t=>t===destination);if(i<1)return [];const from=m.eventTimes[i-1];return [...m.attempts.values()].filter(a=>a.collision_ps===null&&a.sof_ps<=destination&&(a.arrival_ps??m.end)>from).map(a=>({id:a.transfer_id,attempt:a.id,frame:m.transfers.get(a.transfer_id).frame_id,from:a.from_port,to:a.to_port,start:a.sof_ps<from?from:a.sof_ps,end:a.arrival_ps,arrived:a.arrival_ps!==null&&a.arrival_ps<=destination}));}const i=m.eventTimes.findIndex(t=>t===destination);if(i<1)return [];const from=m.eventTimes[i-1];const items=[...m.transfers.values()].filter(t=>t.sof_ps!==null&&t.sof_ps<=destination&&((t.arrival_ps??m.end)>from)).map(t=>({id:t.id,frame:t.frame_id,from:t.from_port,to:t.to_port,start:t.sof_ps<from?from:t.sof_ps,end:t.arrival_ps===null?null:t.arrival_ps,arrived:t.arrival_ps!==null&&t.arrival_ps<=destination}));if(m.composite)items.push(...m.canApi.stepTransfers(m.canModel,from,destination).map(t=>({...t,medium:'can',id:t.requestId,from:t.kind==='tx'?t.source:t.bus,to:t.kind==='tx'?t.bus:t.receiver})));return items;}
  function visibleTransfers(m,vlanId=null){return [...m.transfers.values()].filter(t=>vlanId===null||t.vlan_id===vlanId);}
  return {parseResults,stateAt,transferStateAt,receptionStateAt,stepTransfers,visibleTransfers,owner,compositeStateAt,decodeCanPayload,prepareSchedule,scheduleStateAt,openElapsed,creditNumeratorAt,potentialTokens,nextGateFit,validateIpMulticast,preparePolicyHistory,policyStateAt,tsnStateAt};
});
