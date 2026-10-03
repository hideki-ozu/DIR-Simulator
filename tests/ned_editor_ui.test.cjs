'use strict';
const test = require('node:test');
const assert = require('node:assert/strict');
const I = require('../crates/dir-simulator/src/tool/ned-editor/assets/input.js');
const V = require('../crates/dir-simulator/src/tool/ned-editor/assets/view.js');

test('LF shadow maps BOM, Japanese, emoji and CRLF using byte/codepoint boundaries', () => {
  const raw = '\ufeff日😀\r\nA\nB\rC';
  const shadow = I.shadow(raw);
  assert.equal(shadow.visible, '日😀\nA\nB\nC');
  assert.deepEqual(shadow.boundaries.slice(0, 4), [
    { visible: 0, raw: 1, byte: 3 }, { visible: 1, raw: 2, byte: 6 },
    { visible: 3, raw: 4, byte: 10 }, { visible: 4, raw: 6, byte: 12 }
  ]);
  assert.equal(I.byteToCaret(raw, '0'), 0);
  assert.equal(I.byteToCaret(raw, '6'), 1);
  assert.equal(I.byteToCaret(raw, '8'), 1);
  assert.equal(I.byteToCaret(raw, '8', true), 3);
  assert.equal(I.byteToCaret(raw, '11'), 3);
  assert.equal(I.byteToCaret(raw, '12'), 4);
  assert.equal(I.byteToCaret(raw, '9999999999999999999'), shadow.visible.length);
});

test('local patch retains BOM, comments and every unmodified mixed newline', () => {
  const raw = '\ufeff// 日😀\r\nmodule Main {\n    // comment\r\n}\r';
  const next = I.shadow(raw).visible.replace('Main', 'Main2');
  assert.equal(I.visiblePatch(raw, next), raw.replace('Main', 'Main2'));
  assert.equal(I.visiblePatch(raw, I.shadow(raw).visible), raw);
  assert.equal(I.visiblePatch('\ufeff', 'hello'), '\ufeffhello');
});

test('emoji replacement expands a common surrogate prefix to complete code points', () => {
  assert.equal(I.visiblePatch('\ufeff日😀\r\n末尾\n', '日😃\n末尾\n'), '\ufeff日😃\r\n末尾\n');
  assert.equal(I.visiblePatch('a😀z', 'a🦊z'), 'a🦊z');
  assert.equal(I.visiblePatch('a😀z', 'az'), 'az');
  assert.throws(() => I.visiblePatch('abc', 'a\ud800c'), /文字変換/);
});

test('newlines inserted in an interval use its nearest original newline', () => {
  assert.equal(I.visiblePatch('a\r\nb\nc\r\n', 'a\nb\nnew\nc\n'), 'a\r\nb\nnew\nc\r\n');
  assert.equal(I.visiblePatch('a\r\nb', 'a\nx\nb'), 'a\r\nx\r\nb');
  assert.equal(I.visiblePatch('abc', 'a\nbc'), 'a\nbc');
  assert.equal(I.visiblePatch('a\rb', 'a\nx\nb'), 'a\rx\rb');
});

test('acknowledgement updates the baseline without overwriting newer typing', () => {
  const b = new I.TextBuffer('file-a', '\ufeffold\r\n', 'h1');
  b.edit('new\n'); const sent = b.snapshot(); b.sent = sent;
  b.edit('newest\n');
  assert.equal(b.adopt('server\n', 'h2'), false);
  b.acknowledge(sent, 'h2');
  assert.equal(b.raw, '\ufeffnewest\r\n'); assert.equal(b.hash, 'h2'); assert.equal(b.pending, true);
  assert.equal(b.snapshot().hash, 'h2'); assert.equal(b.ackRaw, '\ufeffnew\r\n');
  const last = b.snapshot(); b.acknowledge(last, 'h3');
  assert.equal(b.pending, false); assert.equal(b.adopt('undo\r\n', 'h4'), true);
  assert.equal(b.visible, 'undo\n');
});

test('blocked buffer preserves unsent text, and IME end explicitly releases waiters', async () => {
  const b = new I.TextBuffer('f', 'module X {}', 'hash');
  b.edit('module X {'); b.blocked = 'syntax error';
  assert.equal(b.adopt('server', 'other'), false); assert.equal(b.raw, 'module X {');
  b.composition(true); let ended = false;
  const wait = b.waitComposition().then(() => { ended = true; });
  await Promise.resolve(); assert.equal(ended, false); b.composition(false); await wait;
  assert.equal(ended, true); assert.equal(b.pending, true);
});

test('JSON body limit counts encoded UTF-8 and escapes, not UTF-16 length', () => {
  const body = { text: '日😀\n"\\' };
  const encoded = JSON.stringify(body), bytes = Buffer.byteLength(encoded);
  assert.equal(I.utf8Length(encoded), bytes);
  assert.equal(I.encodeBody(body, bytes), encoded);
  assert.throws(() => I.encodeBody(body, bytes - 1), /通信上限/);
  assert.equal(I.MAX_BODY, 67108864);
});

test('API keeps credentials only in the required header and supplies JSON content type', async () => {
  let request;
  const api = new I.Api('fixture-secret', async (url, options) => {
    request = { url, options }; return { ok: true, status: 200, json: async () => ({ done: true }) };
  });
  assert.deepEqual(await api.request('/api/commands', { kind: 'undo' }), { done: true });
  assert.equal(request.url, '/api/commands');
  assert.equal(request.options.headers['X-Editor-Session'], 'fixture-secret');
  assert.equal(request.options.headers['Content-Type'], 'application/json');
  assert.equal(request.options.cache, 'no-store'); assert.equal(request.options.method, 'POST');
  assert.equal(request.options.body, '{"kind":"undo"}');
});

function client(api) {
  const c = new I.CommandClient(api); c.sessionId = 'session';
  c.register({ client_id: 'client', writer_epoch: '9007199254740993', next_sequence: '9007199254740994', writer: true });
  c.revision = '9007199254740995'; return c;
}
function terminal(command, extra = {}) {
  return { schema_version: 1, session_id: 'session', command_id: command.command_id, accepted: true, terminal: true, operation_status: 200, current_revision: '9007199254740996', sequence_consumed: true, next_sequence: '9007199254740995', view_sequence: '9', ...extra };
}

test('command DTO preserves all u64 values as strings and forwards opaque acknowledgement', async () => {
  let posted;
  const c = client({ request: async (url, body) => { posted = body; return terminal(body); } });
  await c.send('add_child', { parent_type: 't', child_name: 'n', type_name: 'Controller', position: { x: 80, y: 100 } }, 'opaque');
  assert.deepEqual(posted, {
    schema_version: 1, session_id: 'session', client_id: 'client', writer_epoch: '9007199254740993', client_sequence: '9007199254740994', command_id: 'client:9007199254740994', base_revision: '9007199254740995', kind: 'add_child', payload: { parent_type: 't', child_name: 'n', type_name: 'Controller', position: { x: 80, y: 100 } }, impact_ack: 'opaque'
  });
  assert.equal(c.nextSequence, '9007199254740995'); assert.equal(c.revision, '9007199254740996');
});

test('202 reserves sequence, blocks the next command and waits for a terminal job', async () => {
  let finish, posted, jobSeen;
  const jobRequested = new Promise(resolve => { jobSeen = resolve; });
  const c = client({ request: async (url, body) => {
    if (url === '/api/commands') { posted = body; return terminal(body, { terminal: false, operation_status: 202, job_id: 'job', next_sequence: '1000000000000000000' }); }
    assert.equal(url, '/api/jobs/job'); jobSeen(); return new Promise(resolve => { finish = resolve; });
  } });
  const first = c.send('validate', {}); await jobRequested;
  assert.equal(c.nextSequence, '9007199254740994');
  await assert.rejects(c.send('undo', {}), /前の操作/);
  finish(terminal(posted)); await first; assert.equal(c.nextSequence, '9007199254740995');
});

