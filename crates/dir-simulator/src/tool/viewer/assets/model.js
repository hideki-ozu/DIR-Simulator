/* Pure, exact-time replay of CAN recorded milestones in schema 1 and 2. Works in browsers and Node. */
(function (root, factory) {
  'use strict';
  const api = factory();
  if (typeof module === 'object' && module.exports) module.exports = api;
  else root.DIRViewerModel = api;
})(typeof globalThis === 'object' ? globalThis : this, function () {
  'use strict';
  const MAX_TIME = (1n << 64n) - 1n;
  const UNITS = { ps: 1n, ns: 1000n, us: 1000000n, ms: 1000000000n, s: 1000000000000n };
  const REQUEST_STATES = new Set(['processing', 'waiting_tx', 'pending', 'in_flight', 'success', 'dropped']);
  const RECEIVER_STATES = new Set(['pending', 'received', 'filtered']);
  const fail = message => { throw new Error(message); };
  const object = (value, field) => {
    if (!value || typeof value !== 'object' || Array.isArray(value)) fail(`${field}: オブジェクトが必要です。`);
    return value;
  };
  const array = (value, field) => {
    if (!Array.isArray(value)) fail(`${field}: 配列が必要です。`);
    return value;
  };
  const name = (value, field) => {
    if (typeof value !== 'string' || value.length === 0) fail(`${field}: 空でない文字列が必要です。`);
    return value;
  };
  function decimal(value, field, maximum = MAX_TIME) {
    if (typeof value !== 'string' || !/^(0|[1-9][0-9]*)$/.test(value)) fail(`${field}: 整数の十進文字列が必要です。`);
    const n = BigInt(value);
    if (n > maximum) fail(`${field}: 値が範囲を超えています。`);
    return n;
  }
  const compareTime = (a, b) => a < b ? -1 : a > b ? 1 : 0;
  const unitValue = unit => UNITS[unit] || fail(`未対応の時間単位: ${unit}`);
  function formatTime(time, unit = 'us') {
    const scale = unitValue(unit);
    const sign = time < 0n ? '-' : '';
    const magnitude = time < 0n ? -time : time;
    const integer = magnitude / scale;
    const remainder = magnitude % scale;
    const digits = scale.toString().length - 1;
    return sign + integer.toString() + (remainder ? '.' + remainder.toString().padStart(digits, '0').replace(/0+$/, '') : '');
  }
  function parseTime(text, unit = 'us') {
    if (typeof text !== 'string' || !/^(0|[1-9][0-9]*)(\.[0-9]+)?$/.test(text.trim())) fail('時刻は0以上の十進数で入力してください。');
    const [integer, fraction = ''] = text.trim().split('.');
    const digits = unitValue(unit).toString().length - 1;
    if (fraction.slice(digits).replace(/0/g, '').length) fail('時刻は1 ps単位で指定してください。');
    const time = BigInt(integer) * unitValue(unit) + BigInt(fraction.slice(0, digits).padEnd(digits, '0') || '0');
    if (time > MAX_TIME) fail('時刻がu64の範囲を超えています。');
    return time;
  }
  function fraction(time, start, end) {
    if (end <= start) return 0;
    const clamped = time < start ? start : time > end ? end : time;
    return Number((clamped - start) * 1000000000n / (end - start)) / 1000000000;
  }
  function timeFromFraction(start, end, numerator, denominator = 10000) {
    if (!Number.isFinite(numerator) || !Number.isSafeInteger(denominator) || denominator <= 0) fail('不正な時刻範囲です。');
    const n = BigInt(Math.max(0, Math.min(denominator, Math.round(numerator))));
    return start + (end - start) * n / BigInt(denominator);
  }
  // Validate FD records, then reuse the common recorded-milestone replay.
  // This projection supplies only replay timing; no Classical CAN wire codec is used.
  function parseCanFd(raw) {
    const sim=object(raw.simulation,'simulation'),frames=new Map(),requests=new Map(),receivers=[];
    const keys=(x,k,label)=>{object(x,label);if(Object.keys(x).length!==k.length||k.some(n=>!Object.hasOwn(x,n)))fail(`${label}: キー集合が不正です。`);};
    const end=decimal(sim.end_ps,'end_ps');
    const actual=x=>{if(x===null)return null;const n=decimal(x,'FD time');if(n>end||(!sim.partial&&n===end))fail('FD実績時刻が観測期間外です。');return n;};
    const opt=x=>x===null?null:decimal(x,'FD planned time');
    const unique=new Set(),requestRows=[],receiverRows=[];
    const frameKeys=['format','id','data','dlc','brs','nominal_bits','data_bits','evidence','binding_sha256','nominal_rate','data_rate','fidelity','wire_validation'];
    const requestKeys=['frame_id','source','bus','generated_ps','ready_ps','sof_ps','eof_ps','release_ps','planned_ready_ps','planned_eof_ps','planned_release_ps','state','drop_reason'];
    const receptionKeys=['frame_id','receiver','planned_arrival_ps','planned_completed_ps','arrival_ps','completed_ps','state'];
    for(const row of array(sim.model_records,'FD model_records')) {
      keys(row,['schema_name','schema_version','record_id','subject','request_id','origin_request_id','time_ps','data'],'FD record');
      const key=JSON.stringify([row.schema_name,name(row.record_id,'record_id')]);if(unique.has(key)||row.schema_version!==1||row.origin_request_id!==null)fail('FDレコードの版・一意性が不正です。');unique.add(key);
      name(row.subject,'subject');const updated=decimal(row.time_ps,'FD updated');const d=row.data;
      if(row.schema_name==='dir.canfd.frame') {
        keys(d,frameKeys,'FD frame');
        if(row.request_id!==null||updated!==0n||!['standard','extended'].includes(d.format)||!Number.isInteger(d.id)||d.id<0||d.id>(d.format==='standard'?2047:536870911)||typeof d.data!=='string'||!/^(?:[0-9a-f]{2})*$/.test(d.data)||typeof d.brs!=='boolean')fail('FD frameの型が不正です。');
        const lengths=[0,1,2,3,4,5,6,7,8,12,16,20,24,32,48,64],bytes=d.data.length/2;
        if(lengths.indexOf(bytes)!==d.dlc||!name(d.evidence,'evidence')||Array.from(d.evidence).length>512||! /^[0-9a-f]{64}$/.test(d.binding_sha256)||d.fidelity!=='externally-precomputed-phase-bits'||d.wire_validation!=='structural-only')fail('FD DLC・証跡が不正です。');
        const n=decimal(d.nominal_bits,'N',1000000n),b=decimal(d.data_bits,'D',1000000n),rn=decimal(d.nominal_rate,'Rn',1000000n),rd=decimal(d.data_rate,'Rd',8000000n);
        if(n<1n||rn<1n||rd<rn||(d.brs?(b<1n||b<BigInt(bytes*8)):(b!==0n||n<BigInt(bytes*8))))fail('FD bit数・速度が不正です。');
        frames.set(row.record_id,{...d,source:row.subject,n,b,rn,rd});
      } else if(row.schema_name==='dir.canfd.request') {keys(d,requestKeys,'FD request');requestRows.push(row);}
      else if(row.schema_name==='dir.canfd.reception') {keys(d,receptionKeys,'FD reception');receiverRows.push(row);}
      else fail('未対応のFD schemaです。');
    }
    const legacyRequests=[];
    const states={pending:'processing',queued:'pending',transmitting:'in_flight',serialized:'success',dropped:'dropped'};
    for(const row of requestRows) {
      const d=row.data,f=frames.get(d.frame_id);name(row.request_id,'request_id');
      if(!f||row.record_id!==row.request_id||row.subject!==d.source||f.source!==d.source||!Object.hasOwn(states,d.state)||! /^.+:(0|[1-9][0-9]*)$/.test(row.record_id)||!row.record_id.startsWith(d.frame_id+':'))fail('FD request参照が不正です。');
      const generated=actual(d.generated_ps),ready=actual(d.ready_ps),sof=actual(d.sof_ps),eof=actual(d.eof_ps),release=actual(d.release_ps),plannedReady=decimal(d.planned_ready_ps,'planned ready'),plannedEof=opt(d.planned_eof_ps),plannedRelease=opt(d.planned_release_ps);
      if(generated===null||plannedReady<generated||ready!==null&&ready!==plannedReady||BigInt(row.time_ps)!==(release??eof??sof??ready??generated))fail('FD確定時刻が不正です。');
      if(d.state==='dropped'?d.drop_reason!=='queue_full':d.drop_reason!==null)fail('FD drop理由が不正です。');
      if(sof!==null){const ceil=(a,b)=>a/b+(a%b?1n:0n),den=f.rn*f.rd;if(plannedEof!==sof+ceil(1000000000000n*(f.n*f.rd+f.b*f.rn),den)||plannedRelease!==sof+ceil(1000000000000n*((f.n+3n)*f.rd+f.b*f.rn),den))fail('FD時間と位相bit数が不一致です。');}
      requests.set(row.record_id,{row,d,f});
      legacyRequests.push({request_id:row.record_id,source:d.source,bus:d.bus,status:states[d.state],generated_ps:d.generated_ps,ready_ps:d.ready_ps,sof_ps:d.sof_ps,eof_ps:d.eof_ps,model_fields:{profile:'can.cc.ideal.v1',schema_version:1,planned_eof_ps:d.planned_eof_ps,planned_release_ps:d.planned_release_ps,release_ps:d.release_ps}});
    }
    for(const row of receiverRows) {
      const d=row.data,q=requests.get(row.request_id),arrival=actual(d.arrival_ps),completed=actual(d.completed_ps),pa=decimal(d.planned_arrival_ps,'planned arrival'),pc=decimal(d.planned_completed_ps,'planned completed');
      if(!q||q.d.eof_ps===null||d.frame_id!==q.d.frame_id||d.receiver===q.d.source||row.subject!==d.receiver||row.record_id!==`${row.request_id}:${d.receiver}`||pa<BigInt(q.d.eof_ps)||pc<pa||arrival!==null&&arrival!==pa||completed!==null&&completed!==pc||BigInt(row.time_ps)!==(completed??arrival??BigInt(q.d.eof_ps)))fail('FD reception参照・時刻が不正です。');
      if(!['pending','processing','completed','filtered'].includes(d.state)||(d.state==='pending'?(arrival!==null||completed!==null):d.state==='completed'?(arrival===null||completed===null):(arrival===null||completed!==null)))fail('FD受信状態が不正です。');
      receivers.push(row);
    }
    const projected={...raw,schema_version:1,metadata:{...raw.metadata,models:[{type:'can.cc.ideal.v1'}],initial_state:[]},simulation:{...sim,requests:legacyRequests,receivers:receivers.map(row=>({request_id:row.request_id,receiver:row.data.receiver,status:row.data.state==='completed'?'received':row.data.state==='filtered'?'filtered':'pending',observed_ps:row.data.arrival_ps,received_ps:row.data.completed_ps}))}};
    const replay=parseResults(projected);replay.raw=raw;replay.canfd=true;replay.fdFrames=frames;
    for(const r of replay.requests){r.raw=requests.get(r.id).d;r.fdFrame=requests.get(r.id).f;}
    return replay;
  }
  function parseResults(raw) {
    object(raw, 'results');
    if(raw.schema_version===2&&raw.metadata?.model_profile==='can.fd.precomputed.v1')return parseCanFd(raw);
    if (![1, 2].includes(raw.schema_version)) fail('対応しているresults.jsonはschema_version = 1 / 2です。');
    const profile = raw.schema_version === 2 ? 'can.cc.multibus.v1' : 'can.cc.ideal.v1';
    if (raw.schema_version === 2 && raw.metadata?.model_profile !== profile) fail('schema 2はCAN複数バスの結果に対応しています。');
    const sim = object(raw.simulation, 'simulation');
    if (!['events_exhausted', 'time_limit', 'execution_failed', 'prep_failed'].includes(sim.termination)) fail('不明な終了理由です。');
    if (typeof sim.partial !== 'boolean') fail('simulation.partial: booleanが必要です。');
    const start = decimal(sim.start_ps, 'start_ps');
    const end = decimal(sim.end_ps, 'end_ps');
    if (start !== 0n || end < start) fail('観測期間が不正です。');
    const nodes = new Set(), buses = new Set(), ids = new Map(), queueRecords = new Map();
    const events = [];
    const reached = (value, field) => {
      if (value === null) return null;
      const time = decimal(value, field);
      if (time < start || time > end || (!sim.partial && time === end)) fail(`${field}: 到達時刻が観測期間外です。`);
      return time;
    };
    const event = (time, kind, requestId, node, label) => {
      if (time !== null) events.push({ time, kind, requestId, node, label, order: events.length });
    };
    let requestRows = sim.requests, receiverRows = sim.receivers;
    const forwardRows = [], rxBufferRows = [], envelopeKeys = new Set(), envelopes = new Map();
    function fields(value, expected, label) {
      object(value,label);
      if (Object.keys(value).length !== expected.length || expected.some(key => !Object.hasOwn(value,key))) fail(`${label}: キー集合が不正です。`);
    }
    if (raw.schema_version === 2) {
      if ('requests' in sim || 'receivers' in sim) fail('schema 2はmodel_recordsを使用します。');
      requestRows = []; receiverRows = [];
      for (const row of array(sim.model_records, 'model_records')) {
        fields(row,['schema_name','schema_version','record_id','subject','request_id','origin_request_id','time_ps','data'],'model record');
        object(row.data, 'model record data');
        if (!['can.request', 'can.receiver', 'gw.forward', 'gw.rx_buffer'].includes(row.schema_name) || row.schema_version !== 1) fail('未対応のモデルレコードです。');
        const key = JSON.stringify([row.schema_name,name(row.record_id,'record_id')]);
        if (envelopeKeys.has(key)) fail('モデルレコードが重複しています。');
        envelopeKeys.add(key);
        if (reached(row.time_ps,'model record time') === null) fail('モデルレコードの確定時刻が必要です。');
        envelopes.set(row.data,row);
        if (row.schema_name === 'can.request') {
          if (row.record_id !== row.data.request_id || row.subject !== row.data.source || row.request_id !== row.data.request_id || row.origin_request_id !== row.data.model_fields?.origin_request_id) fail('要求レコードの包絡が不正です。');
          fields(row.data,['request_id','source','bus','status','generated_ps','ready_ps','sof_ps','eof_ps','payload_bits','serialized_bits','model_fields','attempts','retries','drop_reason'],'CAN request data');
          fields(row.data.model_fields,['profile','schema_version','crc15','stuff_bits','frame_bits','intermission_bits','bitrate_bps','planned_eof_ps','planned_release_ps','release_ps','origin_request_id','parent_request_id','gw_hops',...(Object.hasOwn(row.data.model_fields,'tx_enqueued_ps') ? ['tx_enqueued_ps'] : [])],'CAN model fields');
          requestRows.push(row.data);
        } else if (row.schema_name === 'can.receiver') {
          if (row.record_id !== `${row.data.request_id}/${row.data.receiver}` || row.subject !== row.data.receiver || row.request_id !== row.data.request_id) fail('受信レコードの包絡が不正です。');
          fields(row.data,['request_id','receiver','status','observed_ps','received_ps'],'CAN receiver data');
          receiverRows.push(row.data);
        } else if (row.schema_name === 'gw.rx_buffer') {
          fields(row.data,['buffer_id','parent_request_id','origin_request_id','gateway','ingress','capacity','received_ps','released_ps','status','reason','egress'],'Gateway RX data');
          if (row.record_id !== row.data.buffer_id || row.subject !== row.data.ingress || row.request_id !== row.data.parent_request_id || row.origin_request_id !== row.data.origin_request_id) fail('Gateway RXレコードの包絡が不正です。');
          rxBufferRows.push(row.data);
        } else {
          if (row.record_id !== row.data.forward_id || row.subject !== row.data.gateway || row.request_id !== row.data.parent_request_id || row.origin_request_id !== row.data.origin_request_id) fail('Gatewayレコードの包絡が不正です。');
          fields(row.data,['forward_id','parent_request_id','origin_request_id','gateway','ingress','egress','route_id','gw_hops','received_ps','planned_forward_ps','forwarded_ps','child_request_id','status','reason'],'Gateway data');
          forwardRows.push(row.data);
        }
      }
    }
    const requests = array(requestRows, 'requests').map((rawRequest, index) => {
      object(rawRequest, `requests[${index}]`);
      const r = {
        id: name(rawRequest.request_id, 'request_id'), source: name(rawRequest.source, 'source'),
        bus: name(rawRequest.bus, 'bus'), status: rawRequest.status, raw: rawRequest, receivers: [],
        generated: reached(rawRequest.generated_ps, 'generated_ps'), ready: reached(rawRequest.ready_ps, 'ready_ps'),
        sof: reached(rawRequest.sof_ps, 'sof_ps'), eof: reached(rawRequest.eof_ps, 'eof_ps'),
      };
      if (ids.has(r.id)) fail(`要求IDが重複しています: ${r.id}`);
      if (!REQUEST_STATES.has(r.status) || r.generated === null) fail(`${r.id}: 要求の状態が不正です。`);
      const fields = object(rawRequest.model_fields, `${r.id}.model_fields`);
      if (fields.profile !== profile || fields.schema_version !== 1) fail('CANのprofileまたは版が不正です。');
      r.origin = raw.schema_version === 2 ? name(fields.origin_request_id,'origin_request_id') : r.id;
      r.parent = raw.schema_version === 2 ? fields.parent_request_id : null;
      r.hops = raw.schema_version === 2 ? decimal(fields.gw_hops,'gw_hops',65536n) : 0n;
      r.hasTxEnqueued = Object.hasOwn(fields,'tx_enqueued_ps');
      r.txEnqueued = r.hasTxEnqueued ? reached(fields.tx_enqueued_ps,'tx_enqueued_ps') : (r.status === 'dropped' ? null : r.ready);
      if (r.hasTxEnqueued && ((r.txEnqueued !== null && (r.ready === null || r.txEnqueued < r.ready || (r.sof !== null && r.txEnqueued > r.sof))) || (['pending','in_flight','success'].includes(r.status) && r.txEnqueued === null) || (['processing','waiting_tx','dropped'].includes(r.status) && r.txEnqueued !== null) || (r.parent === null && r.txEnqueued !== null && r.txEnqueued !== r.ready))) fail(`${r.id}: TXキュー投入時刻が不正です。`);
      if (r.status === 'waiting_tx' && (!r.hasTxEnqueued || r.parent === null)) fail(`${r.id}: TX容量待機の記録が不正です。`);
      r.release = reached(fields.release_ps, 'release_ps');
      r.plannedEof = fields.planned_eof_ps === null ? null : decimal(fields.planned_eof_ps, 'planned_eof_ps');
      r.plannedRelease = fields.planned_release_ps === null ? null : decimal(fields.planned_release_ps, 'planned_release_ps');
      const stages = [r.generated, r.ready, r.sof, r.eof, r.release];
      for (let i = 1; i < stages.length; i++) {
        if (stages[i] !== null && (stages[i - 1] === null || stages[i] < stages[i - 1])) fail(`${r.id}: 到達時刻の順序が不正です。`);
      }
      const shape = [r.ready !== null, r.sof !== null, r.eof !== null].join(',');
      const shapes = { processing: 'false,false,false', pending: 'true,false,false', waiting_tx: 'true,false,false', in_flight: 'true,true,false', success: 'true,true,true', dropped: 'true,false,false' };
      if (shape !== shapes[r.status]) fail(`${r.id}: 要求状態と到達時刻が一致しません。`);
      if (r.sof === null ? r.plannedEof !== null || r.plannedRelease !== null : r.plannedEof === null || r.plannedRelease === null || r.plannedEof < r.sof || r.plannedRelease < r.plannedEof) fail(`${r.id}: 予定時刻が不正です。`);
      if ((r.eof !== null && r.eof !== r.plannedEof) || (r.release !== null && r.release !== r.plannedRelease)) fail(`${r.id}: 到達時刻と予定時刻が一致しません。`);
      if (raw.schema_version === 2 && BigInt(envelopes.get(rawRequest).time_ps) !== (r.release ?? r.eof ?? r.sof ?? r.txEnqueued ?? r.ready ?? r.generated)) fail(`${r.id}: 包絡の時刻と最後の確定時刻が一致しません。`);
      nodes.add(r.source); buses.add(r.bus); ids.set(r.id, r);
      event(r.generated, 'generated', r.id, r.source, '要求生成');
      event(r.ready, r.status === 'dropped' ? 'dropped' : 'ready', r.id, r.source, r.status === 'dropped' ? '満杯で破棄' : '送信準備完了');
      if (r.hasTxEnqueued) event(r.txEnqueued,'tx_enqueued',r.id,r.source,'TXキュー投入');
      event(r.sof, 'sof', r.id, r.bus, '送信開始');
      event(r.eof, 'eof', r.id, r.bus, '送信成功');
      event(r.release, 'release', r.id, r.bus, 'バス解放');
      return r;
    });
    // A bus cannot start another frame until the reached release of its predecessor.
    const transmissions = new Map();
    for (const r of requests) {
      if (r.sof === null) continue;
      if (!transmissions.has(r.bus)) transmissions.set(r.bus, []);
      transmissions.get(r.bus).push(r);
    }
    for (const frames of transmissions.values()) {
      frames.sort((a, b) => compareTime(a.sof, b.sof));
      for (let i = 1; i < frames.length; i++) {
        if (frames[i - 1].release === null || frames[i - 1].release > frames[i].sof) fail(`${frames[i].bus}: 送信占有区間が重複しています。`);
      }
    }
    const pairs = new Map();
    const receivers = array(receiverRows, 'receivers').map((rawReceiver, index) => {
      object(rawReceiver, `receivers[${index}]`);
      const r = {
        requestId: name(rawReceiver.request_id, 'receiver.request_id'),
        receiver: name(rawReceiver.receiver, 'receiver'), status: rawReceiver.status,
        observed: reached(rawReceiver.observed_ps, 'observed_ps'), received: reached(rawReceiver.received_ps, 'received_ps'), raw: rawReceiver,
      };
      const request = ids.get(r.requestId);
      if (!request || request.eof === null || r.receiver === request.source) fail(`${r.requestId}: 受信先と送信要求の対応が不正です。`);
      let seen = pairs.get(r.requestId);
      if (!seen) { seen = new Set(); pairs.set(r.requestId, seen); }
      if (seen.has(r.receiver)) fail('受信行が重複しています。');
      seen.add(r.receiver);
      r.eof = request.eof;
      if (!RECEIVER_STATES.has(r.status) || (r.observed !== null && r.observed < r.eof) || (r.received !== null && (r.observed === null || r.received < r.observed))) fail(`${r.requestId}: 受信状態・時刻が不正です。`);
      if ((r.status === 'received' && (r.observed === null || r.received === null)) || (r.status === 'filtered' && (r.observed === null || r.received !== null)) || (r.status === 'pending' && r.received !== null)) fail(`${r.requestId}: 受信状態と時刻が一致しません。`);
      if (raw.schema_version === 2) {
        const envelope = envelopes.get(rawReceiver);
        if (envelope.origin_request_id !== request.origin || BigInt(envelope.time_ps) !== (r.received ?? r.observed ?? r.eof)) fail(`${r.requestId}: 受信包絡の元要求または確定時刻が不正です。`);
      }
      request.receivers.push(r); nodes.add(r.receiver);
      event(r.observed, r.status === 'filtered' ? 'filtered' : 'observed', r.requestId, r.receiver, r.status === 'filtered' ? 'フィルタ不一致' : '受信観測');
      event(r.received, 'received', r.requestId, r.receiver, '受信完了');
      return r;
    });
    if (raw.schema_version === 2) {
      for (const r of requests) {
        const origin = ids.get(r.origin), parent = r.parent === null ? null : ids.get(r.parent);
        if (!origin || origin.parent !== null || origin.origin !== origin.id) fail(`${r.id}: 元要求が不正です。`);
        if (r.parent === null ? r.hops !== 0n || r.origin !== r.id : !parent || parent.origin !== r.origin || parent.hops + 1n !== r.hops || parent.eof === null || r.generated < parent.eof || parent.bus === r.bus) fail(`${r.id}: Gatewayの親・hopが不正です。`);
      }
    }
    const forwards = forwardRows.map(rawForward => {
      const f = { id: name(rawForward.forward_id,'forward_id'), gateway: name(rawForward.gateway,'gateway'),
        parent: name(rawForward.parent_request_id,'parent_request_id'), child: rawForward.child_request_id,
        ingress: name(rawForward.ingress,'ingress'), egress: rawForward.egress, status: rawForward.status,
        received: reached(rawForward.received_ps,'GW received_ps'), forwarded: reached(rawForward.forwarded_ps,'GW forwarded_ps'),
        planned: rawForward.planned_forward_ps === null ? null : decimal(rawForward.planned_forward_ps,'GW planned_forward_ps'), raw: rawForward };
      const parent = ids.get(f.parent), child = f.child === null ? null : ids.get(f.child);
      const ingress = parent?.receivers.find(r => r.receiver === f.ingress);
      const hops = decimal(rawForward.gw_hops,'GW hops',65536n);
      if (!parent || rawForward.origin_request_id !== parent.origin || !ingress || ingress.status !== 'received' || ingress.received !== f.received) fail(`${f.id}: Gatewayの受信元が不正です。`);
      if (BigInt(envelopes.get(rawForward).time_ps) !== (f.forwarded ?? f.received)) fail(`${f.id}: 転送包絡の確定時刻が不正です。`);
      if (!['processing','submitted','dropped','filtered'].includes(f.status)) fail('Gatewayの状態が不正です。');
      if (f.status === 'filtered') {
        if (f.egress !== null || f.child !== null || f.planned !== null || f.forwarded !== null || rawForward.route_id !== null || rawForward.reason !== 'no_route' || hops !== parent.hops) fail('Gatewayの経路拒否記録が不正です。');
      } else {
        if (typeof f.egress !== 'string' || typeof rawForward.route_id !== 'string' || f.planned === null || f.planned < f.received || hops !== parent.hops + 1n) fail('Gatewayの経路・予定時刻が不正です。');
        if (f.status === 'processing' ? f.forwarded !== null || f.child !== null || rawForward.reason !== null : f.forwarded !== f.planned) fail('Gatewayの処理状態が不正です。');
        if (f.status === 'submitted' && (!child || child.parent !== f.parent || child.id !== f.id || child.generated !== f.forwarded || child.source !== f.egress || rawForward.reason !== null)) fail('Gatewayのコピー参照が不正です。');
        if (f.status === 'dropped' && (f.child !== null || rawForward.reason !== 'dropped_hop_limit')) fail('Gatewayのhop破棄記録が不正です。');
      }
      event(f.received,'gw_received',f.parent,f.gateway,'Gateway受信');
      event(f.forwarded,f.status === 'dropped' ? 'gw_dropped' : 'gw_submitted',f.child || f.parent,f.gateway,f.status === 'dropped' ? 'hop上限で破棄' : 'コピー要求を生成');
      return f;
    });
    for (const r of requests) if (r.parent !== null && !forwards.some(f => f.child === r.id)) fail(`${r.id}: Gateway転送記録がありません。`);
    const rxBuffers = rxBufferRows.map(rawBuffer => {
      const b = { id:name(rawBuffer.buffer_id,'buffer_id'), parent:name(rawBuffer.parent_request_id,'RX parent'), origin:name(rawBuffer.origin_request_id,'RX origin'), gateway:name(rawBuffer.gateway,'RX gateway'), ingress:name(rawBuffer.ingress,'RX ingress'), capacity:decimal(rawBuffer.capacity,'RX capacity',4294967295n), received:reached(rawBuffer.received_ps,'RX received_ps'), released:reached(rawBuffer.released_ps,'RX released_ps'), status:rawBuffer.status, reason:rawBuffer.reason, egress:array(rawBuffer.egress,'RX egress').map(p=>name(p,'RX egress port')), raw:rawBuffer };
      const parent = ids.get(b.parent), receiver = parent?.receivers.find(r=>r.receiver === b.ingress);
      if (!parent || b.origin !== parent.origin || !receiver || receiver.received !== b.received || b.received === null || b.id !== `rx:${b.parent}/${b.gateway}/${b.ingress}` || new Set(b.egress).size !== b.egress.length) fail(`${b.id}: Gateway RXの受信元が不正です。`);
      if (!['holding','released','dropped'].includes(b.status) || (b.status === 'released' ? b.released === null || b.released < b.received : b.released !== null) || (b.status === 'dropped' ? b.reason !== 'rx_queue_full' : b.reason !== null)) fail(`${b.id}: Gateway RXの状態が不正です。`);
      if (BigInt(envelopes.get(rawBuffer).time_ps) !== (b.released ?? b.received)) fail(`${b.id}: Gateway RX包絡の確定時刻が不正です。`);
      const paths = forwards.filter(f=>f.gateway === b.gateway && f.parent === b.parent && f.ingress === b.ingress && f.egress !== null);
      if (b.status === 'dropped') {
        if (paths.length) fail(`${b.id}: RX破棄後の転送記録があります。`);
      } else {
        if (b.capacity === 0n || JSON.stringify([...new Set(paths.map(f=>f.egress))].sort()) !== JSON.stringify([...b.egress].sort())) fail(`${b.id}: Gateway RXの転送先が不正です。`);
        const terminals = paths.map(f=>{
          if (f.status === 'dropped') return f.forwarded;
          const child = ids.get(f.child);
          return child ? child.txEnqueued ?? (child.status === 'dropped' ? child.ready : null) : null;
        });
        const completed = terminals.every(t=>t !== null);
        const release = terminals.reduce((latest,t)=>t !== null && t > latest ? t : latest,b.received);
        if (b.status === 'released' ? !completed || b.released !== release : completed) fail(`${b.id}: RX解放とTX受付の時刻が一致しません。`);
      }
      event(b.received,b.status === 'dropped' ? 'gw_rx_dropped' : 'gw_rx_hold',b.parent,b.ingress,b.status === 'dropped' ? 'RX満杯で破棄' : 'Gateway RX保持');
      event(b.released,'gw_rx_release',b.parent,b.ingress,'Gateway RX解放');
      return b;
    });
    const rxQueueRecords = new Map();
    array(sim.records, 'records').forEach((record, order) => {
      object(record, `records[${order}]`);
      if (!['queue_length','gw_rx_queue_length'].includes(record.metric)) return;
      const isRx = record.metric === 'gw_rx_queue_length', suffix = isRx ? '.rxQueue' : '.txQueue', records = isRx ? rxQueueRecords : queueRecords;
      const target = name(record.target, 'queue target');
      if (!target.endsWith(suffix)) fail('キュー計測の対象名が不正です。');
      const node = target.slice(0, -suffix.length);
      const time = reached(record.time_ps, 'queue.time_ps');
      if (time === null || record.value_kind !== 'integer') fail('キュー計測の時刻・型が不正です。');
      const value = Number(decimal(record.value, 'queue.value', 4294967295n));
      if (!records.has(node)) records.set(node, []);
      records.get(node).push({ time, value, order }); nodes.add(node);
    });
    for (const samples of [...queueRecords.values(),...rxQueueRecords.values()]) samples.sort((a, b) => compareTime(a.time, b.time) || a.order - b.order);
    const metadata = raw.metadata || {};
    const gatewayPorts = new Set(), gateways = [], gatewayIds = new Set(), gatewayCapacities = new Map(), gatewayRoutes = [];
    const controllers = [];
    if (metadata.topology !== undefined) {
      const seen = new Set();
      for (const row of array(object(metadata.topology,'topology').controllers,'topology.controllers')) {
        const id = name(row.id,'controller id'), bus = name(row.bus,'controller bus');
        if (seen.has(id)) fail('Controller接続が重複しています。');
        seen.add(id); nodes.add(id); buses.add(bus);
        controllers.push({id,bus,txChannelDelay:decimal(row.tx_channel_delay_ps,'TX channel delay'),rxChannelDelay:decimal(row.rx_channel_delay_ps,'RX channel delay')});
      }
    }
    if (Array.isArray(metadata.models)) {
      for (const model of metadata.models) if (model.type && model.type !== profile) fail('CAN以外のモデルは未対応です。');
    }
    if (Array.isArray(metadata.initial_state)) {
      for (const entry of metadata.initial_state) {
        if (!entry || typeof entry.instance !== 'string' || typeof entry.state !== 'string') continue;
        let state; try { state = JSON.parse(entry.state); } catch { continue; }
        if (!state || typeof state !== 'object') continue;
        if (Array.isArray(state.queue)) nodes.add(entry.instance);
        if (state.state === 'idle' && state.profile === profile) buses.add(entry.instance);
      }
    }
    if (Array.isArray(metadata.config)) {
      for (const entry of metadata.config) {
        if (!entry || typeof entry.key !== 'string') continue;
        if (entry.key.endsWith('.queueCapacity')) nodes.add(entry.key.slice(0, -14));
        if (entry.key.endsWith('.bitrate')) buses.add(entry.key.slice(0, -8));
        if (raw.schema_version === 2 && entry.key.startsWith(`@profile:${profile}:`)) {
          let gateway; try { gateway = JSON.parse(entry.value); } catch { fail('Gatewayの正規化設定が不正です。'); }
          const id = name(gateway?.node,'Gateway node');
          if (entry.key !== `@profile:${profile}:${id}` || gatewayIds.has(id)) fail('Gatewayの名前と設定キーが一致しないか重複しています。');
          const ports = array(gateway.ports,'Gateway ports').map(port => name(port,'Gateway port'));
          if (!ports.length) fail('GatewayのControllerがありません。');
          for (const port of ports) {
            if (gatewayPorts.has(port)) fail('GatewayのController所属が重複しています。');
            gatewayPorts.add(port); nodes.add(port);
          }
          gatewayCapacities.set(id, gateway.rx_queue_capacity === undefined ? null : decimal(gateway.rx_queue_capacity,'Gateway RX capacity',4294967295n));
          for (const route of array(gateway.routes,'Gateway routes')) {
            const ingress = name(route.ingress,'route ingress');
            for (const egress of array(route.egress,'route egress')) {
              name(egress,'route egress');
              // Display only explicitly declared Controller membership.
              if (ports.includes(ingress) && ports.includes(egress)) gatewayRoutes.push({gateway:id,id:name(route.id,'route id'),ingress,egress});
            }
          }
          gatewayIds.add(id); gateways.push({ id, ports: ports.sort() });
        }
      }
    }
    const declaredBuses = new Map(controllers.map(c=>[c.id,c.bus]));
    for (const request of requests) {
      if (declaredBuses.has(request.source) && declaredBuses.get(request.source) !== request.bus) fail(`${request.id}: 宣言された送信Controllerのバスと一致しません。`);
      for (const receiver of request.receivers) if (declaredBuses.has(receiver.receiver) && declaredBuses.get(receiver.receiver) !== request.bus) fail(`${request.id}: 宣言された受信Controllerのバスと一致しません。`);
    }
    events.sort((a, b) => compareTime(a.time, b.time) || a.order - b.order);
    const times = new Set([start, end]);
    for (const e of events) times.add(e.time);
    for (const samples of [...queueRecords.values(),...rxQueueRecords.values()]) for (const p of samples) times.add(p.time);
    return { raw, start, end, requests, receivers, forwards, rxBuffers, controllers, gatewayRoutes, gatewayCapacities, rxQueueRecords, gatewayPorts, gateways: gateways.sort((a,b) => a.id < b.id ? -1 : a.id > b.id ? 1 : 0), nodes: [...nodes].sort(), buses: [...buses].sort(), events, eventTimes: [...times].sort(compareTime), queueRecords };
  }
  function requestStateAt(r, time) {
    if (time < r.generated) return 'not_generated';
    if (r.status === 'dropped' && r.ready !== null && time >= r.ready) return 'dropped';
    if (r.eof !== null && time >= r.eof) return 'success';
    if (r.sof !== null && time >= r.sof) return 'in_flight';
    if (r.ready !== null && time >= r.ready) return r.txEnqueued === null || time < r.txEnqueued ? 'waiting_tx' : 'pending';
    return 'processing';
  }
  function originsAt(model,time) {
    const origins = new Map(), byId = new Map(model.requests.map(r=>[r.id,r]));
    for (const r of model.requests) if (r.parent === null && r.generated <= time) {
      origins.set(r.id,{ id:r.id,nativeStatus:requestStateAt(r,time),copies:0,success:0,dropped:0,unfinished:0,deliveries:[] });
    }
    for (const r of model.requests) {
      const origin = origins.get(r.origin);
      if (!origin || r.generated > time) continue;
      if (r.parent !== null) {
        origin.copies++;
        const status = requestStateAt(r,time);
        origin[status === 'success' ? 'success' : status === 'dropped' ? 'dropped' : 'unfinished']++;
      }
      const native = byId.get(r.origin);
      for (const receiver of r.receivers) if (!model.gatewayPorts.has(receiver.receiver) && receiverStateAt(receiver,time) === 'received') {
        origin.deliveries.push({ requestId:r.id,receiver:receiver.receiver,time:receiver.received,pathDelay:receiver.received-native.generated });
      }
    }
    return [...origins.values()];
  }
  function forwardStateAt(f, time) {
    if (time < f.received) return 'not_created';
    if (f.status === 'filtered') return 'filtered';
    if (f.forwarded !== null && time >= f.forwarded) return f.status;
    return 'processing';
  }
  function rxBufferStateAt(b,time) {
    if (time < b.received) return 'not_created';
    if (b.status === 'dropped') return 'dropped';
    return b.released !== null && time >= b.released ? 'released' : 'holding';
  }
  function gatewayTransfersAt(model,time) {
    const requests = new Map(model.requests.map(r=>[r.id,r]));
    return model.forwards.filter(f=>f.egress !== null && time >= f.received).map(f=>{
      const child = requests.get(f.child), state = forwardStateAt(f,time);
      const status = state === 'submitted' ? requestStateAt(child,time) : state;
      const end = child?.ready ?? f.planned;
      return {forward:f,requestId:state === 'submitted' ? f.child : f.parent,source:f.ingress,receiver:f.egress,gateway:f.gateway,
        routeId:f.raw.route_id,admission:child?.txEnqueued ?? null,
        admitted:child?.txEnqueued !== null && child?.txEnqueued !== undefined && time >= child.txEnqueued,
        phase:status,progress:status === 'processing' ? (end === null || end === undefined ? null : fraction(time,f.received,end)) : null};
    });
  }
  function receiverStateAt(r, time) {
    if (time < r.eof) return 'not_created';
    if (r.received !== null && time >= r.received) return 'received';
    if (r.status === 'filtered' && r.observed !== null && time >= r.observed) return 'filtered';
    return 'pending';
  }
  function stateAt(model, time) {
    const counts = { generated: 0, processing: 0, waiting_tx: 0, pending: 0, in_flight: 0, success: 0, dropped: 0, received: 0, filtered: 0, rx_pending: 0 };
    const nodes = model.nodes.map(id => ({ id, queue: 0, processing: 0, waitingTx: [], transmitting: [], received: 0, filtered: 0 }));
    const buses = model.buses.map(id => ({ id, state: 'idle', requestId: null }));
    const byNode = new Map(nodes.map(n => [n.id, n])), byBus = new Map(buses.map(b => [b.id, b]));
    for (const r of model.requests) {
      const status = requestStateAt(r, time);
      if (status === 'not_generated') continue;
      counts.generated++; counts[status]++;
      const node = byNode.get(r.source);
      if (status === 'pending') node.queue++;
      if (status === 'processing') node.processing++;
      if (status === 'waiting_tx') node.waitingTx.push(r.id);
      if (status === 'in_flight') node.transmitting.push(r.id);
      const bus = byBus.get(r.bus);
      if (r.sof !== null && time >= r.sof && (r.release === null || time < r.release)) {
        bus.state = r.eof !== null && time >= r.eof ? 'intermission' : 'transmitting';
        bus.requestId = r.id;
      }
    }
    for (const r of model.receivers) {
      const status = receiverStateAt(r, time);
      if (status === 'not_created') continue;
      if (status === 'pending') counts.rx_pending++;
      else { counts[status]++; byNode.get(r.receiver)[status]++; }
    }
    for (const node of nodes) {
      const samples = model.queueRecords.get(node.id);
      if (!samples || !samples.length) continue;
      let low = 0, high = samples.length;
      while (low < high) { const mid = Math.floor((low + high) / 2); if (samples[mid].time <= time) low = mid + 1; else high = mid; }
      node.queue = low ? samples[low - 1].value : 0;
    }
    for (const gateway of model.gateways) for (const port of gateway.ports) {
      const node = byNode.get(port), buffers = model.rxBuffers.filter(b=>b.gateway === gateway.id && b.ingress === port);
      const held = buffers.filter(b=>rxBufferStateAt(b,time) === 'holding');
      const recorded = model.gatewayCapacities.get(gateway.id) !== null || model.rxBuffers.length > 0 || model.rxQueueRecords.has(port);
      node.rxBuffer = { gateway:gateway.id, capacity:model.gatewayCapacities.get(gateway.id) ?? buffers[0]?.capacity ?? null, occupancy:recorded ? held.length : null, held, dropped:buffers.filter(b=>rxBufferStateAt(b,time) === 'dropped').length };
      const samples = model.rxQueueRecords.get(port) || [];
      for (const sample of samples) { if (sample.time > time) break; node.rxBuffer.occupancy = sample.value; }
    }
    return { counts, nodes, buses };
  }
  // Logical transfer progress, not wire propagation. Only recorded milestones
  // create completion traces; planned EOF is used solely to scale active TX.
  function networkAt(model, time, options = {}) {
    const state = stateAt(model, time);
    const defaultWindow = (model.end - model.start) / 40n;
    const trailWindowPs = options.trailWindowPs === undefined ? (defaultWindow || 1n) : options.trailWindowPs;
    if (typeof trailWindowPs !== 'bigint' || trailWindowPs < 0n) fail('残像期間は0以上のBigIntで指定してください。');
    const tx = [], rx = [], links = new Map(), recent = new Map();
    function connect(node, bus, inferred = false) {
      const key = JSON.stringify([node, bus]);
      if (!links.has(key) || !inferred) links.set(key, { node, bus, inferred });
    }
    function trace(request, receiver, kind, at) {
      if (at === null || time < at || trailWindowPs === 0n || time - at >= trailWindowPs) return;
      const key = JSON.stringify([request.bus, receiver || request.source, receiver === null ? 'tx' : 'rx']);
      const previous = recent.get(key);
      if (previous && previous.time > at) return;
      recent.set(key, { requestId: request.id, source: request.source, bus: request.bus, receiver, kind,
        time: at, age: time - at, opacity: 1 - fraction(time, at, at + trailWindowPs) });
    }
    for (const request of model.requests) {
      connect(request.source, request.bus);
      if (request.sof !== null && time >= request.sof && (request.eof === null || time < request.eof)) {
        const end = request.eof === null ? request.plannedEof : request.eof;
        tx.push({ requestId: request.id, source: request.source, bus: request.bus,
          start: request.sof, end, progress: end === null ? null : fraction(time, request.sof, end) });
      }
      trace(request, null, 'sent', request.eof);
      for (const receiver of request.receivers) {
        connect(receiver.receiver, request.bus);
        if (request.sof !== null && time >= request.sof && time < receiver.eof) {
          // Only known receiver records animate before EOF. This visual phase does
          // not create a committed receive event or infer recipients for partial runs.
          rx.push({ requestId: request.id, source: request.source, bus: request.bus, receiver: receiver.receiver,
            phase: 'frame', start: request.sof, end: receiver.eof,
            progress: fraction(time, request.sof, receiver.eof) });
        } else if (receiverStateAt(receiver, time) === 'pending') {
          const observation = receiver.observed === null || time < receiver.observed;
          const start = observation ? receiver.eof : receiver.observed;
          const end = observation ? receiver.observed : receiver.received;
          rx.push({ requestId: request.id, source: request.source, bus: request.bus, receiver: receiver.receiver,
            phase: observation ? 'observation' : 'processing', start, end,
            progress: end === null ? null : fraction(time, start, end) });
        }
        trace(request, receiver.receiver, 'received', receiver.received);
        if (receiver.status === 'filtered') trace(request, receiver.receiver, 'filtered', receiver.observed);
      }
    }
    for (const controller of model.controllers) connect(controller.id,controller.bus);
    // Legacy results do not export wiring. Observed pairs establish connections;
    // an otherwise idle node on the sole bus is a clearly marked inference.
    if (!model.raw.metadata?.topology && model.buses.length === 1) {
      for (const node of model.nodes) if (!model.gatewayPorts.has(node)) connect(node, model.buses[0], true);
    }
    return { nodes: state.nodes, buses: state.buses, gateways: model.gateways, connections: [...links.values()], routes:model.gatewayRoutes, transfers:gatewayTransfersAt(model,time), tx, rx,
      trails: [...recent.values()], trailWindowPs };
  }
  // Replay the recorded communication crossed by one event step. Zero-delay
  // receivers still have a frame interval, even when they finish at the new cursor.
  function stepTransfers(model, from, to) {
    if (from === to) return [];
    const low = from < to ? from : to, high = from < to ? to : from;
    const active = networkAt(model, to, { trailWindowPs: 0n });
    const transfers = new Map();
    function add(item, kind) {
      const key = JSON.stringify([item.medium || 'can', kind, item.forwardId || item.requestId, item.receiver || null]);
      transfers.set(key, { ...item, kind });
    }
    function crossed(start, end) {
      return start !== null && start <= high && (end === null || end > low) && start < high;
    }
    for (const request of model.requests) {
      if (crossed(request.sof, request.eof)) add({ requestId: request.id, source: request.source, bus: request.bus }, 'tx');
      for (const receiver of request.receivers) {
        if (crossed(request.sof, receiver.eof) || crossed(receiver.eof, receiver.observed)) {
          add({ requestId: request.id, source: request.source, bus: request.bus, receiver: receiver.receiver, phase: 'frame' }, 'rx');
        }
      }
    }
    for (const item of active.tx) add(item, 'tx');
    for (const item of active.rx) if (item.phase !== 'processing') add(item, 'rx');
    const requests = new Map(model.requests.map(r => [r.id, r]));
    const processingForwards = new Set(active.transfers.filter(item => item.phase === 'processing').map(item => item.forward.id));
    for (const forward of model.forwards) {
      if (forward.egress === null) continue;
      const child = requests.get(forward.child);
      const admission = child?.txEnqueued ?? null;
      const completedInStep = admission !== null && admission > low && admission <= high;
      const activeAtDestination = processingForwards.has(forward.id);
      const processingEnd = child?.ready ?? forward.forwarded ?? forward.planned;
      if (!crossed(forward.received, processingEnd) && !completedInStep && !activeAtDestination) continue;
      const item = { medium:'gateway', gateway:forward.gateway, routeId:forward.raw.route_id, forwardId:forward.id,
        parentRequestId:forward.parent, requestId:forward.child || forward.parent, source:forward.ingress, receiver:forward.egress };
      add(item, 'tx');
      // Only committed egress admission establishes internal reception. A planned
      // forward or a capacity-blocked/dropped copy never supplies this leg.
      if (admission !== null && admission <= high) add(item, 'rx');
    }
    const result = [...transfers.values()];
    const canTx = new Map(), canRx = new Map(), gatewayTx = new Map(), gatewayRx = new Map();
    for (const item of result) {
      if (item.medium === 'gateway') (item.kind === 'tx' ? gatewayTx : gatewayRx).set(item.kind === 'tx' ? item.forwardId : item.requestId, item);
      else (item.kind === 'tx' ? canTx : canRx).set(item.kind === 'tx' ? item.requestId : JSON.stringify([item.requestId, item.receiver]), item);
    }
    const children = new Map(result.map(item => [item, []])), pending = new Map();
    for (const item of result) {
      const dependency = item.medium === 'gateway'
        ? item.kind === 'tx' ? canRx.get(JSON.stringify([item.parentRequestId, item.source])) : gatewayTx.get(item.forwardId)
        : item.kind === 'tx' ? gatewayRx.get(item.requestId) : canTx.get(item.requestId);
      item.stage = 0;
      pending.set(item, dependency ? 1 : 0);
      if (dependency) children.get(dependency).push(item);
    }
    // Recorded parent/child relationships form a DAG. Iterate instead of recursing
    // so long Gateway chains retain their order without using the JS call stack.
    const queue = result.filter(item => pending.get(item) === 0);
    for (let index = 0; index < queue.length; index++) {
      const item = queue[index];
      for (const child of children.get(item)) {
        child.stage = item.stage + 1;
        pending.set(child, pending.get(child) - 1);
        if (pending.get(child) === 0) queue.push(child);
      }
    }
    if (queue.length !== result.length) fail('通信のステップ表示に循環があります。');
    for (const item of result) item.untilStage = children.get(item).length ? item.stage + 1 : Infinity;
    return result;
  }
  // Only the selected request and its own receivers/RX buffers belong here.
  // Forwarding events describe other branches; their child generation is
  // already represented by that child's recorded request event.
  function requestEventGroups(model, requestId) {
    if (!model.requests.some(request => request.id === requestId)) return [];
    const kinds = new Set(['generated', 'ready', 'dropped', 'tx_enqueued', 'sof',
      'eof', 'release', 'observed', 'filtered', 'received', 'gw_rx_hold',
      'gw_rx_release', 'gw_rx_dropped']);
    const groups = [];
    for (const event of model.events) {
      if (event.requestId !== requestId || !kinds.has(event.kind)) continue;
      let group = groups[groups.length - 1];
      if (!group || group.time !== event.time) {
        group = { time: event.time, events: [] };
        groups.push(group);
      }
      group.events.push(event);
    }
    return groups;
  }
  return { parseResults, stateAt, networkAt, stepTransfers, requestEventGroups, requestStateAt, receiverStateAt, forwardStateAt, rxBufferStateAt, gatewayTransfersAt, originsAt, formatTime, parseTime, fraction, timeFromFraction };
});
