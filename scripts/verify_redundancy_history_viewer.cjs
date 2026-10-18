'use strict';

// Browser-check the actual, standalone Viewer exports produced by the formal v1.1.6 binary.
const assert = require('node:assert/strict');
const crypto = require('node:crypto');
const fs = require('node:fs');
const os = require('node:os');
const path = require('node:path');
const { pathToFileURL } = require('node:url');
const { chromium } = require('playwright');

const ROOT = path.resolve(__dirname, '..');
const AUDIT = path.dirname(ROOT);
const EVIDENCE = path.join(AUDIT, 'evidence/articles110-111/redundancy/run-v1.1.6-reportfix');
const RESULTS = path.join(ROOT, 'docs/verification/results/redundancy-history');
const ASSETS = path.join(ROOT, 'docs/guide/assets');
const CAPTURE_REPORT = path.join(RESULTS, 'viewer-captures.json');
const MEASUREMENTS = JSON.parse(fs.readFileSync(path.join(RESULTS, 'measurements.json'), 'utf8'));
const EXPECTED_COMMIT = '397900451015c9bca6a33d24feac0a88b9b7b8fe';
const VIEWPORTS = [
  { name: 'desktop', width: 1440, height: 1000 },
  { name: 'mobile', width: 390, height: 844 },
];
const EXPECTED_PATHS = {
  direct: ['Net.a.tx', 'Net.s1.tx_b', 'Net.s2.tx_a'],
  alternate: ['Net.a.tx', 'Net.s1.tx_c', 'Net.s3.tx_c', 'Net.s2.tx_a'],
};

function sha256(value) {
  return crypto.createHash('sha256').update(value).digest('hex');
}

function pngDimensions(value) {
  assert.equal(value.toString('hex', 0, 8), '89504e470d0a1a0a', 'invalid PNG signature');
  return { width: value.readUInt32BE(16), height: value.readUInt32BE(20) };
}

async function setReplayTime(page, timePs) {
  const endPs = '10000000';
  const label = page.locator('#controlled-dashboard .transport strong');
  const slider = page.locator('#controlled-dashboard input[type="range"]');
  const tick = Number(BigInt(timePs) * 10000n / BigInt(endPs));
  await slider.evaluate((element, value) => {
    element.value = String(value);
    element.dispatchEvent(new Event('input', { bubbles: true }));
  }, tick);
  const displayed = `${timePs} ps / ${endPs} ps`;
  await page.waitForFunction(value => {
    const item = document.querySelector('#controlled-dashboard .transport strong');
    return item && item.textContent === value;
  }, displayed);
  return displayed;
}

async function tableRows(page, headingText) {
  const heading = page.locator('#controlled-dashboard h2').filter({ hasText: headingText });
  assert.equal(await heading.count(), 1, `expected one ${headingText} table`);
  return heading.evaluate(element => Array.from(
    element.nextElementSibling.querySelectorAll('tbody tr'),
    row => Array.from(row.children, cell => cell.innerText),
  ));
}

async function expandPayload(page, { kind, timePs, contains }) {
  let rows = page.locator('#controlled-dashboard details').filter({ hasText: kind });
  if (timePs !== undefined) rows = rows.filter({ hasText: `${timePs} ps` });
  if (contains) rows = rows.filter({ hasText: contains });
  assert.equal(await rows.count(), 1, `expected one ${kind} record at ${timePs ?? 'any time'} ps`);
  await rows.locator('summary').click();
  return JSON.parse(await rows.locator('pre').innerText());
}