test('consumed terminal rejection advances sequence and retains command without retransmitting', async () => {
  let requests = 0;
  const c = client({ request: async (url, body) => {
    requests++; throw new I.ApiError('conflict', 409, terminal(body, { operation_status: 409, code: 'E-EDITOR-CONFLICT', message: '版競合' }));
  } });
  await assert.rejects(c.send('replace_source', { file_id: 'f', expected_hash: 'h', text: 'local' }), /版競合/);
  assert.equal(requests, 1); assert.equal(c.nextSequence, '9007199254740995');
  assert.equal(c.inflight, null); assert.equal(c.uncertain, null);
  assert.equal(c.lastCommand.payload.text, 'local');
});

test('unconsumed terminal rejection does not advance sequence', async () => {
  const c = client({ request: async (url, body) => terminal(body, { accepted: false, operation_status: 503, code: 'E-EDITOR-BUSY', sequence_consumed: false }) });
  await assert.rejects(c.send('validate', {})); assert.equal(c.nextSequence, '9007199254740994');
});

test('uncertain response retains exact command and resolves through same-ID lookup only', async () => {
  const calls = []; let posted;
  const c = client({ request: async (url, body) => {
    calls.push(url);
    if (body) { posted = body; throw new Error('connection closed'); }
    assert.equal(url, '/api/commands/client%3A9007199254740994'); return terminal(posted);
  } });
  await assert.rejects(c.send('save_as_project', { destination_id: 'new' }));
  assert.equal(c.uncertain, posted); assert.equal(c.nextSequence, '9007199254740994');
  await assert.rejects(c.send('save_as_project', { destination_id: 'new' }), /前の操作/);
  await c.query(); assert.equal(c.uncertain, null); assert.equal(calls.length, 2);
  assert.equal(calls.filter(url => url === '/api/commands').length, 1);
});

test('oversized command remains a local error, never an uncertain transmission', async () => {
  let calls = 0;
  const c = client({ bodyLimit: 1, request: async () => { calls++; } });
  await assert.rejects(c.send('replace_source', { text: 'large' }), /通信上限/);
  assert.equal(c.inflight, null); assert.equal(c.uncertain, null); assert.equal(calls, 0);
});

test('read-only clients cannot send mutation commands', async () => {
  const c = client({ request: async () => { throw new Error('must not send'); } }); c.writer = false;
  await assert.rejects(c.send('undo', {}), /参照専用/);
});

test('view adoption rejects old scope, session and sequences with exact u64 comparison', () => {
  const current = { schema_version: 1, session_id: 's', request_scope_generation: '3', view_sequence: '9007199254740995' };
  assert.equal(I.acceptView(current, { ...current, view_sequence: '9007199254740994' }, '3', 's'), false);
  assert.equal(I.acceptView(current, { ...current, request_scope_generation: '2', view_sequence: '9007199254740999' }, '3', 's'), false);
  assert.equal(I.acceptView(current, { ...current, session_id: 'old' }, '3', 's'), false);
  assert.equal(I.acceptView(current, { ...current }, '3', 's'), true);
  assert.equal(I.acceptView(current, { ...current, view_sequence: '9007199254740996' }, '3', 's'), true);
});

const graph = { own_gates: [{ key: 'own-in', name: 'in', output: false }, { key: 'own-out', name: 'out', output: true }], nodes: [{ key: 'n', name: 'node', type_name: 'Simple', x: 82, y: 103, gates: [{ key: 'child-in', name: 'rx', output: false }, { key: 'child-out', name: 'tx', output: true }] }] };
test('port roles reverse only at compound boundaries without changing input/output labels', () => {
  const ports = V.ports(graph);
  assert.equal(ports.find(p => p.key === 'own-in').start, true);
  assert.equal(ports.find(p => p.key === 'own-in').output, false);
  assert.equal(ports.find(p => p.key === 'own-out').start, false);
  assert.equal(ports.find(p => p.key === 'child-in').start, false);
  assert.equal(ports.find(p => p.key === 'child-out').start, true);
  assert.equal(ports.find(p => p.key === 'child-out').endpoint, 'node.tx');
  assert.equal(V.findPort(ports, 'node.tx').key, 'child-out');
  assert.equal(V.findPort(ports, 'own-in').endpoint, 'in');
});

test('display geometry preserves backend coordinates and unresolved nodes have no invented gates', () => {
  const geometry = V.nodeGeometry(graph.nodes[0]); assert.equal(geometry.x, 82); assert.equal(geometry.y, 103);
  assert.deepEqual(V.ports({ nodes: [{ key: 'unknown', name: 'x', type_name: 'Missing', unresolved: true, x: 3, y: 4 }], own_gates: [] }), []);
});

test('pending text takes display priority over a past ready state; recovery comes first', () => {
  const b = new I.TextBuffer('f', 'ok', 'h'), app = { view: { analysis: 'ready', files: [], recovery: [] }, buffers: new Map([['f', b]]) };
  assert.equal(V.displayState(app), '実行準備OK'); b.edit('new');
  assert.equal(V.displayState(app), '未検証の入力あり'); app.operation = '上書き保存';
  assert.equal(V.displayState(app), '上書き保存…'); app.view.recovery = [{ state: 'required' }];
  assert.equal(V.displayState(app), '保存の復旧待ち');
});

const hierarchy = {
  types: [{ key: 'main', file_id: 'root-file', name: 'Main', kind: 'network' }, { key: 'group', file_id: 'group-file', name: 'Group', kind: 'module' }, { key: 'controller', file_id: 'controller-file', name: 'Controller', kind: 'simple' }],
  instances: [
    { path: 'Main', type_name: 'Main', type_key: 'main', kind: 'network', depth: 0 },
    { path: 'Main.a', type_name: 'Group', type_key: 'group', kind: 'module', depth: 1 },
    { path: 'Main.a.ctrl', type_name: 'Controller', type_key: 'controller', kind: 'simple', depth: 2 },
    { path: 'Main.b', type_name: 'Group', type_key: 'group', kind: 'module', depth: 1 },
    { path: 'Main.b.ctrl', type_name: 'Controller', type_key: 'controller', kind: 'simple', depth: 2 },
    { path: 'Main.missing', type_name: 'Missing', unresolved: true, depth: 1 },
    { path: 'Main.loop', type_name: 'Main', type_key: 'main', kind: 'network', cycle: true, depth: 1 }
  ]
};

test('instance selection resolves the exact shared type key and sends its file, path and scope generation', () => {
  const scope = I.instanceScope(hierarchy, hierarchy.instances[4]);
  assert.deepEqual(scope, { fileId: 'controller-file', typeKey: 'controller', instancePath: 'Main.b.ctrl' });
  const query = I.sessionQuery({ ...scope, scopeGeneration: '9007199254740999' });
  assert.deepEqual(Object.fromEntries(query), { request_scope_generation: '9007199254740999', file_id: 'controller-file', type_key: 'controller', instance_path: 'Main.b.ctrl' });
  assert.equal(I.sessionQuery({ fileId: 'root-file', typeKey: 'main', scopeGeneration: '2', instancePath: null }).has('instance_path'), false);
  assert.equal(I.instanceScope(hierarchy, hierarchy.instances[5]), null);
  assert.equal(I.instanceScope(hierarchy, { ...hierarchy.instances[1], type_key: 'main' }), null);
  assert.equal(I.instanceScope(hierarchy, { ...hierarchy.instances[1], type_key: 'gone' }), null);
});

test('node navigation keeps the current instance branch and rejects absent, unresolved and mismatched children', () => {
  const node = { name: 'ctrl', type_name: 'Controller' };
  assert.equal(I.childInstance(hierarchy, 'Main.b', node), hierarchy.instances[4]);
  assert.equal(I.childInstance(hierarchy, 'Main.a', node), hierarchy.instances[2]);
  assert.equal(I.childInstance(hierarchy, null, node), null);
  assert.equal(I.childInstance(hierarchy, 'Main.b', { ...node, type_name: 'Group' }), null);
  assert.equal(I.childInstance(hierarchy, 'Main.b', { ...node, unresolved: true }), null);
  assert.equal(I.childInstance(hierarchy, 'Main.gone', node), null);
});

