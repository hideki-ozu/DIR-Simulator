'use strict';
const test = require('node:test');
const assert = require('node:assert/strict');
const fs = require('node:fs');
const vm = require('node:vm');
const V = require('../crates/dir-simulator/src/tool/ned-editor/assets/view.js');

function renderer() {
  const nodes = new Map();
  class Element {
    constructor(tag = 'div') {
      this.tagName = tag; this.children = []; this.dataset = {}; this.value = '';
      this.disabled = false; this.classList = { toggle() {} };
    }
    append(...children) { this.children.push(...children); }
    replaceChildren(...children) { this.children = [...children]; }
    get childElementCount() { return this.children.length; }
    setAttribute(key, value) { this[key] = value; }
    hasAttribute() { return false; }
    addEventListener() {}
    getBoundingClientRect() { return { width: 800, height: 600 }; }
  }
  const document = {
    createElement: tag => new Element(tag),
    createElementNS: (_namespace, tag) => new Element(tag),
    getElementById(id) { if (!nodes.has(id)) nodes.set(id, new Element()); return nodes.get(id); },
    querySelectorAll() { return [...nodes.values()].filter(node => node.dataset.command); }
  };
  for (const [id, command] of [['add-port', 'add_port'], ['create-module', 'create_module'], ['save-as', 'save_as_project']]) {
    document.getElementById(id).dataset.command = command;
  }
  const context = { document };
  vm.runInNewContext(fs.readFileSync(require.resolve('../crates/dir-simulator/src/tool/ned-editor/assets/view.js'), 'utf8'), context);
  const app = { buffers: new Map(), breadcrumbs: [], scopeGeneration: '1', typeKey: 'type', view: { export_roots: [{ id: 'root' }], graph: { kind: 'module', type_name: 'Example', nodes: [], own_gates: [], connections: [] } } };
  return { nodes, app, render: () => context.NEDEditorView.render(app, { writer: true }, {}) };
}

test('recovery-only lock remains visible even when no journal can be listed', () => {
  const view = { recovery_only: true, recovery: [], analysis: 'unchecked' };
  assert.equal(V.recoveryRequired(view), true);
  assert.equal(V.displayState({ view, buffers: new Map() }), '保存の復旧待ち');
  const h = renderer(); Object.assign(h.app.view, view); h.render();
  assert.equal(h.nodes.get('save-as').disabled, true);
  assert.equal(h.nodes.get('source-editor').readOnly, true);
});

test('completed publication awaiting registry checkpoint offers only finalization', () => {
  const h = renderer();
  h.app.view.recovery = [{ id: 'save-id', path: '/exports/project', state: 'finalizing' }];
  h.render();
  const buttons = h.nodes.get('recovery').children[0].children.filter(node => node.tagName === 'button');
  assert.equal(h.nodes.get('recovery-section').hidden, false);
  assert.equal(buttons[0].textContent, '保存を完了'); assert.equal(buttons[0].disabled, false);
  assert.equal(buttons[1].textContent, '保存前へ戻す'); assert.equal(buttons[1].disabled, true);
});

test('invalid journal remains visible with diagnostic and neither destructive recovery action', () => {
  const h = renderer();
  h.app.view.recovery = [{ id: 'save-id', path: '/state/recovery/save-id/manifest.json', state: 'invalid', message: 'Invalid recovery schema/state' }];
  h.render();
  const children = h.nodes.get('recovery').children[0].children;
  assert.ok(children.some(node => node.textContent === 'Invalid recovery schema/state'));
  assert.ok(children.filter(node => node.tagName === 'button').every(node => node.disabled));
});

test('boundary port addition is enabled only on module declarations', () => {
  const h = renderer();
  for (const [kind, enabled] of [['module', true], ['network', false], ['simple', false], ['module', true]]) {
    h.app.view.graph.kind = kind; h.render();
    assert.equal(h.nodes.get('add-port').disabled, !enabled, kind);
    assert.equal(h.nodes.get('create-module').disabled, !['module', 'network'].includes(kind), kind);
  }
});
