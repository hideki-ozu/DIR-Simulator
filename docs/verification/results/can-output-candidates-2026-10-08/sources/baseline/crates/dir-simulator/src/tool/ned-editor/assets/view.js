/* B6: render only the backend projection. Names are always text, never markup. */
(function (root, factory) {
  'use strict';
  const api = factory(root);
  if (typeof module === 'object' && module.exports) module.exports = api;
  else root.NEDEditorView = api;
})(typeof globalThis === 'object' ? globalThis : this, function (root) {
  'use strict';
  const NS = 'http://www.w3.org/2000/svg';
  const labels = { replace_source: 'ソース反映', add_child: '子の追加', delete_child: '子の削除', connect: '接続', reconnect: '再接続', disconnect: '接続の削除', connect_can_pair: 'CAN接続', set_default: 'default変更', set_layout: '配置変更', undo: '元に戻す', redo: 'やり直し', validate: 'エラーチェック', save_as_project: '別フォルダ保存', overwrite_project: '上書き保存', reload: '再読込', recover: '保存の復旧' };
  const commandLabel = kind => labels[kind] || '処理';
  labels.new_project = '新規プロジェクト作成';
  Object.assign(labels, { create_module: '複合モジュール作成', add_gates: 'gate追加', delete_gate: 'gate削除', add_port: '境界ポート追加', set_gateway: 'Gateway設定', delete_gateway: 'Gateway設定解除', set_workload: '送信設定', set_project_settings: 'ネットワーク設定', set_instance_parameter: '実体値設定' });
  function sourceRole(file) {
    const roles = { ned: 'NED', config: 'INI 設定', workload: 'JSON ワークロード', model_config: 'JSON モデル設定' };
    return roles[file?.role] || 'NED';
  }
  function sourceSyntax(file) {
    return (!file?.role || file.role === 'ned') ? sourceRole(file) + ' · ' + textValue(file?.syntax) : sourceRole(file);
  }
  const textValue = value => value === null || value === undefined ? '—' : typeof value === 'object' ? JSON.stringify(value) : String(value);
  const finite = (value, fallback) => typeof value === 'number' && Number.isFinite(value) ? value : fallback;
  function nodeGeometry(node, index = 0) {
    const inputs = (node.gates || []).filter(g => !g.output), outputs = (node.gates || []).filter(g => g.output);
    return { x: finite(node.x, 80 + index % 4 * 240), y: finite(node.y, 100 + Math.floor(index / 4) * 160), width: 180, height: Math.max(76, 50 + Math.max(inputs.length, outputs.length) * 19), inputs, outputs };
  }
  function bounds(graph) {
    const nodes = graph?.nodes || [];
    if (!nodes.length) return { x: 20, y: 40, width: 660, height: Math.max(220, (graph?.own_gates?.length || 0) * 23 + 80) };
    const geometries = nodes.map(nodeGeometry);
    const x = Math.min(...geometries.map(n => n.x)) - 65, y = Math.min(...geometries.map(n => n.y)) - 50;
    return { x, y, width: Math.max(360, Math.max(...geometries.map(n => n.x + n.width)) - x + 65), height: Math.max(200, Math.max(...geometries.map(n => n.y + n.height)) - y + 50, (graph?.own_gates?.length || 0) * 23 + 80) };
  }
  function ports(graph) {
    if (!graph) return [];
    const result = [];
    (graph.nodes || []).forEach((node, index) => {
      const geometry = nodeGeometry(node, index);
      for (const [side, gates] of [[false, geometry.inputs], [true, geometry.outputs]]) gates.forEach((gate, i) => {
        result.push({ key: gate.key, endpoint: gate.endpoint || node.name + '.' + gate.name, name: gate.name, nodeKey: node.key, output: !!gate.output, start: !!gate.output, own: false, x: geometry.x + (side ? geometry.width : 0), y: geometry.y + 47 + i * 19 });
      });
    });
    const box = bounds(graph), own = graph.own_gates || [];
    for (const output of [false, true]) own.filter(g => !!g.output === output).forEach((gate, i) => {
      result.push({ key: gate.key, endpoint: gate.endpoint || gate.name, name: gate.name, output, start: !output, own: true, x: box.x + (output ? box.width : 0), y: box.y + 40 + i * 23 });
    });
    return result;
  }
  function findPort(list, endpoint) { return list.find(p => p.endpoint === endpoint || p.key === endpoint); }
  function path(start, end) {
    const bend = Math.max(40, Math.abs(end.x - start.x) * .45);
    return `M ${start.x} ${start.y} C ${start.x + bend} ${start.y}, ${end.x - bend} ${end.y}, ${end.x} ${end.y}`;
  }
  function displayState(app) {
    const view = app.view || {};
    if (recoveryRequired(view)) return '保存の復旧待ち';
    if (app.operation) return app.operation + '…';
    if (view.busy) return commandLabel(view.busy) + '中';
    if ([...app.buffers.values()].some(b => b.pending || b.composing)) return '未検証の入力あり';
    const syntax = (view.files || []).map(f => f.syntax);
    if (syntax.some(s => ['error', 'invalid', 'syntax_invalid', 'syntax_error', 'failed'].includes(s)) || view.analysis === 'syntax_error') return '構文エラー・ソースを修正してください';
    if (syntax.some(s => ['pending', 'parsing'].includes(s)) || ['pending', 'parsing'].includes(view.analysis)) return '構文解析中';
    const analysis = { ready: '実行準備OK', valid: '実行準備OK', failed: '全体検証NG', invalid: '全体検証NG', unchecked: '全体未検証', unvalidated: '全体未検証', unverified: '全体未検証', stale: '全体未検証', idle: '全体未検証' };
    return analysis[view.analysis] || '全体未検証';
  }
  function capability(view, kind) {
    const value = Array.isArray(view?.capabilities) ? view.capabilities.find(c => c.kind === kind) : view?.capabilities?.[kind];
    if (typeof value === 'boolean') return { enabled: value };
    return value || { enabled: true };
  }
  function recoveryRequired(view) { return view?.recovery_only === true || (view?.recovery || []).some(r => !['completed', 'restored', 'complete', 'resolved'].includes(r.state)); }
  function selectedInstance(app) {
    if (!app.instancePath) return null;
    return (app.view?.instances || []).find(i => i.path === app.instancePath && i.type_key === app.typeKey && !i.unresolved) || null;
  }
  function instanceStructuralBlocked(app) {
    if (!app.instancePath) return false;
    const instance = selectedInstance(app);
    return !instance || !!instance.cycle;
  }
  function filteredInstances(instances, filter = '') {
    const query = filter.trim().toLowerCase();
    if (!query) return instances;
    const visible = new Set();
    for (const instance of instances) {
      if (!(instance.path + ' ' + instance.type_name).toLowerCase().includes(query)) continue;
      let path = instance.path;
      while (path) { visible.add(path); const end = path.lastIndexOf('.'); path = end < 0 ? '' : path.slice(0, end); }
    }
    return instances.filter(i => visible.has(i.path));
  }
  function parameterValues(app, parameter) {
    if (app.scopeLoading) return ['読込中', '読込中'];
    if (!app.instancePath) return ['インスタンスを選択してください', 'インスタンスを選択してください'];
    if (!selectedInstance(app) || app.view?.graph?.type_key !== app.typeKey || parameter.instance_path !== app.instancePath) return ['選択したインスタンスの値は未取得', '選択したインスタンスの値は未取得'];
    const pending = [...app.buffers.values()].some(b => b.pending || b.composing);
    return [parameter.override, pending ? '未反映の入力あり' : parameter.effective ?? '実効値未取得'];
  }
  let scene = { app: null, callbacks: null, scope: null, zoom: 1, panX: 0, panY: 0, gesture: null, preview: null, fitted: false };
  const document = root.document;
  const $ = id => document.getElementById(id);
  function element(tag, value, className) {
    const result = document.createElement(tag);
    if (value !== undefined && value !== null) result.textContent = value;
    if (className) result.className = className;
    return result;
  }
  function svg(tag, attributes = {}, value) {
    const result = document.createElementNS(NS, tag);
    for (const [key, attr] of Object.entries(attributes)) result.setAttribute(key, String(attr));
    if (value !== undefined) result.textContent = value;
    return result;
  }
  function button(label, action, className = '', disabled = false) {
    const result = element('button', label, className); result.type = 'button'; result.disabled = disabled;
    result.onclick = action; return result;
  }
  function options(select, values, chosen) {
    select.replaceChildren();
    for (const item of values) {
      const option = element('option', item.label ?? item.value); option.value = String(item.value); select.append(option);
    }
    if (chosen !== undefined && chosen !== null) select.value = String(chosen);
  }
  function fields(container, rows) {
    const list = element('dl');
    for (const [name, value] of rows) list.append(element('dt', name), element('dd', textValue(value)));
    container.append(list);
  }
  function dialog(config) {
    const dialog = $('editor-dialog');
    if (dialog.open) return Promise.reject(new Error('開いている操作を完了してください。'));
    $('dialog-title').textContent = config.title; $('dialog-submit').textContent = config.submit || '実行';
    $('dialog-submit').disabled = false; $('dialog-error').hidden = true;
    const body = $('dialog-body'); body.replaceChildren();
    if (config.text) body.append(element('p', config.text));
    if (config.impacts?.length) {
      const list = element('ul'); for (const impact of config.impacts) list.append(element('li', impact)); body.append(list);
    }
    if (config.pre !== undefined) body.append(element('pre', config.pre));
    function fieldControl(field, container) {
      const label = element('label', field.label); label.htmlFor = field.id;
      if (field.checkboxes) {
        const wrapper = element('fieldset', null, 'checkbox-field'); wrapper.append(element('legend', field.label));
        const group = element('div', null, 'checkbox-options'); group.id = field.id;
        const read = () => [...group.querySelectorAll('input:checked')].map(input => input.value);
        const refreshChoices = (chosen = read()) => {
          group.replaceChildren();
          for (const choice of typeof field.checkboxes === 'function' ? field.checkboxes() : field.checkboxes) {
            const item = element('label', null, 'checkbox-option'), input = element('input'); input.type = 'checkbox'; input.value = choice.value;
            input.checked = chosen.includes(choice.value); item.append(input, element('span', choice.label || choice.value)); group.append(item);
          }
        };
        refreshChoices(field.value || []);
        wrapper.append(group); container.append(wrapper);
        return { element: group, read, refreshChoices: typeof field.checkboxes === 'function' ? () => refreshChoices() : null, setActive(active) { wrapper.hidden = !active; for (const input of group.querySelectorAll('input')) input.disabled = !active; } };
      }
      const input = element(field.choices ? 'select' : 'input'); input.id = field.id;
      if (field.choices) { options(input, typeof field.choices === 'function' ? field.choices() : field.choices, field.value); input.required = true; }
      else { input.type = 'text'; input.value = field.value ?? ''; input.required = !!field.required; input.autocomplete = 'off'; if (field.placeholder) input.placeholder = field.placeholder; }
      if (field.pattern) input.pattern = field.pattern;
      if (field.suggestions?.length) {
        const list = element('datalist'); list.id = field.id + '-suggestions';
        for (const item of field.suggestions) { const option = element('option', typeof item === 'object' ? item.label : null); option.value = typeof item === 'object' ? item.value : item; list.append(option); }
        input.setAttribute('list', list.id); container.append(list);
      }
      label.append(input); container.append(label);
      if (field.hint) container.append(element('p', field.hint, 'hint'));
      return { element: input, read: () => input.value, refreshChoices: typeof field.choices === 'function' ? () => options(input, field.choices(), input.value) : null, setActive(active) { label.hidden = !active; input.disabled = !active; } };
    }
    const controls = new Map((config.fields || []).map(field => [field.id, fieldControl(field, body)]));
    const collections = new Map();
    const dependentControls = [];
    for (const section of config.sections || []) {
      const container = element('section', null, 'form-section'), rows = [], list = element('div', null, 'form-rows');
      container.append(element('h3', section.label), list); body.append(container);
      let sequence = 0;
      const add = initial => {
        const row = element('fieldset', null, 'form-row'), rowControls = new Map();
        row.dataset.rowId = section.id + '-' + ++sequence;
        row.append(element('legend', section.rowLabel || section.label));
        for (const field of section.fields) {
          const control = fieldControl({ ...field, id: row.dataset.rowId + '-' + field.id, value: initial?.[field.id] ?? field.value }, row);
          rowControls.set(field.id, control);
          if (control.refreshChoices) dependentControls.push(control);
        }
        const read = () => Object.fromEntries([...rowControls].map(([id, control]) => [id, control.read()]));
        const update = () => {
          const values = read();
          for (const field of section.fields) if (field.when) rowControls.get(field.id).setActive(field.when(values));
        };
        for (const control of rowControls.values()) control.element.addEventListener('change', update);
        const record = { row, read }; rows.push(record);
        row.append(button('行を削除', () => {
          rows.splice(rows.indexOf(record), 1);
          for (const control of rowControls.values()) if (control.refreshChoices) dependentControls.splice(dependentControls.indexOf(control), 1);
          row.remove();
        }, 'danger'));
        list.append(row); update();
      };
      for (const row of section.rows || []) add(row);
      container.append(button(section.addLabel || '行を追加', () => add(section.defaultRow || {})));
      collections.set(section.id, () => rows.map(row => row.read()));
    }
    for (const control of controls.values()) control.element.addEventListener('change', () => { for (const dependent of dependentControls) dependent.refreshChoices(); });
    if (config.setup) config.setup();
    dialog.showModal();
    return new Promise(resolve => {
      let done = false;
      const finish = value => {
        if (done) return; done = true;
        dialog.removeEventListener('cancel', cancel); dialog.removeEventListener('close', close);
        dialog.close(); $('dialog-form').onsubmit = null; resolve(value);
      };
      const cancel = event => { event.preventDefault(); finish(null); }, close = () => { if (!dialog.open) finish(null); };
      $('dialog-cancel').onclick = () => finish(null); $('dialog-close').onclick = () => finish(null);
      dialog.addEventListener('cancel', cancel); dialog.addEventListener('close', close);
      $('dialog-form').onsubmit = event => {
        event.preventDefault(); const result = {};
        if (!$('dialog-form').reportValidity()) return;
        for (const [id, control] of controls) result[id] = control.read();
        for (const [id, read] of collections) result[id] = read();
        try { if (config.validate) config.validate(result); }
        catch (error) { $('dialog-error').textContent = error.message; $('dialog-error').hidden = false; return; }
        finish(result);
      };
      const focus = body.querySelector('input,select') || $('dialog-submit'); focus.focus();
    });
  }
  function selectionFresh(app) {
    const selected = app.selection, graph = app.view?.graph;
    if (!selected || !graph) return selected;
    const collection = selected.kind === 'node' ? graph.nodes : selected.kind === 'connection' ? graph.connections : null;
    if (collection) {
      const item = collection.find(item => item.key === selected.item.key);
      if (!item) { app.selection = null; return null; }
      app.selection = { ...selected, item };
    }
    return app.selection;
  }
  function render(app, client, callbacks) {
    if (!document) return;
    const view = app.view || {}, graph = view.graph, b = app.buffers.get(app.fileId);
    const pending = [...app.buffers.values()].some(buffer => buffer.pending || buffer.composing), blocked = [...app.buffers.values()].some(buffer => buffer.blocked);
    const exclusive = !!app.operation && app.operation !== '入力反映待ち';
    const readonly = !client.writer, recovering = recoveryRequired(view), locked = readonly || exclusive || recovering || !!client.uncertain;
    const generalDisabled = readonly || !!app.operation || !!client.inflight || !!view.busy || recovering;
    const graphDisabled = generalDisabled || app.scopeLoading || !graph || !!graph.stale || pending || instanceStructuralBlocked(app);
    const selected = selectionFresh(app);
    $('project-name').textContent = view.project_name || view.origin || 'NEDプロジェクト'; $('project-name').title = view.origin || view.project_name || '';
    $('network-name').textContent = 'network: ' + (view.network || '未選択') + ' · ' + (app.instancePath ? 'インスタンス参照 / 共有型定義の編集' : '型定義の編集');
    $('writer-status').textContent = readonly ? '参照専用' : '編集可能'; $('writer-status').classList.toggle('warning', readonly);
    $('writer-claim').hidden = !readonly; $('writer-claim').disabled = !!client.inflight || !!app.operation;
    $('origin-path').textContent = view.project_origin === 'new' ? '新規プロジェクト（編集中）' : view.origin || '—';
    const exports = (view.outputs || []).filter(o => o.kind === 'managed_export');
    $('output-path').textContent = exports.length ? exports.map(o => o.path).join('\n') : '未出力';
    $('status').textContent = displayState(app) + ' · ' + (view.dirty ? '未保存' : '保存済み／変更なし') + (view.never_exported ? ' · 別フォルダへ未出力' : '') + ' · 版 ' + (view.revision || '—') + ' / 入力 ' + (view.input_revision || '—');
    for (const control of document.querySelectorAll('[data-command]')) {
      const cap = capability(view, control.dataset.command);
      control.disabled = generalDisabled || cap.enabled === false || (control.hasAttribute('data-structural') && graphDisabled) || (control.hasAttribute('data-layout') && graphDisabled);
      control.title = cap.enabled === false ? (cap.reason || '現在利用できません') : '';
    }
    $('undo').disabled ||= !view.can_undo; $('redo').disabled ||= !view.can_redo;
    $('overwrite').disabled ||= !(view.outputs || []).some(o => ['source_project', 'managed_export'].includes(o.kind));
    $('save-as').disabled ||= !(view.export_roots || []).length;
    $('reload').disabled ||= view.project_origin === 'new';
    $('new-project').disabled ||= !(view.templates || []).length;
    const compound = ['module', 'network'].includes(graph?.kind);
    $('create-module').disabled ||= !compound;
    $('add-port').disabled ||= graph?.kind !== 'module';
    $('add-gates').disabled ||= !graph?.gate_editable;
    $('delete-gate').disabled ||= !graph?.gate_editable || !(graph.own_gates || []).length;
    for (const id of ['project-settings', 'gateway-settings', 'workload-settings']) $(id).disabled ||= pending || app.scopeLoading;
    for (const id of ['gateway-settings', 'workload-settings']) {
      $(id).disabled ||= !!view.configuration_errors?.length;
      if (view.configuration_errors?.length) $(id).title = '関連JSONを読み取れません。ソースと診断を修正してください。';
    }
    $('delete-selection').disabled = graphDisabled || !['node', 'connection'].includes(selected?.kind);
    $('flush-source').disabled = generalDisabled || !pending || blocked;
    $('pending-actions').hidden = !client.uncertain && !blocked;
    $('pending-label').textContent = client.uncertain ? '送信した操作の結果が未確定です。ローカル入力は保持しています。' : '反映できない入力を保持しています。診断と最新原文を確認してください。';
    $('check-command').hidden = !client.uncertain; $('check-command').disabled = !!app.operation;
    $('retry-input').hidden = !!client.uncertain || !blocked; $('retry-input').disabled = readonly || !!app.operation;
    const fileList = $('files'); fileList.replaceChildren();
    for (const file of view.files || []) {
      const local = app.buffers.get(file.id);
      const row = button('', () => callbacks.file(file), file.id === app.fileId ? 'active' : '');
      if (file.dirty || local?.pending) row.append(element('span', '● ', 'dirty-mark'));
      row.append(element('span', file.path)); row.title = file.path;
      row.append(element('small', sourceSyntax(file))); fileList.append(row);
    }
    const types = $('types'); types.replaceChildren();
    for (const type of view.types || []) {
      if (app.filter && !type.name.toLowerCase().includes(app.filter.toLowerCase())) continue;
      const row = button(type.name, () => callbacks.type(type), !app.instancePath && type.key === app.typeKey ? 'active' : '');
      row.append(element('small', type.kind)); types.append(row);
    }
    const instances = $('instances'); instances.replaceChildren();
    const typesByKey = new Map((view.types || []).map(t => [t.key, t]));
    for (const instance of filteredInstances(view.instances || [], app.instanceFilter)) {
      const type = typesByKey.get(instance.type_key), unresolved = instance.unresolved || !type || type.name !== instance.type_name;
      const depth = Math.min(8, Math.max(0, Math.trunc(finite(instance.depth, 0))));
      const row = button(instance.path.split('.').pop() + ' · ' + instance.type_name, () => callbacks.instance(instance), 'instance-row depth-' + depth + (instance.path === app.instancePath ? ' active' : ''), unresolved);
      row.dataset.instancePath = instance.path;
      row.title = instance.path + (unresolved ? '（型が未解決）' : instance.cycle ? '（循環参照・展開停止）' : '');
      if (instance.path === app.instancePath) row.setAttribute('aria-current', 'true');
      row.append(element('small', instance.path + (unresolved ? ' · 未解決' : instance.cycle ? ' · 循環参照' : ''))); instances.append(row);
    }
    if (!instances.childElementCount) instances.append(element('p', (view.instances || []).length ? '一致するインスタンスはありません。' : 'networkの階層を取得できません。型定義と診断を確認してください。', 'hint'));
    const builtinPalette = $('builtin-palette'); builtinPalette.replaceChildren();
    for (const type of view.catalog || []) {
      const channel = type.kind === 'channel';
      const row = button((channel ? '↗ ' : '+ ') + (type.label || type.name), () => channel ? callbacks.connect(type.id) : callbacks.addChild(type.id), channel ? 'channel-choice' : '', graphDisabled || capability(view, channel ? 'connect' : 'add_child').enabled === false);
      row.title = (type.description || type.name) + (channel ? ' · 接続時に選択' : ''); row.dataset.catalogId = type.id; builtinPalette.append(row);
    }
    const palette = $('palette'); palette.replaceChildren();
    for (const type of view.types || []) if (['simple', 'module'].includes(type.kind)) {
      const row = button('+ ' + type.name, () => callbacks.addChild(type.name), '', graphDisabled || capability(view, 'add_child').enabled === false);
      row.title = type.name; palette.append(row);
    }
    if (!palette.childElementCount) palette.append(element('p', '配置できるプロジェクトの型はありません。', 'hint'));
    $('source-editor').disabled = !b;
    $('source-editor').readOnly = locked;
    // Do not assign during composition, or move the caret for unchanged text.
    if (b && !b.composing && $('source-editor').value !== b.visible) {
      const source = $('source-editor'), start = source.selectionStart, end = source.selectionEnd, top = source.scrollTop, left = source.scrollLeft;
      source.value = b.visible;
      source.setSelectionRange(Math.min(start, source.value.length), Math.min(end, source.value.length)); source.scrollTop = top; source.scrollLeft = left;
    } else if (!b && !app.buffers.has(app.fileId)) $('source-editor').value = '';
    const file = (view.files || []).find(f => f.id === app.fileId);
    $('source-title').textContent = file?.path || 'ソース';
    const inputState = b?.composing ? 'IME変換中・反映待ち' : b?.blocked ? '入力を保持・反映停止' : b?.sent ? '入力を反映中' : b?.pending ? '未送信の入力' : readonly ? '参照専用' : '入力を反映済み';
    $('source-state').textContent = file ? sourceSyntax(file) + ' · ' + inputState : inputState;
    $('source-editor').setAttribute('aria-label', sourceRole(file) + 'ソース編集');
    if (b) {
      const table = root.NEDEditorInput.shadow(b.raw), formats = [...new Set(table.newlines.map(n => n.style === '\r\n' ? 'CRLF' : n.style === '\r' ? 'CR' : 'LF'))];
      $('source-format').textContent = (table.bom ? 'BOMあり · ' : '') + (formats.join(' / ') || '改行なし') + ' · 原文保持';
    }
    $('graph-title').textContent = graph?.type_name || '構成図';
    $('graph-state').textContent = app.scopeLoading ? '選択した対象を読込中' : graph?.stale ? '過去の正常な図・現在のソースと不一致' : instanceStructuralBlocked(app) ? '階層が未解決または循環参照・構造編集は利用できません' : pending ? '未反映の入力あり・図の編集は反映後に利用できます' : graph ? (app.instancePath ? app.instancePath + ' · 共有型定義の直下構造' : '型定義の直下構造') : '型を選択してください';
    const breadcrumbs = $('breadcrumbs'); breadcrumbs.replaceChildren();
    app.breadcrumbs.forEach((item, index) => { breadcrumbs.append(button(item.name, () => callbacks.breadcrumb(index)), element('span', '›')); });
    $('graph-empty').hidden = !!graph;
    scene.app = app; scene.callbacks = callbacks;
    const scope = (view.project_name || '') + ':' + app.scopeGeneration + ':' + app.typeKey + ':' + (app.instancePath || '');
    if (scene.scope !== scope) { scene.scope = scope; scene.fitted = false; scene.preview = null; scene.lastNodeClick = null; }
    if (!scene.gesture) drawGraph();
    $('selection-name').textContent = selected?.item?.name || selected?.item?.key || selected?.port?.endpoint || '';
    renderProperties(app, callbacks, graphDisabled);
    renderDiagnostics(app, callbacks);
    renderRecovery(view, callbacks, client);
    const capabilities = $('capabilities'); capabilities.replaceChildren();
    capabilities.append(element('p', '通信上限: ' + textValue(view.limits?.body_bytes) + ' bytes'), element('p', '履歴上限: ' + textValue(view.limits?.history_bytes) + ' bytes / ' + textValue(view.limits?.history_operations) + ' 操作'));
    for (const [kind, cap] of Object.entries(view.capabilities || {})) {
      if (cap.enabled === false) capabilities.append(element('p', commandLabel(kind) + ': ' + (cap.reason || '利用不可')));
    }
  }
  function renderProperties(app, callbacks, graphDisabled) {
    const selection = app.selection, container = $('selection-properties'); container.replaceChildren();
    if (selection?.kind === 'node') {
      const node = selection.item;
      fields(container, [['名前', node.name], ['型', node.type_name], ['状態', node.unresolved ? '未解決の型' : '解決済み'], ['座標', textValue(node.x) + ', ' + textValue(node.y)]]);
      const list = element('div'); list.append(element('h3', 'gate'));
      for (const gate of node.gates || []) list.append(element('p', gate.name + ' · ' + (gate.output ? 'output' : 'input'), 'hint'));
      container.append(list);
      const actions = element('div', null, 'property-actions');
      actions.append(button(app.instancePath ? '子インスタンスを開く' : '型内部を開く', () => callbacks.openNode(node), '', !!node.unresolved || app.scopeLoading || (app.instancePath && (!!app.view?.graph?.stale || instanceStructuralBlocked(app)))), button(node.collapsed ? '展開表示' : '折り畳む', () => callbacks.collapse(node), '', graphDisabled), button('子を削除', callbacks.deleteSelection, 'danger', graphDisabled)); container.append(actions);
    } else if (selection?.kind === 'connection') {
      const c = selection.item; fields(container, [['始点', c.start], ['終点', c.end], ['channel', c.channel]]);
      const actions = element('div', null, 'property-actions');
      actions.append(button('端点・channel変更', () => callbacks.reconnect(c), '', graphDisabled), button('接続を削除', callbacks.deleteSelection, 'danger', graphDisabled)); container.append(actions);
    } else if (selection?.kind === 'port') {
      const p = selection.port; fields(container, [['gate', p.endpoint], ['方向', p.output ? 'output' : 'input'], ['内部の役割', p.start ? '始点' : '終点'], ['所属', p.own ? '型自身の境界' : '直下の子']]);
    } else container.append(element('p', '子や接続を選択してください。', 'hint'));
    const parameters = $('parameters'); parameters.replaceChildren();
    $('parameter-context').textContent = app.instancePath ? 'インスタンス: ' + app.instancePath + (app.scopeLoading ? '（読込中）' : !selectedInstance(app) ? '（対応する型を確認できません）' : '') : '型定義: ' + ((app.view?.types || []).find(t => t.key === app.typeKey)?.name || '未選択') + '（インスタンス未選択）';
    for (const parameter of app.scopeLoading ? [] : app.view?.parameters || []) {
      const box = element('article', null, 'parameter'); box.append(element('strong', parameter.name), element('small', ' · ' + parameter.scalar + (parameter.unit ? ' / ' + parameter.unit : '')));
      const [override, effective] = parameterValues(app, parameter);
      fields(box, [['default', parameter.default], ['INI上書き', override], ['実効値', effective]]);
      box.append(button('defaultを編集', () => callbacks.setDefault(parameter), '', graphDisabled || capability(app.view, 'set_default').enabled === false)); parameters.append(box);
      if (app.instancePath) box.append(button('実体値を設定/解除', () => callbacks.instanceParameter(parameter), '', graphDisabled || parameter.instance_path !== app.instancePath || capability(app.view, 'set_instance_parameter').enabled === false));
    }
    if (!parameters.childElementCount) parameters.append(element('p', app.scopeLoading ? 'パラメータを読込中…' : 'この型のパラメータはありません。', 'hint'));
    const connections = $('connections'); connections.replaceChildren();
    for (const connection of app.view?.graph?.connections || []) {
      const row = button(connection.start + ' → ' + connection.end, () => callbacks.select({ kind: 'connection', item: connection }), app.selection?.kind === 'connection' && app.selection.item.key === connection.key ? 'active' : '');
      if (connection.channel) row.append(element('small', connection.channel)); connections.append(row);
    }
  }
  function renderDiagnostics(app, callbacks) {
    const diagnostics = app.view?.diagnostics || [], container = $('diagnostics'); container.replaceChildren();
    $('diagnostic-count').textContent = String(diagnostics.length);
    if (!diagnostics.length) container.append(element('p', '診断はありません。実行可能性はエラーチェックで確認します。', 'hint'));
    for (const diagnostic of diagnostics) {
      const b = app.buffers.get(diagnostic.file_id);
      const old = String(diagnostic.input_revision) !== String(app.view.input_revision) || !!b?.pending;
      const row = button('', () => callbacks.diagnostic(diagnostic), 'diagnostic ' + (diagnostic.severity || '') + (old ? ' old' : ''));
      row.append(element('span', diagnostic.origin || 'editor', 'diagnostic-meta'), element('span', diagnostic.code || '', 'diagnostic-meta'));
      const message = element('span', diagnostic.message);
      if (old) message.append(element('small', '（入力版 ' + diagnostic.input_revision + ' の診断）', 'diagnostic-meta'));
      row.append(message); container.append(row);
    }
  }
  function renderRecovery(view, callbacks, client) {
    const records = view.recovery || [], container = $('recovery'); container.replaceChildren(); $('recovery-section').hidden = !records.length;
    for (const record of records) {
      const box = element('article', null, 'recovery-card'); box.append(element('strong', record.path), element('p', record.state));
      if (record.message) box.append(element('p', record.message));
      const inactive = !client.writer || !!client.inflight || ['completed', 'restored', 'complete', 'resolved', 'invalid'].includes(record.state);
      box.append(button('保存を完了', () => callbacks.recover(record, 'complete'), '', inactive || ['restoring', 'restore'].includes(record.state)), button('保存前へ戻す', () => callbacks.recover(record, 'restore'), '', inactive || ['completing', 'complete_started', 'finalizing'].includes(record.state))); container.append(box);
    }
  }
  function effectiveGraph() {
    const graph = scene.app?.view?.graph;
    if (!graph || !scene.preview) return graph;
    return { ...graph, nodes: graph.nodes.map(n => n.key === scene.preview.key ? { ...n, x: scene.preview.x, y: scene.preview.y } : n) };
  }
  function transform() { return `translate(${scene.panX} ${scene.panY}) scale(${scene.zoom})`; }
  function dimensions() {
    const rect = $('graph').getBoundingClientRect(); return { width: Math.max(1, rect.width), height: Math.max(1, rect.height) };
  }
  function fit() {
    const graph = effectiveGraph(); if (!graph) return;
    const box = bounds(graph), size = dimensions();
    scene.zoom = Math.max(.08, Math.min(1.35, (size.width - 40) / box.width, (size.height - 32) / box.height));
    scene.panX = (size.width - box.width * scene.zoom) / 2 - box.x * scene.zoom;
    scene.panY = (size.height - box.height * scene.zoom) / 2 - box.y * scene.zoom;
    scene.fitted = true; drawGraph();
  }
  function zoom(factor, center) {
    const size = dimensions(), x = center?.x ?? size.width / 2, y = center?.y ?? size.height / 2;
    const next = Math.max(.05, Math.min(5, scene.zoom * factor)), ratio = next / scene.zoom;
    scene.panX = x - (x - scene.panX) * ratio; scene.panY = y - (y - scene.panY) * ratio; scene.zoom = next; drawGraph();
  }
  function drawGraph() {
    const graph = effectiveGraph(), canvas = $('graph');
    if (!scene.bound) { bindGraph(canvas); scene.bound = true; }
    canvas.replaceChildren();
    if (!graph) return;
    if (!scene.fitted) { fit(); return; }
    const definitions = svg('defs'), marker = svg('marker', { id: 'ned-arrow', viewBox: '0 0 10 10', refX: 9, refY: 5, markerWidth: 6, markerHeight: 6, orient: 'auto-start-reverse' });
    marker.append(svg('path', { d: 'M 0 0 L 10 5 L 0 10 z', fill: '#7895ba' })); definitions.append(marker); canvas.append(definitions);
    const world = svg('g', { transform: transform() }), box = bounds(graph), allPorts = ports(graph);
    world.append(svg('rect', { x: box.x, y: box.y, width: box.width, height: box.height, rx: 10, class: 'boundary-box' }), svg('text', { x: box.x + 14, y: box.y + 19, class: 'boundary-label' }, graph.type_name + ' · 型境界'));
    let unresolved = 0;
    for (const connection of graph.connections || []) {
      const start = findPort(allPorts, connection.start), end = findPort(allPorts, connection.end);
      if (!start || !end) {
        const label = svg('text', { x: box.x + 15, y: box.y + box.height - 14 - unresolved++ * 15, class: 'boundary-label', 'data-key': connection.key }, '未解決: ' + connection.start + ' → ' + connection.end);
        label.addEventListener('click', event => { event.stopPropagation(); scene.callbacks.select({ kind: 'connection', item: connection }); });
        world.append(label); continue;
      }
      const selected = scene.app.selection?.kind === 'connection' && scene.app.selection.item.key === connection.key;
      const group = svg('g', { class: 'connection' + (selected ? ' selected' : ''), 'data-key': connection.key });
      const d = path(start, end);
      group.append(svg('path', { d, class: 'connection-line', 'marker-end': 'url(#ned-arrow)' }), svg('path', { d, class: 'connection-hit' }));
      if (connection.channel) group.append(svg('text', { x: (start.x + end.x) / 2, y: (start.y + end.y) / 2 - 6, class: 'connection-label', 'text-anchor': 'middle' }, connection.channel));
      group.addEventListener('click', event => { event.stopPropagation(); scene.callbacks.select({ kind: 'connection', item: connection }); });
      group.addEventListener('dblclick', event => { event.stopPropagation(); if (scene.callbacks.canEditGraph()) scene.callbacks.reconnect(connection); });
      world.append(group);
    }
    (graph.nodes || []).forEach((node, index) => {
      const geometry = nodeGeometry(node, index), selected = scene.app.selection?.kind === 'node' && scene.app.selection.item.key === node.key;
      const group = svg('g', { class: 'graph-node' + (selected ? ' selected' : '') + (node.unresolved ? ' unresolved' : ''), 'data-key': node.key, 'data-node': node.name, transform: `translate(${geometry.x} ${geometry.y})`, tabindex: 0, role: 'button', 'aria-label': node.name + ' · ' + node.type_name });
      group.append(svg('rect', { width: geometry.width, height: geometry.height, class: 'node-body' }), svg('text', { x: 12, y: 22, class: 'node-name' }, node.name), svg('text', { x: 12, y: 37, class: 'node-type' }, (node.unresolved ? '? ' : '') + node.type_name));
      if (!node.collapsed) for (const [output, gates] of [[false, geometry.inputs], [true, geometry.outputs]]) gates.forEach((gate, i) => {
        group.append(svg('text', { x: output ? geometry.width - 10 : 10, y: 50 + i * 19, class: 'port-label', 'text-anchor': output ? 'end' : 'start' }, gate.name));
      });
      group.addEventListener('pointerdown', event => beginNode(event, node));
      group.addEventListener('keydown', event => {
        if (event.key === 'Enter') { event.preventDefault(); scene.callbacks.openNode(node); }
        if (event.key === ' ') { event.preventDefault(); scene.callbacks.select({ kind: 'node', item: node }); }
      });
      world.append(group);
    });
    for (const port of allPorts) {
      const selected = scene.app.selection?.kind === 'port' && scene.app.selection.port.key === port.key;
      const group = svg('g', { 'data-port-key': port.key, 'data-endpoint': port.endpoint });
      const dot = svg('circle', { cx: port.x, cy: port.y, r: 5, class: 'port ' + (port.output ? 'output' : 'input') + (selected ? ' selected' : ''), tabindex: 0, role: 'button', 'aria-label': port.endpoint + ' ' + (port.output ? 'output' : 'input') });
      dot.append(svg('title', {}, port.endpoint + ' · ' + (port.output ? 'output' : 'input') + ' · ' + (port.start ? '始点' : '終点')));
      const selectPort = event => { event.stopPropagation(); scene.callbacks.select({ kind: 'port', port }); };
      dot.addEventListener('pointerdown', event => event.stopPropagation()); dot.addEventListener('click', selectPort);
      dot.addEventListener('keydown', event => { if (event.key === 'Enter' || event.key === ' ') { event.preventDefault(); selectPort(event); } });
      group.append(dot);
      if (port.own) group.append(svg('text', { x: port.x + (port.output ? -10 : 10), y: port.y + 4, class: 'port-label', 'text-anchor': port.output ? 'end' : 'start' }, port.name + ' · ' + (port.output ? 'output' : 'input')));
      world.append(group);
    }
    canvas.append(world);
  }
  function coordinates(event) { const box = $('graph').getBoundingClientRect(); return { x: event.clientX - box.left, y: event.clientY - box.top }; }
  function beginNode(event, node) {
    if (event.button !== 0 || scene.gesture) return;
    event.preventDefault(); event.stopPropagation();
    const at = coordinates(event), geometry = nodeGeometry(node);
    scene.gesture = { kind: 'node', node, pointerId: event.pointerId, x: at.x, y: at.y, startX: geometry.x, startY: geometry.y, revision: scene.app.view.revision, editable: scene.callbacks.canEditGraph(), moved: false };
    scene.callbacks.gesture(true); $('graph').setPointerCapture(event.pointerId);
    scene.callbacks.select({ kind: 'node', item: node });
  }
  function bindGraph(canvas) {
    canvas.addEventListener('pointerdown', event => {
      if (event.button !== 0 || scene.gesture || event.target.closest('.connection,[data-port-key]')) return;
      const at = coordinates(event); scene.gesture = { kind: 'pan', pointerId: event.pointerId, x: at.x, y: at.y, startX: scene.panX, startY: scene.panY, moved: false };
      scene.callbacks.gesture(true); canvas.setPointerCapture(event.pointerId);
    });
    canvas.addEventListener('pointermove', event => {
      const gesture = scene.gesture; if (!gesture || gesture.pointerId !== event.pointerId) return;
      const at = coordinates(event), dx = at.x - gesture.x, dy = at.y - gesture.y;
      if (Math.hypot(dx, dy) > 3) gesture.moved = true;
      if (gesture.kind === 'pan') { scene.panX = gesture.startX + dx; scene.panY = gesture.startY + dy; }
      else if (gesture.editable && gesture.moved) scene.preview = { key: gesture.node.key, x: Math.round(gesture.startX + dx / scene.zoom), y: Math.round(gesture.startY + dy / scene.zoom) };
      drawGraph();
    });
    const finish = (event, cancelled) => {
      const gesture = scene.gesture; if (!gesture || gesture.pointerId !== event.pointerId) return;
      const preview = scene.preview; scene.gesture = null; scene.preview = null; scene.callbacks.gesture(false);
      if (canvas.hasPointerCapture(event.pointerId)) canvas.releasePointerCapture(event.pointerId);
      if (!cancelled && gesture.kind === 'node' && gesture.moved && preview && gesture.editable && String(scene.app.view.revision) === String(gesture.revision)) {
        scene.callbacks.layout({ [gesture.node.name]: { x: preview.x, y: preview.y, collapsed: !!gesture.node.collapsed } });
      } else if (!cancelled && gesture.kind === 'node' && !gesture.moved) {
        // Full projection redraws replace SVG elements between clicks. Track the stable element key.
        const now = event.timeStamp;
        if (scene.lastNodeClick?.key === gesture.node.key && now - scene.lastNodeClick.time < 400) {
          scene.lastNodeClick = null; scene.callbacks.openNode(gesture.node);
        } else scene.lastNodeClick = { key: gesture.node.key, time: now };
      } else if (!cancelled && gesture.kind === 'pan' && !gesture.moved) scene.callbacks.select(null);
      drawGraph();
    };
    canvas.addEventListener('pointerup', event => finish(event, false)); canvas.addEventListener('pointercancel', event => finish(event, true));
    canvas.addEventListener('lostpointercapture', event => { if (scene.gesture) finish(event, true); });
    canvas.addEventListener('wheel', event => { event.preventDefault(); if (!scene.gesture) zoom(event.deltaY < 0 ? 1.12 : 1 / 1.12, coordinates(event)); }, { passive: false });
    if (typeof root.ResizeObserver === 'function') new root.ResizeObserver(() => { if (!scene.gesture) drawGraph(); }).observe(canvas);
  }
  return { textValue, commandLabel, sourceRole, sourceSyntax, nodeGeometry, bounds, ports, findPort, path, displayState, capability, recoveryRequired, selectedInstance, instanceStructuralBlocked, filteredInstances, parameterValues, render, options, dialog, zoom, fit };
});