test('hierarchy filtering retains ancestors and backend instance order without inventing children', () => {
  assert.deepEqual(V.filteredInstances(hierarchy.instances, ' MAIN.B.CTRL ').map(i => i.path), ['Main', 'Main.b', 'Main.b.ctrl']);
  assert.deepEqual(V.filteredInstances(hierarchy.instances, 'controller').map(i => i.path), ['Main', 'Main.a', 'Main.a.ctrl', 'Main.b', 'Main.b.ctrl']);
  assert.deepEqual(V.filteredInstances(hierarchy.instances, 'missing').map(i => i.path), ['Main', 'Main.missing']);
  assert.deepEqual(V.filteredInstances(hierarchy.instances, 'absent'), []);
  assert.equal(V.filteredInstances(hierarchy.instances, ''), hierarchy.instances);
  assert.deepEqual(V.filteredInstances([], ''), []);
});

test('cycle instances can be inspected but structural editing is disabled only in invalid selected contexts', () => {
  const app = { view: hierarchy, typeKey: 'main', instancePath: 'Main.loop' };
  assert.equal(I.instanceScope(hierarchy, hierarchy.instances[6]).instancePath, 'Main.loop');
  assert.equal(V.selectedInstance(app), hierarchy.instances[6]);
  assert.equal(V.instanceStructuralBlocked(app), true);
  app.instancePath = 'Main'; assert.equal(V.instanceStructuralBlocked(app), false);
  app.instancePath = 'Main.missing'; assert.equal(V.instanceStructuralBlocked(app), true);
  app.instancePath = 'Main.gone'; assert.equal(V.instanceStructuralBlocked(app), true);
  app.instancePath = null; assert.equal(V.instanceStructuralBlocked(app), false);
  assert.equal(V.instanceStructuralBlocked({ view: null, instancePath: null }), false);
});

test('parameter values require the selected instance, corresponding graph and current projection', () => {
  const b = new I.TextBuffer('controller-file', 'source', 'h');
  const app = { view: { ...hierarchy, graph: { type_key: 'controller' } }, buffers: new Map([['controller-file', b]]), instancePath: 'Main.b.ctrl', typeKey: 'controller', scopeLoading: false };
  const parameter = { instance_path: 'Main.b.ctrl', override: '17', effective: '17' };
  assert.deepEqual(V.parameterValues(app, parameter), ['17', '17']);
  assert.deepEqual(V.parameterValues(app, { ...parameter, override: null, effective: '0' }), [null, '0']);
  assert.deepEqual(V.parameterValues(app, { ...parameter, effective: null }), ['17', '実効値未取得']);
  assert.deepEqual(V.parameterValues(app, { ...parameter, instance_path: 'Main.a.ctrl' }), ['選択したインスタンスの値は未取得', '選択したインスタンスの値は未取得']);
  b.edit('new source'); assert.deepEqual(V.parameterValues(app, parameter), ['17', '未反映の入力あり']);
  app.scopeLoading = true; assert.deepEqual(V.parameterValues(app, parameter), ['読込中', '読込中']);
  app.scopeLoading = false; app.typeKey = 'main'; assert.equal(V.parameterValues(app, parameter)[0], '選択したインスタンスの値は未取得');
  app.instancePath = null; assert.deepEqual(V.parameterValues(app, parameter), ['インスタンスを選択してください', 'インスタンスを選択してください']);
});

test('browser callbacks navigate instances and breadcrumbs, clear context on type/file selection and retain source buffers', async () => {
  const elements = new Map();
  const document = { hidden: true, getElementById(id) {
    if (!elements.has(id)) elements.set(id, { value: '', hidden: false, addEventListener() {} });
    return elements.get(id);
  }, addEventListener() {} };
  let app, callbacks, sequence = 0, release;
  const requests = [], originalFetch = global.fetch;
  global.fetch = async (url) => {
    if (url === '/api/writer') return { ok: true, status: 200, json: async () => ({ client_id: 'client', writer_epoch: '1', next_sequence: '1', writer: true }) };
    const query = new URL(url, 'http://localhost').searchParams; requests.push(query);
    const type = hierarchy.types.find(t => t.key === query.get('type_key')) || hierarchy.types[0];
    const next = { schema_version: 1, session_id: 'session', revision: '1', input_revision: '1', view_sequence: String(++sequence), request_scope_generation: query.get('request_scope_generation'), ...hierarchy,
      files: hierarchy.types.map(t => ({ id: t.file_id, hash: 'hash' })), source: { file_id: type.file_id, text: 'source ' + type.name, hash: 'hash' }, graph: { type_key: type.key, type_name: type.name, nodes: [] } };
    if (query.get('instance_path') === 'Main.a.ctrl') await new Promise(resolve => { release = resolve; });
    return { ok: true, status: 200, json: async () => next };
  };
  const root = { document, location: { hash: '#session=fixture', pathname: '/', search: '' }, history: { replaceState() {} }, addEventListener() {}, setTimeout() {},
    NEDEditorView: { instanceStructuralBlocked: V.instanceStructuralBlocked, render(state, client, handlers) { app = state; callbacks = handlers; } } };
  try {
    await I.start(root);
    const buffer = app.buffers.get('root-file'); buffer.edit('unsent root source');
    await callbacks.instance(hierarchy.instances[0]);
    await callbacks.openNode({ name: 'b', type_name: 'Group' });
    assert.equal(app.instancePath, 'Main.b'); assert.equal(app.typeKey, 'group'); assert.equal(app.fileId, 'group-file');
    await callbacks.openNode({ name: 'ctrl', type_name: 'Controller' });
    assert.equal(app.instancePath, 'Main.b.ctrl'); assert.equal(requests.at(-1).get('instance_path'), 'Main.b.ctrl');
    assert.equal(app.breadcrumbs[1].instancePath, 'Main.b');
    await callbacks.breadcrumb(1); assert.equal(app.instancePath, 'Main.b');
    await callbacks.type(hierarchy.types[0]); assert.equal(app.instancePath, null); assert.equal(requests.at(-1).has('instance_path'), false);
    assert.equal(app.buffers.get('root-file'), buffer); assert.equal(buffer.raw, 'unsent root source'); assert.equal(buffer.pending, true);
    await callbacks.openNode({ name: 'b', type_name: 'Group' }); assert.equal(app.instancePath, null); assert.equal(app.typeKey, 'group');
    const older = callbacks.instance(hierarchy.instances[2]); await Promise.resolve(); await Promise.resolve();
    assert.equal(app.scopeLoading, true);
    await callbacks.instance(hierarchy.instances[4]);
    assert.equal(app.instancePath, 'Main.b.ctrl');
    release(); await older;
    assert.equal(app.instancePath, 'Main.b.ctrl'); assert.equal(app.view.request_scope_generation, app.scopeGeneration);
    await callbacks.file({ id: 'root-file' }); assert.equal(app.instancePath, null); assert.equal(requests.at(-1).has('instance_path'), false);
    assert.equal(buffer.raw, 'unsent root source');
    const count = requests.length; await callbacks.instance(hierarchy.instances[5]); assert.equal(requests.length, count);
  } finally { global.fetch = originalFetch; if (release) release(); }
});

test('source acknowledgement uses the committed raw hash and retains later typing on a cross-tab race', async () => {
  const b = new I.TextBuffer('file', 'original', 'original-hash');
  b.edit('generation 1 日本語'); const sent = b.snapshot(); b.sent = sent;
  b.edit('generation 2 日本語😀');
  const committedHash = await I.sourceHash(sent.raw);
  const otherHash = await I.sourceHash('other writer');
  assert.throws(() => I.acknowledgeSource(b, sent, committedHash, otherHash), /別のタブ/);
  assert.equal(b.hash, committedHash); assert.notEqual(b.hash, otherHash);
  assert.equal(b.ackRaw, sent.raw); assert.equal(b.raw, 'generation 2 日本語😀');
  assert.equal(b.pending, true); assert.ok(b.blocked);
});