async function inspectState(page, capacity, timePs) {
  const branches = await tableRows(page, '冗長配送の枝');
  assert.equal(branches.length, 4, 'two origins must have four visible branch records');
  const recoveryRows = page.locator('#controlled-dashboard details').filter({ hasText: 'dir.redundancy.recovery' });
  const recoveryLabels = await recoveryRows.locator('summary').evaluateAll(nodes => nodes.map(node => node.innerText));

  if (timePs === '4535000') {
    const seq1Recovery = await expandPayload(page, { kind: 'dir.redundancy.recovery', timePs, contains: 'unicast:1' });
    assert.equal(seq1Recovery.sequence, '1');
    assert.equal(seq1Recovery.decision, 'accept_advance');
    const seq1Branch = await expandPayload(page, { kind: 'dir.redundancy.branch', timePs, contains: 'unicast:1' });
    assert.equal(seq1Branch.sequence, '1');
    assert.deepEqual(seq1Branch.path_ports, EXPECTED_PATHS.direct);
    const reception = await expandPayload(page, { kind: 'ethernet.dynamic.reception', timePs, contains: 'Net.b.rx' });
    assert.equal(reception.status, 'received');
    assert.equal(reception.ingress, 'Net.b.rx');
  } else if (timePs === '7439000') {
    assert(!recoveryLabels.some(label => label.includes('7440000 ps')), 'late recovery must not appear before its receive time');
    const alt0 = branches.find(row => row[1] === 'alternate' && row[2].includes('unicast:0'));
    assert(alt0, 'sequence-0 alternate branch must be visible');
    assert.equal(alt0[3], 'in_flight');
  } else if (timePs === '7440000') {
    const lateRecovery = await expandPayload(page, { kind: 'dir.redundancy.recovery', timePs, contains: 'unicast:0' });
    assert.equal(lateRecovery.sequence, '0');
    assert.equal(lateRecovery.decision, capacity === 1 ? 'stale' : 'duplicate');
    assert.equal(lateRecovery.delivered, false);
    const lateBranch = await expandPayload(page, { kind: 'dir.redundancy.branch', timePs, contains: 'unicast:0' });
    assert.equal(lateBranch.sequence, '0');
    assert.equal(lateBranch.path_id, 'alternate');
    assert.equal(lateBranch.status, capacity === 1 ? 'stale' : 'duplicate');
    assert.deepEqual(lateBranch.path_ports, EXPECTED_PATHS.alternate);
    const reception = await expandPayload(page, { kind: 'ethernet.dynamic.reception', timePs, contains: 'Net.b.rx' });
    assert.equal(reception.status, 'received');
    assert.equal(reception.ingress, 'Net.b.rx');
  } else {
    throw new Error(`unhandled replay point ${timePs}`);
  }

  const branchStatuses = branches.map(row => ({ path: row[1], id: row[2], status: row[3] }));
  return { branches: branchStatuses, recovery_labels: recoveryLabels };
}

