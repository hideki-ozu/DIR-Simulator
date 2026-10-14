'use strict';

// Explicitly gated native IME acceptance: run inside a private X11/D-Bus
// session with IBus/Mozc installed. Keys use XTest; Playwright only observes
// the IME and activates the editor UI. No composition events are injected.
const test = require('node:test');
const assert = require('node:assert/strict');
const fs = require('node:fs');
const os = require('node:os');
const path = require('node:path');
const crypto = require('node:crypto');
const {spawn, spawnSync} = require('node:child_process');
const {once} = require('node:events');

let playwright;
try { playwright = require(process.env.PLAYWRIGHT_MODULE || 'playwright'); }
catch (error) { if (process.env.PLAYWRIGHT_MODULE || error.code !== 'MODULE_NOT_FOUND') throw error; }
const repo = path.resolve(__dirname, '..');
const sleep = ms => new Promise(resolve => setTimeout(resolve, ms));
const sha256 = bytes => crypto.createHash('sha256').update(bytes).digest('hex');
const hashFile = file => sha256(fs.readFileSync(file));
const redact = value => String(value)
  .replace(/(https?:\/\/[^\s#]+)#[^\s"'<>]*/gi, '$1#[redacted]')
  .replace(/unix:[^\s"'<>]+/gi, '[private bus]');

function tree(root) {
  const result = {};
  function visit(directory) {
    for (const entry of fs.readdirSync(directory, {withFileTypes: true})) {
      const file = path.join(directory, entry.name);
      if (entry.isDirectory()) visit(file);
      else result[path.relative(root, file)] = hashFile(file);
    }
  }
  visit(root); return result;
}

async function until(check, description, timeout = 20000) {
  const deadline = Date.now() + timeout;
  while (Date.now() < deadline) {
    const result = await check();
    if (result) return result;
    await sleep(50);
  }
  throw new Error(description + ' timed out');
}

function ownedImeProcesses(config) {
  const owned = [];
  for (const name of fs.readdirSync('/proc')) {
    if (!/^\d+$/.test(name)) continue;
    try {
      const comm = fs.readFileSync('/proc/' + name + '/comm', 'utf8').trim();
      if (!/^(ibus|mozc)/.test(comm)) continue;
      const variables = fs.readFileSync('/proc/' + name + '/environ').toString().split('\0');
      if (variables.includes('XDG_CONFIG_HOME=' + config)) owned.push(Number(name));
    } catch (_) { /* exited */ }
  }
  return owned;
}

test('native Mozc composition and source acknowledgement precede the NED run snapshot', {
  skip: process.env.DIR_NED_NATIVE_IME_PRIVATE === '1' && playwright ? false :
    'Set DIR_NED_NATIVE_IME_PRIVATE=1 inside private X11/D-Bus with Playwright and IBus/Mozc',
  timeout: 180000,
}, async () => {
  assert.ok(process.env.DISPLAY && process.env.DBUS_SESSION_BUS_ADDRESS, 'A private X11/D-Bus session is required.');
  const binary = path.resolve(process.env.DIR_SIMULATOR_BIN || path.join(repo, 'target/debug/dir-simulator'));
  assert.ok(fs.existsSync(binary), 'Build dir-simulator or set DIR_SIMULATOR_BIN.');
  const root = fs.mkdtempSync(path.join(os.tmpdir(), 'ned-ime-'));
  const input = path.join(root, 'input'), exports = path.join(root, 'exports'), state = path.join(root, 'state');
  const evidenceDir = path.resolve(process.env.DIR_NED_NATIVE_IME_EVIDENCE || path.join(root, 'evidence'));
  fs.mkdirSync(path.join(input, 'models/demo'), {recursive: true});
  fs.mkdirSync(exports); fs.mkdirSync(evidenceDir, {recursive: true});
  for (const [from, to] of [['baseline.ini', 'project.ini'], ['baseline.json', 'baseline.json'], ['models/demo/Main.ned', 'models/demo/Main.ned']]) {
    fs.copyFileSync(path.join(repo, 'examples/can', from), path.join(input, to));
  }
  const original = tree(input);
  const env = {...process.env, GTK_IM_MODULE: 'ibus', XMODIFIERS: '@im=ibus', GDK_BACKEND: 'x11'};
  for (const [key, directory] of [['XDG_CONFIG_HOME', 'config'], ['XDG_CACHE_HOME', 'cache'], ['XDG_DATA_HOME', 'data'], ['XDG_RUNTIME_DIR', 'runtime']]) {
    env[key] = path.join(root, directory); fs.mkdirSync(env[key], {mode: 0o700});
  }
  delete env.IBUS_USE_PORTAL; delete env.IBUS_ADDRESS;
  const call = (command, args, timeout = 10000) => {
    const result = spawnSync(command, args, {env, encoding: 'utf8', timeout});
    assert.ifError(result.error);
    assert.equal(result.status, 0, command + ' failed: ' + redact(result.stderr));
    return result.stdout.trim();
  };
  const xdo = args => call(process.env.DIR_NED_XDOTOOL || 'xdotool', args);
  const evidence = {schema_version: 1, case: 'NE-XC28', binary_sha256: hashFile(binary), input_before: original,
    input_backend: 'XTest through xdotool', injected_composition_events: false, chromium_sandbox: true,
    commands: [], screenshots: [], stages: []};
  let browser, page, daemon, editor, editorClosed, releaseAck, closing = false;
  let daemonOutput = '';
  const daemonLog = fs.openSync(path.join(evidenceDir, 'ibus.log'), 'a', 0o600);
  try {
    daemon = spawn('ibus-daemon', ['--xim', '--panel=disable', '--config=disable', '--cache=none'], {env, stdio: ['ignore', 'pipe', 'pipe']});
    const collectDaemon = bytes => { daemonOutput = (daemonOutput + bytes.toString()).slice(0, 65536); };
    daemon.stdout.on('data', collectDaemon); daemon.stderr.on('data', collectDaemon);
    await until(() => {
      const result = spawnSync('ibus', ['address'], {env, encoding: 'utf8', timeout: 1000});
      if (!result.error && result.status === 0 && result.stdout.trim().startsWith('unix:')) {
        env.IBUS_ADDRESS = result.stdout.trim(); return true;
      }
      return false;
    }, 'Private IBus readiness');
    assert.match(call('ibus', ['list-engine']), /mozc-jp/);
    evidence.ibus_version = call('ibus', ['version']);
    evidence.xdotool_version = xdo(['--version']);
    const component = fs.readFileSync('/usr/share/ibus/component/mozc.xml', 'utf8');
    evidence.mozc_version = component.match(/<version>([^<]+)<\/version>/)?.[1];
    evidence.mozc_server_sha256 = hashFile('/usr/lib/mozc/mozc_server');
    evidence.mozc_engine_sha256 = hashFile('/usr/lib/ibus-mozc/ibus-engine-mozc');

    editor = spawn(binary, ['ned-editor', '--config', path.join(input, 'project.ini'), '--export-root', exports, '--state-root', state], {cwd: root, stdio: ['ignore', 'pipe', 'pipe']});
    editorClosed = once(editor, 'close').catch(() => {});
    const startup = await new Promise((resolve, reject) => {
      let output = '';
      const timer = setTimeout(() => reject(new Error('Editor startup timed out')), 20000);
      editor.once('error', () => {clearTimeout(timer); reject(new Error('Editor failed to start'));});
      editor.stdout.on('data', bytes => {
        output += bytes;
        if (!output.includes('\n')) return;
        clearTimeout(timer);
        try {resolve(JSON.parse(output.slice(0, output.indexOf('\n'))));}
        catch (_) {reject(new Error('Invalid editor startup response'));}
      });
      editor.stderr.on('data', () => {});
    });
    const startupUrl = new URL(startup.url), origin = startupUrl.origin;
    const session = new URLSearchParams(startupUrl.hash.slice(1)).get('session');
    assert.ok(session);
    const api = async resource => {
      const response = await fetch(origin + resource, {headers: {'X-Editor-Session': session}});
      assert.equal(response.status, 200, 'Read-only API status for ' + resource.split('/').pop());
      return response.json();
    };
    const view = () => api('/api/session');
    const runs = async () => (await api('/api/runs')).runs;
    browser = await playwright.chromium.launch({headless: false, chromiumSandbox: true, env,
      ...(process.env.PLAYWRIGHT_CHROMIUM_EXECUTABLE ? {executablePath: process.env.PLAYWRIGHT_CHROMIUM_EXECUTABLE} : {}), args: ['--ozone-platform=x11']});
    evidence.browser_version = browser.version();
    const context = await browser.newContext({viewport: {width: 1600, height: 1100}});
    page = await context.newPage(); page.setDefaultTimeout(20000);
    const errors = [];
    page.on('pageerror', error => errors.push(redact(error.message)));
    page.on('request', request => {
      if (request.url() !== origin + '/api/commands' || request.method() !== 'POST') return;
      const body = request.postDataJSON();
      evidence.commands.push({kind: body.kind, command_id: body.command_id, base_revision: body.base_revision,
        source_sha256: body.kind === 'replace_source' ? sha256(Buffer.from(body.payload.text)) : undefined});
    });
    await page.goto(startup.url);
    await page.waitForFunction(() => !document.getElementById('source-editor').disabled);
    await page.locator('#files button').filter({hasText: 'Main.ned'}).first().click();
    await page.waitForFunction(() => document.getElementById('source-title').textContent.endsWith('Main.ned'));
    const current = await view(), file = current.files.find(item => item.path.endsWith('Main.ned'));
    assert.ok(file);
    const initialText = await page.locator('#source-editor').inputValue();
    const prefix = initialText + '\n// ';
    await page.locator('#source-editor').click();
    const windowId = xdo(['search', '--onlyvisible', '--name', 'NED']).split('\n')[0];
    xdo(['windowfocus', '--sync', windowId]);
    xdo(['key', '--clearmodifiers', 'ctrl+End']); xdo(['key', 'Return']);
    xdo(['type', '--clearmodifiers', '--delay', '50', '// ']);
    await until(async () => (await view()).files.find(item => item.id === file.id)?.hash === sha256(Buffer.from(prefix)), 'Comment prefix source acknowledgement');
    await page.waitForFunction(() => document.getElementById('source-state').textContent.includes('反映済み'));
    await page.evaluate(() => {
      window.nativeImeEvents = [];
      for (const kind of ['compositionstart', 'compositionupdate', 'compositionend', 'beforeinput', 'input']) {
        document.getElementById('source-editor').addEventListener(kind, event => window.nativeImeEvents.push({kind, data: event.data, isTrusted: event.isTrusted, isComposing: event.isComposing, inputType: event.inputType}));
      }
    });
    const screenshot = async name => {
      const target = path.join(evidenceDir, name);
      await page.screenshot({path: target, fullPage: true});
      evidence.screenshots.push({file: name, sha256: hashFile(target), bytes: fs.statSync(target).size});
    };
    let announceAck;
    const ackSeen = new Promise(resolve => {announceAck = resolve;});
    const ackGate = new Promise(resolve => {releaseAck = resolve;});
    await page.route(origin + '/api/commands', async route => {
      const body = route.request().postDataJSON();
      if (body.kind !== 'replace_source' || !body.payload.text.includes('日本語')) return route.continue();
      try {
        const response = await route.fetch();
        const envelope = await response.json();
        assert.equal(response.status(), 200); assert.equal(envelope.terminal, true);
        evidence.held_source_ack = {operation_status: envelope.operation_status, revision: envelope.revision,
          source_sha256: sha256(Buffer.from(body.payload.text))};
        announceAck(); await ackGate; await route.fulfill({response});
      } catch (error) {if (!closing) throw error;}
    });
    await page.locator('#source-editor').click();
    xdo(['windowfocus', '--sync', windowId]); xdo(['key', '--clearmodifiers', 'ctrl+End']);
    const selection = spawnSync('ibus', ['engine', 'mozc-jp'], {env, encoding: 'utf8', timeout: 35000});
    assert.ifError(selection.error);
    evidence.engine_selection_exit_code = selection.status;
    await until(() => {const r = spawnSync('ibus', ['engine'], {env, encoding: 'utf8', timeout: 1000}); return r.status === 0 && r.stdout.trim() === 'mozc-jp';}, 'Observed native Mozc activation', 10000);
    evidence.observed_engine = 'mozc-jp';
    xdo(['key', '--clearmodifiers', 'Zenkaku_Hankaku']);
    xdo(['type', '--clearmodifiers', '--delay', '150', 'nihongo']); xdo(['key', 'space']);
    await page.waitForFunction(() => document.getElementById('source-editor').value.includes('日本語') && nativeImeEvents.some(event => event.kind === 'compositionstart') && !nativeImeEvents.some(event => event.kind === 'compositionend'));
    const preedit = await page.evaluate(() => ({text: document.getElementById('source-editor').value, events: nativeImeEvents.slice(), status: document.getElementById('source-state').textContent}));
    assert.ok(preedit.events.some(event => event.kind === 'compositionstart' && event.isTrusted));
    assert.ok(preedit.events.some(event => event.kind === 'compositionupdate' && event.isTrusted));
    assert.equal((await runs()).length, 0);
    await screenshot('native-preedit.png');
    evidence.stages.push({stage: 'native-preedit', source_sha256: sha256(Buffer.from(preedit.text)), source_status: preedit.status, run_count: 0});
    // DOM activation preserves focus until the real UI opens its confirmation
    // dialog. Chromium may naturally commit preedit when that dialog focuses.
    await page.evaluate(() => document.getElementById('run-simulation').click());
    await page.waitForFunction(() => document.getElementById('editor-dialog').open);
    await Promise.race([ackSeen, sleep(10000).then(() => {throw new Error('Native committed source acknowledgement was not intercepted');})]);
    assert.equal((await runs()).length, 0);
    await page.locator('#dialog-submit').click();
    await sleep(500);
    assert.equal(evidence.commands.filter(command => command.kind === 'start_run').length, 0, 'Run command crossed a pending source acknowledgement');
    assert.equal((await runs()).length, 0);
    const committedView = await view();
    assert.equal(committedView.files.find(item => item.id === file.id).hash, evidence.held_source_ack.source_sha256);
    await screenshot('native-source-ack-held.png');
    evidence.stages.push({stage: 'source-ack-held-after-run-confirmation', source_sha256: evidence.held_source_ack.source_sha256, run_count: 0});
    releaseAck();
    const created = await until(async () => {const all = await runs(); assert.ok(all.length <= 1); return all[0];}, 'UI run snapshot creation');
    const run = await until(async () => {const r = await api('/api/runs/' + created.run_id); return ['completed', 'partial', 'prepare_failed', 'execution_failed', 'output_failed', 'integrity_failed'].includes(r.phase) && r;}, 'Native-input run completion', 90000);
    assert.equal(run.phase, 'completed'); assert.equal(run.source_stamp.dirty_at_start, true);
    const frozen = await api('/api/runs/' + run.run_id + '/input/' + encodeURIComponent(file.id));
    assert.equal(frozen.text, prefix + '日本語');
    assert.equal(frozen.frozen_hash, sha256(Buffer.from(frozen.text)));
    assert.equal(frozen.frozen_hash, evidence.held_source_ack.source_sha256);
    fs.writeFileSync(path.join(evidenceDir, 'native-frozen-source.ned'), frozen.text);
    const after = await view();
    assert.equal(after.dirty, true); assert.equal(after.input_digest, run.source_stamp.input_digest);
    assert.equal(after.files.find(item => item.id === file.id).hash, frozen.frozen_hash);
    const events = await page.evaluate(() => nativeImeEvents.slice());
    assert.ok(events.some(event => event.kind === 'compositionend'));
    assert.ok(events.filter(event => event.kind === 'input').every(event => event.isTrusted));
    evidence.events = events;
    evidence.compositionend_trust = events.filter(event => event.kind === 'compositionend').map(event => event.isTrusted);
    evidence.run = {run_id: run.run_id, command_id: run.source_stamp.command_id, phase: run.phase,
      frozen_file_id: file.id, frozen_hash: frozen.frozen_hash, frozen_bytes: Buffer.byteLength(frozen.text),
      frozen_source_artifact: 'native-frozen-source.ned',
      current_dirty: after.dirty, dirty_at_start: run.source_stamp.dirty_at_start, input_digest: run.source_stamp.input_digest};
    evidence.input_after = tree(input); assert.deepEqual(evidence.input_after, original);
    assert.deepEqual(tree(exports), {}); assert.deepEqual(errors, []);
    await screenshot('native-frozen-run-completed.png'); evidence.result = 'passed';
  } catch (error) {evidence.result = 'failed'; evidence.error = redact(error.message); throw error;}
  finally {
    closing = true; releaseAck?.();
    if (browser) await browser.close();
    if (editor && editor.exitCode === null && editor.signalCode === null) editor.kill('SIGTERM');
    if (editorClosed) await Promise.race([editorClosed, sleep(4000)]);
    daemon?.kill();
    await sleep(100);
    const own = ownedImeProcesses(env.XDG_CONFIG_HOME);
    for (const pid of own) {try {process.kill(pid, 'SIGTERM');} catch (_) {}}
    evidence.owned_ime_processes_terminated = own.length;
    const cleanupDeadline = Date.now() + 3000;
    while (ownedImeProcesses(env.XDG_CONFIG_HOME).length && Date.now() < cleanupDeadline) await sleep(50);
    for (const pid of ownedImeProcesses(env.XDG_CONFIG_HOME)) {try {process.kill(pid, 'SIGKILL');} catch (_) {}}
    await sleep(50);
    evidence.owned_ime_processes_remaining = ownedImeProcesses(env.XDG_CONFIG_HOME).length;
    if (evidence.owned_ime_processes_remaining) {evidence.result = 'failed'; evidence.error = 'Owned IME descendants survived cleanup';}
    fs.writeSync(daemonLog, redact(daemonOutput));
    fs.closeSync(daemonLog);
    fs.writeFileSync(path.join(evidenceDir, 'evidence.json'), JSON.stringify(evidence, null, 2));
    console.log('Native IME evidence: ' + path.join(evidenceDir, 'evidence.json'));
    assert.equal(evidence.owned_ime_processes_remaining, 0, 'Owned IME descendants survived cleanup');
  }
});