test('default fetch is bound to the browser global instead of the Api instance', async () => {
  const original = globalThis.fetch;
  try {
    globalThis.fetch = function () { assert.equal(this, globalThis); return Promise.resolve({ok: true, json: async () => ({ready: true})}); };
    assert.deepEqual(await new I.Api('test-only').request('/api/session'), {ready: true});
  } finally { globalThis.fetch = original; }
});

const catalog = [
  { id: '@builtin:Controller', name: 'Controller', label: 'Controller', kind: 'simple' },
  { id: '@builtin:Bus', name: 'Bus', label: 'Bus', kind: 'simple' },
  { id: '@builtin:Gateway', name: 'Gateway', label: 'Gateway', kind: 'simple' },
  { id: '@builtin:Fanout', name: 'Fanout', label: 'Fanout', kind: 'simple' },
  { id: '@builtin:FixedDelay', name: 'FixedDelay', label: 'FixedDelay', kind: 'channel' }
];

test('builtin choices remain available without NED definitions and preserve colliding project names', () => {
  assert.deepEqual(I.placementChoices({ catalog }).map(c => c.value), catalog.slice(0, 4).map(t => t.id));
  const view = { catalog, types: [{ name: 'Controller', kind: 'simple' }, { name: 'FixedDelay', kind: 'channel' }] };
  const choices = I.placementChoices(view);
  assert.equal(choices[0].value, '@builtin:Controller'); assert.equal(choices.at(-1).value, 'Controller');
  assert.notEqual(choices[0].label, choices.at(-1).label);
  assert.deepEqual(I.channelChoices(view).map(c => c.value), ['@builtin:FixedDelay', 'FixedDelay']);
  assert.deepEqual(I.placementChoices(null), []);
});

test('file badges distinguish NED parsing from INI and JSON source roles', () => {
  assert.equal(V.sourceSyntax({ role: 'ned', syntax: 'current' }), 'NED · current');
  assert.equal(V.sourceSyntax({ role: 'config' }), 'INI 設定');
  assert.equal(V.sourceSyntax({ role: 'workload' }), 'JSON ワークロード');
  assert.equal(V.sourceSyntax({ role: 'model_config' }), 'JSON モデル設定');
});

test('CAN pair candidates use Classical and Multibus implementation metadata and unused gates', () => {
  const gates = [{ name: 'send', output: true }, { name: 'receive', output: false }];
  const nodes = [
    { name: 'a', implementation: 'dir.can.Controller', gates },
    { name: 'b', implementation: 'dir.can.MultibusController', gates },
    { name: 'ordinary', implementation: 'custom.Controller', gates },
    { name: 'bus', implementation: 'dir.can.Bus', gates },
    { name: 'multibus', implementation: 'dir.can.MultibusBus', gates },
    { name: 'missing', implementation: 'dir.can.Controller', unresolved: true, gates }
  ];
  assert.deepEqual(I.canPairCandidates({ nodes }).controllers.map(n => n.name), ['a', 'b']);
  assert.deepEqual(I.canPairCandidates({ nodes }).buses.map(n => n.name), ['bus', 'multibus']);
  assert.deepEqual(I.canPairCandidates({ nodes, connections: [{ start: 'a.send', end: 'bus.receive' }] }).controllers.map(n => n.name), ['b']);
});

test('project reset clears old buffers and scope while retaining monotonic response counters', () => {
  const app = { buffers: new Map([['old', new I.TextBuffer('old', 'local', 'h')]]), fileId: 'old', typeKey: 'old-type', instancePath: 'Old.a', breadcrumbs: [{ name: 'Old' }], selection: { kind: 'node' }, scopeGeneration: '9007199254740999', view: { revision: '17', view_sequence: '19', graph: graph, source: {} } };
  I.resetProjectScope(app);
  assert.equal(app.buffers.size, 0); assert.equal(app.fileId, null); assert.equal(app.typeKey, null); assert.equal(app.instancePath, null);
  assert.deepEqual(app.breadcrumbs, []); assert.equal(app.selection, null);
  assert.equal(app.scopeGeneration, '9007199254741000'); assert.equal(app.view.revision, '17'); assert.equal(app.view.view_sequence, '19'); assert.equal(app.view.graph, null);
  const current = { ...app.view, schema_version: 1, session_id: 's', request_scope_generation: app.scopeGeneration };
  assert.equal(I.acceptView(current, { ...current, revision: '16', view_sequence: '20' }, app.scopeGeneration, 's'), false);
});

async function browserHarness(run, mode = 'success') {
  const elements = new Map(), requests = [], dialogs = [], answers = [];
  const projection = {};
  const hooks = {};
  const document = { hidden: true, getElementById(id) {
    if (!elements.has(id)) elements.set(id, { value: '', hidden: false, handlers: {}, addEventListener(name, fn) { this.handlers[name] = fn; } });
    return elements.get(id);
  }, addEventListener() {} };
  let app, callbacks, revision = '1', sequence = 0, fresh = false, uncertain;
  const files = [
    { id: 'ned', path: 'model.ned', role: 'ned', text: 'network Old {}', hash: 'h' },
    { id: 'ini', path: 'omnetpp.ini', role: 'config', text: '[General]\r\nnetwork = Old\r\n', hash: 'i' },
    { id: 'json', path: 'workload.json', role: 'workload', text: '{"requests":[]}', hash: 'j' }
  ];
  const originalFetch = global.fetch;
  const success = body => ({ schema_version: 1, session_id: 'session', command_id: body.command_id, terminal: true, accepted: true, sequence_consumed: true, operation_status: 200, current_revision: revision, next_sequence: String(BigInt(body.client_sequence) + 1n) });
  global.fetch = async (url, options) => {
    const body = options.body ? JSON.parse(options.body) : undefined;
    requests.push({ url, body });
    let data;
    if (url === '/api/writer') data = { client_id: 'client', writer_epoch: '1', next_sequence: '1', writer: true };
    else if (url === '/api/confirmations') data = hooks.confirmation ? await hooks.confirmation(body) : { confirmation: 'discard-nonce', impacts: [] };
    else if (url === '/api/commands') {
      if (body.kind === 'new_project') {
        if (mode === 'reject') data = { ...success(body), accepted: false, operation_status: 409, code: 'E-EDITOR-CONFLICT' };
        else if (mode === 'uncertain') { uncertain = body; throw new Error('connection closed'); }
        else { fresh = true; revision = '2'; data = success(body); }
      } else if (body.kind === 'replace_source') {
        const f = files.find(f => f.id === body.payload.file_id);
        f.text = body.payload.text; f.hash = await I.sourceHash(f.text); revision = String(BigInt(revision) + 1n); data = success(body);
      } else { revision = String(BigInt(revision) + 1n); data = success(body); }
    } else if (url.startsWith('/api/commands/')) { fresh = true; revision = '2'; data = success(uncertain); }
    else {
      const query = new URL(url, 'http://localhost').searchParams;
      const projectFiles = fresh ? [{ id: 'new-ned', path: 'new.ned', role: 'ned', text: 'network NewProject {}', hash: 'new' }] : files;
      const file = projectFiles.find(f => f.id === query.get('file_id')) || projectFiles[0];
      const types = fresh ? [{ key: 'new-module', name: 'Helper', kind: 'module', file_id: 'new-ned' }, { key: 'new-network', name: 'NewProject', kind: 'network', file_id: 'new-ned' }] : [{ key: 'old', name: 'Old', kind: 'network', file_id: 'ned' }];
      const type = types.find(t => t.key === query.get('type_key')) || types[0];
      let workload;
      try { const parsed = JSON.parse(files.find(f => f.id === 'json').text); if (Array.isArray(parsed.generators)) workload = parsed; } catch (_) {}
      data = { schema_version: 1, session_id: 'session', revision, input_revision: revision, view_sequence: String(++sequence), request_scope_generation: query.get('request_scope_generation'), files: projectFiles, types, source: { file_id: file.id, text: file.text, hash: file.hash, role: file.role }, graph: file.role === 'ned' ? { type_key: type.key, type_name: type.name, nodes: [] } : null, dirty: true, never_exported: true, templates: [{ id: 'multibus', name: 'Multibus' }, { id: 'can', name: 'CAN' }], catalog, workload, ...projection };
    }
    return { ok: true, status: 200, json: async () => JSON.parse(JSON.stringify(data)) };
  };
  const root = { document, location: { hash: '#session=fixture', pathname: '/', search: '' }, history: { replaceState() {} }, addEventListener() {}, setTimeout() {}, NEDEditorView: {
    commandLabel: V.commandLabel, instanceStructuralBlocked: V.instanceStructuralBlocked,
    render(state, client, handlers) { app = state; callbacks = handlers; },
    async dialog(config) { dialogs.push(config); const answer = answers.shift(); return typeof answer === 'function' ? answer(config) : answer; }
  } };
  try {
    await I.start(root);
    await run({ get app() { return app; }, get callbacks() { return callbacks; }, elements, requests, dialogs, answers, projection, hooks,
      async setSource(id, text) { const file = files.find(f => f.id === id); file.text = text; file.hash = await I.sourceHash(text); revision = String(BigInt(revision) + 1n); }
    });
  } finally { clearTimeout(app?.timer); global.fetch = originalFetch; }
}

