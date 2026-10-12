'use strict';

const test = require('node:test');
const assert = require('node:assert/strict');
const I = require('../crates/dir-simulator/src/tool/ned-editor/assets/input.js');

function plan(overrides = {}) {
  return {
    plan_id: 'plan-1',
    plan_digest: 'digest-1',
    session_id: 'session-1',
    writer_epoch: '9007199254740993',
    impacts: [{kind: 'updated_file', path: 'models/demo/Main.ned', before_hash: 'before', after_hash: 'after'}],
    next_cursor: null,
    ...overrides,
  };
}

test('edit plan pages concatenate impacts until the final cursor', async () => {
  const calls = [];
  const first = plan({next_cursor: 'page-2'});
  const result = await I.collectPlanPages(async cursor => {
    calls.push(cursor);
    if (cursor === 'page-2') return plan({impacts: [{kind: 'added_file', path: 'editor/Extra.ned'}], next_cursor: 'page-3'});
    if (cursor === 'page-3') return plan({impacts: [{kind: 'layout', path: 'models/demo/Main.ned.layout.json'}]});
    throw new Error('Unexpected cursor: ' + cursor);
  }, first);

  assert.deepEqual(calls, ['page-2', 'page-3']);
  assert.equal(result.next_cursor, 'page-2', 'The returned plan retains the first response envelope.');
  assert.deepEqual(result.impacts.map(impact => impact.path), [
    'models/demo/Main.ned',
    'editor/Extra.ned',
    'models/demo/Main.ned.layout.json',
  ]);
});

test('edit plan pagination rejects a changed plan, digest, session, or writer epoch', async t => {
  const mismatches = [
    ['plan id', {plan_id: 'plan-2'}],
    ['plan digest', {plan_digest: 'digest-2'}],
    ['session', {session_id: 'session-2'}],
    ['writer epoch', {writer_epoch: '9007199254740994'}],
  ];
  for (const [label, changed] of mismatches) {
    await t.test(label, async () => {
      const first = plan({next_cursor: 'next'});
      await assert.rejects(
        I.collectPlanPages(async () => plan({...changed, impacts: []}), first),
        /変更計画が切り替わりました/,
      );
    });
  }
});

test('edit plan pagination rejects a cursor loop', async () => {
  let calls = 0;
  await assert.rejects(I.collectPlanPages(async cursor => {
    calls++;
    assert.equal(cursor, 'repeat');
    return plan({impacts: [{kind: 'layout', path: 'layout.json'}], next_cursor: 'repeat'});
  }, plan({next_cursor: 'repeat'})), /ページが重複しています/);
  assert.equal(calls, 1);
});

test('removing the selected file drops a clean buffer and resets the editor scope', () => {
  const selected = new I.TextBuffer('selected', 'module Main {}', 'hash-selected');
  const other = new I.TextBuffer('other', 'simple Controller {}', 'hash-other');
  const app = {
    fileId: 'selected', typeKey: 'main-key', instancePath: 'Main.a', scopeGeneration: '9007199254740995',
    selection: {kind: 'node'}, breadcrumbs: [{fileId: 'selected'}], buffers: new Map([['selected', selected], ['other', other]]),
  };

  assert.equal(I.reconcileCapturedFiles(app, {files: [{id: 'other'}]}), true);
  assert.equal(app.buffers.has('selected'), false);
  assert.equal(app.buffers.get('other'), other);
  assert.equal(app.fileId, null);
  assert.equal(app.typeKey, null);
  assert.equal(app.instancePath, null);
  assert.equal(app.selection, null);
  assert.deepEqual(app.breadcrumbs, []);
  assert.equal(app.scopeGeneration, '9007199254740996');
});

