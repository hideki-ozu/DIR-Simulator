/* Exact prefix validation for schema-3 registered and cooperative results. */
(function(root,factory){'use strict';const api=factory();if(typeof module==='object'&&module.exports)module.exports=api;else root.DIRControlledModel=api;})(typeof globalThis==='object'?globalThis:this,function(){
  'use strict';
  const U64=(1n<<64n)-1n,U128=(1n<<128n)-1n;
  const extensionProfiles=new Set(['can.ethernet.tsn.v1','ethernet.redundancy.abstract.v1','ethernet.frer.rtag.v1','ethernet.network.composite.v1','ethernet.preemption.abstract.v1','ethernet.control.wire.experimental.v1']);
  function knownSchemas(profile){
    const schemas=new Set(['frame','transfer','reception','control','policy'].map(k=>`ethernet.dynamic.${k}`));
    if(profile!=='ethernet.control.wire.experimental.v1')for(const k of ['gate','credit','policing','decision'])schemas.add(`ethernet.tsn.${k}`);
    if(profile==='can.ethernet.tsn.v1')for(const k of ['can.request','can.receiver','dir.can_ethernet.conversion','dir.can_ethernet.branch','dir.can_ethernet.segment','dir.can_ethernet.terminal_indication','dir.composite.ingress'])schemas.add(k);
    if(['can.ethernet.tsn.v1','ethernet.redundancy.abstract.v1','ethernet.frer.rtag.v1','ethernet.network.composite.v1'].includes(profile))for(const k of ['branch','recovery','origin','timeout'])schemas.add(`dir.redundancy.${k}`);
    if(profile==='can.ethernet.tsn.v1')for(const k of ['packet','assembly','verification','capability'])schemas.add(`dir.mac_merge.${k}`);
    if(profile==='ethernet.network.composite.v1'){
      for(const k of ['packet','assembly','verification','capability'])schemas.add(`dir.mac_merge.${k}`);
      schemas.add('dir.network.clock');
      schemas.add('dir.network.gptp');
      schemas.add('dir.network.phy');
      schemas.add('dir.network.qcc');
      schemas.add('dir.network.segment');
      schemas.add('dir.network.provider');
      schemas.add('dir.network.mac_control');
      schemas.add('dir.network.configured_media');
      for(const k of ['ethernet.wire.boundary','ethernet.wire.membership','ethernet.wire.membership_timer','ethernet.control.wire.event','ethernet.control.wire.policy','ethernet.control.wire.transition','ethernet.control.wire.tree_policy'])schemas.add(k);
      for(const k of ['transfer','fragment','gap','delivery','decision'])schemas.add(`dir.preemption.${k}`);
    }
    if(profile==='ethernet.preemption.abstract.v1'){schemas.delete('ethernet.dynamic.transfer');for(const k of ['transfer','fragment','gap','delivery','decision'])schemas.add(`dir.preemption.${k}`);}
    if(profile==='ethernet.control.wire.experimental.v1')for(const k of ['ethernet.wire.boundary','ethernet.wire.membership','ethernet.wire.membership_timer','ethernet.control.wire.event','ethernet.control.wire.policy','ethernet.control.wire.transition','ethernet.control.wire.tree_policy'])schemas.add(k);
    return schemas;
  }
  const fail=text=>{throw new Error(`実行制御・指標: ${text}`);};
  const obj=(v,label)=>v&&typeof v==='object'&&!Array.isArray(v)?v:fail(`${label} が不正です`);
  const arr=(v,label)=>Array.isArray(v)?v:fail(`${label} は配列が必要です`);
  const str=(v,label)=>typeof v==='string'&&v.length?v:fail(`${label} がありません`);
  function decimal(v,label,max=U64){if(typeof v!=='string'||!/^(0|[1-9][0-9]*)$/.test(v)||BigInt(v)>max)fail(`${label} は範囲内の十進整数が必要です`);return BigInt(v);}
  function sequence(v){if(v===undefined)return null;if(typeof v==='number'){if(!Number.isSafeInteger(v)||v<0)fail('effect seq は精度を失わない整数が必要です');return BigInt(v);}return decimal(v,'effect seq');}
  function order(a,b){if(a.time!==b.time)return a.time<b.time?-1:1;const x=sequence(a.data.effect_seq),y=sequence(b.data.effect_seq);if(x!==null&&y!==null&&x!==y)return x<y?-1:1;return a.id<b.id?-1:a.id>b.id?1:0;}
  function tagStack(wire){
    obj(wire,'wire');
    if(wire.tags===undefined)return wire.tag?[{tpid:'33024',kind:'C',...wire.tag}]:[];
    if(wire.tag!==null)fail('tag と tags が重複しています');
    const tags=arr(wire.tags,'tags');
    if(tags.length<1||tags.length>2)fail('tag stack の深さが不正です');
    const stack=tags.map(value=>{
      const tag=obj(value,'tag');if(Object.keys(tag).length!==2||!Object.hasOwn(tag,'tpid')||!Object.hasOwn(tag,'tci'))fail('tag field が不正です');
      const tpid=decimal(tag.tpid,'TPID',65535n),tci=obj(tag.tci,'TCI');
      if(![33024n,34984n].includes(tpid)||Object.keys(tci).length!==3||['vid','pcp','dei'].some(k=>!Object.hasOwn(tci,k)))fail('TPID/TCI が不正です');
      decimal(tci.vid,'VID',4094n);decimal(tci.pcp,'PCP',7n);decimal(tci.dei,'DEI',1n);
      return {tpid:tag.tpid,kind:tpid===34984n?'S':'C',...tci};
    });
    if(stack[0].kind!=='S'||stack.length===2&&stack[1].kind!=='C')fail('tag stack 順序が不正です');
    const hex=str(wire.mac_hex,'MAC hex');if(!/^(?:[0-9a-f]{2})+$/.test(hex)||hex.length<2*(18+4*stack.length)||decimal(wire.mac_bytes,'MAC bytes')!==BigInt(hex.length/2)||! /^[0-9a-f]{8}$/.test(wire.fcs_hex??''))fail('tagged MAC bytes が不正です');
    for(let i=0;i<stack.length;i++){
      const t=stack[i],offset=24+8*i,expected=Number(t.tpid).toString(16).padStart(4,'0')+((Number(t.pcp)<<13)|(Number(t.dei)<<12)|Number(t.vid)).toString(16).padStart(4,'0');
      if(hex.slice(offset,offset+8)!==expected)fail('wire tag と byte が一致しません');
    }
    let crc=0xffffffff;for(let i=0;i<hex.length-8;i+=2){crc^=parseInt(hex.slice(i,i+2),16);for(let bit=0;bit<8;bit++)crc=(crc>>>1)^((crc&1)?0xedb88320:0);}crc=(crc^0xffffffff)>>>0;
    const fcs=[0,8,16,24].map(shift=>((crc>>>shift)&255).toString(16).padStart(2,'0')).join('');
    if(fcs!==wire.fcs_hex||hex.slice(-8)!==fcs)fail('tagged MAC FCS が一致しません');
    return stack;
  }
  function parseLegacy(raw, parsers = {}) {
    const exact=(value,fields,label)=>{obj(value,label);if(Object.keys(value).length!==fields.length||fields.some(k=>!Object.hasOwn(value,k)))fail(`${label} のfieldが不正です`);};
    exact(raw,['schema_name','schema_version','metadata','control','result'],'legacy envelope');
    if(raw.schema_name!=='dir.controlled.legacy'||raw.schema_version!==4)fail('legacy envelope版が不正です');
    exact(raw.metadata,['model_profile','payload_schema_version'],'legacy metadata');
    const payload=obj(raw.result,'result'),sim=obj(payload.simulation,'simulation'),c=obj(raw.control,'control');
    const profile=raw.metadata.model_profile,innerProfile=payload.schema_version===1?'can.cc.ideal.v1':payload.metadata?.model_profile;
    const can=new Set(['can.cc.ideal.v1','can.cc.multibus.v1','can.fd.precomputed.v1','can.fd.iso.v1']);
    const ethernet=new Set(['ethernet.l2.store-forward.v1','ethernet.l2.qos.v1','ethernet.l2.vlan.v1','ethernet.l2.store-forward.v2','ethernet.l2.100base-t1.v1']);
    const transaction=new Set(['axi4.transaction.v1','soc.shared.v1','ahb.transaction.v1','noc.xy.v1','memory.ipc.transaction.v1']);
    if(profile!==innerProfile||raw.metadata.payload_schema_version!==payload.schema_version||payload.schema_version!==(profile==='can.cc.ideal.v1'?1:2)||![...can,...ethernet,...transaction].includes(profile))fail('legacy profile/schemaが不一致です');
    exact(c,['version','session_id','export_ordinal','state','commit_ordinal','horizon_ps','pending_events','resume_supported','replay_sha256'],'control');
    if(c.version!=='1'||!['Paused','Stopped','Completed','Failed'].includes(c.state)||c.resume_supported!==(c.state==='Paused')||typeof c.replay_sha256!=='string'||!/^[0-9a-f]{64}$/.test(c.replay_sha256))fail('legacy control状態が不正です');
    str(c.session_id,'session_id');decimal(c.export_ordinal,'export ordinal');
    for(const [a,b] of [['commit_ordinal','committed_events'],['horizon_ps','end_ps'],['pending_events','pending_events']])if(decimal(c[a],a)!==decimal(sim[b],b))fail('legacy成功境界が不一致です');
    if(typeof sim.partial!=='boolean'||c.state!=='Completed'&&decimal(sim.last_event_time_ps??'0','last event')!==decimal(c.horizon_ps,'horizon'))fail('legacy観測期間が不一致です');
    if(c.state==='Paused'&&(!sim.partial||sim.termination!=='paused')||c.state==='Stopped'&&(!sim.partial||sim.termination!=='stopped')||c.state==='Completed'&&(sim.partial||!['events_exhausted','time_limit'].includes(sim.termination))||c.state==='Failed'&&(!sim.partial||['paused','stopped','events_exhausted','time_limit'].includes(sim.termination)))fail('legacy終了状態が不一致です');
    const family=can.has(profile)?'can':ethernet.has(profile)?'ethernet':'transaction';
    const parser=parsers[family]??(family==='can'?globalThis.DIRViewerModel:family==='ethernet'?globalThis.DIREthernetModel:globalThis.DIRTransactionModel);
    if(!parser?.parseResults)fail('legacy family parserがありません');
    const model=parser.parseResults(payload,true);
    return {payload,control:c,model,family,profile};
  }
  function parseResults(raw){
    if(raw?.schema_version!==3&&!(raw?.schema_version===2&&extensionProfiles.has(raw?.metadata?.model_profile)))fail('未対応のschema/profileです');
    const meta=obj(raw.metadata,'metadata'),sim=obj(raw.simulation,'simulation');str(meta.model_profile,'profile');
    const end=decimal(sim.end_ps,'end_ps'),committed=decimal(sim.committed_events,'committed_events');
    const control=raw.control===undefined?null:obj(raw.control,'control');
    if(control){
      const fields=['version','session_id','export_ordinal','state','commit_ordinal','horizon_ps','pending_events','resume_supported','replay_sha256'];
      if(Object.keys(control).length!==fields.length||fields.some(k=>!(k in control))||control.version!=='1')fail('control の版・field が不正です');
      str(control.session_id,'session_id');decimal(control.export_ordinal,'export_ordinal');decimal(control.pending_events,'pending_events');
      if(decimal(control.commit_ordinal,'commit_ordinal')!==committed||decimal(control.horizon_ps,'horizon_ps')!==end)fail('control の成功境界が一致しません');
      if(!['Paused','Stopped','Completed','Failed'].includes(control.state)||typeof control.resume_supported!=='boolean'||control.resume_supported!==(control.state==='Paused')||typeof control.replay_sha256!=='string'||!/^[0-9a-f]{64}$/.test(control.replay_sha256))fail('control の状態が不正です');
      if((control.state==='Paused'&&sim.termination!=='paused')||(control.state==='Stopped'&&sim.termination!=='stopped'))fail('停止状態が一致しません');
    }
    const descriptors=new Map();
    for(const d of arr(meta.metrics_v2??[],'metrics_v2')){
      obj(d,'descriptor');const name=str(d.name,'metric name');str(d.unit,'unit');
      if(descriptors.has(name)||decimal(d.version,'descriptor version',4294967295n)===0n||d.value_kind!=='integer'||!['point','gauge'].includes(d.sampling))fail('metric descriptor が不正です');
      const targets=arr(d.targets,'targets');if(!targets.length||new Set(targets).size!==targets.length||targets.some(t=>typeof t!=='string'||!t))fail('metric target が不正です');
      if(!(d.sampling==='gauge'?d.aggregation==='time_mean':['identity','sum','count','min','max','mean'].includes(d.aggregation)))fail('aggregation が不正です');
      const gauges=obj(d.initial_gauges,'initial_gauges');
      if(d.sampling==='gauge'){if(Object.keys(gauges).length!==targets.length||targets.some(t=>!(t in gauges)))fail('初期gauge が不足しています');for(const v of Object.values(gauges))decimal(v,'initial gauge');}
      else if(Object.keys(gauges).length)fail('point に初期gauge は指定できません');
      descriptors.set(name,d);
    }
    const known=extensionProfiles.has(meta.model_profile)?knownSchemas(meta.model_profile):null;
    const catalog=new Map();for(const d of arr(meta.model_schemas??[],'model_schemas')){obj(d,'schema');const name=str(d.schema_name,'schema_name');if(catalog.has(name)||!Number.isInteger(d.schema_version)||d.schema_version<1||(known&&(!known.has(name)||d.schema_version!==1)))fail('schema 宣言が不正です');catalog.set(name,d.schema_version);}
    const ids=new Set();const events=arr(sim.model_records,'model_records').map(r=>{
      obj(r,'model record');const time=decimal(r.time_ps,'record time');if(time>end)fail('成功境界より後のrecord です');
      const name=str(r.schema_name,'record schema'),id=str(r.record_id,'record id'),key=JSON.stringify([name,id]);
      if(catalog.get(name)!==r.schema_version||ids.has(key))fail('未宣言schema 又は重複record です');ids.add(key);obj(r.data,'record data');sequence(r.data.effect_seq);str(r.subject,'record subject');
      const wire=r.data.wire??r.data;if(wire.tags!==undefined)tagStack(wire);
      return {time,kind:name,id,subject:r.subject,data:r.data,raw:r};
    });
    const points=arr(sim.records??[],'records');const seqs=new Set();
    for(const p of points){
      obj(p,'point');const time=decimal(p.time_ps,'point time');if(time>end)fail('成功境界より後のpoint です');
      const seq=decimal(p.seq,'point seq',U128).toString();if(seqs.has(seq))fail('point seq 重複');seqs.add(seq);
      if(p.descriptor_version!==undefined){const d=descriptors.get(p.metric);if(!d||d.version!==p.descriptor_version||!d.targets.includes(p.target)||p.unit!==d.unit)fail('point descriptor が一致しません');decimal(p.value,'point value');}
    }
    const summary=arr(sim.summary??[],'summary');
    for(const p of summary){obj(p,'summary row');if(descriptors.has(p.metric)){const d=descriptors.get(p.metric);if(!d.targets.includes(p.target)||p.aggregation!==d.aggregation)fail('summary descriptor が一致しません');decimal(p.sample_count,'sample_count',U128);if(p.value!==null){if(p.value_kind==='integer')decimal(p.value,'summary integer',U128);else if(typeof p.value!=='number'||!Number.isFinite(p.value))fail('summary number が不正です');}if(p.start_ps!==null&&p.start_ps!==undefined){const start=decimal(p.start_ps,'window start'),stop=decimal(p.end_ps,'window end');if(start>stop||stop>end)fail('window が不正です');}}}
    events.sort(order);
    const frames=new Map(events.filter(e=>e.kind==='ethernet.dynamic.frame').map(e=>[e.id,e]));
    for(const e of events)if(e.kind==='dir.preemption.delivery'){
      const frame=frames.get(e.data.frame_id);if(!frame)fail('配送機会のframe参照がありません');
      e.birth=decimal(frame.data.generated_ps,'frame generation');
    }
    const circuits=new Map(),distributedCircuits=new Map(),byId=new Map(events.map(e=>[JSON.stringify([e.kind,e.id]),e]));
    if(['ethernet.redundancy.abstract.v1','ethernet.frer.rtag.v1','ethernet.network.composite.v1'].includes(meta.model_profile)){
      const transfers=events.filter(e=>e.kind==='ethernet.dynamic.transfer');
      const explicit=meta.network_runtime?.redundancy?.transfer_circuits;
      if(explicit!==undefined){
        const assigned=new Set();
        for(const link of arr(explicit,'transfer circuits')){
          if(link.distributed!==undefined){
            const dc=obj(link.distributed,'distributed cursor'),leg=obj(dc.leg,'distributed leg'),origin=byId.get(JSON.stringify(['dir.redundancy.origin',dc.flow])),transfer=byId.get(JSON.stringify(['ethernet.dynamic.transfer',link.transfer_id]));
            if(!origin||origin.data.placement!=='bridge_pair_v1'||!transfer||transfer.data.frame_id!==origin.data.source_frame_id)fail('分散配置のflow・転送参照が不正です');
            const ports=leg.phase==='prefix'?origin.data.prefix_ports:['suffix','suffix_round'].includes(leg.phase)?origin.data.suffix_ports:leg.phase==='branch'?origin.data.branch_ports?.[leg.path]:null;
            const hop=decimal(link.hop_index,'distributed hop');if(!Array.isArray(ports)||hop>=BigInt(ports.length)||transfer.data.from_port!==ports[Number(hop)])fail('分散配置の物理経路が不正です');
            if(assigned.has(transfer.id))fail('転送が複数の経路に割り当てられています');assigned.add(transfer.id);
            if(leg.phase==='prefix'&&hop===0n){if(transfer.data.parent_transfer_id!==null)fail('prefixに親転送があります');}
            else {
              const parent=byId.get(JSON.stringify(['ethernet.dynamic.transfer',transfer.data.parent_transfer_id])),rx=parent&&byId.get(JSON.stringify(['ethernet.dynamic.reception',`${parent.id}@rx`]));
              if(!parent||!rx||rx.data.status!=='forwarded'||rx.data.ready_ps!==transfer.data.queued_ps||!arr(rx.data.egress_transfer_ids,'egress').includes(transfer.id))fail('分散配置に実受信の根拠がありません');
              if(hop===0n&&leg.phase==='branch'&&origin.data.prefix_reception_id!==rx.id||hop===0n&&leg.phase==='suffix'&&origin.data.winner_reception_id!==rx.id||hop===0n&&leg.phase==='suffix_round'&&origin.data.continuation_rounds?.[Number(leg.round)]?.input_reception_id!==rx.id)fail('分散配置のphase接続が不正です');
            }
            const key=JSON.stringify([dc.flow,leg.phase,leg.path??leg.round??null]),hops=distributedCircuits.get(key)??[];
            if(hops[Number(hop)])fail('分散配置の同じhopに複数の転送があります');hops[Number(hop)]=transfer;distributedCircuits.set(key,hops);if(leg.phase==='branch')circuits.set(link.branch_id,hops);
            continue;
          }
          const branch=byId.get(JSON.stringify(['dir.redundancy.branch',link.branch_id])),transfer=byId.get(JSON.stringify(['ethernet.dynamic.transfer',link.transfer_id]));
          if(!branch||!transfer)fail('枝・転送の参照がありません');
          if(assigned.has(transfer.id))fail('転送が複数の枝・hopに割り当てられています');assigned.add(transfer.id);
          const origin=byId.get(JSON.stringify(['dir.redundancy.origin',branch.data.origin_id]));if(!origin||transfer.data.frame_id!==origin.data.source_frame_id)fail('枝のframe参照が一致しません');
          const hop=decimal(link.hop_index,'hop index');if(hop>=BigInt(arr(branch.data.path_ports,'path ports').length))fail('hop index が経路外です');
          if(transfer.data.from_port!==branch.data.path_ports[Number(hop)])fail('枝の転送portが一致しません');
          const hops=circuits.get(branch.id)??[];if(hops[Number(hop)])fail('同じ枝・hop の転送が重複しています');hops[Number(hop)]=transfer;circuits.set(branch.id,hops);
        }
        for(const hops of circuits.values()){
          const distributed=[...distributedCircuits.values()].includes(hops);
          for(let i=0;i<hops.length;i++)if(!hops[i]||(i===0?!distributed&&hops[i].data.parent_transfer_id!==null:hops[i].data.parent_transfer_id!==hops[i-1].id))fail('枝の転送参照が連続していません');
        }
      }
      for(const branch of events.filter(e=>e.kind==='dir.redundancy.branch')){
        if(explicit!==undefined)continue;
        const origin=byId.get(JSON.stringify(['dir.redundancy.origin',branch.data.origin_id]));if(!origin)fail('枝のorigin参照がありません');
        const ports=arr(branch.data.path_ports,'path ports'),frame=origin.data.source_frame_id;
        let first=transfers.find(t=>t.data.frame_id===frame&&t.data.from_port===ports[0]&&branch.data.first_sof_ps!==null&&t.data.sof_ps===branch.data.first_sof_ps);
        if(!first&&typeof branch.data.current_transfer_id==='string'){
          first=byId.get(JSON.stringify(['ethernet.dynamic.transfer',branch.data.current_transfer_id]));
          const seen=new Set();while(first&&typeof first.data.parent_transfer_id==='string'){if(seen.has(first.id))fail('転送参照が循環しています');seen.add(first.id);first=byId.get(JSON.stringify(['ethernet.dynamic.transfer',first.data.parent_transfer_id]));}
        }
        const hops=[];for(let hop=0;first&&hop<ports.length;hop++){
          if(first.data.frame_id!==frame||first.data.from_port!==ports[hop])fail('枝の転送経路が一致しません');hops.push(first);
          first=transfers.find(t=>t.data.parent_transfer_id===first.id&&t.data.from_port===ports[hop+1]&&t.data.frame_id===frame);
        }
        circuits.set(branch.id,hops);
      }
    }
    return {raw,metadata:meta,simulation:sim,control,end,committed,descriptors,events,points,summary,circuits,distributedCircuits,extension:extensionProfiles.has(meta.model_profile)};
  }
  function birth(e){if(e.kind==='dir.mac_merge.packet')return decimal(e.data.sof_ps,'packet SOF');if(e.birth!==undefined)return e.birth;const d=e.data;if(e.kind==='dir.can_ethernet.terminal_indication')return decimal(d.received_ps,'terminal reception');if(e.kind==='ethernet.dynamic.reception')return decimal(d.observed_ps,'reception observation');for(const k of ['created_ps','queued_ps','generated_ps','start_ps'])if(typeof d[k]==='string')return decimal(d[k],k);return e.time;}
  const actualTimes=['ready_ps','admitted_ps','first_sof_ps','sof_ps','eof_ps','release_ps','arrival_ps','observed_ps','received_ps','completed_ps','end_ps','aborted_ps','invalidated_ps','reset_ps','arrival_retired_ps'];
  function project(e,time){
    if(birth(e)>time)return null;
    const data=JSON.parse(JSON.stringify(e.data)),prefix=e.time>time;
    for(const k of actualTimes)if(typeof data[k]==='string'&&decimal(data[k],k)>time)data[k]=null;
    if(data.sof_ps===null){for(const k of ['planned_eof_ps','planned_release_ps','planned_arrival_ps','policy_epoch_sof'])if(k in data)data[k]=null;}
    if(e.kind==='dir.mac_merge.packet'){const stop=typeof data.aborted_ps==='string'&&decimal(data.aborted_ps,'abort')<time?decimal(data.aborted_ps,'abort'):time;const elapsed=stop>decimal(data.sof_ps,'SOF')?stop-decimal(data.sof_ps,'SOF'):0n;const sent=elapsed*decimal(data.bitrate_bps,'bitrate')/8000000000000n;data.actual_tx_octets=String(sent<BigInt(data.wire_hex.length/2)?sent:BigInt(data.wire_hex.length/2));}
    if(e.kind==='dir.preemption.fragment'||e.kind==='dir.preemption.gap'){data.status=data.end_ps===null?(e.kind.endsWith('fragment')?'in_flight':'pending'):(data.status==='aborted'?'aborted':'completed');if(data.end_ps===null&&'emitted_payload_bytes'in data)delete data.emitted_payload_bytes;}
    else if(e.kind.endsWith('transfer')&&prefix&&data.reset_ps==null){data.status=data.sof_ps===null?'queued':'in_flight';data.drop_reason=null;}
    if(e.kind==='dir.preemption.transfer'&&'partial_mac_bytes'in data&&data.reset_ps==null)data.partial_mac_bytes='0';
    else if(e.kind==='ethernet.dynamic.reception'&&prefix){data.status='processing';data.reason=null;data.ready_ps=null;data.egress_transfer_ids=[];}
    else if(e.kind==='dir.redundancy.branch'&&prefix&&data.completed_ps===null){data.status=data.first_sof_ps===null?'waiting':'in_flight';data.reason=null;data.recovery_id=null;}
    if(Array.isArray(data.targets)){
      for(const t of data.targets)if(typeof t.completed_ps==='string'&&decimal(t.completed_ps,'target completion')>time){t.completed_ps=null;t.completion_reception_id=null;}
      data.completed_count=String(data.targets.filter(t=>t.completed_ps!==null).length);
      if(data.first_sof_ps===null){data.targets=[];data.target_count='0';data.source_transfer_id=null;}
    }
    if(e.kind==='dir.redundancy.origin'&&data.placement==='bridge_pair_v1'){
      if(typeof data.continuation_ps==='string'&&decimal(data.continuation_ps,'continuation')>time){data.continuation_created=false;data.winner_reception_id=null;data.suffix_root=null;data.continuation_ps=null;}
      if(data.completed_ps===null&&!Array.isArray(data.continuation_rounds)){data.completed_count='0';data.delivery_reception_id=null;data.dropped=false;}
      for(const phase of [data.prefix,...(data.branches??[]),data.suffix])if(phase){for(const k of ['first_sof_ps','terminal_ps'])if(phase[k]!==null&&BigInt(phase[k])>time)phase[k]=null;if(phase.terminal_ps===null)phase.failure_reason=null;}
      if(data.prefix?.terminal_ps===null){data.sequence=null;data.prefix_reception_id=null;data.branches=[];}
    }
    if(e.kind==='dir.redundancy.origin'&&Array.isArray(data.continuation_rounds)){data.continuation_rounds=data.continuation_rounds.filter(r=>decimal(r.created_ps,'round created')<=time);for(const r of data.continuation_rounds){for(const k of ['first_sof_ps','terminal_ps'])if(r.suffix[k]!==null&&decimal(r.suffix[k],k)>time)r.suffix[k]=null;if(r.suffix.terminal_ps===null){r.delivery_reception=null;r.delivery_reception_id=null;r.suffix.failure_reason=null;}}data.accepted_count=String(data.continuation_rounds.length);data.completed_count=String(data.continuation_rounds.filter(r=>r.delivery_reception_id!==null).length);data.delivery_reception_id=data.continuation_rounds.find(r=>r.delivery_reception_id!==null)?.delivery_reception_id??null;}
    if(e.kind==='dir.redundancy.origin'&&Array.isArray(data.delivery_indications)){data.delivery_indications=data.delivery_indications.filter(d=>decimal(d.delivered_ps,'delivered')<=time);data.completed_count=String(data.delivery_indications.length);data.delivered_branch_id=data.delivery_indications[0]?.branch_id??null;}
    if(e.kind==='dir.redundancy.origin'){if(data.completed_ps===null&&!Array.isArray(data.continuation_rounds)&&!Array.isArray(data.delivery_indications)){data.completed_count='0';data.delivered_branch_id=null;}if(data.first_sof_ps===null){data.target_count='0';data.target=null;}}
    return {...e,time:prefix?birth(e):e.time,data};
  }
  function at(model,time){
    if(typeof time!=='bigint'||time<0n||time>model.end)fail('再生時刻が不正です');const latest=new Map();for(const d of model.descriptors.values())if(d.sampling==='gauge')for(const target of d.targets)latest.set(JSON.stringify([d.name,target]),{metric:d.name,target,value:d.initial_gauges[target],unit:d.unit,time_ps:'0',initial:true});for(const p of model.points)if(decimal(p.time_ps,'time')<=time)latest.set(JSON.stringify([p.metric,p.target]),p);
    const events=model.extension?model.events.map(e=>project(e,time)).filter(Boolean):model.events.filter(e=>e.time<=time);
    for(const origin of events.filter(e=>e.kind==='dir.redundancy.origin'&&e.data.placement==='bridge_pair_v1')){
      const phases=[['prefix',null,origin.data.prefix],...(Array.isArray(origin.data.continuation_rounds)?origin.data.continuation_rounds.map((round,i)=>['suffix_round',i,round.suffix]):[['suffix',null,origin.data.suffix]]),...(origin.data.branches??[]).map((phase,path)=>['branch',path,phase])];
      for(const [leg,path,phase]of phases){if(!phase)continue;const hops=model.distributedCircuits.get(JSON.stringify([origin.id,leg,path]))??[],available=hops.filter(h=>h&&birth(h)<=time),current=available.at(-1);phase.current_transfer=current?/\/c(0|[1-9][0-9]*)$/.exec(current.id)?.[1]??null:null;}
    }
    for(const branch of events.filter(e=>e.kind==='dir.redundancy.branch')){
      if(branch.data.completed_ps!==null)continue;
      const hops=model.circuits.get(branch.id)??[],available=hops.filter(h=>birth(h)<=time),current=available.at(-1);
      branch.data.recovery_id=null;branch.data.reason=null;
      if(current){const hop=hops.indexOf(current);branch.data.hop_index=String(hop);branch.data.current_transfer_id=current.id;
        branch.data.status=typeof current.data.sof_ps==='string'&&decimal(current.data.sof_ps,'SOF')<=time?'in_flight':'queued';
      }else{branch.data.hop_index=null;branch.data.current_transfer_id=null;branch.data.status='created';}
    }
    for(const e of events.filter(e=>e.kind==='dir.preemption.transfer')){
      const d=e.data,parts=events.filter(f=>f.kind==='dir.preemption.fragment'&&f.data.transfer_id===d.transfer_id),gaps=events.filter(f=>f.kind==='dir.preemption.gap'&&f.data.transfer_id===d.transfer_id);let occupied=0n,completed=0n;
      if(Array.isArray(d.fragment_ids))d.fragment_ids=parts.map(f=>f.id);
      if(Array.isArray(d.gap_ids))d.gap_ids=gaps.map(f=>f.id);
      if(d.kind==='preemptable'&&!parts.some(f=>typeof f.data.offset_bytes==='string'&&decimal(f.data.offset_bytes,'fragment offset')+decimal(f.data.payload_bytes,'fragment payload')===decimal(d.mac_bytes,'MAC bytes')))
        for(const k of ['planned_eof_ps','planned_release_ps','planned_arrival_ps'])if(k in d)d[k]=null;
      for(const f of parts){const start=decimal(f.data.start_ps,'fragment start'),end=f.data.end_ps===null?time:decimal(f.data.end_ps,'fragment end');occupied+=end-start;if(f.data.end_ps!==null&&f.data.status==='completed')completed+=decimal(f.data.payload_bytes,'fragment payload');}
      if(d.kind==='express'&&d.sof_ps!==null){let end=d.release_ps===null?time:decimal(d.release_ps,'release');if(typeof d.reset_ps==='string')end=end<decimal(d.reset_ps,'reset')?end:decimal(d.reset_ps,'reset');occupied=end-decimal(d.sof_ps,'SOF');if(d.eof_ps!==null)completed=decimal(d.mac_bytes,'MAC bytes');}
      d.occupied_ps=occupied.toString();d.completed_mac_bytes=completed.toString();
    }
    events.sort(order);return {events,metrics:[...latest.values()]};
  }
  function times(model){const values=[0n,model.end,...model.points.map(p=>decimal(p.time_ps,'point time'))];for(const e of model.events){values.push(e.time);if(model.extension){values.push(birth(e));for(const k of actualTimes)if(typeof e.data[k]==='string')values.push(decimal(e.data[k],k));}}return [...new Set(values.filter(t=>t<=model.end))].sort((a,b)=>a<b?-1:a>b?1:0);}
  function gptpClocks(events){
    const clocks=new Map();
    for(const e of events){if(e.kind!=='dir.network.gptp')continue;const d=e.data,detail=d.details??{},name=detail.node;if(typeof name!=='string')continue;
      let clock=clocks.get(name);if(!clock){clock={node:name,status:'unsynchronized',grandmaster:null,parent:null,corrected_ps:null,sync_transfer_id:null};clocks.set(name,clock);}
      if(d.kind==='selection'){clock.grandmaster=detail.grandmaster??clock.grandmaster;clock.parent=detail.parent??null;clock.status=detail.synchronized?'grandmaster':clock.corrected_ps===null?'unsynchronized':'holdover';}
      else if(d.kind==='clock_corrected'){clock.status='synchronized';clock.grandmaster=detail.grandmaster??clock.grandmaster;clock.corrected_ps=d.global_ps??e.time.toString();clock.sync_transfer_id=detail.sync_transfer_id??null;}
      else if(d.kind==='holdover')clock.status='holdover';
    }
    return [...clocks.values()].sort((a,b)=>a.node.localeCompare(b.node));
  }
  function pausePorts(events,time){
    const ports=new Map();for(const e of events){if(e.kind!=='dir.network.mac_control'||e.time>time)continue;const d=e.data,s=d.state;if(!s)continue;const timer=s.timer,expires=timer?decimal(timer.expires_ps,'PAUSE expiry'):null;ports.set(d.port,{port:d.port,link_up:s.link_up,transmit_enabled:s.transmit_enabled,rx_enabled:s.rx_enabled,tx_enabled:s.tx_enabled,paused:expires!==null&&expires>time,remaining_ps:expires!==null&&expires>time?String(expires-time):'0',expires_ps:timer?.expires_ps??null,pending_quanta:s.pending?.quanta??null,received_ps:timer?.request?.received_ps??s.pending?.received_ps??null,loaded_ps:timer?.loaded_ps??null});}return [...ports.values()];
  }
  return {tagStack,parseResults,parseLegacy,at,decimal,times,gptpClocks,pausePorts,extensionProfiles};
});
