/* Pure, exact-time replay of schema-1 recorded milestones. Works in browsers and Node. */
(function (root, factory) {
  'use strict';
  const api = factory();
  if (typeof module === 'object' && module.exports) module.exports = api;
  else root.DIRViewerModel = api;
})(typeof globalThis === 'object' ? globalThis : this, function () {
  'use strict';
  const MAX_TIME = (1n << 64n) - 1n;
  const UNITS = { ps: 1n, ns: 1000n, us: 1000000n, ms: 1000000000n, s: 1000000000000n };
  const REQUEST_STATES = new Set(['processing', 'pending', 'in_flight', 'success', 'dropped']);
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
  function parseResults(raw) {
    object(raw, 'results');
    if (raw.schema_version !== 1) fail('対応しているresults.jsonはschema_version = 1です。');
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
    const requests = array(sim.requests, 'requests').map((rawRequest, index) => {
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
      if (fields.profile !== 'can.cc.ideal.v1' || fields.schema_version !== 1) fail('このビューアはcan.cc.ideal.v1の結果に対応しています。');
      r.release = reached(fields.release_ps, 'release_ps');
      r.plannedEof = fields.planned_eof_ps === null ? null : decimal(fields.planned_eof_ps, 'planned_eof_ps');
      r.plannedRelease = fields.planned_release_ps === null ? null : decimal(fields.planned_release_ps, 'planned_release_ps');
      const stages = [r.generated, r.ready, r.sof, r.eof, r.release];
      for (let i = 1; i < stages.length; i++) {
        if (stages[i] !== null && (stages[i - 1] === null || stages[i] < stages[i - 1])) fail(`${r.id}: 到達時刻の順序が不正です。`);
      }
      const shape = [r.ready !== null, r.sof !== null, r.eof !== null].join(',');
      const shapes = { processing: 'false,false,false', pending: 'true,false,false', in_flight: 'true,true,false', success: 'true,true,true', dropped: 'true,false,false' };
      if (shape !== shapes[r.status]) fail(`${r.id}: 要求状態と到達時刻が一致しません。`);
      if (r.sof === null ? r.plannedEof !== null || r.plannedRelease !== null : r.plannedEof === null || r.plannedRelease === null || r.plannedEof < r.sof || r.plannedRelease < r.plannedEof) fail(`${r.id}: 予定時刻が不正です。`);
      if ((r.eof !== null && r.eof !== r.plannedEof) || (r.release !== null && r.release !== r.plannedRelease)) fail(`${r.id}: 到達時刻と予定時刻が一致しません。`);
      nodes.add(r.source); buses.add(r.bus); ids.set(r.id, r);
      event(r.generated, 'generated', r.id, r.source, '要求生成');
      event(r.ready, r.status === 'dropped' ? 'dropped' : 'ready', r.id, r.source, r.status === 'dropped' ? '満杯で破棄' : '送信準備完了');
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
    const receivers = array(sim.receivers, 'receivers').map((rawReceiver, index) => {
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
      request.receivers.push(r); nodes.add(r.receiver);
      event(r.observed, r.status === 'filtered' ? 'filtered' : 'observed', r.requestId, r.receiver, r.status === 'filtered' ? 'フィルタ不一致' : '受信観測');
      event(r.received, 'received', r.requestId, r.receiver, '受信完了');
      return r;
    });
    array(sim.records, 'records').forEach((record, order) => {
      object(record, `records[${order}]`);
      if (record.metric !== 'queue_length') return;
      const target = name(record.target, 'queue target');
      if (!target.endsWith('.txQueue')) fail('キュー計測の対象名が不正です。');
      const node = target.slice(0, -8);
      const time = reached(record.time_ps, 'queue.time_ps');
      if (time === null || record.value_kind !== 'integer') fail('キュー計測の時刻・型が不正です。');
      const value = Number(decimal(record.value, 'queue.value', 4294967295n));
      if (!queueRecords.has(node)) queueRecords.set(node, []);
      queueRecords.get(node).push({ time, value, order }); nodes.add(node);
    });
    for (const samples of queueRecords.values()) samples.sort((a, b) => compareTime(a.time, b.time) || a.order - b.order);
    const metadata = raw.metadata || {};
    if (Array.isArray(metadata.models)) {
      for (const model of metadata.models) if (model.type && model.type !== 'can.cc.ideal.v1') fail('CAN以外のモデルは未対応です。');
    }
    if (Array.isArray(metadata.initial_state)) {
      for (const entry of metadata.initial_state) {
        if (!entry || typeof entry.instance !== 'string' || typeof entry.state !== 'string') continue;
        let state; try { state = JSON.parse(entry.state); } catch { continue; }
        if (!state || typeof state !== 'object') continue;
        if (Array.isArray(state.queue)) nodes.add(entry.instance);
        if (state.state === 'idle' && state.profile === 'can.cc.ideal.v1') buses.add(entry.instance);
      }
    }
    if (Array.isArray(metadata.config)) {
      for (const entry of metadata.config) {
        if (!entry || typeof entry.key !== 'string') continue;
        if (entry.key.endsWith('.queueCapacity')) nodes.add(entry.key.slice(0, -14));
        if (entry.key.endsWith('.bitrate')) buses.add(entry.key.slice(0, -8));
      }
    }
    events.sort((a, b) => compareTime(a.time, b.time) || a.order - b.order);
    const times = new Set([start, end]);
    for (const e of events) times.add(e.time);
    for (const samples of queueRecords.values()) for (const p of samples) times.add(p.time);
    return { raw, start, end, requests, receivers, nodes: [...nodes].sort(), buses: [...buses].sort(), events, eventTimes: [...times].sort(compareTime), queueRecords };
  }
  function requestStateAt(r, time) {
    if (time < r.generated) return 'not_generated';
    if (r.status === 'dropped' && r.ready !== null && time >= r.ready) return 'dropped';
    if (r.eof !== null && time >= r.eof) return 'success';
    if (r.sof !== null && time >= r.sof) return 'in_flight';
    if (r.ready !== null && time >= r.ready) return 'pending';
    return 'processing';
  }
  function receiverStateAt(r, time) {
    if (time < r.eof) return 'not_created';
    if (r.received !== null && time >= r.received) return 'received';
    if (r.status === 'filtered' && r.observed !== null && time >= r.observed) return 'filtered';
    return 'pending';
  }
  function stateAt(model, time) {
    const counts = { generated: 0, processing: 0, pending: 0, in_flight: 0, success: 0, dropped: 0, received: 0, filtered: 0, rx_pending: 0 };
    const nodes = model.nodes.map(id => ({ id, queue: 0, processing: 0, transmitting: [], received: 0, filtered: 0 }));
    const buses = model.buses.map(id => ({ id, state: 'idle', requestId: null }));
    const byNode = new Map(nodes.map(n => [n.id, n])), byBus = new Map(buses.map(b => [b.id, b]));
    for (const r of model.requests) {
      const status = requestStateAt(r, time);
      if (status === 'not_generated') continue;
      counts.generated++; counts[status]++;
      const node = byNode.get(r.source);
      if (status === 'pending') node.queue++;
      if (status === 'processing') node.processing++;
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
        if (receiverStateAt(receiver, time) === 'pending') {
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
    // Schema 1 does not export wiring. Observed pairs establish connections;
    // an otherwise idle node on the sole bus is a clearly marked inference.
    if (model.buses.length === 1) {
      for (const node of model.nodes) connect(node, model.buses[0], true);
    }
    return { nodes: state.nodes, buses: state.buses, connections: [...links.values()], tx, rx,
      trails: [...recent.values()], trailWindowPs };
  }
  return { parseResults, stateAt, networkAt, requestStateAt, receiverStateAt, formatTime, parseTime, fraction, timeFromFraction };
});