test('removed unsent, composing, and sent buffers stay orphaned and recover their raw text when the file returns', () => {
  const unsent = new I.TextBuffer('unsent', '\ufeff// local\r\nmodule A {}', 'hash-a');
  unsent.edit('\ufeff// edited\nmodule A {}');
  const composing = new I.TextBuffer('composing', 'module B {}', 'hash-b');
  composing.composition(true);
  const sent = new I.TextBuffer('sent', 'module C {}', 'hash-c');
  sent.edit('module C {\n  // sent\n}');
  sent.sent = sent.snapshot();
  const buffers = new Map([['unsent', unsent], ['composing', composing], ['sent', sent]]);
  const app = {fileId: 'unsent', typeKey: 'type-a', instancePath: 'Main.a', scopeGeneration: '7', buffers};
  const retainedRaw = new Map([...buffers].map(([id, buffer]) => [id, buffer.raw]));

  assert.equal(I.reconcileCapturedFiles(app, {files: []}), false);
  for (const [id, buffer] of buffers) {
    assert.equal(app.buffers.get(id), buffer);
    assert.equal(buffer.orphaned, true, id);
    assert.match(buffer.blocked, /ファイルが編集モデルから除かれました/, id);
    assert.equal(buffer.raw, retainedRaw.get(id), id);
  }
  assert.equal(app.fileId, 'unsent', 'The selected orphan stays open while its raw input is retained.');

  assert.equal(I.reconcileCapturedFiles(app, {files: [...buffers.keys()].map(id => ({id}))}), false);
  for (const [id, buffer] of buffers) {
    assert.equal(buffer.orphaned, false, id);
    assert.equal(buffer.blocked, null, id);
    assert.equal(buffer.raw, retainedRaw.get(id), id);
  }
  assert.equal(unsent.pending, true);
  assert.equal(composing.composing, true);
  assert.ok(sent.sent);
});

test('same-file COW Undo and Redo rebind the selected instance to the surviving type key', () => {
  const app = {
    fileId: 'main-file', typeKey: 'clone-type-key', instancePath: 'Main.c', scopeGeneration: '20',
    selection: {kind: 'node', item: {key: 'stale-node'}}, breadcrumbs: [{name: 'Main'}],
    buffers: new Map([['main-file', new I.TextBuffer('main-file', 'network Main {}', 'main-hash')]]),
  };
  const file = {id: 'main-file', role: 'ned', syntax: 'parsed'};
  const original = {key: 'original-type-key', file_id: 'main-file', name: 'demo.Controller'};
  const clone = {key: 'clone-type-key', file_id: 'main-file', name: 'demo.ControllerDetached'};
  const undone = {
    files: [file], types: [original],
    instances: [{path: 'Main.c', type_key: original.key, type_name: original.name, unresolved: false}],
  };
  const redone = {
    files: [file], types: [original, clone],
    instances: [{path: 'Main.c', type_key: clone.key, type_name: clone.name, unresolved: false}],
  };

  assert.equal(I.reconcileCapturedFiles(app, undone), true);
  assert.equal(app.fileId, 'main-file', 'Same-file Undo should retain the existing source file.');
  assert.equal(app.typeKey, original.key);
  assert.equal(app.instancePath, 'Main.c');
  assert.equal(app.selection, null);
  assert.deepEqual(app.breadcrumbs, []);
  assert.equal(app.scopeGeneration, '21');

  assert.equal(I.reconcileCapturedFiles(app, redone), true);
  assert.equal(app.fileId, 'main-file', 'Same-file Redo should retain the existing source file.');
  assert.equal(app.typeKey, clone.key);
  assert.equal(app.instancePath, 'Main.c');
  assert.equal(app.scopeGeneration, '22');
});

test('a disappeared selected type clears its stale key and advances scope generation', () => {
  const app = {
    fileId: 'main-file', typeKey: 'old-type-key', instancePath: null, scopeGeneration: '41',
    selection: {kind: 'node'}, breadcrumbs: [{name: 'old type'}],
    buffers: new Map([['main-file', new I.TextBuffer('main-file', 'network Main {}', 'main-hash')]]),
  };
  const next = {
    files: [{id: 'main-file', role: 'ned', syntax: 'parsed'}],
    types: [{key: 'renamed-type-key', file_id: 'main-file', name: 'demo.Renamed'}],
    instances: [],
  };

  assert.equal(I.reconcileCapturedFiles(app, next), true);
  assert.equal(app.fileId, 'main-file');
  assert.equal(app.typeKey, null);
  assert.equal(app.instancePath, null);
  assert.equal(app.selection, null);
  assert.deepEqual(app.breadcrumbs, []);
  assert.equal(app.scopeGeneration, '42');
});

