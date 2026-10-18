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
const EVIDENCE = path.join(AUDIT, 'evidence/articles110-111/preemption/run-v1.1.6');
const RESULTS = path.join(ROOT, 'docs/verification/results/preemption-fragments');
const ASSETS = path.join(ROOT, 'docs/guide/assets');
const CAPTURE_REPORT = path.join(RESULTS, 'viewer-captures.json');
const MEASUREMENTS = JSON.parse(fs.readFileSync(path.join(RESULTS, 'measurements.json'), 'utf8'));
const EXPECTED_COMMIT = '397900451015c9bca6a33d24feac0a88b9b7b8fe';
const VIEWPORTS = [
  { name: 'desktop', width: 1440, height: 1000 },
  { name: 'mobile', width: 390, height: 844 },
];

function sha256(value) {
  return crypto.createHash('sha256').update(value).digest('hex');
}

function pngDimensions(value) {
  assert.equal(value.toString('hex', 0, 8), '89504e470d0a1a0a', 'invalid PNG signature');
  return { width: value.readUInt32BE(16), height: value.readUInt32BE(20) };
}

function targetName(target) {
  return `${target.label}-t${target.timePs}ps`;
}

async function setReplayTime(page, timePs, endPs) {
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

async function inspectPrefix(page, target, sofPs, expectedFirstFragmentStatus) {
  const transfer = page.locator('#controlled-dashboard details').filter({
    hasText: 'dir.preemption.transfer',
  }).filter({ hasText: 'express:0/v0/c1' });
  assert.equal(await transfer.count(), 1, 'express transfer detail must exist exactly once');
  await transfer.locator('summary').click();
  const data = JSON.parse(await transfer.locator('pre').innerText());
  if (BigInt(target.timePs) >= BigInt(sofPs)) {
    assert.equal(data.sof_ps, sofPs);
    assert.equal(data.status, 'in_flight');
  } else {
    assert.equal(data.sof_ps, null);
    assert.equal(data.status, 'queued');
  }

  const heading = page.locator('#controlled-dashboard h2').filter({ hasText: '分割送信と再開待ち' });
  assert.equal(await heading.count(), 1, 'preemption fragment table must be visible');
  const fragmentRows = await heading.evaluate(element => Array.from(
    element.nextElementSibling.querySelectorAll('tbody tr'),
    row => Array.from(row.children, cell => cell.innerText),
  ));
  assert(fragmentRows.length > 0, 'at least the current slow fragment must be visible');
  const firstFragment = fragmentRows.find(row => row[1] === 'Net.a.tx' && row[3] === '0');
  assert(firstFragment, 'the first Net.a.tx slow fragment must be present');
  assert.equal(firstFragment[7], expectedFirstFragmentStatus);
  return { express: data, visible_fragments: fragmentRows };
}

async function main() {
  assert.equal(MEASUREMENTS.status, 'passed');
  assert.equal(MEASUREMENTS.source_commit, EXPECTED_COMMIT);
  const staging = fs.mkdtempSync(path.join(os.tmpdir(), 'guide-preemption-viewer-'));
  const files = [];
  const browserErrors = [];
  const browser = await chromium.launch({ headless: true });
  try {
    const captures = [];
    for (const size of [20, 40, 100]) {
      const condition = MEASUREMENTS.conditions[`fragment-${size}`];
      const viewer = path.join(EVIDENCE, `fragment-${size}/viewer.html`);
      assert.equal(sha256(fs.readFileSync(viewer)), condition.viewer_sha256, `unexpected viewer hash: ${viewer}`);
      const sofPs = BigInt(condition.summary.express.sof_ps);
      const targets = [
        { label: 'common', timePs: 100000n, expectedFirstFragmentStatus: 'in_flight' },
        { label: 'before-sof', timePs: sofPs - 200n, expectedFirstFragmentStatus: 'in_flight' },
        { label: 'at-sof', timePs: sofPs, expectedFirstFragmentStatus: 'completed' },
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
          const current = await setReplayTime(page, String(target.timePs), MEASUREMENTS.conditions[`fragment-${size}`].summary.end_ps || '2000000');
          const prefix = await inspectPrefix(page, target, String(sofPs), target.expectedFirstFragmentStatus);
          const filename = `guide-preemption-fragments-${size}-${targetName(target)}-${viewport.name}.png`;
          const png = await page.screenshot({ fullPage: true, animations: 'disabled' });
          const dimensions = pngDimensions(png);
          const staged = path.join(staging, filename);
          fs.writeFileSync(staged, png);
          files.push({ staged, final: path.join(ASSETS, filename) });
          const pageCheck = { page_errors: pageErrors, console_errors: consoleErrors, external_requests: externalRequests, failed_requests: failedRequests };
          browserErrors.push({ filename, ...pageCheck });
          assert.deepEqual(pageCheck, { page_errors: [], console_errors: [], external_requests: [], failed_requests: [] }, filename);
          captures.push({
            condition: `fragment-${size}`, viewer: path.relative(AUDIT, viewer), target: target.label,
            current_time_ps: String(target.timePs), displayed_time: current, viewport,
            screenshot: `docs/guide/assets/${filename}`, bytes: png.length, sha256: sha256(png), ...dimensions,
            express_prefix: { status: prefix.express.status, queued_ps: prefix.express.queued_ps, sof_ps: prefix.express.sof_ps },
            first_fragment_status: prefix.visible_fragments.find(row => row[1] === 'Net.a.tx' && row[3] === '0')[7],
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
      status: 'passed', issue: 110, source_tag: MEASUREMENTS.source_tag,
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
