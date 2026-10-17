'use strict';

// Rebuild public example results and capture the real Viewer and NED Editor UI.
// Run from the repository root with FUNCTION_REFERENCE_EVIDENCE_ROOT set to an
// external, empty evidence directory and PLAYWRIGHT_MODULE set to Playwright.
const assert = require('node:assert/strict');
const crypto = require('node:crypto');
const fs = require('node:fs');
const path = require('node:path');
const {spawn, spawnSync} = require('node:child_process');
const {pathToFileURL} = require('node:url');
const {chromium} = require(process.env.PLAYWRIGHT_MODULE || 'playwright');

const repo = process.cwd();
const evidenceRoot = path.resolve(process.env.FUNCTION_REFERENCE_EVIDENCE_ROOT || path.join(repo, '..', 'evidence'));
const imageRoot = path.join(repo, 'docs/guide/assets/reference');
const binary = path.join(repo, 'target/debug/dir-simulator');
const sourceScope = ['Cargo.toml', 'Cargo.lock', 'crates'];
const sourceSha = spawnSync('git', ['rev-parse', 'HEAD'], {cwd: repo, encoding: 'utf8'}).stdout.trim();
const sha256 = bytes => crypto.createHash('sha256').update(bytes).digest('hex');
const hashFile = file => sha256(fs.readFileSync(file));
const publicPath = file => path.relative(repo, file).split(path.sep).join('/');
const evidencePath = file => path.relative(evidenceRoot, file).split(path.sep).join('/');
const scrub = value => String(value)
  .replaceAll(repo, '[source-root]')
  .replaceAll(evidenceRoot, '[evidence-root]')
  .replace(/\/home\/[^\s"'<>]+/g, '[local-path]')
  .replace(/(https?:\/\/127\.0\.0\.1:\d+)\/[a-f0-9]{64}(?=$|[/?#\s"'<>])/gi, '$1/[capability-redacted]')
  .replace(/#(?:session|token|secret|editor_session)=[^&\s"'<>]*/gi, '#session=[redacted]');

const cases = [
  {
    id: 'can',
    config: 'examples/guide/can/minimal.ini',
    inputFiles: ['examples/guide/can/minimal.ini', 'examples/guide/can/minimal.json', 'examples/guide/can/models/demo/Main.ned'],
    jump: {selector: '#jump-time', unitSelector: '#time-unit', unit: 'us', value: '238'},
    request: 'a:0',
    image: 'can-viewer.png',
  },
  {
    id: 'gateway',
    config: 'examples/guide/gateway-delay/delay0.ini',
    inputFiles: ['examples/guide/gateway-delay/delay0.ini', 'examples/guide/gateway-delay/delay0.json', 'examples/guide/gateway-delay/workload.json', 'examples/guide/gateway-delay/models/gateway/Main.ned', 'examples/guide/gateway-delay/models/gateway/Types.ned'],
    jump: {selector: '#jump-time', unitSelector: '#time-unit', unit: 'us', value: '400'},
    request: 'source:0',
    image: 'gateway-viewer.png',
  },
  {
    id: 'ethernet',
    config: 'examples/guide/ethernet-psfp/pass.ini',
    inputFiles: ['examples/guide/ethernet-psfp/pass.ini', 'examples/guide/ethernet-psfp/pass.json', 'examples/guide/ethernet-psfp/workload.json', 'examples/guide/ethernet-psfp/models/ethtsn/Main.ned'],
    jump: {selector: '#eth-jump-value', button: '#eth-jump', value: '1248000', timeSelector: '#eth-time'},
    request: 'psfp:1',
    image: 'ethernet-viewer.png',
  },
  {
    id: 'transaction',
    config: 'examples/guide/axi-strobes/strobe5.ini',
    inputFiles: ['examples/guide/axi-strobes/strobe5.ini', 'examples/guide/axi-strobes/strobe5.json', 'examples/guide/axi-strobes/model.json', 'examples/guide/axi-strobes/models/strobes/Main.ned'],
    jump: {selector: '#txn-jump-value', button: '#txn-jump', value: '90000', timeSelector: '#txn-time'},
    request: 'read:0',
    image: 'transaction-viewer.png',
  },
];

function mkdir(directory) { fs.mkdirSync(directory, {recursive: true}); }
function imageDimensions(file) {
  const png = fs.readFileSync(file);
  assert.equal(png.toString('hex', 0, 8), '89504e470d0a1a0a', `${path.basename(file)} is not a PNG.`);
  return {width: png.readUInt32BE(16), height: png.readUInt32BE(20)};
}

async function captureDetail(page, scenario, destination) {
  await page.setViewportSize({width: 1600, height: 1100});

  if (scenario.id === 'can') {
    const request = await page.locator('#selected-id').innerText();
    const fields = await page.locator('#detail-fields').innerText();
    const recordedTimes = await page.locator('#detail-times').innerText();
    const receivers = await page.locator('#detail-receivers').innerText();
    assert.equal(request, scenario.request);
    assert(fields.includes('Main.a') && recordedTimes.includes('238000000'));
    assert(receivers.includes('Main.b') && receivers.includes('Main.c'));
    await page.locator('.inspector').screenshot({path: destination});
    return {
      mode: 'locator screenshot', selector: '.inspector', viewport: page.viewportSize(),
      scroll_y: await page.evaluate(() => window.scrollY), image_dimensions: imageDimensions(destination),
      displayed_data: {request_id: request, request_fields: fields, recorded_times: recordedTimes, receivers},
    };
  }

  if (scenario.id === 'gateway') {
    const request = await page.locator('#selected-id').innerText();
    const requestFields = await page.locator('#detail-fields').innerText();
    const copyRows = await page.locator('#gateway-rows tr').evaluateAll(rows => rows.map(row => row.innerText));
    assert.equal(request, scenario.request);
    assert(requestFields.includes('2件') && copyRows.length === 2);
    assert(copyRows.some(row => row.includes('Main.gw.b')) && copyRows.some(row => row.includes('Main.gw.c')));
    const capture = await captureScrolledViewport(page, ['.inspector', '#gateway-panel'], destination);
    return {
      ...capture,
      displayed_data: {
        request_id: request,
        request_fields: requestFields,
        recorded_times: await page.locator('#detail-times').innerText(),
        receivers: await page.locator('#detail-receivers').innerText(),
        gateway_copy_rows: copyRows,
        gateway_summary: await page.locator('#gateway-note').innerText(),
      },
    };
  }

  if (scenario.id === 'ethernet') {
    const detail = await page.locator('#eth-details').innerText();
    assert(detail.includes(scenario.request) && detail.includes('Source Priority'));
    assert(detail.includes('Source VLAN') && detail.includes('Hop wire bits'));
    assert(detail.includes('Net.b') && detail.includes('1248000'));
    await page.locator('#eth-details').screenshot({path: destination});
    return {
      mode: 'locator screenshot', selector: '#eth-details', viewport: page.viewportSize(),
      scroll_y: await page.evaluate(() => window.scrollY), image_dimensions: imageDimensions(destination),
      displayed_data: {transfer_detail: detail},
    };
  }

  if (scenario.id === 'transaction') {
    const request = await page.locator('#txn-details').innerText();
    assert(request.includes(scenario.request) && request.includes('read_data') && request.includes('OKAY'));
    const summaries = page.locator('#txn-resources details:not([open]) summary');
    assert.equal(await summaries.count(), 1, 'Expected one collapsed final resource record.');
    await summaries.click();
    const finalResources = await page.locator('#txn-resources').innerText();
    assert(finalResources.includes('Main.ram') && finalResources.includes('data_hex'));
    const capture = await captureScrolledViewport(page, ['#txn-details', '#txn-resources'], destination);
    return {
      ...capture,
      displayed_data: {request_detail: request, final_resources: finalResources},
    };
  }

  throw new Error(`No detail capture is defined for ${scenario.id}.`);
}

async function captureScrolledViewport(page, selectors, destination) {
  const measured = await page.evaluate(selectors => Object.fromEntries(selectors.map(selector => {
    const element = document.querySelector(selector);
    if (!element) return [selector, null];
    const rect = element.getBoundingClientRect();
    return [selector, {top: rect.top + window.scrollY, bottom: rect.bottom + window.scrollY}];
  })), selectors);
  assert(selectors.every(selector => measured[selector]), 'A detail screenshot target is missing.');
  const start = Math.min(...Object.values(measured).map(rect => rect.top));
  const end = Math.max(...Object.values(measured).map(rect => rect.bottom));
  const height = Math.ceil(end - start + 40);
  assert(height <= 4000, `Detail screenshot viewport is unexpectedly tall (${height}px).`);
  await page.setViewportSize({width: 1600, height});
  const maxScroll = await page.evaluate(() => Math.max(0, document.documentElement.scrollHeight - window.innerHeight));
  const requestedScroll = Math.max(0, Math.floor(start - 20));
  await page.evaluate(scrollY => window.scrollTo(0, scrollY), Math.min(requestedScroll, maxScroll));
  const visible = await page.evaluate(selectors => Object.fromEntries(selectors.map(selector => {
    const rect = document.querySelector(selector).getBoundingClientRect();
    return [selector, {top: Math.round(rect.top), bottom: Math.round(rect.bottom)}];
  })), selectors);
  const viewport = page.viewportSize();
  for (const [selector, rect] of Object.entries(visible)) {
    assert(rect.top >= 0 && rect.bottom <= viewport.height, `${selector} is outside the detail screenshot viewport.`);
  }
  await page.screenshot({path: destination});
  return {
    mode: 'scrolled viewport screenshot', selectors, viewport,
    scroll_y: await page.evaluate(() => window.scrollY), visible_rects: visible,
    image_dimensions: imageDimensions(destination),
  };
}

async function screenshotRedacted(page, destination) {
  const absolutePathPattern = /^(?:\/|[A-Za-z]:\\)/;
  const navigationPaths = page.locator('#files button span').filter({hasText: absolutePathPattern});
  const sourceTitle = page.locator('#source-title').filter({hasText: absolutePathPattern});
  const navigationCount = await navigationPaths.count();
  assert(navigationCount > 0, 'Editor navigation path labels were not available for privacy masking.');
  assert.equal(await sourceTitle.count(), 1, 'Editor source path title was not available for privacy masking.');
  await page.screenshot({
    path: destination,
    mask: [navigationPaths, sourceTitle],
    maskColor: '#eef2f7',
  });
  return navigationCount + 1;
}

function recordCommand(argv, label) {
  const result = spawnSync(binary, argv, {cwd: repo, encoding: 'utf8', maxBuffer: 16 * 1024 * 1024});
  const log = [
    `command: ./target/debug/dir-simulator ${argv.join(' ')}`,
    `exit_code: ${result.status === null ? 'signal' : result.status}`,
    scrub(result.stdout || ''),
    scrub(result.stderr || ''),
  ].filter(Boolean).join('\n') + '\n';
  fs.writeFileSync(path.join(evidenceRoot, 'logs', `${label}.log`), log);
  assert.equal(result.status, 0, `${label} command failed; see external evidence logs.`);
  return argv;
}

async function main() {
  assert.match(sourceSha, /^[0-9a-f]{40}$/);
  assert.ok(fs.existsSync(binary), 'Build target/debug/dir-simulator before capture.');
  const sourceStatus = spawnSync('git', ['status', '--porcelain', '--untracked-files=no', '--', ...sourceScope], {cwd: repo, encoding: 'utf8'});
  const sourceDiff = spawnSync('git', ['diff', 'HEAD', '--', ...sourceScope], {cwd: repo, encoding: 'utf8', maxBuffer: 16 * 1024 * 1024});
  assert.equal(sourceStatus.status, 0, 'Could not verify the Rust/Cargo source worktree state.');
  assert.equal(sourceDiff.status, 0, 'Could not calculate the Rust/Cargo source diff.');
  assert.equal(sourceStatus.stdout.trim(), '', 'Rust/Cargo source paths must be clean for capture.');
  assert.equal(sourceDiff.stdout, '', 'Rust/Cargo source diff must be clean for capture.');
  mkdir(path.join(evidenceRoot, 'results'));
  mkdir(path.join(evidenceRoot, 'viewers'));
  mkdir(path.join(evidenceRoot, 'editor/exports'));
  mkdir(path.join(evidenceRoot, 'editor/state'));
  mkdir(path.join(evidenceRoot, 'logs'));
  mkdir(imageRoot);

  const evidence = {
    status: 'capturing',
    source_commit: sourceSha,
    source_sha: sourceSha,
    build: {
      required_command: 'cargo build --locked -p dir-simulator',
      executed_by_capture_script: false,
      binary: {path: publicPath(binary), sha256: hashFile(binary)},
    },
    provenance: {
      capture_runner: {path: publicPath(__filename), sha256: hashFile(__filename)},
      cargo_lock_sha256: hashFile(path.join(repo, 'Cargo.lock')),
      source_integrity: {
        scope: sourceScope,
        status: 'clean',
        tracked_diff_sha256: sha256(sourceDiff.stdout),
      },
    },
    browser: null,
    screenshots: [],
    errors: {page: [], console: [], failed_requests: [], external_requests: []},
  };
  const browser = await chromium.launch({headless: true});
  evidence.browser = {name: 'Chromium', version: browser.version(), viewport: {width: 1440, height: 1100}};
  const page = await browser.newPage({viewport: evidence.browser.viewport});
  const errors = evidence.errors;
  page.on('pageerror', error => errors.page.push(scrub(error.message)));
  page.on('console', message => { if (message.type() === 'error') errors.console.push(scrub(message.text())); });
  page.on('requestfailed', request => errors.failed_requests.push({resource: classifyResource(request.url()), reason: scrub(request.failure()?.errorText || 'unknown')}));
  page.on('request', request => {
    const url = request.url();
    if (!url.startsWith('file:') && !/^http:\/\/127\.0\.0\.1:\d+\//.test(url)) errors.external_requests.push(classifyResource(url));
  });

  try {
    for (const scenario of cases) {
      await page.setViewportSize({width: 1440, height: 1100});
      const resultDir = path.join(evidenceRoot, 'results', scenario.id);
      const viewer = path.join(evidenceRoot, 'viewers', `${scenario.id}-viewer.html`);
      mkdir(resultDir);
      scenario.commands = [
        recordCommand(['validate', '--config', scenario.config], `${scenario.id}-validate`),
        recordCommand(['run', '--config', scenario.config, '--output', path.relative(repo, resultDir)], `${scenario.id}-run`),
        recordCommand(['view', '--input', path.relative(repo, path.join(resultDir, 'results.json')), '--output', path.relative(repo, viewer)], `${scenario.id}-view`),
      ];
      const manifest = JSON.parse(fs.readFileSync(path.join(resultDir, 'manifest.json'), 'utf8'));
      const result = JSON.parse(fs.readFileSync(path.join(resultDir, 'results.json'), 'utf8'));
      assert.equal(manifest.status, 'complete', `${scenario.id} manifest is not complete.`);
      assert.equal(manifest.partial, false, `${scenario.id} result is partial.`);
      assert.ok(fs.statSync(viewer).size > 10000, `${scenario.id} Viewer HTML is unexpectedly small.`);

      await page.goto(pathToFileURL(viewer).href);
      const dashboard = scenario.id === 'ethernet' ? '#ethernet-dashboard' : scenario.id === 'transaction' ? '#transaction-dashboard' : '#dashboard';
      await page.locator(dashboard).waitFor({state: 'visible'});
      assert.equal(await page.locator('#error-banner').isVisible(), false, `${scenario.id} Viewer rejected actual output.`);

      if (scenario.jump.unitSelector) await page.locator(scenario.jump.unitSelector).selectOption(scenario.jump.unit);
      await page.locator(scenario.jump.selector).fill(scenario.jump.value);
      await page.locator(scenario.jump.button || '#jump').click();
      if (scenario.jump.timeSelector) assert((await page.locator(scenario.jump.timeSelector).innerText()).replaceAll(',', '').includes(scenario.jump.value));
      else assert.equal((await page.locator('#current-ps').innerText()).replaceAll(',', '').replace(/\s*ps$/, ''), scenario.id === 'can' ? '238000000' : '400000000');

      const requestTable = scenario.id === 'ethernet' ? '#eth-rows' : scenario.id === 'transaction' ? '#txn-rows' : '#request-rows';
      const selectedRow = page.locator(`${requestTable} tr`).filter({hasText: scenario.request}).first();
      await selectedRow.waitFor({state: 'visible'});
      if (scenario.id === 'gateway') {
        assert.equal(await page.locator('#request-rows tr').count(), 3);
        assert((await page.locator('#request-rows').innerText()).includes('Main.busA'));
      }
      await selectedRow.click();
      if (scenario.id === 'can' || scenario.id === 'gateway') assert.equal(await page.locator('#selected-id').innerText(), scenario.request);
      if (scenario.id === 'ethernet') assert((await page.locator('#eth-details').innerText()).includes(scenario.request));
      if (scenario.id === 'transaction') assert((await page.locator('#txn-details').innerText()).includes(scenario.request));
      await page.evaluate(() => window.scrollTo(0, 0));
      await page.screenshot({path: path.join(imageRoot, scenario.image)});

      evidence.screenshots.push({
        file: publicPath(path.join(imageRoot, scenario.image)),
        bytes: fs.statSync(path.join(imageRoot, scenario.image)).size,
        sha256: hashFile(path.join(imageRoot, scenario.image)),
        source_commit: sourceSha,
        scenario: scenario.id,
        selected_time: scenario.id === 'can' ? '238000000 ps' : scenario.id === 'gateway' ? '400000000 ps' : `${scenario.jump.value} ps`,
        selected_request: scenario.request,
        input_hashes: Object.fromEntries(scenario.inputFiles.map(file => [file, hashFile(path.join(repo, file))])),
        command_args: scenario.commands,
        result: {status: manifest.status, partial: manifest.partial, termination: result.simulation?.termination || result.simulation?.termination_reason || null},
        browser: browser.version(),
        errors: {page: errors.page.slice(), console: errors.console.slice(), failed_requests: errors.failed_requests.slice(), external_requests: errors.external_requests.slice()},
      });

      const detailFile = path.join(imageRoot, `${scenario.id}-detail.png`);
      const detailCapture = await captureDetail(page, scenario, detailFile);
      evidence.screenshots.push({
        file: publicPath(detailFile), bytes: fs.statSync(detailFile).size, sha256: hashFile(detailFile),
        source_commit: sourceSha, scenario: scenario.id,
        selected_time: scenario.id === 'can' ? '238000000 ps' : scenario.id === 'gateway' ? '400000000 ps' : `${scenario.jump.value} ps`,
        selected_request: scenario.request,
        input_hashes: Object.fromEntries(scenario.inputFiles.map(file => [file, hashFile(path.join(repo, file))])),
        command_args: scenario.commands,
        result: {status: manifest.status, partial: manifest.partial, termination: result.simulation?.termination || result.simulation?.termination_reason || null},
        browser: browser.version(), capture: detailCapture,
        errors: {page: errors.page.slice(), console: errors.console.slice(), failed_requests: errors.failed_requests.slice(), external_requests: errors.external_requests.slice()},
      });
    }

    await page.setViewportSize({width: 1440, height: 1100});
    const editorArgs = [
      'ned-editor', '--config', 'examples/can/baseline.ini',
      '--export-root', path.relative(repo, path.join(evidenceRoot, 'editor/exports')),
      '--state-root', path.relative(repo, path.join(evidenceRoot, 'editor/state')),
    ];
    evidence.editor_command_args = editorArgs;
    const editor = spawn(binary, editorArgs, {cwd: repo, stdio: ['ignore', 'pipe', 'pipe']});
    let editorStderr = '';
    editor.stderr.on('data', bytes => { editorStderr += bytes.toString('utf8'); });
    const startup = await new Promise((resolve, reject) => {
      let output = '';
      const timeout = setTimeout(() => reject(new Error('NED Editor startup timed out.')), 20000);
      editor.once('error', error => { clearTimeout(timeout); reject(error); });
      editor.once('close', (code, signal) => { clearTimeout(timeout); reject(new Error(`NED Editor exited before startup (${code}/${signal || 'none'}): ${scrub(editorStderr)}`)); });
      editor.stdout.on('data', bytes => {
        output += bytes.toString('utf8');
        const newline = output.indexOf('\n');
        if (newline < 0) return;
        clearTimeout(timeout);
        try { resolve(JSON.parse(output.slice(0, newline))); } catch (error) { reject(new Error(`NED Editor returned invalid startup data: ${scrub(error.message)}`)); }
      });
    });
    try {
      await page.goto(startup.url);
      await page.waitForFunction(() => !document.querySelector('#source-editor')?.disabled && !document.querySelector('#create-file')?.disabled);
      const sourceButton = page.locator('#files button').filter({hasText: 'Main.ned'}).first();
      await sourceButton.click();
      await page.waitForFunction(() => document.querySelector('#source-title')?.textContent.endsWith('Main.ned'));
      const mainType = page.locator('#types button').filter({hasText: 'demo.Main'}).first();
      await mainType.click();
      await page.waitForSelector('#graph .graph-node');
      await page.locator('#graph .graph-node').first().click();
      const nodeProperties = await page.locator('#selection-properties').innerText();
      assert(nodeProperties.includes('型') && nodeProperties.includes('解決済み'));
      assert((await page.locator('#source-editor').inputValue()).includes('network Main'));
      await page.locator('.sidebar').evaluate(element => { element.scrollTop = 0; });
      const firstRedactionCount = await screenshotRedacted(page, path.join(imageRoot, 'ned-editor.png'));

      const instance = page.locator('#instances button').filter({hasText: 'Main.a'}).first();
      await instance.click();
      await page.waitForFunction(() => document.querySelector('#parameter-context')?.textContent.includes('Main.a'));
      await page.waitForFunction(() => document.querySelector('#parameters')?.textContent.includes('queueCapacity'));
      const parameters = await page.locator('#parameters').innerText();
      assert(parameters.includes('queueCapacity'), 'Selected instance properties do not show queueCapacity.');
      await page.locator('.sidebar').evaluate(element => { element.scrollTop = 0; });
      const settingsRedactionCount = await screenshotRedacted(page, path.join(imageRoot, 'ned-editor-settings.png'));

      for (const [file, sourcePaths, state, redactionCount] of [
        ['ned-editor.png', ['examples/can/baseline.ini', 'examples/can/baseline.json', 'examples/can/models/demo/Main.ned'], {type: 'demo.Main', selected_node: 'a', source: 'models/demo/Main.ned'}, firstRedactionCount],
        ['ned-editor-settings.png', ['examples/can/baseline.ini', 'examples/can/baseline.json', 'examples/can/models/demo/Main.ned'], {instance: 'Main.a', parameter: 'queueCapacity', source: 'models/demo/Main.ned'}, settingsRedactionCount],
      ]) {
        const absolute = path.join(imageRoot, file);
        evidence.screenshots.push({
          file: publicPath(absolute), bytes: fs.statSync(absolute).size, sha256: hashFile(absolute), source_commit: sourceSha,
          scenario: 'ned-editor', ui_state: state,
      redactions: {absolute_path_regions: redactionCount, areas: ['file navigation paths', 'source title'], method: 'Playwright native screenshot mask'},
          input_hashes: Object.fromEntries(sourcePaths.map(item => [item, hashFile(path.join(repo, item))])),
          command_args: editorArgs, browser: browser.version(),
          errors: {page: errors.page.slice(), console: errors.console.slice(), failed_requests: errors.failed_requests.slice(), external_requests: errors.external_requests.slice()},
        });
      }
    } finally {
      editor.kill('SIGTERM');
      await new Promise(resolve => {
        if (editor.exitCode !== null || editor.signalCode !== null) return resolve();
        editor.once('close', resolve);
        setTimeout(resolve, 3000).unref();
      });
      fs.writeFileSync(path.join(evidenceRoot, 'logs', 'ned-editor.log'), scrub(editorStderr));
    }

    assert.deepEqual(errors.page, []);
    assert.deepEqual(errors.console, []);
    assert.deepEqual(errors.failed_requests, []);
    assert.deepEqual(errors.external_requests, []);
    evidence.status = 'passed';
    fs.writeFileSync(path.join(imageRoot, 'screenshot-evidence.json'), JSON.stringify(evidence, null, 2) + '\n');
    console.log(`PASS: ${evidence.screenshots.length} real Chromium screenshots; evidence at ${publicPath(path.join(imageRoot, 'screenshot-evidence.json'))}.`);
  } finally {
    await browser.close();
  }
}

function classifyResource(value) {
  try {
    const url = new URL(value);
    return `${url.protocol}//${url.hostname}${url.pathname.replace(/[a-f0-9]{64}/gi, '[capability]')}`;
  } catch (_) {
    return 'unclassified-resource';
  }
}

main().catch(error => {
  console.error(scrub(error.stack || error.message));
  process.exitCode = 1;
});