test('New confirmation cancellation and terminal rejection preserve buffers and selected scope', async () => {
  for (const mode of ['cancel', 'reject']) await browserHarness(async h => {
    const b = h.app.buffers.get('ned');
    h.app.instancePath = 'Old.a'; h.app.breadcrumbs = [{ name: 'Old' }]; h.app.selection = { kind: 'port' };
    h.answers.push({ 'project-template': 'multibus', 'project-name-input': 'NewProject' }, mode === 'cancel' ? null : {});
    await h.elements.get('new-project').onclick();
    assert.equal(h.app.buffers.get('ned'), b); assert.equal(h.app.instancePath, 'Old.a'); assert.equal(h.app.breadcrumbs.length, 1); assert.equal(h.app.typeKey, 'old');
    assert.match(h.dialogs[1].text, /未保存または未出力/);
    const commands = h.requests.filter(r => r.url === '/api/commands');
    assert.equal(commands.length, mode === 'cancel' ? 0 : 1);
    if (commands.length) assert.equal(commands[0].body.payload.discard_ack, 'discard-nonce');
  }, mode);
});

test('successful New and uncertain same-ID lookup clear old context then select the new network', async () => {
  for (const mode of ['success', 'uncertain']) await browserHarness(async h => {
    h.app.instancePath = 'Old.a'; h.app.breadcrumbs = [{ name: 'Old' }]; h.app.selection = { kind: 'port' };
    const old = h.app.buffers.get('ned');
    h.answers.push({ 'project-template': 'multibus', 'project-name-input': 'NewProject' }, {});
    await h.elements.get('new-project').onclick();
    if (mode === 'uncertain') {
      assert.equal(h.app.buffers.get('ned'), old); assert.equal(h.app.instancePath, 'Old.a');
      await h.elements.get('check-command').onclick();
    }
    assert.equal(h.app.buffers.has('ned'), false); assert.equal(h.app.fileId, 'new-ned'); assert.equal(h.app.typeKey, 'new-network');
    assert.equal(h.app.instancePath, null); assert.deepEqual(h.app.breadcrumbs, []); assert.equal(h.app.selection, null);
    assert.equal(h.app.view.revision, '2');
    assert.equal(h.requests.filter(r => r.url === '/api/commands').length, 1);
    const lastScope = new URL(h.requests.filter(r => r.url.startsWith('/api/session?')).at(-1).url, 'http://localhost').searchParams;
    assert.equal(lastScope.has('instance_path'), false); assert.equal(lastScope.get('type_key'), 'new-network');
  }, mode);
});

test('INI and JSON navigation edits and flushes through the same lossless source command', async () => {
  await browserHarness(async h => {
    for (const id of ['ini', 'json']) {
      await h.callbacks.file({ id });
      assert.equal(h.app.fileId, id); assert.equal(h.app.typeKey, null); assert.equal(h.app.instancePath, null);
      const b = h.app.buffers.get(id), before = b.raw;
      b.edit(b.visible + '\n// edit 日本語😀');
      await h.elements.get('flush-source').onclick();
      assert.equal(b.pending, false); assert.equal(b.blocked, null);
      const payload = h.requests.filter(r => r.body?.kind === 'replace_source').at(-1).body.payload;
      assert.equal(payload.file_id, id); assert.equal(payload.text, b.raw);
      assert.ok(payload.text.startsWith(before));
      if (id === 'ini') assert.ok(payload.text.includes('\r\n// edit'));
    }
    assert.equal(h.app.buffers.has('ned'), true); assert.equal(h.app.buffers.has('ini'), true); assert.equal(h.app.buffers.has('json'), true);
  });
});

test('instance choices use resolved Controller metadata and exact descendant path boundaries', () => {
  const instances = [
    { path: 'Main.gateway.in', implementation: 'dir.can.MultibusController' },
    { path: 'Main.gateway.out', implementation: 'dir.can.Controller' },
    { path: 'Main.gatewayOther.a', implementation: 'dir.can.Controller' },
    { path: 'Main.gateway.bus', implementation: 'dir.can.MultibusBus' },
    { path: 'Main.gateway.missing', implementation: 'dir.can.Controller', unresolved: true }
  ];
  assert.deepEqual(I.controllerChoices({ instances }, 'Main.gateway').map(i => i.value), ['Main.gateway.in', 'Main.gateway.out']);
  assert.equal(I.controllerChoices({ instances }).length, 3);
});

test('Gateway structured payload supports fanout and rejects duplicate IDs, foreign ports and reversed ranges', () => {
  const ports = ['Main.gw.in', 'Main.gw.out1', 'Main.gw.out2'];
  const route = { id: 'route1', ingress: ports[0], egress: ports.slice(1), format: 'standard', id_min: '256', id_max: '511' };
  const form = { ports, processing_delay: '20us', hop_limit: '16', rx_queue_capacity: '64', routes: [route] };
  assert.deepEqual(I.gatewayPayload('Main.gw', form), { node: 'Main.gw', ports, processing_delay: '20us', hop_limit: 16, rx_queue_capacity: 64, routes: [{ ...route, id_min: 256, id_max: 511 }] });
  assert.throws(() => I.gatewayPayload('Main.gw', { ...form, routes: [route, route] }), /重複/);
  assert.throws(() => I.gatewayPayload('Main.gw', { ...form, routes: [{ ...route, egress: ['Main.other'] }] }), /別々のポート/);
  assert.throws(() => I.gatewayPayload('Main.gw', { ...form, routes: [{ ...route, egress: [ports[0]] }] }), /別々のポート/);
  assert.throws(() => I.gatewayPayload('Main.gw', { ...form, routes: [{ ...route, id_min: '512' }] }), /上限以下/);
  assert.throws(() => I.gatewayPayload('Main.gw', { ...form, routes: [{ ...route, id_max: '2048' }] }), /整数/);
  assert.equal(I.gatewayPayload('Main.gw', { ...form, rx_queue_capacity: '0' }).rx_queue_capacity, 0);
  assert.throws(() => I.gatewayPayload('Main.gw', { ...form, rx_queue_capacity: '-1' }), /整数/);
});

