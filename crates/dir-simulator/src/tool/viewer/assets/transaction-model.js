/* Exact-time replay for AXI, shared buses, mesh packets and memory/IPC journals. */
(function(root,factory){'use strict';const api=factory();if(typeof module==='object'&&module.exports)module.exports=api;else root.DIRTransactionModel=api;})(typeof globalThis==='object'?globalThis:this,function(){
  'use strict';
  const profiles=new Set(['axi4.transaction.v1','soc.shared.v1','ahb.transaction.v1','noc.xy.v1','memory.ipc.transaction.v1']);
  const fields={
    'axi.transaction':'manager interconnect target operation address beats generated_ps eligible_ps status grant_ps completed_ps response read_data drop_reason',
    'axi.handshake':'channel beat valid_since_ps address data_hex wstrb last response',
    'axi.memory':'base size data_hex',
    'soc.transaction':'source target operation address bytes generated_ps status start_ps completed_ps response drop_reason active_plan',
    'soc.transfer':'hop from to start_ps end_ps bytes response',
    'memory-ipc.request':'model op actor status reason generated_ps started_ps planned_completion_ps completed_ps address length input_hex output_hex slot port dispatch_ordinal bank row row_hit message_id src dst src_address dst_address committed_bytes data_done_ps planned_notify_ps notified_ps',
    'memory-ipc.memory':'kind size hex open_rows ports refresh_pending refresh_started_ps refresh_planned_end_ps refresh_ended_ps queue',
    'memory-ipc.shared':'slots ready active queue',
    'memory-ipc.mailbox':'messages active queue',
    'memory-ipc.dma':'active queue child chunk_index chunk_hex',
    'memory-ipc.notification':'message_id receivers enqueued_ps planned_ps delivered_ps'
  };
  const fail=message=>{throw new Error(`Transaction: ${message}`);};
  const object=(value,label)=>{if(!value||typeof value!=='object'||Array.isArray(value))fail(`${label}: object required`);return value;};
  const array=(value,label)=>{if(!Array.isArray(value))fail(`${label}: array required`);return value;};
  const name=(value,label)=>{if(typeof value!=='string'||!value.length)fail(`${label}: string required`);return value;};
  const decimal=(value,label)=>{if(typeof value!=='string'||!/^(0|[1-9][0-9]*)$/.test(value))fail(`${label}: decimal string required`);const n=BigInt(value);if(n>((1n<<64n)-1n))fail(`${label}: overflow`);return n;};
  const keys=(value,set,label)=>{object(value,label);const names=set.split(' ');if(Object.keys(value).length!==names.length||names.some(k=>!Object.hasOwn(value,k)))fail(`${label}: invalid fields`);};
  const compare=(a,b)=>a<b?-1:a>b?1:0;
  function parseResults(raw){
    object(raw,'result');if(raw.schema_version!==2||!profiles.has(raw.metadata?.model_profile))fail('unsupported profile');
    const profile=raw.metadata.model_profile,sim=object(raw.simulation,'simulation'),start=decimal(sim.start_ps,'start'),end=decimal(sim.end_ps,'end');
    if(start!==0n||end<start||typeof sim.partial!=='boolean')fail('invalid observation interval');
    const prefix=profile==='axi4.transaction.v1'?'axi':profile==='memory.ipc.transaction.v1'?'memory-ipc':profile.split('.')[0];
    const allowed=prefix==='axi'?['axi.handshake','axi.memory','axi.transaction']:prefix==='memory-ipc'?Object.keys(fields).filter(k=>k.startsWith('memory-ipc.')):[`${prefix}.transaction`,`${prefix}.transfer`];
    const registered=array(raw.metadata.model_schemas,'model_schemas').map(s=>{keys(s,'schema_name schema_version','schema');if(s.schema_version!==1)fail('unsupported schema version');return s.schema_name;});
    if(registered.length!==allowed.length||new Set(registered).size!==allowed.length||allowed.some(s=>!registered.includes(s)))fail('schema registration mismatch');
    const rows=[],requests=new Map(),transfers=[],resources=[],ids=new Set(),times=new Set([start,end]);
    const actual=(value,label)=>{if(value===null)return null;const t=decimal(value,label);if(t>end||(!sim.partial&&t===end))fail(`${label}: actual time beyond observation`);times.add(t);return t;};
    for(const row of array(sim.model_records,'model_records')){
      keys(row,'schema_name schema_version record_id subject request_id origin_request_id time_ps data','record');
      if(!allowed.includes(row.schema_name)||row.schema_version!==1)fail('unsupported model record');
      name(row.record_id,'record ID');name(row.subject,'subject');
      for(const k of ['request_id','origin_request_id'])if(row[k]!==null)name(row[k],k);
      const id=`${row.schema_name}:${row.record_id}`;if(ids.has(id))fail('duplicate model record');ids.add(id);
      const updated=decimal(row.time_ps,'record time');if(updated>end||(!sim.partial&&updated===end&&updated!==0n))fail('record time beyond observation');times.add(updated);
      const schema=row.schema_name.replace(/^(ahb|noc)\./,'soc.');keys(row.data,fields[schema],'record data');
      const d=row.data;
      for(const [k,value]of Object.entries(d)){
        if(k.endsWith('_ps')&&value!==null){if(k.startsWith('planned_')||k==='eligible_ps')decimal(value,k);else actual(value,k);}
        if(['address','src_address','dst_address','length','bytes','beats','base','size','hop','beat','wstrb','dispatch_ordinal','committed_bytes','chunk_index'].includes(k)&&value!==null)decimal(value,k);
        if((k.endsWith('_hex')||k==='hex'||k==='data_hex')&&value!==null&&(typeof value!=='string'||!/^(?:[0-9a-f]{2})*$/.test(value)))fail('invalid byte hex');
        if(['slot','port','bank','row'].includes(k)&&value!==null&&(!Number.isSafeInteger(value)||value<0))fail('invalid native index');
      }
      const normalized={raw:row,id:row.record_id,subject:row.subject,updated,data:d};rows.push(normalized);
      if(row.schema_name.endsWith('.transaction')||row.schema_name==='memory-ipc.request'){
        if(row.request_id!==row.record_id||requests.has(row.record_id))fail('invalid request identity');
        const generated=actual(d.generated_ps,'generated'),began=actual(d.grant_ps??d.start_ps??d.started_ps??null,'start'),completed=actual(d.completed_ps,'completed');
        if(generated===null||(began!==null&&began<generated)||(completed!==null&&completed<(began??generated)))fail('invalid request time order');
        const states=prefix==='memory-ipc'?['awaiting_admission','queued','active','completed','rejected','setup','reading','writing','notifying','failed']:['pending','active','completed','dropped'];
        if(!states.includes(d.status))fail('invalid request status');
        if(['completed','rejected','failed'].includes(d.status)!==(completed!==null))fail('request completion mismatch');
        if(prefix!=='memory-ipc'&&d.status==='active'&&began===null)fail('active request without start');
        if(d.active_plan!==undefined&&d.active_plan!==null){keys(d.active_plan,'resource hop start_ps planned_end_ps','active plan');decimal(d.active_plan.hop,'active hop');const a=actual(d.active_plan.start_ps,'active start'),b=decimal(d.active_plan.planned_end_ps,'planned end');if(b<=a)fail('invalid active plan');}
        requests.set(row.record_id,{...normalized,generated,start:began,completed,state:d.status,source:d.manager??d.source??d.actor??row.subject,target:d.target??row.subject});
      }else if(row.schema_name.endsWith('.transfer')){
        const a=actual(d.start_ps,'transfer start'),b=actual(d.end_ps,'transfer end');if(a===null||b===null||b<=a||updated!==b)fail('invalid transfer interval');
        transfers.push({...normalized,request:row.request_id,from:d.from,to:d.to,start:a,end:b});
      }else if(row.schema_name==='axi.handshake'){
        if(!['AW','W','B','AR','R'].includes(d.channel)||d.valid_since_ps===null||decimal(d.valid_since_ps,'VALID')>updated)fail('invalid handshake');
        transfers.push({...normalized,request:row.request_id,from:null,to:null,start:decimal(d.valid_since_ps,'VALID'),end:updated,handshake:true});
      }else resources.push(normalized);
    }
    for(const t of transfers){const r=requests.get(t.request);if(!r)fail('unknown transfer request');if(t.start<r.generated)fail('transfer precedes generation');if(t.handshake){const reverse=['B','R'].includes(t.data.channel);t.from=reverse?r.target:r.source;t.to=reverse?r.source:r.target;}}
    for(const r of requests.values())if(r.raw.origin_request_id!==null&&!requests.has(r.raw.origin_request_id))fail('unknown DMA parent');
    for(const point of sim.records||[])if(point.time_ps!==null&&point.time_ps!==undefined){const t=decimal(point.time_ps,'point time');if(t>end)fail('point beyond observation');times.add(t);}
    return {raw,profile,start,end,requests,transfers,resources,rows,eventTimes:[...times].sort(compare)};
  }
  function requestStateAt(request,t){if(t<request.generated)return 'not_generated';if(request.completed!==null&&t>=request.completed)return request.state;if(request.state==='dropped'&&t>=request.updated)return 'dropped';if(request.start!==null&&t>=request.start)return request.updated<=t?request.state==='completed'?'active':request.state:'active';return 'pending';}
  function stateAt(model,t){if(typeof t!=='bigint'||t<model.start||t>model.end)fail('cursor outside observation');const counts={generated:0,pending:0,active:0,completed:0,dropped:0};for(const r of model.requests.values()){const s=requestStateAt(r,t);if(s==='not_generated')continue;counts.generated++;if(['completed'].includes(s))counts.completed++;else if(['rejected','failed','dropped'].includes(s))counts.dropped++;else if(s==='pending'||s==='queued'||s==='awaiting_admission')counts.pending++;else counts.active++;}return counts;}
  function stepTransfers(model,t){let prior=model.start;for(const time of model.eventTimes){if(time>=t)break;prior=time;}return model.transfers.filter(x=>x.start<t&&x.end>prior||x.handshake&&x.end===t).map(x=>({id:x.id,request:x.request,from:x.from,to:x.to,start:x.start,end:x.end}));}
  function timelineSegments(request,end){
    const terminal=['dropped','rejected','failed'].includes(request.state);
    const finished=request.completed??(request.state==='dropped'?request.updated:null);
    const segments=[];
    if(request.start===null){
      const until=finished??end;
      if(until>request.generated||!terminal)segments.push({start:request.generated,end:until,state:'pending',dashed:false});
    }else{
      if(request.start>request.generated)segments.push({start:request.generated,end:request.start,state:'pending',dashed:false});
      segments.push({start:request.start,end:finished??end,state:'active',dashed:finished===null});
    }
    if(terminal)segments.push({start:finished,end:finished,state:request.state,dashed:false});
    return segments;
  }
  return {profiles,parseResults,requestStateAt,stateAt,stepTransfers,timelineSegments};
});
