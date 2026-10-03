'use strict';
// UI-only real-browser construction and standalone execution of a three-bus Gateway fanout.
const assert = require('node:assert/strict');
const fs = require('node:fs');
const os = require('node:os');
const path = require('node:path');
const {spawn, spawnSync} = require('node:child_process');
const {once} = require('node:events');

const repo = path.resolve(__dirname, '..');
let playwright;
try { playwright = require(process.env.PLAYWRIGHT_MODULE || 'playwright'); }
catch (error) { if (process.env.PLAYWRIGHT_MODULE || error.code !== 'MODULE_NOT_FOUND') throw error; }
const binary = process.env.DIR_SIMULATOR_BIN || path.join(repo, 'target/debug/dir-simulator');
const sleep = ms => new Promise(resolve => setTimeout(resolve, ms));
const redact = value => String(value)
  .replace(/(https?:\/\/[^\s#]+)#[^\s"'<>]*/gi, '$1#[redacted]')
  .replace(/#(?:session|token|secret|editor_session)=[^&\s"'<>]*/gi, '#session=[redacted]');

function projectSummary(view) {
  return {
    project_name: view?.project_name,
    network: view?.network,
    analysis: view?.analysis,
    revision: view?.revision,
    input_revision: view?.input_revision,
    graph: view?.graph && {
      type_name: view.graph.type_name,
      nodes: (view.graph.nodes || []).map(node => ({name: node.name, type_name: node.type_name})),
      connections: (view.graph.connections || []).map(connection => ({start: connection.start, end: connection.end})),
      own_gates: (view.graph.own_gates || []).map(gate => ({name: gate.name, output: gate.output})),
    },
    diagnostics: (view?.diagnostics || []).map(diagnostic => ({code: diagnostic.code, stage: diagnostic.stage, message: diagnostic.message})),
  };
}

async function main() {
  assert.ok(fs.existsSync(binary), 'Build dir-simulator before running this browser test.');
  const root = fs.mkdtempSync(path.join(os.tmpdir(), 'dir-editor-composition-'));
  const input = path.join(root, 'input');
  const exports = path.join(root, 'exports');
  const state = path.join(root, 'state');
  const simulationOutput = path.join(root, 'simulation');
  fs.mkdirSync(path.join(input, 'models/demo'), {recursive: true});
  fs.mkdirSync(exports);
  fs.mkdirSync(simulationOutput);
  fs.copyFileSync(path.join(repo, 'examples/can/baseline.ini'), path.join(input, 'project.ini'));
  fs.copyFileSync(path.join(repo, 'examples/can/baseline.json'), path.join(input, 'baseline.json'));
  fs.copyFileSync(path.join(repo, 'examples/can/models/demo/Main.ned'), path.join(input, 'models/demo/Main.ned'));

  const editor = spawn(binary, ['ned-editor', '--config', path.join(input, 'project.ini'), '--export-root', exports, '--state-root', state], {
    cwd: root,
    stdio: ['ignore', 'pipe', 'pipe'],
  });
  const editorClosed = once(editor, 'close');
  let browser;
  const pageErrors = [];
  try {
    const startup = await new Promise((resolve, reject) => {
      let out = '', stderr = '';
      const timer = setTimeout(() => reject(new Error('Editor startup timed out.')), 20000);
      editor.stderr.on('data', bytes => { stderr += bytes; });
      editor.once('error', () => { clearTimeout(timer); reject(new Error('Editor process could not start.')); });
      editor.once('close', (code, signal) => {
        clearTimeout(timer);
        reject(new Error(`Editor exited during startup (code ${code}, signal ${signal || 'none'}): ${redact(stderr)}`));
      });
      editor.stdout.on('data', bytes => {
        out += bytes;
        const newline = out.indexOf('\n');
        if (newline < 0) return;
        clearTimeout(timer);
        try { resolve(JSON.parse(out.slice(0, newline))); }
        catch (_) { reject(new Error('Editor returned an invalid startup response.')); }
      });
    });
    // Keep the capability fragment and session credential in memory only.
    const startupUrl = new URL(startup.url);
    const session = new URLSearchParams(startupUrl.hash.slice(1)).get('session');
    assert.ok(session, 'Editor startup did not provide a session capability.');
    const origin = startupUrl.origin;
    let apiScopeGeneration = 1;
    const rootSessionView = async () => {
      const response = await fetch(origin + '/api/session', {headers: {'X-Editor-Session': session}});
      assert.equal(response.status, 200, `Read-only session view returned HTTP ${response.status}.`);
      return response.json();
    };
    const sessionView = async (scope = {}) => {
      if (!scope.typeName && !scope.typeKey && !scope.instancePath) return rootSessionView();
      const rootView = await rootSessionView();
      const instance = scope.instancePath && rootView.instances?.find(item => item.path === scope.instancePath);
      const type = scope.typeName
        ? rootView.types?.find(item => item.name === scope.typeName)
        : rootView.types?.find(item => item.key === (scope.typeKey || instance?.type_key));
      assert.ok(type, `Read-only view cannot resolve the requested type scope: ${scope.typeName || scope.typeKey || scope.instancePath}.`);
      const query = new URLSearchParams({file_id: type.file_id, type_key: type.key, request_scope_generation: String(apiScopeGeneration++)});
      if (scope.instancePath) query.set('instance_path', scope.instancePath);
      const response = await fetch(origin + '/api/session?' + query, {headers: {'X-Editor-Session': session}});
      assert.equal(response.status, 200, `Read-only scoped session view returned HTTP ${response.status}.`);
      return response.json();
    };

    browser = await playwright.chromium.launch({headless: true});
    const context = await browser.newContext({viewport: {width: 1600, height: 1050}});
    const page = await context.newPage();
    page.setDefaultTimeout(20000);
    page.on('pageerror', error => pageErrors.push(redact(error.message)));
    page.on('console', message => {
      if (message.type() === 'error' && !message.text().startsWith('Failed to load resource')) pageErrors.push(redact(message.text()));
    });
    await page.goto(startup.url);
    await page.waitForFunction(() => !document.getElementById('source-editor').disabled);
    assert.equal(new URL(page.url()).hash, '', 'The UI should remove the startup fragment after capturing its capability.');

    const waitDialog = async title => {
      await page.waitForFunction(expected => {
        const dialog = document.getElementById('editor-dialog');
        return dialog.open && document.getElementById('dialog-title').textContent === expected;
      }, title);
    };
    const selectOrFill = async (id, value) => {
      const field = page.locator(`#${id}`);
      if (await field.evaluate(element => element.tagName === 'SELECT')) await field.selectOption(String(value));
      else await field.fill(String(value));
    };
    const fillDialog = async (fields = {}) => {
      for (const [id, value] of Object.entries(fields)) await selectOrFill(id, value);
    };
    const selectChecks = async (id, values) => {
      const group = page.locator(`#${id}`);
      const wanted = new Set(values);
      const boxes = group.locator('input[type="checkbox"]');
      for (let index = 0; index < await boxes.count(); index++) {
        const box = boxes.nth(index);
        const value = await box.getAttribute('value');
        if (wanted.has(value)) await box.check();
        else if (await box.isChecked()) await box.uncheck();
      }
      assert.deepEqual((await group.locator('input:checked').evaluateAll(items => items.map(item => item.value))).sort(), [...wanted].sort());
    };
    const waitRenderedRevision = async view => {
      await page.waitForFunction(({revision, inputRevision}) => {
        const status = document.getElementById('status').textContent;
        return !document.getElementById('editor-dialog').open &&
          status.includes(`版 ${revision} / 入力 ${inputRevision}`);
      }, {revision: String(view.revision), inputRevision: String(view.input_revision)});
    };
    const waitStableView = async (predicate, description, afterRevision, scope = {}) => {
      const deadline = Date.now() + 30000;
      let previousKey = null;
      let stablePolls = 0;
      let latest;
      while (Date.now() < deadline) {
        latest = await sessionView(scope);
        const revisionAdvanced = afterRevision === undefined || BigInt(latest.revision) > BigInt(afterRevision);
        const stableKey = `${latest.revision}:${latest.input_revision}:${latest.analysis}:${latest.busy || ''}`;
        if (revisionAdvanced && !latest.busy && predicate(latest)) {
          stablePolls = stableKey === previousKey ? stablePolls + 1 : 1;
          if (stablePolls >= 2) {
            await waitRenderedRevision(latest);
            return latest;
          }
        } else stablePolls = 0;
        previousKey = stableKey;
        await sleep(120);
      }
      throw new Error(`${description} did not reach a stable editor revision: ${JSON.stringify(projectSummary(latest))}`);
    };
    const waitForScope = async typeName => {
      await page.waitForFunction(expected => {
        const state = document.getElementById('graph-state').textContent;
        return document.getElementById('graph-title').textContent === expected && !state.includes('読込中');
      }, typeName);
      const view = await sessionView({typeName});
      assert.equal(view.graph?.type_name, typeName, `Read-only view did not reach graph ${typeName}.`);
      return view;
    };
    const observeConfirmation = async (beforeRevision, requiredTitle, allowedTitles = []) => {
      const titles = new Set([...(requiredTitle ? [requiredTitle] : []), ...allowedTitles]);
      const deadline = Date.now() + 30000;
      while (Date.now() < deadline) {
        const state = await page.locator('#editor-dialog').evaluate(dialog => ({open: dialog.open, title: document.getElementById('dialog-title').textContent, confirmation: !document.querySelector('#dialog-body input, #dialog-body select')}));
        if (state.open && state.confirmation && state.title !== '') {
          if (titles.has(state.title)) {
            await page.locator('#dialog-submit').click();
            if (requiredTitle) assert.equal(state.title, requiredTitle, 'A required UI confirmation title changed.');
            return state.title;
          }
        }
        const view = await sessionView();
        if (BigInt(view.revision) > BigInt(beforeRevision)) {
          assert.equal(state.open, false, 'The editor completed a command while an unexpected dialog remained open.');
          if (requiredTitle) throw new Error(`Expected confirmation dialog “${requiredTitle}” was not shown.`);
          return null;
        }
        await sleep(60);
      }
      throw new Error(`The editor did not show a confirmation or complete the command. Expected: ${requiredTitle || [...titles].join(', ') || 'none'}.`);
    };
    const dialogCommand = async ({open, formTitle, fields, checks, confirmTitle, confirmTitles, predicate, description, scope}) => {
      const before = await sessionView();
      await open();
      await waitDialog(formTitle);
      await fillDialog(fields);
      for (const [id, values] of Object.entries(checks || {})) await selectChecks(id, values);
      await page.locator('#dialog-submit').click();
      const confirmation = await observeConfirmation(before.revision, confirmTitle, confirmTitles || []);
      return waitStableView(predicate, description || formTitle, before.revision, scope).then(async view => {
        if (confirmation && confirmTitle) assert.equal(confirmation, confirmTitle);
        return view;
      });
    };
    const settingTitles = ['Gateway設定', '送信設定', 'ネットワーク設定', '実体値設定'];

    let view = await sessionView();
    view = await dialogCommand({
      open: () => page.locator('#new-project').click(),
      formTitle: '新規プロジェクト',
      fields: {'project-template': 'multibus-empty', 'project-name-input': 'Composed'},
      confirmTitle: '新規プロジェクトを作成',
      confirmTitles: ['新規プロジェクトを作成'],
      predicate: current => current.project_name === 'Composed' && current.graph?.type_name === 'Composed.Main' && current.graph.nodes.length === 0,
      description: 'Create New Multibus-empty project',
    });
    assert.equal(view.project_template, 'multibus-empty');

    view = await dialogCommand({
      open: () => page.locator('#create-module').click(),
      formTitle: '複合モジュールを作成',
      fields: {type_name: 'Router', child_name: 'gw'},
      confirmTitle: '型定義の変更',
      predicate: current => current.graph?.type_name === 'Composed.Main' && current.graph.nodes.some(node => node.name === 'gw' && node.type_name === 'Composed.Router'),
      description: 'Create Router module instance gw',
    });
    await page.locator('#graph .graph-node[data-node="gw"]').click();
    await page.locator('#selection-properties button').filter({hasText: '型内部を開く'}).click();
    view = await waitForScope('Composed.Router');

    for (const port of ['a', 'b', 'c']) {
      view = await dialogCommand({
        open: () => page.locator('#add-port').click(),
        formTitle: '境界ポートを追加',
        fields: {child_name: port, input_gate: `receive_${port}`, output_gate: `send_${port}`},
        confirmTitle: '型定義の変更',
        scope: {typeName: 'Composed.Router'},
        predicate: current => current.graph?.type_name === 'Composed.Router' && current.graph.nodes.some(node => node.name === port) &&
          current.graph.own_gates.some(gate => gate.name === `receive_${port}` && !gate.output) &&
          current.graph.own_gates.some(gate => gate.name === `send_${port}` && gate.output),
        description: `Add Router boundary port ${port}`,
      });
    }
    await page.locator('#breadcrumbs button').first().click();
    view = await waitForScope('Composed.Main');

    const addBuiltin = async (catalogId, childName) => {
      return dialogCommand({
        open: () => page.locator(`[data-catalog-id="${catalogId}"]`).click(),
        formTitle: '子を追加',
        fields: {'child-type': catalogId, 'child-name': childName},
        confirmTitle: '型定義の変更',
        predicate: current => current.graph?.type_name === 'Composed.Main' && current.graph.nodes.some(node => node.name === childName),
        description: `Place ${catalogId} child ${childName}`,
      });
    };
    const customizeBusGatePairs = async busName => {
      const current = await sessionView();
      const busNode = current.graph.nodes.find(node => node.name === busName);
      assert.ok(busNode, `Missing ${busName} in read-only project view.`);
      const busType = busNode.type_name;
      await page.locator('#types button').filter({hasText: busType}).first().click();
      await waitForScope(busType);

      const addPair = async (inputName, outputName) => dialogCommand({
        open: () => page.locator('#add-gates').click(),
        formTitle: 'input / output gateを追加',
        fields: {input_gate: inputName, output_gate: outputName},
        confirmTitle: '型定義の変更',
        scope: {typeName: busType},
        predicate: latest => latest.graph?.type_name === busType && latest.graph.own_gates.some(gate => gate.name === inputName && !gate.output) && latest.graph.own_gates.some(gate => gate.name === outputName && gate.output),
        description: `Add ${busType} gate pair ${inputName}/${outputName}`,
      });
      const deletePair = async (inputName, outputName) => dialogCommand({
        open: () => page.locator('#delete-gate').click(),
        formTitle: 'gateを削除',
        fields: {gate_name: inputName, paired_gate: outputName},
        confirmTitle: '型定義の変更',
        scope: {typeName: busType},
        predicate: latest => latest.graph?.type_name === busType && !latest.graph.own_gates.some(gate => gate.name === inputName || gate.name === outputName),
        description: `Delete ${busType} gate pair ${inputName}/${outputName}`,
      });

      // The built-in Bus pair uses rx_* as input and tx_* as output. Add a temporary
      // pair to stay above the Bus minimum while freeing both requested gate names.
      await addPair('_swap_tmp_in', '_swap_tmp_out');
      await deletePair('rx_a', 'tx_a');
      await addPair('tx_a', 'rx_a');
      await deletePair('rx_b', 'tx_b');
      await addPair('tx_b', 'rx_b');
      await deletePair('_swap_tmp_in', '_swap_tmp_out');
      await page.locator('#types button').filter({hasText: 'Composed.Main'}).first().click();
      return waitForScope('Composed.Main');
    };

    for (const busName of ['busA', 'busB', 'busC']) {
      view = await addBuiltin('@builtin:Bus', busName);
      view = await customizeBusGatePairs(busName);
    }
    for (const controllerName of ['endA', 'endB', 'endC']) view = await addBuiltin('@builtin:Controller', controllerName);

    const pairCan = async (controller, bus) => {
      const before = await sessionView();
      await page.locator('#can-pair').click();
      await waitDialog('CAN TX/RXを一組で接続');
      await fillDialog({'can-controller': controller, 'can-bus': bus, 'can-input': 'tx_b', 'can-output': 'rx_b'});
      await page.locator('#dialog-submit').click();
      await observeConfirmation(before.revision, '型定義の変更');
      return waitStableView(current => current.graph?.type_name === 'Composed.Main' && current.graph.connections.length === before.graph.connections.length + 2,
        `CAN-pair ${controller} to ${bus}`, before.revision);
    };
    for (const [controller, bus] of [['endA', 'busA'], ['endB', 'busB'], ['endC', 'busC']]) view = await pairCan(controller, bus);

    const connect = async (from, to) => dialogCommand({
      open: () => page.locator('#connect').click(),
      formTitle: 'gateを接続',
      fields: {'connect-from': from, 'connect-to': to},
      confirmTitle: '型定義の変更',
      predicate: current => current.graph?.type_name === 'Composed.Main' && current.graph.connections.some(connection => connection.start === from && connection.end === to),
      description: `Connect ${from} to ${to}`,
    });
    for (const [port, bus] of [['a', 'busA'], ['b', 'busB'], ['c', 'busC']]) {
      view = await connect(`gw.send_${port}`, `${bus}.tx_a`);
      view = await connect(`${bus}.rx_a`, `gw.receive_${port}`);
    }
    assert.equal(view.graph.nodes.length, 7, 'Main should contain gw, three Bus modules, and three end Controllers.');
    assert.equal(view.graph.connections.length, 12, 'Each of three buses should have two CAN pairs.');
    for (const busName of ['busA', 'busB', 'busC']) {
      const busType = view.graph.nodes.find(node => node.name === busName).type_name;
      const type = view.types.find(candidate => candidate.name === busType);
      assert.ok(type, `Bus type ${busType} should be present in the API projection.`);
    }

    // Set one egress Controller's instance override through the Parameters form.
    await page.locator('#instances button[data-instance-path="Main.gw.b"]').click();
    await page.waitForFunction(() => document.getElementById('parameter-context').textContent.includes('Main.gw.b'));
    const queueParameter = page.locator('#parameters .parameter').filter({hasText: 'queueCapacity'});
    await queueParameter.locator('button').filter({hasText: '実体値を設定/解除'}).click();
    await waitDialog('queueCapacity の実体値');
    await fillDialog({literal: '8'});
    const beforeParameter = await sessionView();
    await page.locator('#dialog-submit').click();
    await observeConfirmation(beforeParameter.revision, null, settingTitles);
    const gwB = (await sessionView()).instances.find(instance => instance.path === 'Main.gw.b');
    assert.ok(gwB, 'Gateway egress Controller Main.gw.b should exist in the read-only hierarchy.');
    view = await waitStableView(current => current.parameters?.some(parameter => parameter.instance_path === 'Main.gw.b' && parameter.name === 'queueCapacity' && parameter.override === '8'),
      'Set Main.gw.b queueCapacity to 8', beforeParameter.revision, {typeKey: gwB.type_key, instancePath: 'Main.gw.b'});

    await page.locator('#gateway-settings').click();
    await waitDialog('Gatewayを選択');
    await fillDialog({node: 'Main.gw', action: 'set'});
    await page.locator('#dialog-submit').click();
    await waitDialog('Gateway設定');
    await selectChecks('ports', ['Main.gw.a', 'Main.gw.b', 'Main.gw.c']);
    await fillDialog({processing_delay: '10us', hop_limit: '16', rx_queue_capacity: '64'});
    await page.locator('#editor-dialog button').filter({hasText: 'ルートを追加'}).click();
    await fillDialog({
      'routes-1-id': 'fanout',
      'routes-1-ingress': 'Main.gw.a',
      'routes-1-format': 'standard',
      'routes-1-id_min': '256',
      'routes-1-id_max': '256',
    });
    await selectChecks('routes-1-egress', ['Main.gw.b', 'Main.gw.c']);
    const beforeGateway = await sessionView();
    await page.locator('#dialog-submit').click();
    await observeConfirmation(beforeGateway.revision, null, settingTitles);
    view = await waitStableView(current => current.gateway_settings?.some(gateway => gateway.node === 'Main.gw' &&
      gateway.ports.length === 3 && gateway.routes?.some(route => route.id === 'fanout' && route.ingress === 'Main.gw.a' &&
        route.egress.length === 2 && route.egress.includes('Main.gw.b') && route.egress.includes('Main.gw.c') &&
        route.format === 'standard' && String(route.id_min) === '256' && String(route.id_max) === '256')),
    'Set Gateway fanout route', beforeGateway.revision);

    await page.locator('#workload-settings').click();
    await waitDialog('送信設定');
    await page.locator('#editor-dialog button').filter({hasText: '送信定義を追加'}).click();
    await fillDialog({
      'generators-1-id': 'native',
      'generators-1-kind': 'can.explicit.v1',
      'generators-1-node': 'Main.endA',
      'generators-1-format': 'standard',
      'generators-1-frame_id': '256',
      'generators-1-data': '01020304',
      'generators-1-times': '1ms',
    });
    const beforeWorkload = await sessionView();
    await page.locator('#dialog-submit').click();
    await observeConfirmation(beforeWorkload.revision, null, settingTitles);
    view = await waitStableView(current => current.workload?.generators?.length === 1 && current.workload.generators[0].node === 'Main.endA',
      'Set explicit native workload', beforeWorkload.revision);

    await page.locator('#project-settings').click();
    await waitDialog('ネットワーク設定');
    await fillDialog({sim_time_limit: '10ms', metrics_window: '1ms', max_events: '1000000', max_delta_cycles: '1000'});
    const beforeProjectSettings = await sessionView();
    await page.locator('#dialog-submit').click();
    await observeConfirmation(beforeProjectSettings.revision, null, settingTitles);
    view = await waitStableView(current => current.project_settings?.sim_time_limit === '10ms' && current.project_settings?.metrics_window === '1ms' &&
      String(current.project_settings?.max_events) === '1000000' && String(current.project_settings?.max_delta_cycles) === '1000',
    'Set simulation and metrics limits', beforeProjectSettings.revision);

    await page.locator('#validate').click();
    view = await waitStableView(current => current.analysis === 'ready' && !current.diagnostics?.length,
      'Validate completed three-bus Gateway model');
    await page.waitForFunction(() => document.getElementById('status').textContent.includes('実行準備OK'));
    assert.match(await page.locator('#status').innerText(), /実行準備OK/);

    await page.locator('#save-as').click();
    await waitDialog('別フォルダにプロジェクトを保存');
    const exportRootOptions = await page.locator('#export-root option').evaluateAll(options => options.map(option => option.value));
    assert.ok(exportRootOptions.length > 0, 'The editor should expose an allowed export root.');
    await fillDialog({'export-root': exportRootOptions[0], 'destination-name': 'composed'});
    await page.locator('#dialog-submit').click();
    const exported = await waitStableView(current => current.outputs?.some(output => output.kind === 'managed_export' && output.path.endsWith('/composed')),
      'Save completed project as composed');
    await waitRenderedRevision(exported);

    const projectDirectory = path.join(exports, 'composed');
    const projectConfig = path.join(projectDirectory, 'project.ini');
    const validate = spawnSync(binary, ['validate', '--config', projectConfig], {cwd: root, encoding: 'utf8'});
    assert.equal(validate.status, 0, `CLI validate failed (${validate.status}): ${redact(validate.stderr || validate.stdout)}`);
    const validation = JSON.parse(validate.stdout);
    assert.equal(validation.status, 'valid');
    assert.equal(validation.network, 'Composed.Main');

    const run = spawnSync(binary, ['run', '--config', projectConfig, '--output', simulationOutput], {cwd: root, encoding: 'utf8'});
    assert.equal(run.status, 0, `Standalone CLI run failed (${run.status}): ${redact(run.stderr || run.stdout)}`);
    const result = JSON.parse(fs.readFileSync(path.join(simulationOutput, 'results.json'), 'utf8'));
    const records = result.simulation?.model_records;
    assert.ok(Array.isArray(records), 'Multibus run should publish schema 2 model records.');
    const requests = records.filter(record => record.schema_name === 'can.request');
    const nativeRequests = requests.filter(record => record.data.model_fields.parent_request_id === null);
    const generatedRequests = requests.filter(record => record.data.model_fields.parent_request_id !== null);
    const forwards = records.filter(record => record.schema_name === 'gw.forward');
    assert.equal(nativeRequests.length, 1, 'Expected one native CAN request.');
    assert.equal(generatedRequests.length, 2, 'Expected two Gateway-generated CAN requests.');
    assert.equal(forwards.length, 2, 'Expected two generated Gateway forwards.');
    assert.deepEqual(forwards.map(record => record.data.egress).sort(), ['Main.gw.b', 'Main.gw.c']);
    assert.equal(requests.length, 3);
    assert.deepEqual(requests.map(record => record.data.status), ['success', 'success', 'success']);

    // Result records expose payload length and the Classical CAN CRC, while the
    // captured workload source in result metadata carries the exact configured bytes.
    const workloadSource = result.metadata?.sources?.find(source => source.logical_path === 'workload');
    assert.ok(workloadSource, 'Run provenance should include the saved workload input.');
    const savedWorkload = JSON.parse(workloadSource.content_utf8);
    assert.deepEqual(savedWorkload.generators[0].frame, {format: 'standard', id: 256, data: '01020304'});
    assert.deepEqual(requests.map(record => record.data.payload_bits), ['32', '32', '32']);
    const modelFields = requests.map(record => record.data.model_fields);
    assert.equal(new Set(modelFields.map(fields => fields.crc15)).size, 1, 'Native and forwarded frames should retain the same payload CRC.');
    assert.equal(new Set(requests.map(record => record.data.serialized_bits)).size, 1, 'Native and forwarded frames should retain the same serialized length.');

    assert.deepEqual(pageErrors, [], `Unexpected browser errors: ${pageErrors.join(' | ')}`);
    console.log('NED composition browser test passed: empty Multibus project, Router boundary ports, three custom-gated buses, Gateway fanout, UI settings, export, validation, and standalone run.');
  } finally {
    if (browser) await browser.close();
    if (editor.exitCode === null) editor.kill('SIGTERM');
    await Promise.race([editorClosed, sleep(3000)]);
    fs.rmSync(root, {recursive: true, force: true});
  }
}

require('node:test')('NED editor UI-only empty-project to three-bus Gateway fanout', {skip: playwright ? false : 'Playwright is not installed; set PLAYWRIGHT_MODULE to run the browser gate', timeout: 240000}, async () => {
  try { await main(); }
  catch (error) {
    if (error.stack) error.stack = redact(error.stack);
    error.message = redact(error.message);
    throw error;
  }
});
