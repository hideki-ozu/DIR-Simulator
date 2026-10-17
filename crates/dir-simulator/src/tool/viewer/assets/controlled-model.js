/* Exact prefix validation for schema-3 registered and cooperative results. */
(function(root,factory){'use strict';const api=factory();if(typeof module==='object'&&module.exports)module.exports=api;else root.DIRControlledModel=api;})(typeof globalThis==='object'?globalThis:this,function(){
  'use strict';
  const U64=(1n<<64n)-1n,U128=(1n<<128n)-1n;
  const extensionProfiles=new Set(['can.ethernet.tsn.v1','ethernet.redundancy.abstract.v1','ethernet.preemption.abstract.v1','ethernet.control.wire.experimental.v1']);
  function knownSchemas(profile){
    const schemas=new Set(['frame','transfer','reception','control','policy'].map(k=>`ethernet.dynamic.${k}`));
    if(profile!=='ethernet.control.wire.experimental.v1')for(const k of ['gate','credit','policing','decision'])schemas.add(`ethernet.tsn.${k}`);
    if(profile==='can.ethernet.tsn.v1')for(const k of ['can.request','can.receiver','dir.can_ethernet.conversion','dir.can_ethernet.branch','dir.can_ethernet.segment','dir.composite.ingress'])schemas.add(k);
    if(profile==='ethernet.redundancy.abstract.v1')for(const k of ['branch','recovery','origin'])schemas.add(`dir.redundancy.${k}`);
    if(profile==='ethernet.preemption.abstract.v1'){schemas.delete('ethernet.dynamic.transfer');for(const k of ['transfer','fragment','gap','delivery','decision'])schemas.add(`dir.preemption.${k}`);}
    if(profile==='ethernet.control.wire.experimental.v1')for(const k of ['ethernet.wire.boundary','ethernet.wire.membership','ethernet.wire.membership_timer','ethernet.control.wire.event','ethernet.control.wire.policy','ethernet.control.wire.transition'])schemas.add(k);
    return schemas;
  }
  const fail=text=>{throw new Error(`実行制御・指標: ${text}`);};
  const obj=(v,label)=>v&&typeof v==='object'&&!Array.isArray(v)?v:fail(`${label} が不正です`);
  const arr=(v,label)=>Array.isArray(v)?v:fail(`${label} は配列が必要です`);
  const str=(v,label)=>typeof v==='string'&&v.length?v:fail(`${label} がありません`);
  function decimal(v,label,max=U64){if(typeof v!=='string'||!/^(0|[1-9][0-9]*)$/.test(v)||BigInt(v)>max)fail(`${label} は範囲内の十進整数が必要です`);return BigInt(v);}
  function order(a,b){if(a.time!==b.time)return a.time<b.time?-1:1;const x=a.data.effect_seq,y=b.data.effect_seq;if(typeof x==='string'&&typeof y==='string'){const xx=decimal(x,'effect seq'),yy=decimal(y,'effect seq');if(xx!==yy)return xx<yy?-1:1;}return a.id<b.id?-1:a.id>b.id?1:0;}
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
      if(catalog.get(name)!==r.schema_version||ids.has(key))fail('未宣言schema 又は重複record です');ids.add(key);obj(r.data,'record data');str(r.subject,'record subject');
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
    const circuits=new Map(),byId=new Map(events.map(e=>[JSON.stringify([e.kind,e.id]),e]));
    if(meta.model_profile==='ethernet.redundancy.abstract.v1'){
      const transfers=events.filter(e=>e.kind==='ethernet.dynamic.transfer');
      const explicit=meta.network_runtime?.redundancy?.transfer_circuits;
      if(explicit!==undefined){
        const assigned=new Set();
        for(const link of arr(explicit,'transfer circuits')){
          const branch=byId.get(JSON.stringify(['dir.redundancy.branch',link.branch_id])),transfer=byId.get(JSON.stringify(['ethernet.dynamic.transfer',link.transfer_id]));
          if(!branch||!transfer)fail('枝・転送の参照がありません');
          if(assigned.has(transfer.id))fail('転送が複数の枝・hopに割り当てられています');assigned.add(transfer.id);
          const origin=byId.get(JSON.stringify(['dir.redundancy.origin',branch.data.origin_id]));if(!origin||transfer.data.frame_id!==origin.data.source_frame_id)fail('枝のframe参照が一致しません');
          const hop=decimal(link.hop_index,'hop index');if(hop>=BigInt(arr(branch.data.path_ports,'path ports').length))fail('hop index が経路外です');
          if(transfer.data.from_port!==branch.data.path_ports[Number(hop)])fail('枝の転送portが一致しません');
          const hops=circuits.get(branch.id)??[];if(hops[Number(hop)])fail('同じ枝・hop の転送が重複しています');hops[Number(hop)]=transfer;circuits.set(branch.id,hops);
        }
        for(const hops of circuits.values())for(let i=0;i<hops.length;i++){
          if(!hops[i]||(i===0?hops[i].data.parent_transfer_id!==null:hops[i].data.parent_transfer_id!==hops[i-1].id))fail('枝の転送参照が連続していません');
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
    return {raw,metadata:meta,simulation:sim,control,end,committed,descriptors,events,points,summary,circuits,extension:extensionProfiles.has(meta.model_profile)};
  }
  function birth(e){if(e.birth!==undefined)return e.birth;const d=e.data;if(e.kind==='ethernet.dynamic.reception')return decimal(d.observed_ps,'reception observation');for(const k of ['created_ps','queued_ps','generated_ps','start_ps'])if(typeof d[k]==='string')return decimal(d[k],k);return e.time;}
  const actualTimes=['ready_ps','admitted_ps','first_sof_ps','sof_ps','eof_ps','release_ps','arrival_ps','observed_ps','received_ps','completed_ps','end_ps'];
  function project(e,time){
    if(birth(e)>time)return null;
    const data=JSON.parse(JSON.stringify(e.data)),prefix=e.time>time;
    for(const k of actualTimes)if(typeof data[k]==='string'&&decimal(data[k],k)>time)data[k]=null;
    if(data.sof_ps===null){for(const k of ['planned_eof_ps','planned_release_ps','planned_arrival_ps','policy_epoch_sof'])if(k in data)data[k]=null;}
    if(e.kind==='dir.preemption.fragment'||e.kind==='dir.preemption.gap')data.status=data.end_ps===null?(e.kind.endsWith('fragment')?'in_flight':'pending'):'completed';
    else if(e.kind.endsWith('transfer')&&prefix){data.status=data.sof_ps===null?'queued':'in_flight';data.drop_reason=null;}
    else if(e.kind==='ethernet.dynamic.reception'&&prefix){data.status='processing';data.reason=null;data.ready_ps=null;data.egress_transfer_ids=[];}
    else if(e.kind==='dir.redundancy.branch'&&prefix){data.status=data.first_sof_ps===null?'waiting':'in_flight';data.reason=null;data.recovery_id=null;}
    if(Array.isArray(data.targets)){
      for(const t of data.targets)if(typeof t.completed_ps==='string'&&decimal(t.completed_ps,'target completion')>time){t.completed_ps=null;t.completion_reception_id=null;}
      data.completed_count=String(data.targets.filter(t=>t.completed_ps!==null).length);
      if(data.first_sof_ps===null){data.targets=[];data.target_count='0';data.source_transfer_id=null;}
    }
    if(e.kind==='dir.redundancy.origin'){if(data.completed_ps===null){data.completed_count='0';data.delivered_branch_id=null;}if(data.first_sof_ps===null){data.target_count='0';data.target=null;}}
    return {...e,time:prefix?birth(e):e.time,data};
  }
  function at(model,time){
    if(typeof time!=='bigint'||time<0n||time>model.end)fail('再生時刻が不正です');const latest=new Map();for(const d of model.descriptors.values())if(d.sampling==='gauge')for(const target of d.targets)latest.set(JSON.stringify([d.name,target]),{metric:d.name,target,value:d.initial_gauges[target],unit:d.unit,time_ps:'0',initial:true});for(const p of model.points)if(decimal(p.time_ps,'time')<=time)latest.set(JSON.stringify([p.metric,p.target]),p);
    const events=model.extension?model.events.map(e=>project(e,time)).filter(Boolean):model.events.filter(e=>e.time<=time);
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
      for(const f of parts){const start=decimal(f.data.start_ps,'fragment start'),end=f.data.end_ps===null?time:decimal(f.data.end_ps,'fragment end');occupied+=end-start;if(f.data.end_ps!==null)completed+=decimal(f.data.payload_bytes,'fragment payload');}
      if(d.kind==='express'&&d.sof_ps!==null){const end=d.release_ps===null?time:decimal(d.release_ps,'release');occupied=end-decimal(d.sof_ps,'SOF');if(d.eof_ps!==null)completed=decimal(d.mac_bytes,'MAC bytes');}
      d.occupied_ps=occupied.toString();d.completed_mac_bytes=completed.toString();
    }
    events.sort(order);return {events,metrics:[...latest.values()]};
  }
  function times(model){const values=[0n,model.end,...model.points.map(p=>decimal(p.time_ps,'point time'))];for(const e of model.events){values.push(e.time);if(model.extension){values.push(birth(e));for(const k of actualTimes)if(typeof e.data[k]==='string')values.push(decimal(e.data[k],k));}}return [...new Set(values.filter(t=>t<=model.end))].sort((a,b)=>a<b?-1:a>b?1:0);}
  return {parseResults,at,decimal,times,extensionProfiles};
});