async function main() {
  assert.equal(MEASUREMENTS.status, 'passed');
  assert.equal(MEASUREMENTS.source_commit, EXPECTED_COMMIT);
  const staging = fs.mkdtempSync(path.join(os.tmpdir(), 'guide-redundancy-viewer-'));
  const files = [];
  const browserErrors = [];
  const browser = await chromium.launch({ headless: true });
  try {
    const captures = [];
    for (const capacity of [1, 4]) {
      const condition = MEASUREMENTS.conditions[`capacity-${capacity}`];
      const viewer = path.join(EVIDENCE, `capacity-${capacity}/viewer.html`);
      assert.equal(sha256(fs.readFileSync(viewer)), condition.viewer_sha256, `unexpected viewer hash: ${viewer}`);
      const targets = [
        { label: 'sequence1-accepted', timePs: '4535000' },
        { label: 'late-sequence0-before', timePs: '7439000' },
        { label: 'late-sequence0-arrival', timePs: '7440000' },
      ];
      for (const target of targets) {
        for (const viewport of VIEWPORTS) {
          const page = await browser.newPage({ viewport: { width: viewport.width, height: viewport.height } });
          const pageErrors = [];
          const consoleErrors = [];
          const externalRequests = [];
          const failedRequests = [];
          page.on('pageerror', error => pageErrors.push(error.message));
          page.on('console', message => { if (message.type() === 'error') consoleErrors.push(message.text()); });
          page.on('request', request => { if (!request.url().startsWith('file:')) externalRequests.push(request.url()); });
          page.on('requestfailed', request => failedRequests.push({ url: request.url(), error: request.failure()?.errorText }));
          await page.goto(pathToFileURL(viewer).href);
          await page.locator('#controlled-dashboard').waitFor({ state: 'visible' });
          assert.equal(await page.locator('#error-banner').isVisible(), false);
          const displayed = await setReplayTime(page, target.timePs);
          const prefix = await inspectState(page, capacity, target.timePs);

          // Verify rewind restores the same visible direct/alternate branch state.
          if (target.timePs === '7440000') {
            await setReplayTime(page, '4535000');
            const rewindRows = await tableRows(page, '冗長配送の枝');
            assert.equal(rewindRows.filter(row => row[1] === 'direct' && row[3] === 'delivered').length, 2);
            assert.equal(rewindRows.filter(row => row[1] === 'alternate' && row[3] === 'in_flight').length, 2);
            await setReplayTime(page, target.timePs);
            const restore = await tableRows(page, '冗長配送の枝');
            assert.equal(restore.find(row => row[1] === 'alternate' && row[2].includes('unicast:0'))[3], capacity === 1 ? 'stale' : 'duplicate');
            // Re-expand the two rows so the screenshot preserves the late decision evidence.
            await expandPayload(page, { kind: 'dir.redundancy.recovery', timePs: target.timePs, contains: 'unicast:0' });
            await expandPayload(page, { kind: 'dir.redundancy.branch', timePs: target.timePs, contains: 'unicast:0' });
            await expandPayload(page, { kind: 'ethernet.dynamic.reception', timePs: target.timePs, contains: 'Net.b.rx' });
          }

          const filename = `guide-redundancy-history-capacity-${capacity}-${target.label}-t${target.timePs}ps-${viewport.name}.png`;
          const png = await page.screenshot({ fullPage: true, animations: 'disabled' });
          const dimensions = pngDimensions(png);
          const staged = path.join(staging, filename);
          fs.writeFileSync(staged, png);
          files.push({ staged, final: path.join(ASSETS, filename) });
          const pageCheck = { page_errors: pageErrors, console_errors: consoleErrors, external_requests: externalRequests, failed_requests: failedRequests };
          browserErrors.push({ filename, ...pageCheck });
          assert.deepEqual(pageCheck, { page_errors: [], console_errors: [], external_requests: [], failed_requests: [] }, filename);
          captures.push({
            condition: `capacity-${capacity}`, viewer: path.relative(AUDIT, viewer), target: target.label,
            current_time_ps: target.timePs, displayed_time: displayed, viewport,
            screenshot: `docs/guide/assets/${filename}`, bytes: png.length, sha256: sha256(png), ...dimensions,
            observed_branches: prefix.branches, recovery_records_visible: prefix.recovery_labels.length,
            browser: pageCheck,
          });
          await page.close();
        }
      }
    }

    fs.mkdirSync(ASSETS, { recursive: true });
    fs.mkdirSync(RESULTS, { recursive: true });
    assert(!fs.existsSync(CAPTURE_REPORT), `refusing to overwrite ${CAPTURE_REPORT}`);
    for (const file of files) assert(!fs.existsSync(file.final), `refusing to overwrite ${file.final}`);
    for (const file of files) fs.copyFileSync(file.staged, file.final, fs.constants.COPYFILE_EXCL);
    const report = {
      status: 'passed', issue: 111, source_tag: MEASUREMENTS.source_tag,
      source_commit: MEASUREMENTS.source_commit, binary_sha256: MEASUREMENTS.binary_sha256,
      browser_version: browser.version(), captures, browser_errors: browserErrors,
    };
    fs.writeFileSync(CAPTURE_REPORT, `${JSON.stringify(report, null, 2)}\n`, { flag: 'wx' });
    process.stdout.write(`${JSON.stringify(report, null, 2)}\n`);
  } finally {
    await browser.close();
    fs.rmSync(staging, { recursive: true, force: true });
  }
}

main().catch(error => { console.error(error); process.exitCode = 1; });
