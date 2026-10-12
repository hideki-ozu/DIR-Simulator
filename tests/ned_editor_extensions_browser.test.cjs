'use strict';

// Real-browser coverage for UI-created NED files, instance-type COW, and a
// simultaneous child-name swap. All model edits are made through the editor UI.
const test = require('node:test');
const assert = require('node:assert/strict');
const fs = require('node:fs');
const os = require('node:os');
const path = require('node:path');
const crypto = require('node:crypto');
const {spawn, spawnSync} = require('node:child_process');
const {once} = require('node:events');

const repo = path.resolve(__dirname, '..');
let playwright;
try { playwright = require(process.env.PLAYWRIGHT_MODULE || 'playwright'); }
catch (error) { if (process.env.PLAYWRIGHT_MODULE || error.code !== 'MODULE_NOT_FOUND') throw error; }
const binary = path.resolve(process.env.DIR_SIMULATOR_BIN || path.join(repo, 'target/debug/dir-simulator'));
const sleep = ms => new Promise(resolve => setTimeout(resolve, ms));
const sha256 = bytes => crypto.createHash('sha256').update(bytes).digest('hex');
const hashFile = file => sha256(fs.readFileSync(file));
const redact = value => String(value)
  .replace(/(https?:\/\/127\.0\.0\.1:\d+)\/[a-f0-9]{64}(?=$|[/?#\s"'<>])/gi, '$1/[viewer-capability-redacted]')
  .replace(/(https?:\/\/[^\s#]+)#[^\s"'<>]*/gi, '$1#[redacted]')
  .replace(/#(?:session|token|secret|editor_session)=[^&\s"'<>]*/gi, '#session=[redacted]');

function snapshotTree(root) {
  const files = {};
  const visit = directory => {
    for (const entry of fs.readdirSync(directory, {withFileTypes: true}).sort((a, b) => a.name.localeCompare(b.name))) {
      const absolute = path.join(directory, entry.name);
      if (entry.isDirectory()) visit(absolute);
      else files[path.relative(root, absolute).split(path.sep).join('/')] = hashFile(absolute);
    }
  };
  visit(root);
  return files;
}

function listFiles(root) {
  const result = [];
  const visit = directory => {
    for (const entry of fs.readdirSync(directory, {withFileTypes: true})) {
      const absolute = path.join(directory, entry.name);
      if (entry.isDirectory()) visit(absolute);
      else result.push(absolute);
    }
  };
  visit(root);
  return result.sort();
}

async function main() {
  assert.ok(fs.existsSync(binary), 'Build dir-simulator before running this browser test.');
  const root = fs.mkdtempSync(path.join(os.tmpdir(), 'dir-ned-extension-browser-'));
  const input = path.join(root, 'input');
  const exports = path.join(root, 'exports');
  const state = path.join(root, 'state');
  const evidenceDir = path.join(root, 'evidence');
  const simulation = path.join(root, 'simulation');
  fs.mkdirSync(path.join(input, 'models/demo'), {recursive: true});
  fs.mkdirSync(exports);
  fs.mkdirSync(evidenceDir);
  const fixtureFiles = [
    ['examples/can/baseline.ini', 'project.ini'],
    ['examples/can/baseline.json', 'baseline.json'],
    ['examples/can/models/demo/Main.ned', 'models/demo/Main.ned'],
  ];
  for (const [source, destination] of fixtureFiles) fs.copyFileSync(path.join(repo, source), path.join(input, destination));
  const inputBefore = snapshotTree(input);
  const evidence = {root, input_before: inputBefore, stages: [], screenshots: []};

  const editor = spawn(binary, ['ned-editor', '--config', path.join(input, 'project.ini'), '--export-root', exports, '--state-root', state], {
    cwd: root,
    stdio: ['ignore', 'pipe', 'pipe'],
  });
  const editorClosed = once(editor, 'close').catch(() => {});
  let browser;
  let page;
  let viewerPage;
  let viewerCapability = '';
  const viewerPageErrors = [];
  const pageErrors = [];
  const http404Classifications = [];
  const consoleErrorSources = [];
  let outputProject;
  let savedTree;

  const addScreenshot = async name => {
    if (!page || page.isClosed()) return;
    const file = path.join(evidenceDir, name);
    await page.screenshot({path: file, fullPage: true});
    evidence.screenshots.push({surface: 'editor', path: file, sha256: hashFile(file), bytes: fs.statSync(file).size});
  };
  const addViewerScreenshot = async name => {
    if (!viewerPage || viewerPage.isClosed()) return;
    const file = path.join(evidenceDir, name);
    await viewerPage.screenshot({path: file, fullPage: true});
    evidence.screenshots.push({surface: 'viewer', path: file, sha256: hashFile(file), bytes: fs.statSync(file).size});
  };

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
    const startupUrl = new URL(startup.url);
    const session = new URLSearchParams(startupUrl.hash.slice(1)).get('session');
    assert.ok(session, 'Editor startup did not provide a session capability.');
    const origin = startupUrl.origin;
    let requestGeneration = 1;
    const getView = async query => {
      const suffix = query ? '?' + query : '';
      const response = await fetch(origin + '/api/session' + suffix, {headers: {'X-Editor-Session': session}});
      assert.equal(response.status, 200, `Read-only session view returned HTTP ${response.status}.`);
      return response.json();
    };
    const rootView = () => getView('');
    const viewForFile = fileId => getView(new URLSearchParams({file_id: fileId, request_scope_generation: String(requestGeneration++)}));
    const classifyUrl = value => {
      if (!value) return 'location-unavailable';
      let url;
      try { url = new URL(value); } catch (_) { return 'location-unavailable'; }
      if (url.origin === origin) {
        return url.pathname === '/favicon.ico' ? 'editor:favicon.ico' : url.pathname.startsWith('/api/') ? 'editor:api' : url.pathname.startsWith('/assets/') ? 'editor:asset' : url.pathname === '/' ? 'editor:document' : 'editor:other-route';
      }
      if (/^\/[a-f0-9]{64}$/i.test(url.pathname)) return 'viewer:capability-route';
      return url.pathname === '/' ? 'viewer:document' : 'viewer:asset';
    };
    const seen404Responses = new WeakSet();
    const record404 = response => {
      if (response.status() !== 404) return;
      if (seen404Responses.has(response)) return;
      seen404Responses.add(response);
      http404Classifications.push(classifyUrl(response.url()));
    };
    const typeView = async typeName => {
      const rootViewData = await rootView();
      const type = rootViewData.types.find(item => item.name === typeName);
      assert.ok(type, `Read-only session view has no type ${typeName}.`);
      const query = new URLSearchParams({file_id: type.file_id, type_key: type.key, request_scope_generation: String(requestGeneration++)});
      const view = await getView(query);
      assert.equal(view.graph?.type_name, typeName);
      return view;
    };

    const chromiumExecutable = process.env.PLAYWRIGHT_CHROMIUM_EXECUTABLE;
    browser = await playwright.chromium.launch({headless: true, ...(chromiumExecutable ? {executablePath: chromiumExecutable} : {})});
    evidence.browser_version = browser.version();
    evidence.binary_sha256 = hashFile(binary);
    const context = await browser.newContext({viewport: {width: 1600, height: 1100}});
    page = await context.newPage();
    context.on('response', record404);
    context.on('page', popup => {
      if (popup === page) return;
      popup.on('response', record404);
      popup.on('pageerror', error => viewerPageErrors.push(redact(error.message)));
      popup.on('console', message => {
        if (message.type() === 'error') viewerPageErrors.push(redact(message.text()));
      });
    });
    page.setDefaultTimeout(20000);
    page.on('response', record404);
    page.on('pageerror', error => pageErrors.push(redact(error.message)));
    page.on('console', message => {
      if (message.type() === 'error') {
        pageErrors.push(redact(message.text()));
        consoleErrorSources.push(classifyUrl(message.location().url));
      }
    });
    await page.goto(startup.url);
    await page.waitForFunction(() => !document.getElementById('source-editor').disabled);
    await page.waitForFunction(() => !document.getElementById('create-file').disabled);
    assert.equal(new URL(page.url()).hash, '', 'The editor should remove the startup fragment after capturing its capability.');

    const waitDialog = async title => page.waitForFunction(expected => {
      const dialog = document.getElementById('editor-dialog');
      return dialog.open && document.getElementById('dialog-title').textContent === expected;
    }, title);
    const waitView = async (predicate, description, timeoutMs = 30000) => {
      const deadline = Date.now() + timeoutMs;
      let last;
      while (Date.now() < deadline) {
        last = await rootView();
        if (!last.busy && predicate(last)) return last;
        await sleep(100);
      }
      throw new Error(`${description} did not become visible through GET /api/session: ${JSON.stringify({revision: last?.revision, files: last?.files?.map(file => file.path), busy: last?.busy})}`);
    };
    const apiFetch = resource => fetch(origin + resource, {headers: {'X-Editor-Session': session}});
    const apiJson = async resource => {
      const response = await apiFetch(resource);
      assert.equal(response.status, 200, `Read-only ${resource.split('/').slice(-1)[0]} request returned HTTP ${response.status}.`);
      return response.json();
    };
    const readRuns = async () => (await apiJson('/api/runs')).runs || [];
    const readRun = runId => apiJson('/api/runs/' + encodeURIComponent(runId));
    const terminalPhases = new Set(['completed','partial','prepare_failed','execution_failed','output_failed','cancelled','timed_out','resource_limited','interrupted','integrity_failed']);
    const waitRun = async (runId, description, timeoutMs = 120000) => {
      const deadline = Date.now() + timeoutMs;
      let last;
      while (Date.now() < deadline) {
        last = await readRun(runId);
        if (terminalPhases.has(last.phase)) return last;
        await sleep(100);
      }
      throw new Error(`${description} did not reach a terminal phase; last phase ${last?.phase || 'unknown'}.`);
    };
    const startRunFromUi = async () => {
      const before = new Set((await readRuns()).map(run => run.run_id));
      await page.locator('#run-simulation').click();
      await waitDialog('現在の編集内容を実行');
      await page.locator('#run-timeout-seconds').fill('120');
      await page.locator('#dialog-submit').click();
      const deadline = Date.now() + 30000;
      while (Date.now() < deadline) {
        const created = (await readRuns()).filter(run => !before.has(run.run_id));
        if (created.length === 1) return created[0].run_id;
        assert.ok(created.length <= 1, 'A single UI run request created multiple run records.');
        await sleep(100);
      }
      throw new Error('The UI run request did not create a record through GET /api/runs.');
    };
    const readRunArtifacts = async runId => apiJson('/api/runs/' + encodeURIComponent(runId) + '/artifacts');
    const readResultArtifact = async runId => {
      const listing = await readRunArtifacts(runId);
      const entry = (listing.artifacts || []).find(artifact => artifact.filename === 'results.json');
      assert.ok(entry, 'Published run artifacts did not include results.json.');
      const response = await apiFetch(entry.url);
      assert.equal(response.status, 200, 'Authenticated results.json download failed.');
      const bytes = Buffer.from(await response.arrayBuffer());
      const digest = sha256(bytes);
      assert.equal(digest, entry.sha256, 'Downloaded results.json did not match its artifact digest.');
      return {entry, digest, bytes, result: JSON.parse(bytes.toString('utf8'))};
    };
    const optionRecords = locator => locator.locator('option').evaluateAll(items => items.map(item => ({value: item.value, label: item.textContent.trim()})));
    const chooseOption = async (selector, predicate, description) => {
      const select = page.locator(selector);
      const choices = await optionRecords(select);
      const choice = choices.find(predicate);
      assert.ok(choice, `${description}; choices were: ${choices.map(item => item.label).join(' | ')}`);
      await select.selectOption(choice.value);
      return choice;
    };

    let view = await rootView();
    assert.ok(view.writer?.client_id, 'Editor UI did not claim its initial writer lease.');
    const mainFile = view.files.find(file => file.role === 'ned' && file.path.endsWith('Main.ned'));
    const workloadFile = view.files.find(file => file.role === 'workload');
    assert.ok(mainFile, 'Fixture Main.ned was not captured.');
    assert.ok(workloadFile, 'Fixture baseline.json was not captured as workload input.');
    evidence.main_file_id = mainFile.id;
    evidence.workload_file_id = workloadFile.id;

    // Create the extra NED file through the two UI forms, then inspect its plan.
    await page.locator('#create-file').click();
    await waitDialog('新規入力ファイル');
    await chooseOption('#new-file-template', item => item.value === 'ned.module_empty', 'The empty NED module template is unavailable.');
    await page.locator('#dialog-submit').click();
    await waitDialog('新規ファイルの場所と名前');
    await chooseOption('#new-file-root', item => true, 'The project has no NED root.');
    await page.locator('#new-file-path').fill('editor/Extra.ned');
    await page.locator('#new-file-name').fill('Extra');
    await page.locator('#new-file-newline').selectOption('LF');
    await page.locator('#new-file-bom').selectOption('false');
    await page.locator('#dialog-submit').click();
    await waitDialog('変更計画を確認');
    const createPlan = await page.locator('#dialog-body').innerText();
    assert.match(createPlan, /editor\/Extra\.ned/);
    assert.match(createPlan, /module Extra/);
    assert.deepEqual(snapshotTree(input), inputBefore, 'Creating an editor file changed the read-only source project on disk.');
    const createRevision = BigInt((await rootView()).revision);
    await page.locator('#dialog-submit').click();
    view = await waitView(current => BigInt(current.revision) > createRevision && current.files.some(file => file.role === 'ned' && file.path.endsWith('Extra.ned')), 'UI-created Extra.ned');
    const extraFile = view.files.find(file => file.role === 'ned' && file.path.endsWith('Extra.ned'));
    assert.ok(extraFile);
    const createdId = extraFile.id;
    assert.match(await page.locator('#source-title').innerText(), /Main\.ned$/, 'Creating a file should keep the current source selection stable.');
    evidence.stages.push({stage: 'create-file', file_id: createdId, plan_contains_path: true, plan_contains_module: true, revision: view.revision});

    // Undo and redo must remove/recreate the clean file without changing its stable id.
    const undoRevision = BigInt(view.revision);
    await page.locator('#undo').click();
    view = await waitView(current => BigInt(current.revision) > undoRevision && !current.files.some(file => file.id === createdId) && current.can_redo, 'Undo of the added NED file');
    await page.waitForFunction(() => ![...document.querySelectorAll('#files button')].some(button => button.textContent.includes('Extra.ned')));
    assert.equal(await page.locator('#files button').filter({hasText: 'Extra.ned'}).count(), 0);
    assert.match(await page.locator('#source-title').innerText(), /Main\.ned$/, 'Undo should keep the selected source file.');
    const redoRevision = BigInt(view.revision);
    await page.locator('#redo').click();
    view = await waitView(current => BigInt(current.revision) > redoRevision && current.files.some(file => file.id === createdId) && !current.can_redo, 'Redo of the added NED file');
    assert.match(await page.locator('#source-title').innerText(), /Main\.ned$/, 'Redo should keep the selected source file.');
    assert.equal((await viewForFile(createdId)).source?.file_id, createdId);
    evidence.stages.push({stage: 'undo-redo', stable_file_id: createdId, selected_source: await page.locator('#source-title').innerText()});

    // Create a visible layout through the UI before detaching so rename can be checked against it.
    await page.locator('#types button').filter({hasText: 'demo.Main'}).first().click();
    await page.waitForFunction(() => document.getElementById('graph-title').textContent === 'demo.Main');
    const beforeLayoutView = await typeView('demo.Main');
    const beforeLayout = Object.fromEntries(beforeLayoutView.graph.nodes.map(node => [node.name, {x: node.x, y: node.y}]));
    const alignRevision = BigInt(beforeLayoutView.revision);
    await page.locator('#align').click();
    const aligned = await waitView(current => BigInt(current.revision) > alignRevision && current.files.some(file => file.id === mainFile.id), 'UI layout alignment');
    const alignedMain = await typeView('demo.Main');
    const alignedPositions = Object.fromEntries(alignedMain.graph.nodes.map(node => [node.name, {x: node.x, y: node.y}]));
    assert.notDeepEqual(alignedPositions, beforeLayout, 'The UI layout command did not alter the baseline placement.');
    evidence.stages.push({stage: 'layout', revision: aligned.revision, positions: alignedPositions});

    // Select Main.a and detach its shared Controller into the already-created Extra.ned file.
    await page.locator('#instances [data-instance-path="Main.a"]').click();
    await page.waitForFunction(() => document.getElementById('parameter-context').textContent.includes('Main.a'));
    await page.locator('#detach-instance').click();
    await waitDialog('実体の型の複製先');
    await page.locator('#clone-destination').selectOption('existing:' + createdId);
    await page.locator('#dialog-submit').click();
    await waitDialog('複製する型の名前');
    const proposalInput = page.locator('#clone-name-0');
    const proposedCloneName = await proposalInput.inputValue();
    assert.match(proposedCloneName, /^[A-Za-z_][A-Za-z0-9_]*$/, 'The UI did not provide a valid COW name proposal.');
    // The proposal is read from the visible field and accepted as presented.
    await page.locator('#dialog-submit').click();
    await waitDialog('変更計画を確認');
    const cowPlan = await page.locator('#dialog-body').innerText();
    assert.match(cowPlan, /Extra\.ned/);
    assert.ok(cowPlan.includes(proposedCloneName), 'The COW plan does not show the name proposed by the UI.');
    await page.locator('#dialog-submit').click();
    view = await waitView(current => {
      const instances = new Map((current.instances || []).map(instance => [instance.path, instance]));
      return current.files.some(file => file.id === createdId) &&
        instances.get('Main.a')?.type_name !== 'demo.Controller' &&
        instances.get('Main.b')?.type_name === 'demo.Controller' &&
        instances.get('Main.c')?.type_name === 'demo.Controller';
    }, 'COW of Main.a while retaining Main.b/Main.c bindings');
    const cloneQname = view.instances.find(instance => instance.path === 'Main.a').type_name;
    const cloneType = view.types.find(type => type.name === cloneQname);
    assert.ok(cloneType, `COW type ${cloneQname} is absent from the project type list.`);
    assert.equal(cloneType.file_id, createdId, 'COW did not place the clone in the selected existing NED file.');
    const mainAfterCow = await viewForFile(mainFile.id);
    const extraAfterCow = await viewForFile(createdId);
    assert.match(mainAfterCow.source.text, new RegExp(`a\\s*:\\s*${cloneQname.replace(/[.*+?^${}()|[\]\\]/g, '\\$&')}`));
    assert.match(extraAfterCow.source.text, new RegExp(`(?:simple|module)\\s+${proposedCloneName}\\b`));
    evidence.stages.push({stage: 'cow', clone_qname: cloneQname, clone_proposal_from_ui: proposedCloneName, main_a: view.instances.find(instance => instance.path === 'Main.a').type_name, main_b: view.instances.find(instance => instance.path === 'Main.b').type_name, main_c: view.instances.find(instance => instance.path === 'Main.c').type_name});

    // Select Main again and rename the clone plus swap a/b in one dialog transaction.
    await page.locator('#types button').filter({hasText: 'demo.Main'}).first().click();
    await page.waitForFunction(() => document.getElementById('graph-title').textContent === 'demo.Main');
    const beforeSwapView = await typeView('demo.Main');
    const beforeSwapPositions = Object.fromEntries(beforeSwapView.graph.nodes.map(node => [node.name, {x: node.x, y: node.y}]));
    await page.locator('#rename-symbols').click();
    await waitDialog('一括リネーム');
    const symbolValue = async (selector, predicate, description) => {
      const choices = await optionRecords(page.locator(selector));
      const choice = choices.find(predicate);
      assert.ok(choice, `${description}; choices were: ${choices.map(item => item.label).join(' | ')}`);
      return choice.value;
    };
    const selectRename = async (row, symbolValueToRename, newName) => {
      await page.locator(`#renames-${row}-symbol`).selectOption(symbolValueToRename);
      await page.locator(`#renames-${row}-name`).fill(newName);
    };
    const mainA = await symbolValue('#renames-1-symbol', item => item.label.startsWith('demo.Main · child · a'), 'Main.a rename symbol is unavailable.');
    const mainB = await symbolValue('#renames-1-symbol', item => item.label.startsWith('demo.Main · child · b'), 'Main.b rename symbol is unavailable.');
    const cloneTypeSymbol = await symbolValue('#renames-1-symbol', item => item.label.startsWith(`${cloneQname} · type · `) && item.label.endsWith(' · ' + proposedCloneName), 'The cloned type rename symbol is unavailable.');
    await selectRename(1, mainA, 'b');
    await page.locator('#dialog-body .form-section button').filter({hasText: '変更を追加'}).click();
    await selectRename(2, mainB, 'a');
    await page.locator('#dialog-body .form-section button').filter({hasText: '変更を追加'}).click();
    const renamedCloneName = proposedCloneName + '_Renamed';
    await selectRename(3, cloneTypeSymbol, renamedCloneName);
    await page.locator('#dialog-submit').click();
    await waitDialog('変更計画を確認');
    const renamePlan = await page.locator('#dialog-body').innerText();
    assert.ok(renamePlan.includes(renamedCloneName), 'The plan does not show the cloned type rename.');
    assert.match(renamePlan, /demo\.Main/);
    await page.locator('#dialog-submit').click();

    const renamed = await waitView(current => {
      const instances = new Map((current.instances || []).map(instance => [instance.path, instance]));
      return instances.get('Main.a')?.type_name === 'demo.Controller' &&
        instances.get('Main.b')?.type_name?.endsWith('.' + renamedCloneName) &&
        instances.get('Main.c')?.type_name === 'demo.Controller';
    }, 'simultaneous clone rename and Main.a/Main.b swap');
    const renamedCloneQname = renamed.instances.find(instance => instance.path === 'Main.b').type_name;
    assert.ok(renamed.types.some(type => type.name === renamedCloneQname && type.file_id === createdId));
    const finalMainView = await typeView('demo.Main');
    const finalMainSource = await viewForFile(mainFile.id);
    const finalExtraSource = await viewForFile(createdId);
    assert.match(finalMainSource.source.text, /a\s*:\s*demo\.Controller\s*;/);
    assert.match(finalMainSource.source.text, new RegExp(`b\\s*:\\s*${renamedCloneQname.replace(/[.*+?^${}()|[\]\\]/g, '\\$&')}`));
    assert.match(finalExtraSource.source.text, new RegExp(`(?:simple|module)\\s+${renamedCloneName}\\b`));
    assert.doesNotMatch(finalExtraSource.source.text, new RegExp(`(?:simple|module)\\s+${proposedCloneName}\\b`));

    const finalPositions = Object.fromEntries(finalMainView.graph.nodes.map(node => [node.name, {x: node.x, y: node.y}]));
    assert.deepEqual(finalPositions.a, beforeSwapPositions.b, 'The original b layout placement was not remapped to the new a name.');
    assert.deepEqual(finalPositions.b, beforeSwapPositions.a, 'The original a layout placement was not remapped to the new b name.');
    const workloadView = await viewForFile(workloadFile.id);
    const workload = JSON.parse(workloadView.source.text);
    const instancePaths = new Set(renamed.instances.map(instance => instance.path));
    for (const generator of workload.generators) assert.ok(instancePaths.has(generator.node), `JSON workload reference ${generator.node} no longer resolves.`);
    const expectedWorkload = JSON.parse(fs.readFileSync(path.join(input, 'baseline.json'), 'utf8'));
    for (const generator of expectedWorkload.generators) {
      if (generator.node === 'Main.a') generator.node = 'Main.b';
      else if (generator.node === 'Main.b') generator.node = 'Main.a';
    }
    assert.deepEqual(workload, expectedWorkload, 'The rename should update only schema-resolved instance paths in the workload JSON.');
    assert.deepEqual(snapshotTree(input), inputBefore, 'Renaming editor symbols changed the read-only source project on disk.');
    evidence.stages.push({stage: 'rename-and-swap', renamed_clone_qname: renamedCloneQname, main_a: renamed.instances.find(instance => instance.path === 'Main.a').type_name, main_b: renamed.instances.find(instance => instance.path === 'Main.b').type_name, main_c: renamed.instances.find(instance => instance.path === 'Main.c').type_name, positions: finalPositions, json_references: workload.generators.map(generator => generator.node)});

    // Exercise same-file COW history while an instance scope is selected. Undo
    // removes the selected clone type, so the UI must rebind Main.c to its prior
    // type; Redo must restore the clone selection in the same file.
    await page.waitForFunction(() => {
      const row = document.querySelector('#instances [data-instance-path="Main.c"]');
      return row && !row.disabled;
    });
    const mainCRow = page.locator('#instances [data-instance-path="Main.c"]');
    await mainCRow.click();
    await page.waitForFunction(() => {
      const row = document.querySelector('#instances [data-instance-path="Main.c"]');
      return row?.getAttribute('aria-current') === 'true' && document.getElementById('parameter-context').textContent.includes('Main.c');
    });
    await page.locator('#detach-instance').click();
    await waitDialog('実体の型の複製先');
    const selectedCowScope = await page.locator('#dialog-body').innerText();
    assert.match(selectedCowScope, /Main\.c/, 'The COW dialog did not retain the selected Main.c scope.');
    await page.locator('#clone-destination').selectOption('existing:' + mainFile.id);
    await page.locator('#dialog-submit').click();
    await waitDialog('複製する型の名前');
    const sameFileProposal = await page.locator('#clone-name-0').inputValue();
    assert.match(sameFileProposal, /^[A-Za-z_][A-Za-z0-9_]*$/);
    await page.locator('#dialog-submit').click();
    await waitDialog('変更計画を確認');
    assert.ok((await page.locator('#dialog-body').innerText()).includes(sameFileProposal));
    await page.locator('#dialog-submit').click();
    const sameFileCow = await waitView(current => {
      const instance = current.instances.find(item => item.path === 'Main.c');
      return instance?.type_name !== 'demo.Controller';
    }, 'same-file COW for Main.c');
    const sameFileCloneQname = sameFileCow.instances.find(item => item.path === 'Main.c').type_name;
    const sameFileCloneType = sameFileCow.types.find(type => type.name === sameFileCloneQname);
    assert.ok(sameFileCloneType);
    assert.equal(sameFileCloneType.file_id, mainFile.id, 'Same-file COW moved the clone to another file.');
    await page.waitForFunction(expected => document.getElementById('graph-title').textContent === expected && document.getElementById('parameter-context').textContent.includes('Main.c'), sameFileCloneQname);

    const sameCowRevision = BigInt(sameFileCow.revision);
    await page.locator('#undo').click();
    const sameFileUndo = await waitView(current => BigInt(current.revision) > sameCowRevision && current.instances.find(item => item.path === 'Main.c')?.type_name === 'demo.Controller', 'same-file COW Undo rebind');
    assert.ok(sameFileUndo.types.some(type => type.key === sameFileCow.instances.find(item => item.path === 'Main.c').type_key && type.file_id === mainFile.id) === false, 'Undo left the selected COW type in the model.');
    await page.waitForFunction(() => document.getElementById('graph-title').textContent === 'demo.Controller' && document.getElementById('parameter-context').textContent.includes('Main.c'));
    assert.equal(await page.locator('#instances [data-instance-path="Main.c"]').getAttribute('aria-current'), 'true');

    const sameUndoRevision = BigInt(sameFileUndo.revision);
    await page.locator('#redo').click();
    const sameFileRedo = await waitView(current => BigInt(current.revision) > sameUndoRevision && current.instances.find(item => item.path === 'Main.c')?.type_name === sameFileCloneQname, 'same-file COW Redo rebind');
    assert.equal(sameFileRedo.instances.find(item => item.path === 'Main.c').type_key, sameFileCow.instances.find(item => item.path === 'Main.c').type_key, 'Redo did not restore the stable clone type key.');
    assert.equal(sameFileRedo.types.find(type => type.name === sameFileCloneQname)?.file_id, mainFile.id);
    await page.waitForFunction(expected => document.getElementById('graph-title').textContent === expected && document.getElementById('parameter-context').textContent.includes('Main.c'), sameFileCloneQname);
    const finalSameFileSource = await viewForFile(mainFile.id);
    assert.match(finalSameFileSource.source.text, new RegExp(`c\\s*:\\s*${sameFileCloneQname.replace(/[.*+?^${}()|[\]\\]/g, '\\$&')}`));
    evidence.stages.push({stage: 'same-file-cow-undo-redo', proposal_from_ui: sameFileProposal, clone_qname: sameFileCloneQname, stable_type_key: sameFileRedo.instances.find(item => item.path === 'Main.c').type_key, undo_type: sameFileUndo.instances.find(item => item.path === 'Main.c').type_name, redo_type: sameFileRedo.instances.find(item => item.path === 'Main.c').type_name, file_id: mainFile.id});
    await addScreenshot('model-after-same-file-cow-redo.png');

    // Save through the editor UI, then validate and run the exported standalone project.
    await page.locator('#save-as').click();
    await waitDialog('別フォルダにプロジェクトを保存');
    await chooseOption('#export-root', item => true, 'No allowed export root is available.');
    await page.locator('#destination-name').fill('ned-extension-final');
    await page.locator('#dialog-submit').click();
    view = await waitView(current => (current.outputs || []).some(output => output.kind === 'managed_export' && output.path.endsWith('ned-extension-final')), 'Save As of the edited project');
    outputProject = path.join(exports, 'ned-extension-final');
    const exportedConfig = path.join(outputProject, 'project.ini');
    assert.ok(fs.existsSync(exportedConfig), 'Save As did not write project.ini.');
    const exportedFiles = listFiles(outputProject);
    const exportedMain = exportedFiles.find(file => path.basename(file) === 'Main.ned');
    const exportedExtra = exportedFiles.find(file => path.basename(file) === 'Extra.ned');
    const exportedWorkload = exportedFiles.find(file => path.basename(file) === 'workload.json');
    const exportedLayout = exportedFiles.find(file => path.basename(file) === 'Main.ned.layout.json');
    assert.ok(exportedMain, 'Save As omitted Main.ned.');
    assert.ok(exportedExtra, 'Save As omitted the UI-created and COW-target NED file.');
    assert.ok(exportedWorkload, 'Save As omitted the fixture workload JSON.');
    assert.ok(exportedLayout, 'Save As omitted the UI-created Main layout.');
    const exportedMainText = fs.readFileSync(exportedMain, 'utf8');
    const exportedExtraText = fs.readFileSync(exportedExtra, 'utf8');
    assert.match(exportedMainText, /a\s*:\s*demo\.Controller\s*;/);
    assert.match(exportedMainText, new RegExp(`b\\s*:\\s*${renamedCloneQname.replace(/[.*+?^${}()|[\]\\]/g, '\\$&')}`));
    assert.match(exportedMainText, new RegExp(`c\\s*:\\s*${sameFileCloneQname.replace(/[.*+?^${}()|[\]\\]/g, '\\$&')}`));
    assert.match(exportedExtraText, new RegExp(`(?:simple|module)\\s+${renamedCloneName}\\b`));
    const exportedJson = JSON.parse(fs.readFileSync(exportedWorkload, 'utf8'));
    assert.deepEqual(exportedJson.generators.map(generator => generator.node), workload.generators.map(generator => generator.node));
    const layout = JSON.parse(fs.readFileSync(exportedLayout, 'utf8'));
    assert.deepEqual(layout.types['demo.Main'].nodes.a, {x: finalPositions.a.x, y: finalPositions.a.y, collapsed: false});
    assert.deepEqual(layout.types['demo.Main'].nodes.b, {x: finalPositions.b.x, y: finalPositions.b.y, collapsed: false});

    const validation = spawnSync(binary, ['validate', '--config', exportedConfig], {cwd: root, encoding: 'utf8'});
    assert.equal(validation.status, 0, redact(validation.stderr || validation.stdout));
    const validationResult = JSON.parse(validation.stdout);
    assert.equal(validationResult.status, 'valid');
    fs.mkdirSync(simulation);
    const run = spawnSync(binary, ['run', '--config', exportedConfig, '--output', simulation], {cwd: root, encoding: 'utf8'});
    assert.equal(run.status, 0, redact(run.stderr || run.stdout));
    assert.ok(fs.existsSync(path.join(simulation, 'results.json')), 'CLI run did not produce results.json.');
    const results = JSON.parse(fs.readFileSync(path.join(simulation, 'results.json'), 'utf8'));
    assert.equal(results.simulation?.partial, false, 'The standalone CLI run returned a partial simulation.');

    await addScreenshot('saved-project.png');
    savedTree = snapshotTree(outputProject);
    evidence.output_project = outputProject;
    evidence.output_files = savedTree;
    evidence.cli = {
      validate_status: validation.status,
      validate_result: validationResult,
      run_status: run.status,
      run_result_sha256: hashFile(path.join(simulation, 'results.json')),
    };
    evidence.page_errors = pageErrors;
    assert.deepEqual(pageErrors, [], 'Browser console or page errors were recorded.');
    const inputAfter = snapshotTree(input);
    assert.deepEqual(inputAfter, inputBefore, 'The editor changed the original copied fixture on disk.');
    evidence.input_after = inputAfter;
    evidence.stages.push({stage: 'save-as-cli', exported_main: exportedMain, exported_extra: exportedExtra, exported_workload: exportedWorkload, exported_layout: exportedLayout, output_hashes: savedTree});

    // The copied fixture is proven unchanged above, then removed before either
    // asynchronous run. This makes a successful UI run depend on the editor's
    // captured model and the durable run snapshot, never a later disk fallback.
    const projectIni = view.files.find(file => file.path === 'project.ini' || file.path.endsWith('/project.ini'));
    assert.ok(projectIni, 'The captured project.ini is unavailable in the editor model.');
    const cleanBeforeRun = await rootView();
    assert.equal(cleanBeforeRun.dirty, false, 'The UI-saved project should be clean before the first run.');
    fs.rmSync(input, {recursive: true, force: true});
    assert.equal(fs.existsSync(input), false, 'The copied source tree should be removed before the UI run.');
    assert.equal(fs.existsSync(outputProject), true, 'The separately saved project should remain available.');
    evidence.input_removed_before_run = true;

    const normalRunId = await startRunFromUi();
    const normalRun = await waitRun(normalRunId, 'Normal UI run');
    assert.equal(normalRun.phase, 'completed', 'The unmodified captured project should complete normally.');
    assert.equal(normalRun.artifact_state, 'published');
    assert.equal(normalRun.source_stamp.dirty_at_start, false);
    assert.equal(normalRun.source_stamp.project_instance_id, cleanBeforeRun.project_instance_id);
    assert.equal(String(normalRun.source_stamp.input_revision), String(cleanBeforeRun.input_revision));
    assert.equal(normalRun.source_stamp.input_digest, cleanBeforeRun.input_digest);
    const afterNormalRun = await rootView();
    for (const key of ['revision', 'input_revision', 'input_digest', 'dirty']) {
      assert.equal(afterNormalRun[key], cleanBeforeRun[key], `The normal run changed project history field ${key}.`);
    }
    const normalArtifact = await readResultArtifact(normalRunId);
    assert.equal(normalArtifact.result.simulation?.partial, false);
    assert.equal(normalArtifact.result.simulation?.termination, normalRun.report?.termination);
    const normalCardSelector = `#runs article.run-card[data-run-id="${normalRunId}"]`;
    await page.locator(normalCardSelector).waitFor();
    await page.waitForFunction(id => {
      const card = document.querySelector('#runs article.run-card[data-run-id="' + id + '"]');
      return card?.querySelector('h3')?.textContent === '完了';
    }, normalRunId);
    await addScreenshot('normal-run-completed.png');
    evidence.normal_ui_run = {
      run_id: normalRunId,
      phase: normalRun.phase,
      dirty_at_start: normalRun.source_stamp.dirty_at_start,
      source_stamp: {
        project_instance_id: normalRun.source_stamp.project_instance_id,
        input_revision: normalRun.source_stamp.input_revision,
        input_digest: normalRun.source_stamp.input_digest,
      },
      history_unchanged: true,
      termination: normalRun.report.termination,
      exit_code: normalRun.report.exit_code,
      results_sha256: normalArtifact.digest,
      results_bytes: normalArtifact.bytes.length,
    };

    // Change only the captured INI through the editor. The completed run must
    // now be visibly historical while its frozen sources remain independent.
    await page.locator('#files button').filter({hasText: 'project.ini'}).first().click();
    await page.waitForFunction(() => document.getElementById('source-title').textContent.endsWith('project.ini'));
    const iniBeforeEdit = await viewForFile(projectIni.id);
    assert.doesNotMatch(iniBeforeEdit.source.text, /^\s*max-events\s*=/m);
    // Stop well inside the full run, after enough completed traffic exists for
    // the partial-result Viewer to expose an event to step through.
    const partialIniText = iniBeforeEdit.source.text.replace(/\s*$/, '') + '\nmax-events = 200\n';
    const beforeEditView = await rootView();
    await page.locator('#source-editor').fill(partialIniText);
    const editedView = await waitView(current => BigInt(current.input_revision) > BigInt(beforeEditView.input_revision), 'UI edit of captured project.ini');
    assert.notEqual(editedView.input_digest, beforeEditView.input_digest);
    assert.equal(editedView.dirty, true);
    const iniAfterEdit = await viewForFile(projectIni.id);
    assert.equal(iniAfterEdit.source.text, partialIniText);
    await page.waitForFunction(id => {
      const card = document.querySelector('#runs article.run-card[data-run-id="' + id + '"]');
      return card?.querySelector('.run-source')?.textContent.includes('過去の入力の結果');
    }, normalRunId);
    await addScreenshot('normal-run-marked-past-after-input-edit.png');
    evidence.stages.push({stage: 'past-run-after-input-edit', prior_run_id: normalRunId, input_revision: editedView.input_revision, input_digest: editedView.input_digest, prior_run_marked_past: true});

    const partialRunId = await startRunFromUi();
    const partialRun = await waitRun(partialRunId, 'Partial UI run');
    assert.equal(partialRun.phase, 'partial');
    assert.equal(partialRun.artifact_state, 'published');
    assert.equal(partialRun.source_stamp.dirty_at_start, true);
    assert.equal(String(partialRun.source_stamp.input_revision), String(editedView.input_revision));
    assert.equal(partialRun.source_stamp.input_digest, editedView.input_digest);
    assert.equal(partialRun.report?.exit_code, 3, 'The event-limited run should publish with the partial-result exit code.');
    assert.equal(typeof partialRun.report?.termination, 'string');
    assert.ok(partialRun.report.termination.length > 0, 'The partial report must preserve its actual termination.');
    const partialArtifact = await readResultArtifact(partialRunId);
    assert.equal(partialArtifact.result.simulation?.partial, true);
    assert.equal(partialArtifact.result.simulation?.termination, partialRun.report.termination);
    const partialCardSelector = `#runs article.run-card[data-run-id="${partialRunId}"]`;
    const partialCard = page.locator(partialCardSelector);
    await partialCard.waitFor();
    await page.waitForFunction(id => {
      const card = document.querySelector('#runs article.run-card[data-run-id="' + id + '"]');
      return card?.dataset.phase === 'partial';
    }, partialRunId);
    await addScreenshot('partial-run-published.png');
    evidence.partial_ui_run = {
      run_id: partialRunId,
      phase: partialRun.phase,
      artifact_state: partialRun.artifact_state,
      dirty_at_start: partialRun.source_stamp.dirty_at_start,
      termination: partialRun.report.termination,
      exit_code: partialRun.report.exit_code,
      results_sha256: partialArtifact.digest,
      results_bytes: partialArtifact.bytes.length,
    };

    // Retrieve every frozen input after the original source tree is gone, and
    // prove both its saved digest and text against the authenticated API.
    const frozenListing = await apiJson('/api/runs/' + encodeURIComponent(partialRunId) + '/inputs');
    assert.ok(frozenListing.files?.length, 'The partial run has no frozen input list.');
    const frozenInputs = [];
    let frozenIni;
    for (const file of frozenListing.files) {
      const source = await apiJson('/api/runs/' + encodeURIComponent(partialRunId) + '/input/' + encodeURIComponent(file.file_id));
      const textHash = sha256(Buffer.from(source.text, 'utf8'));
      assert.equal(source.readonly, true);
      assert.equal(source.frozen_hash, file.frozen_hash);
      assert.equal(textHash, file.frozen_hash, `Frozen source digest mismatch for ${file.original_logical_path}.`);
      frozenInputs.push({path: file.original_logical_path, sha256: textHash, bytes: Buffer.byteLength(source.text, 'utf8')});
      if (file.original_logical_path === projectIni.path || file.original_logical_path.endsWith('/project.ini')) {
        assert.equal(source.text, partialIniText);
        frozenIni = {file_id: file.file_id, path: file.original_logical_path, sha256: textHash, text: source.text};
      }
    }
    assert.ok(frozenIni, 'The frozen source list omitted project.ini.');
    assert.equal(fs.existsSync(input), false, 'The source tree unexpectedly reappeared while reading frozen inputs.');
    evidence.frozen_inputs_after_source_removal = frozenInputs;

    await partialCard.getByRole('button', {name: '実行時の原文'}).click();
    await waitDialog('実行時の入力原文');
    const frozenChoice = await chooseOption('#run-input-file', item => item.value === frozenIni.file_id, 'Frozen project.ini is unavailable in the run-input picker.');
    await page.locator('#run-input-file').selectOption(frozenChoice.value);
    await page.locator('#dialog-submit').click();
    await waitDialog('実行時の原文（参照専用）');
    const shownFrozenInput = await page.locator('#dialog-body').innerText();
    assert.ok(shownFrozenInput.includes(frozenIni.sha256));
    assert.ok(shownFrozenInput.includes('max-events = 200'));
    await page.locator('#dialog-submit').click();

    // Build the Viewer from the published partial run and open its capability
    // in the requested no-opener new tab. Never put the capability URL in logs.
    await partialCard.getByRole('button', {name: 'Viewerを準備'}).click();
    await page.waitForFunction(id => {
      const card = document.querySelector('#runs article.run-card[data-run-id="' + id + '"]');
      return card?.querySelector('a')?.textContent === 'Viewerを開く';
    }, partialRunId);
    const viewerLink = partialCard.getByRole('link', {name: 'Viewerを開く'});
    const viewerHref = await viewerLink.getAttribute('href');
    const viewerURL = new URL(viewerHref);
    viewerCapability = viewerURL.pathname.slice(1);
    assert.equal(viewerURL.protocol, 'http:');
    assert.equal(viewerURL.hostname, '127.0.0.1');
    assert.ok(viewerURL.port.length > 0);
    assert.notEqual(viewerURL.origin, origin, 'The Viewer must use a separate loopback origin.');
    assert.equal(/^\/[a-f0-9]{64}$/.test(viewerURL.pathname), true, 'The Viewer URL should use one opaque capability path.');
    assert.equal(await viewerLink.getAttribute('target'), '_blank');
    assert.match(await viewerLink.getAttribute('rel'), /noopener/);
    const viewerPageEvent = context.waitForEvent('page');
    await viewerLink.click();
    viewerPage = await viewerPageEvent;
    viewerPage.setDefaultTimeout(20000);
    await viewerPage.waitForLoadState('domcontentloaded');
    assert.equal(await viewerPage.evaluate(() => window.opener === null), true, 'The Viewer tab must not retain window.opener.');
    await viewerPage.waitForFunction(() => {
      const dashboard = document.getElementById('dashboard');
      return dashboard && !dashboard.hidden;
    });
    assert.equal(await viewerPage.locator('#error-banner').isVisible(), false, 'The generated Viewer reported a load error.');
    await viewerPage.waitForFunction(() => {
      const next = document.getElementById('next-time');
      return next && !next.disabled;
    });
    const beforeViewerStep = await viewerPage.locator('#current-ps').innerText();
    await viewerPage.locator('#next-time').click();
    await viewerPage.waitForFunction(previous => document.getElementById('current-ps').textContent !== previous, beforeViewerStep);
    await addViewerScreenshot('partial-viewer-after-next-event.png');
    const cspErrors = viewerPageErrors.filter(message => /content security policy|\bcsp\b/i.test(message));
    assert.deepEqual(cspErrors, [], 'The generated Viewer reported a CSP error.');
    evidence.viewer = {
      origin: viewerURL.origin,
      capability_url_redacted: true,
      separate_loopback_origin: viewerURL.origin !== origin,
      opened_in_new_tab: true,
      window_opener_null: true,
      next_time_enabled: true,
      current_ps_before: beforeViewerStep,
      current_ps_after: await viewerPage.locator('#current-ps').innerText(),
      csp_errors: cspErrors,
    };

    await partialCard.getByRole('button', {name: '閲覧リンクを失効'}).click();
    await page.waitForFunction(id => {
      const card = document.querySelector('#runs article.run-card[data-run-id="' + id + '"]');
      return card && card.querySelector('button')?.textContent !== '閲覧リンクを失効' && !card.querySelector('a');
    }, partialRunId);
    const revokedRun = await readRun(partialRunId);
    assert.equal(revokedRun.viewer_state, 'revoked');
    const revokedViewerResponse = await fetch(viewerURL.href);
    assert.equal(revokedViewerResponse.status, 410, 'The old Viewer capability should return HTTP 410 after UI revocation.');
    evidence.viewer.revoked_http_status = revokedViewerResponse.status;

    await partialCard.getByRole('button', {name: '実行を削除'}).click();
    await waitDialog('実行結果を削除');
    await page.locator('#dialog-submit').click();
    await page.waitForFunction(id => !document.querySelector('#runs article.run-card[data-run-id="' + id + '"]'), partialRunId);
    const deletedResponse = await apiFetch('/api/runs/' + encodeURIComponent(partialRunId));
    assert.equal(deletedResponse.status, 410, 'Deleted run records should return HTTP 410 Gone.');
    assert.equal((await readRuns()).some(run => run.run_id === partialRunId), false);
    evidence.partial_ui_run.deleted_from_api = true;
    evidence.page_errors = pageErrors;
    evidence.viewer_console_errors = viewerPageErrors;
    evidence.http_404_classifications = http404Classifications;
    evidence.console_error_sources = consoleErrorSources;
    assert.deepEqual(pageErrors, [], 'Browser console or page errors were recorded.');
    assert.equal(JSON.stringify(evidence).includes(session), false, 'The editor session secret must not be stored in evidence.');
    assert.equal(JSON.stringify(evidence).includes(viewerCapability), false, 'The Viewer capability must not be stored in evidence.');
  } catch (error) {
    evidence.failure = redact(error.stack || error.message);
    throw error;
  } finally {
    try { await addScreenshot('last-visible-state.png'); } catch (error) { evidence.screenshot_error = redact(error.message); }
    if (page) evidence.page_errors = pageErrors;
    evidence.viewer_console_errors = viewerPageErrors;
    evidence.http_404_classifications = http404Classifications;
    evidence.console_error_sources = consoleErrorSources;
    if (root) {
      const evidencePath = path.join(evidenceDir, 'evidence.json');
      try { fs.writeFileSync(evidencePath, JSON.stringify(evidence, null, 2) + '\n'); }
      catch (error) { evidence.write_error = redact(error.message); }
      console.log(`NED extension browser evidence: ${evidencePath}`);
      for (const screenshot of evidence.screenshots) console.log(`Screenshot SHA-256 ${screenshot.sha256}: ${screenshot.path}`);
    }
    if (browser) await browser.close();
    if (editor.exitCode === null && editor.signalCode === null) editor.kill('SIGTERM');
    await Promise.race([editorClosed, sleep(4000)]);
  }
}

test('NED editor extensions work through the real UI and produce a runnable exported project', {
  skip: playwright ? false : 'Playwright is not installed; set PLAYWRIGHT_MODULE to run the browser gate',
  timeout: 420000,
}, async () => {
  await main();
});