test('workload form canonicalizes periodic/explicit rows and rejects duplicate and unsafe values', () => {
  const periodic = { id: 'periodic', kind: 'can.periodic.v1', node: 'Main.a', format: 'standard', frame_id: '256', data: '01 02 aB', start: '0ps', phase: '10us', period: '1ms', end: '', count: '' };
  const explicit = { id: 'explicit', kind: 'can.explicit.v1', node: 'Main.b', format: 'extended', frame_id: '536870911', data: '', times: '0ps, 1ms\n2ms' };
  assert.deepEqual(I.workloadPayload({ generators: [periodic, explicit] }), { generators: [
    { id: 'periodic', kind: 'can.periodic.v1', node: 'Main.a', frame: { format: 'standard', id: 256, data: '0102aB' }, start: '0ps', phase: '10us', period: '1ms' },
    { id: 'explicit', kind: 'can.explicit.v1', node: 'Main.b', frame: { format: 'extended', id: 536870911, data: '' }, times: ['0ps', '1ms', '2ms'] }
  ] });
  const limited = I.workloadPayload({ generators: [{ ...periodic, end: '5ms', count: '10' }] }).generators[0];
  assert.equal(limited.end, '5ms'); assert.equal(limited.count, 10);
  assert.throws(() => I.workloadPayload({ generators: [periodic, periodic] }), /重複/);
  assert.throws(() => I.workloadPayload({ generators: [{ ...periodic, count: '9007199254740992' }] }), /整数/);
  assert.throws(() => I.workloadPayload({ generators: [{ ...periodic, data: 'abc' }] }), /16進数/);
  assert.throws(() => I.workloadPayload({ generators: [{ ...periodic, data: '01'.repeat(9) }] }), /16進数/);
  assert.deepEqual(I.workloadPayload({ generators: [{ ...explicit, times: '' }] }).generators[0].times, []);
  assert.equal(I.workloadPayload({ generators: [{ ...periodic, count: '0' }] }).generators[0].count, 0);
  assert.deepEqual(I.workloadPayload({ generators: [] }), { generators: [] });
});

test('gate forms produce atomic direction pairs and reject duplicate or invalid identifiers', () => {
  assert.deepEqual(I.gatePayload('type', { mode: 'single', gate_name: 'incoming', direction: 'input' }), { type_key: 'type', gates: [{ name: 'incoming', output: false }] });
  assert.deepEqual(I.gatePayload('bus', { mode: 'single', input_gate: 'receive_d', output_gate: 'send_d' }, true), { type_key: 'bus', gates: [{ name: 'receive_d', output: false }, { name: 'send_d', output: true }] });
  assert.throws(() => I.gatePayload('type', { mode: 'pair', input_gate: 'same', output_gate: 'same' }), /異なるgate名/);
  assert.throws(() => I.gatePayload('type', { mode: 'single', gate_name: 'a.b' }), /gate名/);
});

test('composition dialogs use structural confirmations and atomic boundary-port commands', async () => {
  await browserHarness(async h => {
    h.answers.push({ type_name: 'CustomGateway', child_name: 'gateway' }, {});
    await h.elements.get('create-module').onclick();
    let posted = h.requests.filter(r => r.url === '/api/commands').at(-1).body;
    assert.equal(posted.kind, 'create_module'); assert.equal(posted.payload.parent_type, 'old');
    assert.equal(posted.payload.type_name, 'CustomGateway'); assert.equal(posted.impact_ack, 'discard-nonce');
    h.answers.push({ child_name: 'port3', input_gate: 'receive_port3', output_gate: 'send_port3' }, {});
    await h.elements.get('add-port').onclick();
    posted = h.requests.filter(r => r.url === '/api/commands').at(-1).body;
    assert.equal(posted.kind, 'add_port'); assert.deepEqual(posted.payload.position, { x: 80, y: 100 });
    assert.equal(posted.payload.type_key, 'old'); assert.equal(posted.payload.input_gate, 'receive_port3');
    h.app.view.graph.implementation = 'dir.can.MultibusBus'; h.app.view.graph.own_gates = [{ name: 'receive_new', output: false }, { name: 'send_new', output: true }];
    h.answers.push({ gate_name: 'receive_new', paired_gate: 'send_new' }, {});
    await h.elements.get('delete-gate').onclick();
    posted = h.requests.filter(r => r.url === '/api/commands').at(-1).body;
    assert.deepEqual(posted.payload, { type_key: 'old', gate_name: 'receive_new', paired_gate: 'send_new' });
  });
});

test('configuration commands confirm impacts, preserve count strings for INI and block unreadable JSON', async () => {
  await browserHarness(async h => {
    const settings = { sim_time_limit: '20ms', metrics_window: '1ms', max_events: '9007199254740993', max_delta_cycles: '1000' };
    h.answers.push(settings);
    await h.elements.get('project-settings').onclick();
    let posted = h.requests.filter(r => r.url === '/api/commands').at(-1).body;
    assert.equal(posted.kind, 'set_project_settings'); assert.deepEqual(posted.payload, settings); assert.equal(posted.impact_ack, 'discard-nonce');
    const generators = [{ id: 'one', kind: 'can.explicit.v1', node: 'Main.a', format: 'standard', frame_id: '256', data: '01', times: '0ps, 1ms' }];
    h.answers.push({ generators });
    await h.elements.get('workload-settings').onclick();
    posted = h.requests.filter(r => r.url === '/api/commands').at(-1).body;
    assert.equal(posted.kind, 'set_workload'); assert.deepEqual(posted.payload.generators[0].times, ['0ps', '1ms']);
    h.app.instancePath = 'Main.a'; h.projection.parameters = [{ name: 'queueCapacity', instance_path: 'Main.a', override: '64' }]; h.answers.push({ literal: '' });
    await h.callbacks.instanceParameter({ name: 'queueCapacity', instance_path: 'Main.a', override: '64' });
    posted = h.requests.filter(r => r.url === '/api/commands').at(-1).body;
    assert.deepEqual(posted.payload, { instance_path: 'Main.a', parameter_name: 'queueCapacity', literal: null });
    const before = h.requests.filter(r => r.url === '/api/commands').length;
    h.projection.configuration_errors = ['invalid workload JSON'];
    await h.elements.get('workload-settings').onclick();
    assert.equal(h.requests.filter(r => r.url === '/api/commands').length, before);
    assert.match(h.elements.get('notice-text').textContent, /読み取れません/);
  });
});

test('workload settings flush pending JSON before constructing rows and show the latest count', async () => {
  await browserHarness(async h => {
    const generator = { id: 'send', kind: 'can.periodic.v1', node: 'Old.a', start: '0ps', period: '1ms', count: 5, frame: { format: 'standard', id: 256, data: '01' } };
    await h.setSource('json', JSON.stringify({ schema_version: 1, generators: [generator] }));
    await h.callbacks.file({ id: 'json' });
    const b = h.app.buffers.get('json');
    b.edit(JSON.stringify({ schema_version: 1, generators: [{ ...generator, count: 10 }] }));
    h.answers.push(config => {
      assert.equal(config.sections[0].rows[0].count, '10');
      const replace = h.requests.filter(r => r.body?.kind === 'replace_source');
      assert.equal(replace.length, 1); assert.match(replace[0].body.payload.text, /"count":10/);
      return { generators: config.sections[0].rows };
    });
    await h.elements.get('workload-settings').onclick();
    assert.equal(b.pending, false);
    const posted = h.requests.filter(r => r.body?.kind === 'set_workload').at(-1).body;
    assert.equal(posted.payload.generators[0].count, 10);
  });
});

test('settings submission rejects changed raw buffers or remote revision instead of overwriting newer JSON', async () => {
  for (const change of ['buffer', 'remote', 'scope']) await browserHarness(async h => {
    const generator = { id: 'send', kind: 'can.periodic.v1', node: 'Old.a', start: '0ps', period: '1ms', count: 5, frame: { format: 'standard', id: 256, data: '' } };
    const original = JSON.stringify({ schema_version: 1, generators: [generator] });
    const newer = JSON.stringify({ schema_version: 1, generators: [{ ...generator, count: 10 }] });
    await h.setSource('json', original); await h.callbacks.file({ id: 'json' });
    const b = h.app.buffers.get('json');
    h.answers.push(async config => {
      assert.equal(config.sections[0].rows[0].count, '5');
      if (change === 'buffer') b.edit(newer);
      if (change === 'remote') await h.setSource('json', newer);
      if (change === 'scope') h.app.scopeGeneration = String(BigInt(h.app.scopeGeneration) + 1n);
      return { generators: config.sections[0].rows };
    });
    await h.elements.get('workload-settings').onclick();
    assert.equal(h.requests.some(r => r.body?.kind === 'set_workload'), false);
    assert.match(h.elements.get('notice-text').textContent, /開き直してください/);
    if (change === 'buffer') { assert.equal(b.raw, newer); assert.equal(b.pending, true); }
    if (change === 'remote') assert.equal(b.raw, newer);
  });
});