test('pending text prevents scope reconciliation while the NED parse is unresolved', () => {
  const buffer = new I.TextBuffer('main-file', 'network Main {}', 'main-hash');
  buffer.edit('network Main { // local pending');
  const app = {
    fileId: 'main-file', typeKey: 'old-type-key', instancePath: 'Main.a', scopeGeneration: '73',
    selection: {kind: 'node'}, breadcrumbs: [{name: 'Main'}], buffers: new Map([['main-file', buffer]]),
  };
  const next = {
    files: [{id: 'main-file', role: 'ned', syntax: 'syntax_error'}],
    types: [],
    instances: [],
  };
  const raw = buffer.raw;

  assert.equal(I.reconcileCapturedFiles(app, next), false);
  assert.equal(app.fileId, 'main-file');
  assert.equal(app.typeKey, 'old-type-key');
  assert.equal(app.instancePath, 'Main.a');
  assert.equal(app.scopeGeneration, '73');
  assert.equal(buffer.raw, raw);
  assert.equal(buffer.pending, true);
});

test('edit impact descriptions preserve file and symbol names as display strings', () => {
  assert.equal(I.editImpactDescription({kind: 'added_file', path: 'editor/Extra.ned'}), '新規ファイル: editor/Extra.ned');
  assert.equal(I.editImpactDescription({kind: 'layout', path: 'models/demo/Main.ned.layout.json'}), '配置の変更: models/demo/Main.ned.layout.json');
  assert.equal(
    I.editImpactDescription({kind: 'updated_file', path: 'demo.Main.a<->b', before_hash: 'old', after_hash: 'new'}),
    '原文の変更: demo.Main.a<->b\nold → new',
  );
});

test('run history is current by project and input identity, regardless of layout-only revision changes', () => {
  const view = {project_instance_id: 'project-a', revision: '19', input_revision: '7', input_digest: 'input-hash'};
  const run = {source_stamp: {project_instance_id: 'project-a', revision: '18', input_revision: '7', input_digest: 'input-hash'}};

  assert.equal(I.runIsPast(view, run), false, 'A layout-only editor revision does not make the frozen input historical.');
  assert.equal(I.runIsPast(view, {stamp: run.source_stamp}), false, 'The legacy stamp field remains readable.');
  for (const changed of [
    {project_instance_id: 'project-b'},
    {input_revision: '6'},
    {input_digest: 'other-hash'},
  ]) {
    assert.equal(I.runIsPast(view, {source_stamp: {...run.source_stamp, ...changed}}), true, JSON.stringify(changed));
  }
  assert.equal(I.runIsPast(view, {}), true, 'Missing source provenance is treated as historical.');
});

test('Viewer links require a separate loopback HTTP origin with an explicit port and no fragment or credentials', () => {
  const editorOrigin = 'http://127.0.0.1:4310';
  assert.equal(I.viewerURL('http://127.0.0.1:4311/viewer/opaque-capability', editorOrigin), 'http://127.0.0.1:4311/viewer/opaque-capability');

  for (const value of [
    'http://127.0.0.1:4310/viewer/same-origin',
    'http://127.0.0.1/viewer/no-explicit-port',
    'https://127.0.0.1:4311/viewer/https',
    'http://localhost:4311/viewer/localhost',
    'http://[::1]:4311/viewer/ipv6-loopback',
    'http://127.0.0.1.evil:4311/viewer/host-suffix',
    'http://user:password@127.0.0.1:4311/viewer/credentials',
    'http://127.0.0.1:4311/viewer/capability#fragment',
  ]) {
    assert.throws(() => I.viewerURL(value, editorOrigin), /Viewerの閲覧先を確認できません/, value);
  }
});

test('run artifact download sends the editor session and returns the binary response', async () => {
  const bytes = new Uint8Array([0, 1, 2, 255]);
  const blob = new Blob([bytes], {type: 'application/octet-stream'});
  let request;
  const api = new I.Api('session-secret', async (path, options) => {
    request = {path, options};
    return {ok: true, blob: async () => blob};
  });

  assert.equal(await api.download('/api/runs/run-1/artifacts/artifact-1'), blob);
  assert.equal(request.path, '/api/runs/run-1/artifacts/artifact-1');
  assert.equal(request.options.method, 'GET');
  assert.equal(request.options.headers['X-Editor-Session'], 'session-secret');
  assert.equal(request.options.cache, 'no-store');
  assert.equal(request.options.credentials, 'same-origin');
});

