/* B2: lossless local text buffers and serialized commands. No NED interpretation. */
(function (root, factory) {
  'use strict';
  const api = factory();
  if (typeof module === 'object' && module.exports) module.exports = api;
  else { root.NEDEditorInput = api; api.start(root).catch(() => {}); }
})(typeof globalThis === 'object' ? globalThis : this, function () {
  'use strict';
  const encoder = new TextEncoder();
  const MAX_BODY = 64 * 1024 * 1024;
  const delay = ms => new Promise(resolve => setTimeout(resolve, ms));
  function validUnicode(text) {
    for (let i = 0; i < text.length; i++) {
      const c = text.charCodeAt(i);
      if (c >= 0xd800 && c <= 0xdbff) {
        const next = text.charCodeAt(++i);
        if (!(next >= 0xdc00 && next <= 0xdfff)) return false;
      } else if (c >= 0xdc00 && c <= 0xdfff) return false;
    }
    return true;
  }
  const utf8Length = text => encoder.encode(text).length;
  function shadow(raw) {
    const bom = raw.startsWith('\ufeff');
    let r = bom ? 1 : 0, u = 0, bytes = bom ? 3 : 0, visible = '';
    const boundaries = [{ visible: 0, raw: r, byte: bytes }], newlines = [];
    while (r < raw.length) {
      let token = String.fromCodePoint(raw.codePointAt(r));
      const start = r;
      if (token === '\r') {
        token = raw[r + 1] === '\n' ? '\r\n' : '\r';
        newlines.push({ visible: u, raw: start, style: token });
      } else if (token === '\n') newlines.push({ visible: u, raw: start, style: token });
      const v = token[0] === '\r' ? '\n' : token;
      visible += v; r += token.length; u += v.length; bytes += utf8Length(token);
      boundaries.push({ visible: u, raw: r, byte: bytes });
    }
    return { visible, boundaries, newlines, bom };
  }
  function newlineStyle(table, at) {
    let nearest = null, distance = Infinity;
    for (const item of table.newlines) {
      const d = Math.abs(item.visible - at);
      if (d < distance) { nearest = item.style; distance = d; }
    }
    if (nearest) return nearest;
    const counts = new Map();
    for (const item of table.newlines) counts.set(item.style, (counts.get(item.style) || 0) + 1);
    let best = '\n', count = counts.get(best) || 0;
    for (const [style, n] of counts) if (n > count) { best = style; count = n; }
    return best;
  }
  function visiblePatch(raw, nextVisible) {
    if (!validUnicode(nextVisible)) throw new Error('文字変換を確定してから反映してください。');
    nextVisible = nextVisible.replace(/\r\n?/g, '\n');
    const table = shadow(raw), previous = table.visible;
    if (previous === nextVisible) return raw;
    let start = 0;
    while (start < previous.length && start < nextVisible.length && previous[start] === nextVisible[start]) start++;
    const map = new Map(table.boundaries.map(b => [b.visible, b]));
    while (!map.has(start)) start--;
    let oldEnd = previous.length, newEnd = nextVisible.length;
    while (oldEnd > start && newEnd > start && previous[oldEnd - 1] === nextVisible[newEnd - 1]) { oldEnd--; newEnd--; }
    while (!map.has(oldEnd)) { oldEnd++; newEnd++; }
    const replacement = nextVisible.slice(start, newEnd).replace(/\n/g, newlineStyle(table, start));
    return raw.slice(0, map.get(start).raw) + replacement + raw.slice(map.get(oldEnd).raw);
  }
  function byteToCaret(raw, byte, end = false) {
    const table = shadow(raw).boundaries;
    const offset = typeof byte === 'bigint' ? byte : BigInt(byte || '0');
    let previous = table[0];
    for (const b of table) {
      if (BigInt(b.byte) === offset) return b.visible;
      if (BigInt(b.byte) > offset) return end ? b.visible : previous.visible;
      previous = b;
    }
    return previous.visible;
  }
  class TextBuffer {
    constructor(fileId, raw, hash) {
      this.fileId = fileId; this.raw = raw; this.ackRaw = raw; this.hash = hash;
      this.generation = 0; this.ackedGeneration = 0; this.composing = false;
      this.blocked = null; this.sent = null; this.waiters = [];
    }
    get visible() { return shadow(this.raw).visible; }
    get pending() { return this.generation !== this.ackedGeneration; }
    edit(visible) {
      const next = visiblePatch(this.raw, visible);
      if (next !== this.raw) { this.raw = next; this.generation++; }
      return this.visible;
    }
    snapshot() { return { generation: this.generation, raw: this.raw, hash: this.hash, fileId: this.fileId }; }
    acknowledge(sent, hash) {
      this.hash = hash; this.ackRaw = sent.raw; this.ackedGeneration = sent.generation;
      this.sent = null; this.blocked = null;
      // The current raw text remains the newer local generation, if there is one.
    }
    adopt(raw, hash) {
      if (this.pending || this.composing || this.sent) return false;
      this.raw = raw; this.ackRaw = raw; this.hash = hash; this.blocked = null;
      return true;
    }
    composition(active) {
      this.composing = active;
      if (!active) { for (const resolve of this.waiters.splice(0)) resolve(); }
    }
    async waitComposition() { if (this.composing) await new Promise(resolve => this.waiters.push(resolve)); }
  }
  async function sourceHash(raw) {
    const digest = await globalThis.crypto.subtle.digest('SHA-256', new TextEncoder().encode(raw));
    return [...new Uint8Array(digest)].map(byte => byte.toString(16).padStart(2, '0')).join('');
  }
  function acknowledgeSource(buffer, sent, committedHash, observedHash) {
    buffer.acknowledge(sent, committedHash);
    if (observedHash !== committedHash) {
      buffer.blocked = '反映後に別のタブで原文が変更されました。入力を保持しています。「入力を再送信」で競合を確認してください。';
      throw new Error(buffer.blocked);
    }
  }
  function encodeBody(value, limit = MAX_BODY) {
    const json = JSON.stringify(value);
    if (!validUnicode(json)) throw new Error('送信できない文字が含まれています。入力は保持しています。');
    const maximum = Math.min(MAX_BODY, Number(limit) || MAX_BODY);
    if (utf8Length(json) > maximum) throw new Error('入力が通信上限を超えています。原文を保持したまま送信を停止しました。');
    return json;
  }
  class ApiError extends Error {
    constructor(message, status, data) { super(message); this.status = status; this.data = data; }
  }
  class Api {
    constructor(secret, fetcher = globalThis.fetch.bind(globalThis)) { this.secret = secret; this.fetcher = fetcher; this.bodyLimit = MAX_BODY; }
    async request(path, body) {
      const controller = new AbortController(), timeout = setTimeout(() => controller.abort(), 20000);
      try {
        const headers = { 'X-Editor-Session': this.secret };
        const options = { method: body === undefined ? 'GET' : 'POST', headers, cache: 'no-store', credentials: 'same-origin', signal: controller.signal };
        if (body !== undefined) { options.body = encodeBody(body, this.bodyLimit); headers['Content-Type'] = 'application/json'; }
        const response = await this.fetcher(path, options);
        let data;
        try { data = await response.json(); } catch (_) { throw new ApiError('サーバーの応答を読み取れません。処理結果を照会してください。', response.status); }
        if (!response.ok) throw new ApiError(data.message || '操作を完了できませんでした。', response.status, data);
        return data;
      } finally { clearTimeout(timeout); }
    }
  }
  function succeeded(envelope) {
    const status = envelope.operation_status;
    return envelope.accepted !== false && !/^E-/.test(envelope.code || '') &&
      !(/^\d+$/.test(String(status)) && Number(status) >= 400) && !['failed', 'rejected', 'conflict', 'expired', 'superseded'].includes(status);
  }
  class CommandClient {
    constructor(api, changed = () => {}) {
      this.api = api; this.changed = changed; this.sessionId = ''; this.clientId = '';
      this.writerEpoch = '0'; this.nextSequence = '1'; this.revision = '0';
      this.writer = false; this.inflight = null; this.uncertain = null; this.lastCommand = null;
    }
    register(data) {
      this.clientId = data.client_id || this.clientId; this.writerEpoch = String(data.writer_epoch ?? data.epoch ?? '0');
      this.nextSequence = String(data.next_sequence ?? this.nextSequence); this.writer = data.writer === true;
    }
    apply(envelope) {
      if (envelope.session_id !== this.sessionId || (envelope.command_id && envelope.command_id !== this.inflight?.command_id)) {
        throw new Error('操作に対応しない応答です。処理結果を照会してください。');
      }
      if (envelope.current_revision !== undefined) this.revision = String(envelope.current_revision);
      // A 202 reserves the number. Advance only once the operation is terminal.
      if (envelope.terminal && envelope.sequence_consumed) {
        this.nextSequence = String(envelope.next_sequence ?? (BigInt(this.inflight.client_sequence) + 1n));
      }
      this.changed(envelope);
    }
    async settle(envelope) {
      let pause = 250;
      while (!envelope.terminal) {
        this.apply(envelope);
        if (!envelope.job_id) throw new Error('処理結果が未確定です。結果を照会してください。');
        await delay(pause); pause = Math.min(2000, Math.round(pause * 1.3));
        envelope = await this.api.request('/api/jobs/' + encodeURIComponent(envelope.job_id));
      }
      this.apply(envelope);
      const command = this.inflight;
      this.inflight = null; this.uncertain = null; this.changed(envelope);
      if (!succeeded(envelope)) {
        const error = new ApiError(envelope.message || '操作が拒否されました。入力は保持しています。', envelope.operation_status, envelope);
        error.command = command; throw error;
      }
      return envelope;
    }
    async send(kind, payload, impactAck) {
      if (this.inflight) throw new Error('前の操作の処理結果を確認してください。');
      if (!this.writer) throw new Error('このタブは参照専用です。編集権限を取得してください。');
      const command = {
        schema_version: 1, session_id: this.sessionId, client_id: this.clientId,
        writer_epoch: this.writerEpoch, client_sequence: this.nextSequence,
        command_id: this.clientId + ':' + this.nextSequence, base_revision: this.revision, kind, payload
      };
      if (impactAck !== undefined) command.impact_ack = impactAck;
      // Reject oversized requests before declaring an uncertain transmission.
      encodeBody(command, this.api.bodyLimit);
      this.inflight = command; this.lastCommand = command; this.changed();
      try {
        let envelope;
        try { envelope = await this.api.request('/api/commands', command); }
        catch (error) {
          if (error.data?.terminal !== undefined) envelope = error.data;
          else throw error;
        }
        return await this.settle(envelope);
      } catch (error) {
        if (this.inflight) { this.uncertain = command; error.uncertain = true; }
        this.changed(); throw error;
      }
    }
    async query() {
      if (!this.uncertain) throw new Error('照会待ちの操作はありません。');
      let envelope;
      try { envelope = await this.api.request('/api/commands/' + encodeURIComponent(this.uncertain.command_id)); }
      catch (error) { if (error.data?.terminal !== undefined) envelope = error.data; else throw error; }
      return this.settle(envelope);
    }
  }
  function acceptView(current, next, scopeGeneration, sessionId) {
    if (!next || next.schema_version !== 1 || next.session_id !== sessionId) return false;
    if (String(next.request_scope_generation) !== String(scopeGeneration)) return false;
    try {
      return !current || (BigInt(next.view_sequence) >= BigInt(current.view_sequence) &&
        (current.revision === undefined || next.revision === undefined || BigInt(next.revision) >= BigInt(current.revision)));
    } catch (_) { return false; }
  }
  function sessionQuery(app) {
    const query = new URLSearchParams({ request_scope_generation: app.scopeGeneration });
    if (app.fileId) query.set('file_id', app.fileId);
    if (app.typeKey) query.set('type_key', app.typeKey);
    if (app.instancePath) query.set('instance_path', app.instancePath);
    return query;
  }
  function instanceScope(view, instance) {
    if (!instance || instance.unresolved || !instance.type_key) return null;
    const type = (view?.types || []).find(t => t.key === instance.type_key && t.name === instance.type_name);
    return type ? { fileId: type.file_id, typeKey: type.key, instancePath: instance.path } : null;
  }
  function childInstance(view, instancePath, node) {
    if (!instancePath || node.unresolved) return null;
    return (view?.instances || []).find(i => i.path === instancePath + '.' + node.name && i.type_name === node.type_name && instanceScope(view, i)) || null;
  }
  function resetProjectScope(app) {
    clearTimeout(app.timer);
    app.buffers.clear(); app.fileId = null; app.typeKey = null; app.instancePath = null;
    app.breadcrumbs = []; app.selection = null; app.pendingText = null; app.scopeLoading = true;
    app.scopeGeneration = (BigInt(app.scopeGeneration) + 1n).toString();
    app.projectReplaced = true;
    // Keep counters so an older response cannot replace the new project's projection.
    if (app.view) app.view = { ...app.view, graph: null, source: null, files: [], types: [], instances: [], parameters: [] };
  }
  function placementChoices(view) {
    return [
      ...(view?.catalog || []).filter(t => ['simple', 'module'].includes(t.kind)).map(t => ({ value: t.id, label: (t.label || t.name) + '（標準）' })),
      ...(view?.types || []).filter(t => ['simple', 'module'].includes(t.kind)).map(t => ({ value: t.name, label: t.name + '（プロジェクト）' }))
    ];
  }
  function channelChoices(view) {
    return [
      ...(view?.catalog || []).filter(t => t.kind === 'channel').map(t => ({ value: t.id, label: (t.label || t.name) + '（標準）' })),
      ...(view?.types || []).filter(t => t.kind === 'channel').map(t => ({ value: t.name, label: t.name + '（プロジェクト）' }))
    ];
  }
  function canPairCandidates(graph) {
    const nodes = graph?.nodes || [], used = new Set((graph?.connections || []).flatMap(c => [c.start, c.end]));
    return {
      controllers: nodes.filter(n => !n.unresolved && ['dir.can.Controller', 'dir.can.MultibusController'].includes(n.implementation) && n.gates?.filter(g => g.output).length === 1 && n.gates?.filter(g => !g.output).length === 1 && n.gates.every(g => !used.has(n.name + '.' + g.name))),
      buses: nodes.filter(n => !n.unresolved && ['dir.can.Bus', 'dir.can.MultibusBus'].includes(n.implementation) && n.gates?.some(g => !g.output) && n.gates?.some(g => g.output))
    };
  }
  const controllerClasses = ['dir.can.Controller', 'dir.can.MultibusController'];
  const busClasses = ['dir.can.Bus', 'dir.can.MultibusBus'];
  function controllerChoices(view, ancestor) {
    return (view?.instances || []).filter(i => !i.unresolved && controllerClasses.includes(i.implementation) && (!ancestor || i.path.startsWith(ancestor + '.'))).map(i => ({ value: i.path, label: i.path }));
  }
  function gatewayNodeChoices(view) {
    const choices = (view?.instances || []).filter(i => i.kind === 'module' && !i.unresolved && !i.cycle).map(i => ({ value: i.path, label: i.path, available: true }));
    for (const settings of view?.gateway_settings || []) if (!choices.some(c => c.value === settings.node)) choices.push({ value: settings.node, label: settings.node + '（配置なし・設定解除のみ）', available: false });
    return choices;
  }
  function gatewayPortChoices(view, node, settings) {
    const choices = controllerChoices(view, node);
    for (const port of settings?.ports || []) if (!choices.some(c => c.value === port)) choices.push({ value: port, label: port + '（配置なし・既存設定）' });
    return choices;
  }
  function integer(value, label, maximum = Number.MAX_SAFE_INTEGER, minimum = 0) {
    if (!/^\d+$/.test(String(value)) || !Number.isSafeInteger(Number(value)) || Number(value) < minimum || Number(value) > maximum) throw new Error(label + 'を ' + minimum + '〜' + maximum + ' の整数で指定してください。');
    return Number(value);
  }
  function uniqueRows(rows, label) {
    const ids = new Set();
    for (const row of rows) {
      if (!row.id?.trim()) throw new Error(label + 'のIDを入力してください。');
      if (ids.has(row.id)) throw new Error(label + 'のIDが重複しています: ' + row.id);
      ids.add(row.id);
    }
  }
  function gatewayPayload(node, form) {
    const ports = form.ports || [], routes = form.routes || [];
    if (!ports.length) throw new Error('Gatewayのポートを1つ以上選択してください。');
    uniqueRows(routes, 'ルート');
    return { node, ports, processing_delay: form.processing_delay, hop_limit: integer(form.hop_limit, 'hop limit', 65535, 1), rx_queue_capacity: integer(form.rx_queue_capacity, 'RX容量', 0xffffffff), routes: routes.map(row => {
      if (!ports.includes(row.ingress) || !(row.egress || []).length || row.egress.some(port => !ports.includes(port) || port === row.ingress)) throw new Error('ルートの入口と出口は選択した別々のポートを指定してください。');
      const max = row.format === 'extended' ? 0x1fffffff : 0x7ff;
      const id_min = integer(row.id_min, 'CAN ID下限', max), id_max = integer(row.id_max, 'CAN ID上限', max);
      if (id_min > id_max) throw new Error('CAN ID下限は上限以下にしてください。');
      return { id: row.id, ingress: row.ingress, egress: row.egress, format: row.format, id_min, id_max };
    }) };
  }
  function workloadPayload(form) {
    const rows = form.generators || [];
    uniqueRows(rows, '送信定義');
    return { generators: rows.map(row => {
      const frame = { format: row.format, id: integer(row.frame_id, 'CAN ID', row.format === 'extended' ? 0x1fffffff : 0x7ff), data: (row.data || '').replace(/\s/g, '') };
      if (!/^(?:[0-9a-fA-F]{2}){0,8}$/.test(frame.data)) throw new Error('データは0〜8バイトの16進数で指定してください。');
      if (!row.node) throw new Error('送信するControllerを選択してください。');
      const generator = { id: row.id, kind: row.kind, node: row.node, frame };
      if (row.kind === 'can.explicit.v1') {
        generator.times = String(row.times || '').split(/[,\r\n]+/).map(time => time.trim()).filter(Boolean);
      } else if (row.kind === 'can.periodic.v1') {
        if (!row.start || !row.period) throw new Error('周期送信の開始時刻と周期を指定してください。');
        Object.assign(generator, { start: row.start, phase: row.phase || '0ps', period: row.period });
        if (row.end) generator.end = row.end;
        if (row.count !== '' && row.count !== null && row.count !== undefined) generator.count = integer(row.count, '送信回数', Number.MAX_SAFE_INTEGER);
      } else throw new Error('送信形式を選択してください。');
      return generator;
    }) };
  }
  function gatePayload(typeKey, form, pairOnly = false) {
    const pair = pairOnly || form.mode === 'pair';
    const gates = pair ? [{ name: form.input_gate, output: false }, { name: form.output_gate, output: true }] : [{ name: form.gate_name, output: form.direction === 'output' }];
    if (gates.some(g => !/^[A-Za-z_][A-Za-z0-9_]*$/.test(g.name)) || new Set(gates.map(g => g.name)).size !== gates.length) throw new Error('異なるgate名を英数字と _ で指定してください。');
    return { type_key: typeKey, gates };
  }
  function settingsSnapshot(app, client) {
    return {
      session: app.view?.session_id, revision: String(app.view?.revision), inputRevision: String(app.view?.input_revision),
      scope: app.scopeGeneration, fileId: app.fileId, typeKey: app.typeKey, instancePath: app.instancePath,
      writerEpoch: client.writerEpoch,
      hashes: JSON.stringify((app.view?.files || []).map(f => [f.id, f.hash]).sort((a, b) => a[0].localeCompare(b[0]))),
      buffers: [...app.buffers.values()].map(b => ({ fileId: b.fileId, generation: b.generation, hash: b.hash }))
    };
  }
  function assertSettingsSnapshot(app, client, captured) {
    const latest = settingsSnapshot(app, client);
    if (!client.writer || client.uncertain || client.inflight) throw new Error('編集権限と送信済み操作の結果を確認し、設定を開き直してください。');
    if (['session', 'revision', 'inputRevision', 'scope', 'fileId', 'typeKey', 'instancePath', 'writerEpoch', 'hashes'].some(key => latest[key] !== captured[key]) ||
      [...app.buffers.values()].some(b => b.pending || b.composing || b.blocked || b.sent) ||
      captured.buffers.some(old => { const b = app.buffers.get(old.fileId); return !b || b.generation !== old.generation || b.hash !== old.hash; })) {
      throw new Error('設定を開いた後に原文または選択対象が変更されました。入力は保持しています。設定を閉じて開き直してください。');
    }
  }

  async function start(root) {
    const document = root.document;
    if (!document) return;
    // Capture once and immediately remove the fragment. Never put credentials in DOM/storage/logs.
    const fragment = root.location.hash.slice(1);
    const params = new URLSearchParams(fragment);
    const secret = params.get('session') || params.get('token') || params.get('secret') || params.get('editor_session') || fragment;
    root.history.replaceState(null, '', root.location.pathname + root.location.search);
    const $ = id => document.getElementById(id), V = root.NEDEditorView;
    const app = {
      view: null, fileId: null, typeKey: null, instancePath: null, scopeGeneration: '1', buffers: new Map(),
      selection: null, breadcrumbs: [], operation: null, flushPromise: null, timer: null,
      filter: '', instanceFilter: '', pendingText: null, refreshBusy: false, gesture: null, scopeLoading: false
    };
    const api = new Api(secret), client = new CommandClient(api, () => render());
    const buffer = () => app.buffers.get(app.fileId);
    const localPending = () => [...app.buffers.values()].some(b => b.pending || b.composing);
    function notice(message) { $('notice-text').textContent = message; $('notice').hidden = !message; }
    function errorMessage(error) { return error instanceof ApiError || error?.uncertain ? error.message : (error?.message || '操作を完了できませんでした。'); }
    function render() { if (V) V.render(app, client, callbacks); }
    async function refresh() {
      const query = sessionQuery(app);
      const next = await api.request('/api/session?' + query);
      if (!client.sessionId) client.sessionId = next.session_id;
      if (!acceptView(app.view, next, app.scopeGeneration, client.sessionId)) return false;
      const replaced = app.projectReplaced;
      const initial = !app.view || replaced;
      app.projectReplaced = false;
      app.scopeLoading = false;
      app.view = next; client.revision = String(next.revision);
      if (next.writer && BigInt(next.writer.epoch) >= BigInt(client.writerEpoch)) {
        client.writer = next.writer.client_id === client.clientId;
        client.writerEpoch = String(next.writer.epoch);
      }
      api.bodyLimit = Math.min(MAX_BODY, Number(next.limits?.body_bytes) || MAX_BODY);
      if (next.source) {
        const source = next.source;
        if (!app.buffers.has(source.file_id)) app.buffers.set(source.file_id, new TextBuffer(source.file_id, source.text, source.hash));
        else app.buffers.get(source.file_id).adopt(source.text, source.hash);
      }
      if (!app.fileId && next.source) app.fileId = next.source.file_id;
      if (!app.typeKey && next.graph) app.typeKey = next.graph.type_key;
      if ((replaced || !app.fileId || (initial && !app.typeKey)) && next.files?.length) {
        const initialType = next.types?.find(t => t.kind === 'network') || next.types?.find(t => t.kind === 'module') || next.types?.[0];
        app.fileId = initialType?.file_id || next.files[0].id; app.typeKey = initialType?.key || null;
        app.scopeGeneration = (BigInt(app.scopeGeneration) + 1n).toString();
        return refresh();
      }
      render(); return true;
    }
    async function navigate(fileId, typeKey, trail, instancePath = null) {
      app.fileId = fileId; app.typeKey = typeKey || null; app.instancePath = instancePath; app.selection = null;
      app.scopeLoading = true;
      if (trail !== undefined) app.breadcrumbs = trail;
      app.scopeGeneration = (BigInt(app.scopeGeneration) + 1n).toString();
      render(); await refresh();
    }
    async function navigateInstance(instance, trail = []) {
      const scope = instanceScope(app.view, instance);
      if (!scope) { notice('このインスタンスの型は未解決です。ソースと診断を確認してください。'); return; }
      return navigate(scope.fileId, scope.typeKey, trail, scope.instancePath);
    }
    async function flushAll() {
      clearTimeout(app.timer);
      if (app.flushPromise) return app.flushPromise;
      app.flushPromise = (async () => {
        if (client.uncertain) throw new Error('送信済み操作の処理結果を照会してください。');
        for (;;) {
          const sorted = [...app.buffers.values()].sort((a, b) => a.fileId.localeCompare(b.fileId));
          for (const b of sorted) {
            await b.waitComposition();
            if (!b.pending) continue;
            if (b.blocked) throw new Error('反映できない入力があります。「入力を再送信」で確認してください。');
            const sent = b.snapshot(); b.sent = sent; render();
            app.pendingText = { buffer: b, sent };
            try {
              const committedHash = await sourceHash(sent.raw);
              await client.send('replace_source', { file_id: b.fileId, expected_hash: sent.hash, text: sent.raw });
              await refresh();
              const hash = app.view.files?.find(f => f.id === b.fileId)?.hash;
              if (!hash) throw new Error('原文の反映結果を確認できません。表示を更新してください。');
              acknowledgeSource(b, sent, committedHash, hash); app.pendingText = null; render();
            } catch (error) {
              b.blocked = errorMessage(error);
              if (!client.uncertain) { b.sent = null; app.pendingText = null; }
              try { await refresh(); } catch (_) { /* retain both buffer and command */ }
              throw error;
            }
          }
          if (![...app.buffers.values()].some(b => b.pending || b.composing)) break;
        }
      })();
      try { await app.flushPromise; } finally { app.flushPromise = null; render(); }
    }
    function debounce() {
      clearTimeout(app.timer);
      if (buffer()?.composing || app.operation || !client.writer || buffer()?.blocked) return;
      app.timer = setTimeout(() => flushAll().catch(error => notice(errorMessage(error))), 300);
    }
    async function confirmCommand(kind, payload, heading, extra = '') {
      const preview = await api.request('/api/confirmations', {
        client_id: client.clientId, writer_epoch: client.writerEpoch, base_revision: client.revision, kind, payload
      });
      const impacts = Array.isArray(preview.impacts) ? preview.impacts : [];
      // No user prompt is necessary when the backend reports no shared impact.
      if (impacts.length || extra) {
        const answer = await V.dialog({ title: heading, submit: '確認して実行', text: extra, impacts });
        if (!answer) return null;
      }
      return preview.confirmation;
    }
    async function perform(kind, payload, options = {}) {
      if (app.operation) throw new Error('処理中の操作が完了するまでお待ちください。');
      if (!client.writer) throw new Error('編集権限を取得してください。');
      if (app.gesture) throw new Error('ドラッグを終了してから操作してください。');
      if (options.snapshot) assertSettingsSnapshot(app, client, options.snapshot);
      app.operation = '入力反映待ち'; render();
      try {
        if (options.snapshot) {
          app.operation = '設定の確認'; render();
          await refresh(); assertSettingsSnapshot(app, client, options.snapshot);
        } else if (!options.recovery) await flushAll();
        app.operation = V.commandLabel(kind); render();
        if (options.structural && (!app.view?.graph || app.view.graph.stale || V.instanceStructuralBlocked(app))) throw new Error('現在のソースに対応する構成図と、解決済みの階層を選択してください。');
        let impact;
        if (options.structural || options.configuration || kind === 'set_default') {
          impact = await confirmCommand(kind, payload, options.configuration ? V.commandLabel(kind) : '型定義の変更', options.configuration ? '' : '型定義を編集します。表示された利用箇所にも反映されます。');
          if (impact === null) return false;
        }
        if (kind === 'overwrite_project' || kind === 'reload') {
          const target = app.view.outputs?.find(o => o.id === payload.destination_id);
          const text = kind === 'reload' ? '元プロジェクトを読み直します。内部の編集内容と履歴は破棄されます。' : '保存前に全体を検証し、選択したプロジェクトを上書きします。\n対象: ' + (target?.path || '');
          const confirmation = await confirmCommand(kind, payload, kind === 'reload' ? '元から再読込' : '上書き保存', text);
          if (confirmation === null) return false;
          payload = { ...payload, [kind === 'reload' ? 'discard_ack' : 'overwrite_ack']: confirmation };
        }
        if (kind === 'new_project') {
          const warning = app.view?.dirty || app.view?.never_exported
            ? '現在のプロジェクトには未保存または未出力の内容があります。新規作成すると、内部の編集内容と履歴を破棄します。必要な内容は先に別フォルダへ保存してください。'
            : '現在のプロジェクトを新しいテンプレートで置き換えます。内部の編集履歴は破棄されます。';
          const confirmation = await confirmCommand(kind, payload, '新規プロジェクトを作成', warning);
          if (confirmation === null) return false;
          payload = { ...payload, discard_ack: confirmation };
        }
        let envelope;
        if (options.snapshot) { await refresh(); assertSettingsSnapshot(app, client, options.snapshot); }
        try { envelope = await client.send(kind, payload, impact); }
        catch (error) {
          // A protected layout requires its own server-issued confirmation, bound to the same target/revision.
          if (kind === 'overwrite_project' && !client.uncertain && /LAYOUT/.test(error.data?.code || '')) {
            await refresh();
            const ack = await confirmCommand('replace_layout', { destination_id: payload.destination_id }, '配置ファイルを置換', '保護された配置ファイルを置き換えます。');
            if (ack === null) return false;
            const overwriteAck = await confirmCommand('overwrite_project', { destination_id: payload.destination_id }, '上書き対象の再確認', '保存先の現在の状態を確認します。');
            if (overwriteAck === null) return false;
            envelope = await client.send(kind, { ...payload, overwrite_ack: overwriteAck, replace_layout_ack: ack });
          } else throw error;
        }
        if (kind === 'reload') {
          // Source input is locked after flush and throughout reload, so no unsent generation is discarded here.
          app.buffers.clear(); app.selection = null;
        }
        if (kind === 'new_project') resetProjectScope(app);
        await refresh(); notice(envelope.message || V.commandLabel(kind) + 'が完了しました。');
        return true;
      } finally {
        try { await refresh(); } catch (_) { /* visible error is reported by the caller */ }
        app.operation = null; render(); debounce();
      }
    }
    async function retryInput() {
      if (client.uncertain) return queryCommand();
      const blocked = [...app.buffers.values()].filter(b => b.blocked);
      if (!blocked.length) return flushAll();
      for (const b of blocked) {
        const q = new URLSearchParams({ file_id: b.fileId, request_scope_generation: 'retry' });
        const latest = await api.request('/api/session?' + q);
        if (latest.session_id !== client.sessionId) throw new Error('セッションが変更されています。ローカル原文をコピーしてから開き直してください。');
        if (!latest.source || latest.source.file_id !== b.fileId) throw new Error('最新原文を取得できません。');
        if (latest.source.hash !== b.hash) {
          const answer = await V.dialog({ title: '原文の競合を確認', submit: 'ローカル入力を反映', text: 'サーバーの原文が変わっています。下の最新原文を確認し、保持したローカル入力で置き換える場合だけ反映してください。', pre: latest.source.text });
          if (!answer) return;
          b.hash = latest.source.hash; b.ackRaw = latest.source.text;
        }
        b.blocked = null;
      }
      await refresh(); await flushAll(); notice('入力を反映しました。');
    }
    async function queryCommand() {
      const text = app.pendingText, kind = client.uncertain?.kind;
      try {
        const envelope = await client.query();
        if (kind === 'reload') app.buffers.clear();
        if (kind === 'new_project') resetProjectScope(app);
        await refresh();
        if (text) {
          const hash = app.view.files?.find(f => f.id === text.buffer.fileId)?.hash;
          if (!hash) throw new Error('反映後の原文を取得できません。');
          acknowledgeSource(text.buffer, text.sent, await sourceHash(text.sent.raw), hash); app.pendingText = null;
        }
        notice(envelope.message || '操作の完了を確認しました。'); debounce();
      } catch (error) {
        if (text) { text.buffer.blocked = errorMessage(error); if (!client.uncertain) { text.buffer.sent = null; app.pendingText = null; } }
        throw error;
      } finally { await refresh(); render(); }
    }
    async function newProject() {
      const templates = app.view?.templates || [];
      const result = await V.dialog({ title: '新規プロジェクト', submit: '内容を確認', text: 'テンプレートから編集用のプロジェクトを作成します。保存先は「別フォルダに保存」で指定します。', fields: [
        { id: 'project-template', label: 'テンプレート', choices: templates.map(t => ({ value: t.id, label: t.name + (t.description ? ' · ' + t.description : '') })) },
        { id: 'project-name-input', label: 'プロジェクト名', required: true, value: 'NewProject', pattern: '[A-Za-z_][A-Za-z0-9_]*', placeholder: 'NewProject', hint: '英字または _ で始まる、英数字と _ の名前を指定してください。' }
      ] });
      if (result) await perform('new_project', { template: result['project-template'], project_name: result['project-name-input'] });
    }
    async function saveAs() {
      const check = () => {
        if (!client.writer) throw new Error('編集権限を取得してください。');
        if (app.operation || client.inflight || client.uncertain) throw new Error('処理中または送信済みの操作の結果を確認してください。');
        if (app.gesture || V.recoveryRequired?.(app.view)) throw new Error('ドラッグまたは保存の復旧を完了してから保存してください。');
      };
      check();
      const result = await V.dialog({ title: '別フォルダにプロジェクトを保存', submit: '検証して保存', text: 'INI・全NED・関連JSON・配置を一式保存します。許可された保存先の下に、新規または空のフォルダを指定してください。', fields: [
        { id: 'export-root', label: '保存先の親フォルダ', choices: (app.view.export_roots || []).map(r => ({ value: r.id, label: r.path })) },
        { id: 'destination-name', label: '相対フォルダ名', required: true, placeholder: 'edited-project' }
      ] });
      if (!result) return;
      check();
      const target = await api.request('/api/destinations', { client_id: client.clientId, writer_epoch: client.writerEpoch, export_root_id: result['export-root'], relative_directory: result['destination-name'] });
      await perform('save_as_project', { destination_id: target.id });
    }
    async function overwrite() {
      const targets = (app.view.outputs || []).filter(o => ['source_project', 'managed_export'].includes(o.kind));
      const result = await V.dialog({ title: '上書きするプロジェクトを選択', submit: '対象を確認', text: '元プロジェクトまたは管理済みの保存先を選択してください。', fields: [{ id: 'overwrite-target', label: '上書き対象', choices: targets.map(t => ({ value: t.id, label: t.path + (t.kind === 'source_project' ? '（読込元）' : '') })) }] });
      if (result) await perform('overwrite_project', { destination_id: result['overwrite-target'] });
    }
    const graph = () => app.view?.graph;
    function typeForNode(node) { return (app.view.types || []).find(t => t.key === node.type_name || t.name === node.type_name); }
    function newPosition() {
      const occupied = new Set((graph()?.nodes || []).map(n => `${n.x},${n.y}`));
      let slot = 0; while (occupied.has(`${80 + slot % 4 * 240},${100 + Math.floor(slot / 4) * 160}`)) slot++;
      return { x: 80 + slot % 4 * 240, y: 100 + Math.floor(slot / 4) * 160 };
    }
    const nameField = (id, label, value) => ({ id, label, value, required: true, pattern: '[A-Za-z_][A-Za-z0-9_]*' });
    async function prepareSettings() {
      if (app.operation || client.inflight || client.uncertain) throw new Error('処理中または送信済みの操作の結果を確認してください。');
      if (!client.writer) throw new Error('編集権限を取得してください。');
      if (app.gesture || V.recoveryRequired?.(app.view)) throw new Error('ドラッグまたは保存の復旧を完了してから設定を開いてください。');
      app.operation = '入力反映待ち'; render();
      try {
        await flushAll();
        app.operation = '設定の読込'; render();
        await refresh();
        const snapshot = settingsSnapshot(app, client);
        assertSettingsSnapshot(app, client, snapshot);
        return snapshot;
      } finally { app.operation = null; render(); debounce(); }
    }
    async function createModule() {
      const result = await V.dialog({ title: '複合モジュールを作成', submit: '作成して配置', text: '空の複合型を定義し、現在の構成図に子として配置します。内部はダブルクリックで開けます。', fields: [nameField('type_name', '型名', 'GatewayGroup'), nameField('child_name', '配置する子の名前', 'gateway')] });
      if (result) await perform('create_module', { parent_type: app.typeKey, ...result, position: newPosition() }, { structural: true });
    }
    async function addGates() {
      const pairOnly = busClasses.includes(graph()?.implementation);
      const mode = pairOnly ? { mode: 'pair' } : await V.dialog({ title: 'gateの追加方法', fields: [{ id: 'mode', label: '追加方法', choices: [{ value: 'single', label: '単独のgate' }, { value: 'pair', label: 'input / outputの一組' }] }] });
      if (!mode) return;
      const pair = mode.mode === 'pair';
      const result = await V.dialog({ title: pair ? 'input / output gateを追加' : 'gateを追加', submit: '追加', text: 'gate名は方向から独立しています。既存の接続には影響しません。', fields: pair ? [nameField('input_gate', 'input gate名', 'receive_new'), nameField('output_gate', 'output gate名', 'send_new')] : [nameField('gate_name', 'gate名', 'receive_new'), { id: 'direction', label: '方向', choices: [{ value: 'input', label: 'input' }, { value: 'output', label: 'output' }] }], validate: result => gatePayload(app.typeKey, { ...result, ...mode }, pairOnly) });
      if (result) await perform('add_gates', gatePayload(app.typeKey, { ...result, ...mode }, pairOnly), { structural: true });
    }
    async function deleteGate() {
      const gates = graph()?.own_gates || [], pairOnly = busClasses.includes(graph()?.implementation);
      const fields = pairOnly ? [
        { id: 'gate_name', label: '削除するinput gate', choices: gates.filter(g => !g.output).map(g => ({ value: g.name, label: g.name })) },
        { id: 'paired_gate', label: '削除するoutput gate', choices: gates.filter(g => g.output).map(g => ({ value: g.name, label: g.name })) }
      ] : [{ id: 'gate_name', label: '削除するgate', choices: gates.map(g => ({ value: g.name, label: g.name + (g.output ? ' (output)' : ' (input)') })) }];
      const result = await V.dialog({ title: 'gateを削除', submit: '削除を確認', text: '内部や、この型を使用する外側で接続中のgateは削除できません。Busはinput/outputの一組を削除し、最低2組を残します。', fields });
      if (result) await perform('delete_gate', { type_key: app.typeKey, ...result }, { structural: true });
    }
    async function addPort() {
      const result = await V.dialog({ title: '境界ポートを追加', submit: '追加', text: '内部Controllerと境界input/output、2本の内部接続を一括で追加します。この複合型を外側からBusに接続できます。', fields: [nameField('child_name', '内部Controller名', 'port1'), nameField('input_gate', '境界input gate名', 'receive_port1'), nameField('output_gate', '境界output gate名', 'send_port1')] });
      if (result) await perform('add_port', { type_key: app.typeKey, ...result, position: newPosition() }, { structural: true });
    }
    async function projectSettings() {
      const snapshot = await prepareSettings();
      const settings = app.view.project_settings || {};
      const result = await V.dialog({ title: 'ネットワーク設定', submit: '反映', text: '時間は単位を付けて指定します（例: 10ms）。イベント数とdelta cycle数は整数です。保存時に全体を検証します。', fields: [
        { id: 'sim_time_limit', label: 'シミュレーション時間', value: settings.sim_time_limit || '10ms', required: true },
        { id: 'metrics_window', label: 'メトリクス集計幅', value: settings.metrics_window || '1ms', required: true },
        { id: 'max_events', label: '最大イベント数', value: settings.max_events || '1000000', required: true, pattern: '[0-9]+' },
        { id: 'max_delta_cycles', label: '最大delta cycle数', value: settings.max_delta_cycles || '1000', required: true, pattern: '[0-9]+' }
      ], validate: () => assertSettingsSnapshot(app, client, snapshot) });
      if (result) await perform('set_project_settings', result, { configuration: true, snapshot });
    }
    function configurationReadable() {
      if (app.view.configuration_errors?.length) throw new Error('関連JSONを読み取れません。ソースと診断を修正してから設定を開いてください。');
    }
    async function gatewaySettings() {
      const snapshot = await prepareSettings();
      configurationReadable();
      const nodes = gatewayNodeChoices(app.view);
      const owner = app.instancePath || (app.view.instances || []).find(i => i.type_key === app.typeKey && i.kind === 'network')?.path;
      const selectedChild = app.selection?.kind === 'node' && owner ? owner + '.' + app.selection.item.name : null;
      const initialNode = nodes.some(i => i.value === selectedChild) ? selectedChild : nodes.some(i => i.value === app.instancePath) ? app.instancePath : nodes[0]?.value;
      const actionsFor = node => (nodes.find(i => i.value === node)?.available ? [{ value: 'set', label: 'Gateway設定を作成・編集' }] : []).concat({ value: 'delete', label: 'Gateway設定を解除' });
      const validateNode = form => {
        assertSettingsSnapshot(app, client, snapshot);
        if (!nodes.some(n => n.value === form.node)) throw new Error('対象のGatewayを選択してください。');
        if (form.action !== 'delete' && !nodes.find(n => n.value === form.node)?.available) throw new Error('配置がないGatewayは設定解除のみ実行できます。');
      };
      const chosen = await V.dialog({ title: 'Gatewayを選択', fields: [
        { id: 'node', label: '複合モジュールのインスタンス', choices: nodes, value: initialNode },
        { id: 'action', label: '操作', choices: actionsFor(initialNode) }
      ], setup: () => {
        $('node').addEventListener('change', () => {
          const choices = actionsFor($('node').value), current = $('action').value;
          V.options($('action'), choices, choices.some(c => c.value === current) ? current : choices[0]?.value);
        });
      }, validate: validateNode });
      if (!chosen) return;
      validateNode(chosen);
      if (chosen.action === 'delete') { await perform('delete_gateway', { node: chosen.node }, { configuration: true, snapshot }); return; }
      const settings = (app.view.gateway_settings || []).find(g => g.node === chosen.node) || {};
      const ports = gatewayPortChoices(app.view, chosen.node, settings);
      const selectedPorts = () => ports.filter(p => [...$('ports').querySelectorAll('input:checked')].some(input => input.value === p.value));
      const result = await V.dialog({ title: 'Gateway設定', submit: '反映', text: '内部Controllerをポートとして選択し、入口から1つ以上の出口へのルートを設定します。配置がない既存ポートも保持します。解除する場合はチェックを外し、関連ルートも編集してください。CAN ID範囲は10進数です。', fields: [
        { id: 'ports', label: 'Gatewayポート', checkboxes: ports, value: settings.ports || ports.map(p => p.value) },
        { id: 'processing_delay', label: '処理遅延', value: settings.processing_delay || '0ps', required: true },
        { id: 'hop_limit', label: 'hop limit', value: String(settings.hop_limit ?? 16), required: true, pattern: '[0-9]+' },
        { id: 'rx_queue_capacity', label: 'RXキュー容量', value: String(settings.rx_queue_capacity ?? 64), required: true, pattern: '[0-9]+' }
      ], sections: [{ id: 'routes', label: '転送ルート', rowLabel: 'ルート', rows: settings.routes || [], defaultRow: { format: 'standard', id_min: '0', id_max: '2047', egress: [] }, addLabel: 'ルートを追加', fields: [
        { id: 'id', label: 'ルートID', required: true }, { id: 'ingress', label: '入口ポート', choices: selectedPorts }, { id: 'egress', label: '出口ポート（複数選択可）', checkboxes: selectedPorts },
        { id: 'format', label: 'フレーム形式', choices: [{ value: 'standard', label: 'standard (11 bit)' }, { value: 'extended', label: 'extended (29 bit)' }] },
        { id: 'id_min', label: 'CAN ID下限', required: true, pattern: '[0-9]+' }, { id: 'id_max', label: 'CAN ID上限', required: true, pattern: '[0-9]+' }
      ] }], validate: form => { assertSettingsSnapshot(app, client, snapshot); gatewayPayload(chosen.node, form); } });
      if (result) await perform('set_gateway', gatewayPayload(chosen.node, result), { configuration: true, snapshot });
    }
    async function workloadSettings() {
      const snapshot = await prepareSettings();
      configurationReadable();
      const rows = (app.view.workload?.generators || []).map(g => ({ ...g, format: g.frame?.format, frame_id: String(g.frame?.id ?? 0), data: g.frame?.data || '', times: (g.times || []).join(', '), count: g.count == null ? '' : String(g.count), end: g.end || '', phase: g.phase || '0ps' }));
      const periodic = row => row.kind === 'can.periodic.v1', explicit = row => row.kind === 'can.explicit.v1';
      const result = await V.dialog({ title: '送信設定', submit: '反映', text: '送信定義を追加・編集・削除します。時間には単位を付け、CAN IDは10進数、データは16進数で入力します。空の回数・終了時刻は上限を指定しません。', sections: [{ id: 'generators', label: '送信定義', rowLabel: '送信定義', rows, addLabel: '送信定義を追加', defaultRow: { kind: 'can.periodic.v1', format: 'standard', frame_id: '256', data: '', start: '0ps', phase: '0ps', period: '1ms', end: '', count: '10', times: '0ps' }, fields: [
        { id: 'id', label: '送信定義ID', required: true }, { id: 'kind', label: '送信形式', choices: [{ value: 'can.periodic.v1', label: '周期送信' }, { value: 'can.explicit.v1', label: '時刻を明示' }] },
        { id: 'node', label: '送信Controller', choices: controllerChoices(app.view) },
        { id: 'format', label: 'フレーム形式', choices: [{ value: 'standard', label: 'standard (11 bit)' }, { value: 'extended', label: 'extended (29 bit)' }] },
        { id: 'frame_id', label: 'CAN ID', required: true, pattern: '[0-9]+' }, { id: 'data', label: 'データ（16進数・最大8バイト）', placeholder: '01020304' },
        { id: 'times', label: '送信時刻（カンマ区切り・空欄で送信なし）', when: explicit },
        { id: 'start', label: '開始時刻', required: true, when: periodic }, { id: 'phase', label: '位相', when: periodic }, { id: 'period', label: '周期', required: true, when: periodic },
        { id: 'end', label: '終了時刻（省略可）', when: periodic }, { id: 'count', label: '送信回数（省略可）', pattern: '[0-9]*', when: periodic }
      ] }], validate: form => { assertSettingsSnapshot(app, client, snapshot); workloadPayload(form); } });
      if (result) await perform('set_workload', workloadPayload(result), { configuration: true, snapshot });
    }
    async function instanceParameter(parameter) {
      const targetPath = app.instancePath;
      const snapshot = await prepareSettings();
      if (app.instancePath !== targetPath) throw new Error('選択対象が変更されました。インスタンスを選択して設定を開き直してください。');
      if (!app.instancePath || parameter.instance_path !== app.instancePath) throw new Error('設定するインスタンスを階層から選択してください。');
      const latest = (app.view.parameters || []).find(p => p.name === parameter.name && p.instance_path === targetPath);
      if (!latest) throw new Error('最新のパラメータを取得できません。インスタンスを選択して設定を開き直してください。');
      const result = await V.dialog({ title: latest.name + ' の実体値', submit: '反映', text: 'INIの選択インスタンスに値を設定します。空欄にすると上書きを解除し、型のdefaultを使用します。', fields: [{ id: 'literal', label: '値（空欄で解除）', value: latest.override ?? latest.default ?? '' }], validate: () => assertSettingsSnapshot(app, client, snapshot) });
      if (result) await perform('set_instance_parameter', { instance_path: targetPath, parameter_name: latest.name, literal: result.literal || null }, { configuration: true, snapshot });
    }
    async function addChild(typeName) {
      const result = await V.dialog({ title: '子を追加', submit: '追加', fields: [
        { id: 'child-type', label: '配置する型', choices: placementChoices(app.view), value: typeName },
        { id: 'child-name', label: '子の名前', required: true, placeholder: 'controller2' }
      ] });
      if (!result) return;
      const nodes = graph()?.nodes || [], occupied = new Set(nodes.map(n => `${n.x},${n.y}`));
      let slot = 0; while (occupied.has(`${80 + slot % 4 * 240},${100 + Math.floor(slot / 4) * 160}`)) slot++;
      await perform('add_child', { parent_type: app.typeKey, child_name: result['child-name'], type_name: result['child-type'], position: { x: 80 + slot % 4 * 240, y: 100 + Math.floor(slot / 4) * 160 } }, { structural: true });
    }
    function endpointChoices(start) {
      return V.ports(graph()).filter(p => p.start === start).map(p => ({ value: p.endpoint, label: p.endpoint + ' (' + (p.output ? 'output' : 'input') + (p.own ? '・境界' : '') + ')' }));
    }
    async function connect(channel) {
      const result = await V.dialog({ title: 'gateを接続', submit: '接続', text: '境界gateの方向は、型内部から見た始点・終点として表示しています。', fields: [
        { id: 'connect-from', label: '始点', choices: endpointChoices(true), value: app.selection?.kind === 'port' && app.selection.port.start ? app.selection.port.endpoint : undefined },
        { id: 'connect-to', label: '終点', choices: endpointChoices(false), value: app.selection?.kind === 'port' && !app.selection.port.start ? app.selection.port.endpoint : undefined },
        { id: 'connect-channel', label: 'channel（省略可）', suggestions: channelChoices(app.view), value: channel }
      ] });
      if (result) {
        const payload = { parent_type: app.typeKey, from: result['connect-from'], to: result['connect-to'] };
        if (result['connect-channel']) payload.channel = result['connect-channel'];
        await perform('connect', payload, { structural: true });
      }
    }
    async function reconnect(connection) {
      const result = await V.dialog({ title: '接続の端点を変更', submit: '変更', fields: [
        { id: 'endpoint-side', label: '変更する側', choices: [{ value: 'start', label: '始点: ' + connection.start }, { value: 'end', label: '終点: ' + connection.end }] },
        { id: 'new-endpoint', label: '新しい端点', choices: V.ports(graph()).map(p => ({ value: p.endpoint, label: p.endpoint + (p.start ? '（始点）' : '（終点）') })) },
        { id: 'reconnect-channel', label: 'channel（空欄で解除）', value: connection.channel || '', suggestions: channelChoices(app.view) }
      ] });
      if (result) await perform('reconnect', { connection_key: connection.key, endpoint_side: result['endpoint-side'], new_endpoint: result['new-endpoint'], channel: result['reconnect-channel'] || null }, { structural: true });
    }
    async function canPair() {
      const nodes = graph()?.nodes || [];
      // Roles come from implementation metadata and available projected gates.
      const { controllers, buses } = canPairCandidates(graph());
      const result = await V.dialog({ title: 'CAN TX/RXを一組で接続', submit: '一組を接続', text: 'ControllerとBus、およびBusのinput/output gateを明示的に選択してください。gateは増設しません。', fields: [
        { id: 'can-controller', label: 'Controller', choices: controllers.map(n => ({ value: n.name, label: n.name })) },
        { id: 'can-bus', label: 'Bus', choices: buses.map(n => ({ value: n.name, label: n.name })) },
        { id: 'can-input', label: 'Bus input gate', choices: [] }, { id: 'can-output', label: 'Bus output gate', choices: [] },
        { id: 'can-tx-channel', label: 'TX channel（省略可）', suggestions: channelChoices(app.view) },
        { id: 'can-rx-channel', label: 'RX channel（省略可）', suggestions: channelChoices(app.view) }
      ], setup: () => {
        const update = () => {
          const bus = nodes.find(n => n.name === $('can-bus').value), used = new Set((graph().connections || []).flatMap(c => [c.start, c.end]));
          V.options($('can-input'), (bus?.gates || []).filter(g => !g.output && !used.has(bus.name + '.' + g.name)).map(g => ({ value: g.name, label: g.name })));
          V.options($('can-output'), (bus?.gates || []).filter(g => g.output && !used.has(bus.name + '.' + g.name)).map(g => ({ value: g.name, label: g.name })));
        };
        $('can-bus').addEventListener('change', update); update();
      } });
      if (!result) return;
      const payload = { parent_type: app.typeKey, controller: result['can-controller'], bus: result['can-bus'], input_gate: result['can-input'], output_gate: result['can-output'] };
      if (result['can-tx-channel']) payload.tx_channel = result['can-tx-channel'];
      if (result['can-rx-channel']) payload.rx_channel = result['can-rx-channel'];
      await perform('connect_can_pair', payload, { structural: true });
    }
    async function deleteSelection() {
      const selection = app.selection;
      if (selection?.kind === 'node') await perform('delete_child', { element_key: selection.item.key }, { structural: true });
      if (selection?.kind === 'connection') await perform('disconnect', { connection_key: selection.item.key }, { structural: true });
    }
    async function setDefault(parameter) {
      const result = await V.dialog({ title: parameter.name + ' のdefault', submit: '反映', text: 'NEDの字句をそのまま入力します。空欄はdefaultを解除します。', fields: [{ id: 'default-literal', label: 'default字句', value: parameter.default ?? '' }] });
      if (result) await perform('set_default', { parameter_key: parameter.key, literal: result['default-literal'] === '' ? null : result['default-literal'] });
    }
    async function align() {
      const positions = {};
      (graph()?.nodes || []).forEach((n, i) => { positions[n.name] = { x: 80 + i % 4 * 240, y: 100 + Math.floor(i / 4) * 160, collapsed: !!n.collapsed }; });
      await perform('set_layout', { type_key: app.typeKey, positions });
    }
    async function gotoDiagnostic(diagnostic) {
      if (!diagnostic.file_id) return;
      if (diagnostic.file_id !== app.fileId) await navigate(diagnostic.file_id, null, []);
      const b = buffer();
      if (!b || b.pending || (diagnostic.source_hash && diagnostic.source_hash !== b.hash) || String(diagnostic.input_revision) !== String(app.view.input_revision)) {
        notice('診断の位置は現在の原文に対応していません。入力を反映し、再検証してください。'); return;
      }
      if (diagnostic.start_byte !== undefined && diagnostic.start_byte !== null) {
        const start = byteToCaret(b.raw, diagnostic.start_byte), end = byteToCaret(b.raw, diagnostic.end_byte ?? diagnostic.start_byte, true);
        $('source-editor').focus(); $('source-editor').setSelectionRange(start, end);
        const line = b.visible.slice(0, start).split('\n').length;
        $('source-editor').scrollTop = Math.max(0, (line - 4) * 20); caret();
      }
    }
    function caret() {
      const source = $('source-editor'), before = source.value.slice(0, source.selectionStart), lines = before.split('\n');
      $('caret-position').textContent = '行 ' + lines.length + ' · 列 ' + (lines[lines.length - 1].length + 1);
    }
    const safe = fn => (...args) => Promise.resolve().then(() => fn(...args)).catch(error => { notice(errorMessage(error)); render(); });
    const callbacks = {
      file: safe(file => navigate(file.id, null, [])),
      type: safe(type => navigate(type.file_id, type.key, [])),
      instance: safe(instance => navigateInstance(instance)),
      select: selection => { app.selection = selection; render(); },
      openNode: safe(node => {
        if (app.scopeLoading || !graph()) { notice('選択した対象の読込完了を待ってください。'); return; }
        const trail = [...app.breadcrumbs, { fileId: app.fileId, typeKey: app.typeKey, instancePath: app.instancePath, name: app.instancePath || graph().type_name || app.typeKey }];
        if (app.instancePath) {
          if (app.scopeLoading || graph()?.stale || V.instanceStructuralBlocked(app)) { notice('現在の階層を展開できません。インスタンス一覧から選択してください。'); return; }
          const instance = childInstance(app.view, app.instancePath, node);
          if (instance) return navigateInstance(instance, trail);
          notice('対応する子インスタンスを確認できません。ソースと診断を確認してください。'); return;
        }
        const type = typeForNode(node);
        if (type && !node.unresolved) return navigate(type.file_id, type.key, trail);
        notice('この型は未解決です。ソースと診断を確認してください。');
      }),
      breadcrumb: safe(index => { const item = app.breadcrumbs[index]; return navigate(item.fileId, item.typeKey, app.breadcrumbs.slice(0, index), item.instancePath || null); }),
      addChild: safe(addChild), connect: safe(connect), reconnect: safe(reconnect), deleteSelection: safe(deleteSelection), setDefault: safe(setDefault), instanceParameter: safe(instanceParameter),
      diagnostic: safe(gotoDiagnostic), recover: safe(async (record, action) => {
        const answer = await V.dialog({ title: action === 'complete' ? '保存を完了' : '保存前へ戻す', submit: '復旧を実行', text: (action === 'complete' ? '保存途中のプロジェクトを完成させます。' : '保存による変更を、保存前の状態へ戻します。') + '\n対象: ' + record.path });
        if (answer) return perform('recover', { recovery_id: record.id, action }, { recovery: true });
      }),
      layout: safe(positions => perform('set_layout', { type_key: app.typeKey, positions })),
      gesture: active => { app.gesture = active; },
      collapse: safe(node => perform('set_layout', { type_key: app.typeKey, positions: { [node.name]: { x: node.x, y: node.y, collapsed: !node.collapsed } } })),
      canEditGraph: () => client.writer && !app.scopeLoading && !app.operation && !client.inflight && !graph()?.stale && !V.instanceStructuralBlocked(app) && !localPending() && !V.recoveryRequired(app.view)
    };
    $('notice-close').onclick = () => notice('');
    $('refresh').onclick = safe(refresh);
    $('writer-claim').onclick = safe(async () => {
      const answer = await V.dialog({ title: 'このタブで編集', submit: '編集権限を取得', text: '他のタブは参照専用になります。このタブのローカル入力は保持します。' });
      if (!answer) return;
      const result = await api.request('/api/writer', { action: 'claim', client_id: client.clientId, expected_writer_epoch: client.writerEpoch });
      client.register(result); await refresh(); debounce();
    });
    $('undo').onclick = safe(() => perform('undo', {})); $('redo').onclick = safe(() => perform('redo', {}));
    $('validate').onclick = safe(() => perform('validate', {})); $('save-as').onclick = safe(saveAs);
    $('new-project').onclick = safe(newProject);
    $('project-settings').onclick = safe(projectSettings); $('gateway-settings').onclick = safe(gatewaySettings); $('workload-settings').onclick = safe(workloadSettings);
    $('create-module').onclick = safe(createModule); $('add-gates').onclick = safe(addGates); $('delete-gate').onclick = safe(deleteGate); $('add-port').onclick = safe(addPort);
    $('overwrite').onclick = safe(overwrite); $('reload').onclick = safe(() => perform('reload', {}));
    $('flush-source').onclick = safe(flushAll); $('check-command').onclick = safe(queryCommand); $('retry-input').onclick = safe(retryInput);
    $('add-child').onclick = safe(() => addChild()); $('connect').onclick = safe(() => connect()); $('can-pair').onclick = safe(canPair);
    $('delete-selection').onclick = safe(deleteSelection); $('align').onclick = safe(align);
    $('zoom-in').onclick = () => V.zoom(1.2); $('zoom-out').onclick = () => V.zoom(1 / 1.2); $('zoom-fit').onclick = () => V.fit();
    $('type-filter').oninput = event => { app.filter = event.target.value; render(); };
    $('instance-filter').oninput = event => { app.instanceFilter = event.target.value; render(); };
    const source = $('source-editor');
    source.addEventListener('keydown', event => {
      if (event.key === 'Tab' && !event.ctrlKey && !event.metaKey && !event.altKey && !event.shiftKey && !event.isComposing && !source.disabled && !source.readOnly) {
        event.preventDefault(); source.setRangeText('    ', source.selectionStart, source.selectionEnd, 'end');
        source.dispatchEvent(new Event('input', { bubbles: true }));
      }
    });
    source.addEventListener('input', () => {
      const b = buffer(); if (!b) return;
      // IME may temporarily expose half a surrogate. Keep its DOM composition until completion.
      if (b.composing && !validUnicode(source.value)) return;
      try { b.edit(source.value); render(); caret(); debounce(); } catch (error) { notice(errorMessage(error)); }
    });
    source.addEventListener('compositionstart', () => { buffer()?.composition(true); clearTimeout(app.timer); render(); });
    source.addEventListener('compositionend', () => {
      const b = buffer(); if (!b) return;
      try { b.edit(source.value); } catch (error) { notice(errorMessage(error)); }
      b.composition(false); render(); debounce();
    });
    source.addEventListener('select', caret); source.addEventListener('keyup', caret); source.addEventListener('click', caret);
    source.addEventListener('beforeinput', event => {
      if (['historyUndo', 'historyRedo'].includes(event.inputType)) { event.preventDefault(); safe(() => perform(event.inputType === 'historyUndo' ? 'undo' : 'redo', {}))(); }
    });
    document.addEventListener('keydown', event => {
      if ($('editor-dialog').open || event.isComposing || buffer()?.composing) return;
      if ((event.ctrlKey || event.metaKey) && !event.altKey) {
        const key = event.key.toLowerCase();
        if (key === 's') { event.preventDefault(); safe(saveAs)(); }
        if (key === 'z' || key === 'y') { event.preventDefault(); safe(() => perform(key === 'y' || event.shiftKey ? 'redo' : 'undo', {}))(); }
      }
      if (event.key === 'Delete' && document.activeElement === $('graph')) { event.preventDefault(); safe(deleteSelection)(); }
      if (event.key === 'Escape') { app.selection = null; render(); }
    });
    root.addEventListener('beforeunload', event => {
      if (localPending() || app.view?.dirty || client.inflight) { event.preventDefault(); event.returnValue = ''; }
    });
    // Same-revision parse completion can change the projection. Poll read-only state without changing scope.
    const poll = async () => {
      try {
        if (!document.hidden && !app.refreshBusy && !app.gesture) { app.refreshBusy = true; await refresh(); }
      } catch (_) { /* command failures are surfaced immediately; passive polling is quiet */ }
      finally { app.refreshBusy = false; root.setTimeout(poll, 1800); }
    };
    if (!secret) { notice('起動時に表示されたURLを開いてください。接続情報がありません。'); return; }
    try {
      client.register(await api.request('/api/writer', { action: 'register' }));
      await refresh(); poll();
    } catch (error) { notice(errorMessage(error)); render(); }
  }
  return { MAX_BODY, utf8Length, validUnicode, shadow, visiblePatch, byteToCaret, TextBuffer, sourceHash, acknowledgeSource, encodeBody, ApiError, Api, CommandClient, succeeded, acceptView, sessionQuery, instanceScope, childInstance, resetProjectScope, placementChoices, channelChoices, canPairCandidates, controllerChoices, gatewayNodeChoices, gatewayPortChoices, gatewayPayload, workloadPayload, gatePayload, settingsSnapshot, assertSettingsSnapshot, start };
});