test('network and instance settings use refreshed projections rather than earlier button snapshots', async () => {
  await browserHarness(async h => {
    await h.callbacks.file({ id: 'ini' });
    const b = h.app.buffers.get('ini'); b.edit(b.visible + '// newer raw setting\n');
    h.projection.project_settings = { sim_time_limit: '30ms', metrics_window: '3ms', max_events: '100', max_delta_cycles: '20' };
    h.answers.push(config => { assert.equal(config.fields.find(f => f.id === 'sim_time_limit').value, '30ms'); return null; });
    await h.elements.get('project-settings').onclick(); assert.equal(b.pending, false);
    h.app.instancePath = 'Old.a';
    h.projection.parameters = [{ name: 'queueCapacity', instance_path: 'Old.a', override: '128', default: '64' }];
    h.answers.push(config => { assert.equal(config.fields[0].value, '128'); return { literal: '256' }; });
    await h.callbacks.instanceParameter({ name: 'queueCapacity', instance_path: 'Old.a', override: '32' });
    const posted = h.requests.filter(r => r.body?.kind === 'set_instance_parameter').at(-1).body;
    assert.equal(posted.payload.literal, '256');
  });
});

test('Gateway settings bind both node selection and route forms to the current source snapshot', async () => {
  await browserHarness(async h => {
    h.projection.instances = [{ path: 'Old.gw', kind: 'module' }, { path: 'Old.gw.a', implementation: 'dir.can.MultibusController' }, { path: 'Old.gw.b', implementation: 'dir.can.MultibusController' }];
    const settings = { node: 'Old.gw', ports: ['Old.gw.a', 'Old.gw.b'], processing_delay: '20us', hop_limit: 16, rx_queue_capacity: 64, routes: [] };
    h.projection.gateway_settings = [settings];
    h.answers.push({ node: 'Old.gw', action: 'set' }, async config => {
      assert.equal(config.fields.find(f => f.id === 'processing_delay').value, '20us');
      await h.setSource('ini', '[General]\nnetwork = Changed\n');
      return { ports: settings.ports, processing_delay: '30us', hop_limit: '16', rx_queue_capacity: '64', routes: [] };
    });
    await h.elements.get('gateway-settings').onclick();
    assert.equal(h.requests.some(r => r.body?.kind === 'set_gateway'), false);
    assert.match(h.elements.get('notice-text').textContent, /開き直してください/);
  });
});

test('settings snapshot also rejects drift after a newer edit has already been acknowledged', () => {
  const b = new I.TextBuffer('json', 'old', 'hash1'), app = { view: { session_id: 's', revision: '1', input_revision: '1', files: [{ id: 'json', hash: 'hash1' }] }, buffers: new Map([['json', b]]), scopeGeneration: '1', fileId: 'json', typeKey: null, instancePath: null };
  const client = { writer: true, writerEpoch: '1' }, captured = I.settingsSnapshot(app, client);
  I.assertSettingsSnapshot(app, client, captured);
  b.edit('new'); b.acknowledge(b.snapshot(), 'hash1');
  assert.equal(b.pending, false);
  assert.throws(() => I.assertSettingsSnapshot(app, client, captured), /開き直してください/);
});

test('settings recheck source after shared-impact confirmation and preserve a remote edit', async () => {
  await browserHarness(async h => {
    const settings = { sim_time_limit: '20ms', metrics_window: '1ms', max_events: '100', max_delta_cycles: '1000' };
    h.hooks.confirmation = async () => ({ confirmation: 'discard-nonce', impacts: ['shared configuration'] });
    h.answers.push(settings, async () => { await h.setSource('ini', '[General]\nnetwork = Changed\n'); return {}; });
    await h.elements.get('project-settings').onclick();
    assert.equal(h.requests.some(r => r.url === '/api/commands' && r.body?.kind === 'set_project_settings'), false);
    assert.match(h.elements.get('notice-text').textContent, /開き直してください/);
  });
});

test('settings opening waits for IME completion before taking the source snapshot', async () => {
  await browserHarness(async h => {
    await h.callbacks.file({ id: 'ini' });
    const b = h.app.buffers.get('ini'); b.edit(b.visible + '// IME\n'); b.composition(true);
    h.answers.push(null);
    const opening = h.elements.get('project-settings').onclick();
    await Promise.resolve(); await Promise.resolve();
    assert.equal(h.dialogs.length, 0);
    assert.equal(h.requests.some(r => r.body?.kind === 'replace_source'), false);
    b.composition(false); await opening;
    assert.equal(h.dialogs.length, 1); assert.equal(b.pending, false);
    assert.equal(h.requests.filter(r => r.body?.kind === 'replace_source').length, 1);
  });
});

test('Gateway choices retain orphan settings nodes and unavailable configured ports', () => {
  const view = { instances: [{ path: 'Main.gw', kind: 'module' }, { path: 'Main.gw.a', implementation: 'dir.can.MultibusController' }], gateway_settings: [{ node: 'Main.gw', ports: ['Main.gw.a', 'Main.gw.missing'] }, { node: 'Main.deleted', ports: ['Main.deleted.a'] }] };
  const nodes = I.gatewayNodeChoices(view);
  assert.deepEqual(nodes.map(n => n.value), ['Main.gw', 'Main.deleted']);
  assert.equal(nodes[0].available, true); assert.equal(nodes[1].available, false);
  assert.match(nodes[1].label, /配置なし・設定解除のみ/);
  const ports = I.gatewayPortChoices(view, 'Main.gw', view.gateway_settings[0]);
  assert.deepEqual(ports.map(p => p.value), ['Main.gw.a', 'Main.gw.missing']);
  assert.match(ports[1].label, /配置なし・既存設定/);
  assert.deepEqual(I.gatewayPortChoices(view, 'Main.gw', null), I.controllerChoices(view, 'Main.gw'));
});

test('Gateway settings can delete an orphan entry and reject configuring an absent compound', async () => {
  for (const action of ['delete', 'set']) await browserHarness(async h => {
    h.projection.instances = [];
    h.projection.gateway_settings = [{ node: 'Old.deleted', ports: ['Old.deleted.a', 'Old.deleted.b'], processing_delay: '0ps', hop_limit: 16, rx_queue_capacity: 64, routes: [] }];
    h.answers.push(config => {
      const nodes = config.fields.find(f => f.id === 'node').choices;
      assert.equal(nodes[0].value, 'Old.deleted'); assert.match(nodes[0].label, /設定解除のみ/);
      assert.deepEqual(config.fields.find(f => f.id === 'action').choices.map(c => c.value), ['delete']);
      return { node: 'Old.deleted', action };
    });
    await h.elements.get('gateway-settings').onclick();
    const commands = h.requests.filter(r => r.url === '/api/commands');
    assert.equal(commands.length, action === 'delete' ? 1 : 0);
    if (action === 'delete') { assert.equal(commands[0].body.kind, 'delete_gateway'); assert.deepEqual(commands[0].body.payload, { node: 'Old.deleted' }); }
    else assert.match(h.elements.get('notice-text').textContent, /設定解除のみ/);
  });
});