test('run artifact download turns an HTTP failure into an ApiError with server details', async () => {
  const api = new I.Api('session-secret', async () => ({
    ok: false,
    status: 410,
    json: async () => ({message: '実行結果は削除されています。', code: 'E-RUN-DELETED'}),
  }));

  await assert.rejects(api.download('/api/runs/run-1/artifacts/artifact-1'), error => {
    assert.ok(error instanceof I.ApiError);
    assert.equal(error.status, 410);
    assert.equal(error.message, '実行結果は削除されています。');
    assert.equal(error.data.code, 'E-RUN-DELETED');
    return true;
  });
});

function runRenderer(clock = Date) {
  const fs = require('node:fs');
  const vm = require('node:vm');
  const nodes = new Map();
  class Element {
    constructor(tag) {
      this.tagName = tag; this.children = []; this.dataset = {}; this.value = ''; this.hidden = false; this.disabled = false;
      this.classList = {toggle() {}}; this.listeners = {};
    }
    append(...children) { this.children.push(...children); }
    replaceChildren(...children) { this.children = [...children]; }
    get childElementCount() { return this.children.length; }
    setAttribute(name, value) { this[name] = value; }
    hasAttribute() { return false; }
    addEventListener(name, handler) { (this.listeners[name] ||= []).push(handler); }
    getBoundingClientRect() { return {width: 800, height: 600}; }
    setSelectionRange() {}
  }
  const document = {
    createElement: tag => new Element(tag),
    createElementNS: (_namespace, tag) => new Element(tag),
    getElementById(id) { if (!nodes.has(id)) nodes.set(id, new Element('div')); return nodes.get(id); },
    querySelectorAll() { return []; },
  };
  const context = {document, NEDEditorInput: I, Date: clock};
  vm.runInNewContext(fs.readFileSync(require.resolve('../crates/dir-simulator/src/tool/ned-editor/assets/view.js'), 'utf8'), context);
  return {nodes, render: (app, client, callbacks = {}) => context.NEDEditorView.render(app, client, callbacks)};
}

test('Viewer expires exactly at its deadline, invalidates the render cache and permits preparation', () => {
  let now=1000;
  class Clock extends Date { static now() {return now;} }
  const h=runRenderer(Clock), app={buffers:new Map(),breadcrumbs:[],scopeGeneration:'1',typeKey:null,fileId:null,
    view:{files:[],outputs:[]},runsReadonly:false,viewerURLs:new Map([['run',{url:'http://127.0.0.1:2/cap',expires:'2000'}]]),
    runs:[{run_id:'run',phase:'completed',artifact_state:'published',viewer_state:'ready'}]};
  const callbacks={prepareViewer(){}};
  const actions=()=>h.nodes.get('runs').children[0].children.find(node=>node.className==='run-actions').children;
  h.render(app,{writer:true},callbacks);assert(actions().some(node=>node.tagName==='a'));
  now=1999;h.render(app,{writer:true},callbacks);assert(actions().some(node=>node.tagName==='a'));
  now=2000;h.render(app,{writer:true},callbacks);
  assert(!actions().some(node=>node.tagName==='a'),'expired links must disappear without changing server rows');
  assert(actions().some(node=>node.textContent==='Viewerを準備'&&!node.disabled));
  assert.equal(app.viewerURLs.has('run'),false);
});

function renderedText(node) {
  return [node.textContent || '', ...(node.children || []).map(renderedText)].join('');
}