test('Gateway dialogs preserve missing checked ports and route values until explicitly unchecked', async () => {
  for (const operation of ['edit-delay', 'remove-port']) await browserHarness(async h => {
    const configuredPorts = ['Old.gw.a', 'Old.gw.b', 'Old.gw.missing'];
    const routes = [{ id: 'route', ingress: 'Old.gw.a', egress: ['Old.gw.b', 'Old.gw.missing'], format: 'standard', id_min: 0, id_max: 2047 }];
    if (operation === 'edit-delay') routes.push({ id: 'missing-ingress', ingress: 'Old.gw.missing', egress: ['Old.gw.a'], format: 'standard', id_min: 0, id_max: 2047 });
    h.projection.instances = [{ path: 'Old.gw', kind: 'module' }, { path: 'Old.gw.a', implementation: 'dir.can.MultibusController' }, { path: 'Old.gw.b', implementation: 'dir.can.MultibusController' }];
    h.projection.gateway_settings = [{ node: 'Old.gw', ports: configuredPorts, processing_delay: '20us', hop_limit: 16, rx_queue_capacity: 64, routes }];
    h.answers.push({ node: 'Old.gw', action: 'set' }, async config => {
      const fv = formView();
      h.elements.set('ports', { querySelectorAll: selector => fv.document.getElementById('ports').querySelectorAll(selector) });
      const pending = fv.view.dialog(config);
      const group = fv.document.getElementById('ports');
      const missing = group.querySelectorAll('input').find(i => i.value === 'Old.gw.missing');
      assert.equal(missing.checked, true);
      assert.match(missing.parent.children[1].textContent, /配置なし・既存設定/);
      const missingEgress = fv.document.getElementById('routes-1-egress').querySelectorAll('input').find(i => i.value === 'Old.gw.missing');
      assert.equal(missingEgress.checked, true);
      if (operation === 'edit-delay') assert.equal(fv.document.getElementById('routes-2-ingress').value, 'Old.gw.missing');
      if (operation === 'remove-port') { missing.checked = false; group.dispatch('change'); }
      fv.document.getElementById('processing_delay').value = '30us';
      fv.document.getElementById('dialog-form').onsubmit({ preventDefault() {} });
      assert.equal(fv.document.getElementById('editor-dialog').open, false);
      return pending;
    });
    await h.elements.get('gateway-settings').onclick();
    const payload = h.requests.filter(r => r.url === '/api/commands' && r.body.kind === 'set_gateway').at(-1).body.payload;
    assert.equal(payload.processing_delay, '30us');
    assert.deepEqual(payload.ports, operation === 'edit-delay' ? configuredPorts : configuredPorts.slice(0, 2));
    assert.deepEqual(payload.routes[0].egress, operation === 'edit-delay' ? ['Old.gw.b', 'Old.gw.missing'] : ['Old.gw.b']);
    if (operation === 'edit-delay') assert.equal(payload.routes[1].ingress, 'Old.gw.missing');
  });
});

function formView() {
  const nodes = new Map();
  class Node {
    constructor(tag) { this.tagName = tag; this.children = []; this.listeners = {}; this.dataset = {}; this.value = ''; this.hidden = false; this.disabled = false; this.classList = { toggle() {} }; }
    set id(value) { this._id = value; nodes.set(value, this); }
    get id() { return this._id; }
    append(...children) { for (const child of children) { this.children.push(child); child.parent = this; if (this.tagName === 'select' && !this.value) this.value = child.value; } }
    replaceChildren(...children) { this.children = []; this.value = ''; this.append(...children); }
    remove() { this.parent.children.splice(this.parent.children.indexOf(this), 1); }
    querySelectorAll(selector) { const found = []; for (const child of this.children) { if ((selector === 'input' || selector === 'input:checked') && child.tagName === 'input' && (selector !== 'input:checked' || child.checked)) found.push(child); found.push(...child.querySelectorAll(selector)); } return found; }
    querySelector() { return this.querySelectorAll('input')[0] || null; }
    setAttribute(name, value) { this[name] = value; }
    addEventListener(name, fn) { (this.listeners[name] ||= []).push(fn); }
    removeEventListener(name, fn) { this.listeners[name] = (this.listeners[name] || []).filter(f => f !== fn); }
    dispatch(name) { for (const fn of this.listeners[name] || []) fn({ preventDefault() {} }); }
    focus() {}
    showModal() { this.open = true; }
    close() { this.open = false; }
    reportValidity() { return true; }
  }
  const document = { createElement: tag => new Node(tag), getElementById(id) { if (!nodes.has(id)) { const node = new Node('div'); node.id = id; } return nodes.get(id); } };
  const context = { document };
  require('node:vm').runInNewContext(require('node:fs').readFileSync(require.resolve('../crates/dir-simulator/src/tool/ned-editor/assets/view.js'), 'utf8'), context);
  return { view: context.NEDEditorView, document, nodes };
}

test('a queued close event from the preceding dialog cannot close the next settings step', async () => {
  const {view,document}=formView();
  const first=view.dialog({title:'Select',fields:[{id:'node',label:'Node',value:'Main.gw'}]});
  document.getElementById('dialog-form').onsubmit({preventDefault(){}});
  await first;
  const second=view.dialog({title:'Settings',fields:[{id:'delay',label:'Delay',value:'10us'}]});
  const dialog=document.getElementById('editor-dialog');
  dialog.dispatch('close');
  assert.equal(dialog.open,true);
  document.getElementById('dialog-form').onsubmit({preventDefault(){}});
  assert.equal((await second).delay,'10us');
  const external=view.dialog({title:'External close'});
  dialog.close();dialog.dispatch('close');
  assert.equal(await external,null);
});

test('repeatable forms retain invalid rows, allow removal/addition and collect only live rows', async () => {
  const { view, document, nodes } = formView();
  const pending = view.dialog({ title: 'Rows', sections: [{ id: 'generators', label: 'Rows', rows: [{ id: 'same' }, { id: 'same' }], defaultRow: { id: 'added' }, fields: [{ id: 'id', label: 'ID' }] }], validate: form => {
    const ids = form.generators.map(g => g.id); if (new Set(ids).size !== ids.length) throw new Error('duplicate');
  } });
  document.getElementById('dialog-form').onsubmit({ preventDefault() {} });
  assert.equal(document.getElementById('editor-dialog').open, true);
  assert.equal(document.getElementById('dialog-error').textContent, 'duplicate');
  const rows = document.getElementById('dialog-body').children.find(n => n.className === 'form-section').children[1];
  rows.children[1].children.at(-1).onclick();
  assert.equal(rows.children.length, 1);
  rows.parent.children.at(-1).onclick();
  assert.equal(rows.children.length, 2);
  assert.equal(nodes.get('generators-3-id').value, 'added');
  document.getElementById('dialog-form').onsubmit({ preventDefault() {} });
  const result = await pending;
  assert.deepEqual(JSON.parse(JSON.stringify(result.generators)), [{ id: 'same' }, { id: 'added' }]);
});

test('repeatable route choices track checked Gateway ports and disabled conditional inputs', async () => {
  const { view, document } = formView();
  const choices = [{ value: 'Main.gw.a', label: 'A' }, { value: 'Main.gw.b', label: 'B' }];
  const selected = () => choices.filter(c => document.getElementById('ports').querySelectorAll('input:checked').some(i => i.value === c.value));
  const pending = view.dialog({ title: 'Gateway', fields: [{ id: 'ports', label: 'Ports', checkboxes: choices, value: choices.map(c => c.value) }], sections: [{ id: 'routes', label: 'Routes', rows: [{ ingress: 'Main.gw.a', egress: ['Main.gw.b'], mode: 'short' }], fields: [{ id: 'ingress', label: 'Ingress', choices: selected }, { id: 'egress', label: 'Egress', checkboxes: selected }, { id: 'mode', label: 'Mode', choices: [{ value: 'short' }, { value: 'long' }] }, { id: 'extra', label: 'Extra', required: true, when: row => row.mode === 'long' }] }] });
  assert.equal(document.getElementById('routes-1-extra').disabled, true);
  document.getElementById('routes-1-mode').value = 'long'; document.getElementById('routes-1-mode').dispatch('change');
  assert.equal(document.getElementById('routes-1-extra').disabled, false);
  document.getElementById('ports').querySelectorAll('input')[1].checked = false; document.getElementById('ports').dispatch('change');
  assert.equal(document.getElementById('routes-1-egress').querySelectorAll('input').length, 1);
  document.getElementById('dialog-form').onsubmit({ preventDefault() {} });
  const result = await pending;
  assert.deepEqual(JSON.parse(JSON.stringify(result.ports)), ['Main.gw.a']);
  assert.deepEqual(JSON.parse(JSON.stringify(result.routes[0].egress)), []);
});