test('run cards show partial and recovered outcomes while readonly mode locks run mutations', () => {
  const h = runRenderer();
  const app = {
    buffers: new Map(), breadcrumbs: [], scopeGeneration: '1', typeKey: null, fileId: null,
    view: {project_instance_id: 'project-a', revision: '9', input_revision: '3', input_digest: 'live-hash', files: [], outputs: []},
    runsReadonly: true, runError: '', viewerURLs: new Map(),
    runs: [
      {run_id: 'partial-run', phase: 'partial', artifact_state: 'published', source_stamp: {revision: '8', input_revision: '2', input_digest: 'old-hash', project_instance_id: 'project-a'}, report: {termination: 'deadline', committed_events: '4'}},
      {run_id: 'recovered-run', phase: 'interrupted', artifact_state: 'published', source_stamp: {revision: '9', input_revision: '3', input_digest: 'live-hash', project_instance_id: 'project-a'}, recovered_result: {manifest_hash: 'manifest-hash'}},
      {run_id: 'running-run', phase: 'running', artifact_state: 'none', source_stamp: {revision: '9', input_revision: '3', input_digest: 'live-hash', project_instance_id: 'project-a'}},
    ],
  };
  const client = {writer: false};
  h.render(app, client);

  const cards = h.nodes.get('runs').children.filter(node => node.className === 'run-card');
  assert.deepEqual(cards.map(card => card.dataset.runId), ['partial-run', 'recovered-run', 'running-run']);
  assert.equal(cards[0].dataset.phase, 'partial');
  assert.match(renderedText(cards[0]), /部分結果/);
  assert.match(renderedText(cards[0]), /過去の入力の結果/);
  assert.match(renderedText(cards[0]), /確定イベント: 4/);
  assert.match(renderedText(cards[1]), /公開済み結果を回復しました/);
  assert.doesNotMatch(renderedText(cards[1]), /過去の入力の結果/);
  assert.match(h.nodes.get('runs-status').textContent, /別のサービスが実行を管理/);

  const runningActions = cards[2].children.find(node => node.className === 'run-actions');
  const stop = runningActions.children.find(node => node.textContent === '停止');
  assert.equal(stop.disabled, true, 'Read-only mode must disable cancellation.');
  const recoveredActions = cards[1].children.find(node => node.className === 'run-actions');
  const deleteRun = recoveredActions.children.find(node => node.textContent === '実行を削除');
  const prepareViewer = recoveredActions.children.find(node => node.textContent === 'Viewerを準備');
  assert.equal(deleteRun.disabled, true, 'Read-only mode must disable deletion.');
  assert.equal(prepareViewer.disabled, true, 'Read-only mode must disable Viewer preparation.');
});

test('run cards render nested diagnostics and keep published results downloadable when Viewer preparation failed', () => {
  const h = runRenderer();
  const app = {
    buffers: new Map(), breadcrumbs: [], scopeGeneration: '1', typeKey: null, fileId: null,
    view: {project_instance_id: 'project-a', revision: '12', input_revision: '4', input_digest: 'live-hash', files: [], outputs: []},
    runsReadonly: false, runError: '', viewerURLs: new Map(),
    runs: [{
      run_id: 'viewer-failed-run', phase: 'execution_failed', artifact_state: 'published', viewer_state: 'failed',
      source_stamp: {revision: '12', input_revision: '4', input_digest: 'live-hash', project_instance_id: 'project-a'},
      diagnostics: [
        {phase: 'prepare', diagnostic: {code: 'E-NED', message: 'bad <script>'}},
        {phase: 'monitor', error: {code: 'E-EDITOR-RUN', message: 'disk full'}},
        {phase: 'worker', message: 'worker reported warning'},
      ],
    }],
  };
  h.render(app, {writer: true});

  const card = h.nodes.get('runs').children.find(node => node.dataset.runId === 'viewer-failed-run');
  const text = renderedText(card);
  assert.match(text, /実行失敗/);
  assert.match(text, /prepare: E-NED: bad <script>/);
  assert.match(text, /monitor: E-EDITOR-RUN: disk full/);
  assert.match(text, /worker: worker reported warning/);
  assert.doesNotMatch(text, /—: —/);
  assert.match(text, /結果は生成済みです。Viewerを表示できませんでした。/);

  const actions = card.children.find(node => node.className === 'run-actions');
  assert.ok(actions.children.some(node => node.textContent === 'Viewerを再準備'));
  assert.ok(actions.children.some(node => node.textContent === '結果ファイルを取得'), 'Published artifacts remain downloadable after Viewer failure.');
});
