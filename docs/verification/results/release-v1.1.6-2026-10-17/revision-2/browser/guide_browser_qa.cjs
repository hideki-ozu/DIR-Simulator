const fs = require('fs');
const path = require('path');
const http = require('http');
const net = require('net');
const crypto = require('crypto');
const { execFileSync } = require('child_process');
const { chromium } = require('/home/hideki/DIR-Simulator-audits/spec-contract-gaps-20261014/browser-tests/node_modules/playwright');

const sourceRoot = '/home/hideki/DIR-Simulator-audits/release-v1.1.6-20261017/source';
const buildRoot = path.join(sourceRoot, 'build/guide');
const evidenceRoot = path.join(sourceRoot, 'tmp/release-v1.1.6-publication/revision-2/preflight/browser');
const runId = new Date().toISOString().replace(/[:.]/g, '-');
const outDir = path.join(evidenceRoot, 'run-' + runId);
const viewports = [
  { name: 'desktop', width: 1440, height: 1000 },
  { name: 'mobile', width: 390, height: 844 },
];
const expectedReferences = [
  'reference/DIR機能・動作仕様ガイド.html',
  'reference/共通基盤の機能・動作仕様.html',
  'reference/画面操作の機能・動作仕様.html',
  'reference/CAN・Gatewayの機能・動作仕様.html',
  'reference/Ethernetの機能・動作仕様.html',
  'reference/SoC・メモリ・IPCの機能・動作仕様.html',
  'reference/後続10分野の現行実装と利用境界.html',
  'reference/設定・固定値・規格・変更検討.html',
];
const expectedNewEthernetGuides = [
  'EthernetのMAC学習とagingによる転送先.html',
  'Ethernetのlink復旧時刻と受信判定.html',
  'Ethernetの源別multicastとINCLUDE・EXCLUDE.html',
  'Ethernetの動的VID登録と有効期限.html',
  'Ethernetの抽象STPとリンクcostによる配送経路.html',
];
const expectedNavigationTargets = expectedReferences.concat(expectedNewEthernetGuides);
const expectedBodyTargets = [expectedReferences[0]].concat(expectedNewEthernetGuides);
const report = {
  run_label: 'v1.1.6-current-guide-browser-qa-20261017',
  scope: 'Current merged MkDocs build browser QA only. Historical execution measurements and historical site-browser reports are not re-run or rewritten.',
  source_root: sourceRoot,
  build_root: buildRoot,
  evidence_dir: outDir,
  viewports: viewports,
  expected_page_count: 39,
  expected_references: expectedReferences,
  expected_new_ethernet_guides: expectedNewEthernetGuides,
  browser: {},
  server: {},
  current_site: { page_count: 0, pages: [], link_routes: [], link_fragments: [], navigation: [], navigation_skips: [] },
  historical_site_browser_reports: { expected_count: 21, before: [], after: [] },
  repository_status: {},
  errors: { page: [], console: [], request: [], http: [] },
  checks: [],
};
function check(name, ok, details) {
  const row = { name: name, ok: Boolean(ok), details: details || {} };
  report.checks.push(row);
  return row;
}
function walkHtml(dir, rel) {
  rel = rel || '';
  let out = [];
  for (const ent of fs.readdirSync(dir, { withFileTypes: true })) {
    const childRel = path.posix.join(rel, ent.name);
    const full = path.join(dir, ent.name);
    if (ent.isDirectory()) out = out.concat(walkHtml(full, childRel));
    else if (ent.isFile() && ent.name.toLowerCase().endsWith('.html')) out.push(childRel);
  }
  return out;
}
function sha256(file) {
  return crypto.createHash('sha256').update(fs.readFileSync(file)).digest('hex');
}
function listHistoricalReports() {
  const root = path.join(sourceRoot, 'docs/verification/results');
  const found = [];
  function visit(dir) {
    if (!fs.existsSync(dir)) return;
    for (const ent of fs.readdirSync(dir, { withFileTypes: true })) {
      const full = path.join(dir, ent.name);
      if (ent.isDirectory()) visit(full);
      else if (ent.isFile() && ent.name === 'site-browser.json') found.push(full);
    }
  }
  visit(root);
  return found.sort();
}
function historicalHashes() {
  return listHistoricalReports().map(file => ({
    path: path.relative(sourceRoot, file).split(path.sep).join('/'),
    sha256: sha256(file),
  }));
}
function statusSnapshot() {
  return execFileSync('git', ['status', '--porcelain', '--untracked-files=all'], {
    cwd: sourceRoot, encoding: 'utf8',
  });
}
function statusHash(status) {
  return crypto.createHash('sha256').update(status).digest('hex');
}
function mimeType(file) {
  const ext = path.extname(file).toLowerCase();
  const types = {
    '.html': 'text/html; charset=utf-8', '.css': 'text/css; charset=utf-8',
    '.js': 'text/javascript; charset=utf-8', '.json': 'application/json; charset=utf-8',
    '.png': 'image/png', '.jpg': 'image/jpeg', '.jpeg': 'image/jpeg',
    '.svg': 'image/svg+xml', '.gif': 'image/gif', '.webp': 'image/webp',
    '.woff': 'font/woff', '.woff2': 'font/woff2', '.ttf': 'font/ttf',
    '.ico': 'image/x-icon', '.zip': 'application/zip',
  };
  return types[ext] || 'application/octet-stream';
}
function startStaticServer(root) {
  const resolvedRoot = path.resolve(root);
  const server = http.createServer((req, res) => {
    let pathname;
    try {
      pathname = decodeURIComponent(new URL(req.url, 'http://127.0.0.1').pathname);
    } catch {
      res.writeHead(400); res.end('Bad URL'); return;
    }
    let file = path.resolve(resolvedRoot, '.' + pathname);
    if (file !== resolvedRoot && !file.startsWith(resolvedRoot + path.sep)) {
      res.writeHead(403); res.end('Forbidden'); return;
    }
    try {
      if (fs.statSync(file).isDirectory()) file = path.join(file, 'index.html');
    } catch {}
    if (!fs.existsSync(file) || !fs.statSync(file).isFile()) {
      res.writeHead(404); res.end('Not found'); return;
    }
    res.writeHead(200, { 'Content-Type': mimeType(file), 'Cache-Control': 'no-store' });
    fs.createReadStream(file).pipe(res);
  });
  return new Promise((resolve, reject) => {
    server.once('error', reject);
    server.listen(0, '127.0.0.1', () => {
      const address = server.address();
      resolve({ server: server, port: address.port, base: 'http://127.0.0.1:' + address.port + '/' });
    });
  });
}
function attachErrors(page, viewportName) {
  page.on('pageerror', e => report.errors.page.push({ viewport: viewportName, url: page.url(), error: e.message }));
  page.on('console', m => {
    if (m.type() === 'error') report.errors.console.push({ viewport: viewportName, url: page.url(), message: m.text() });
  });
  page.on('requestfailed', r => report.errors.request.push({
    viewport: viewportName, page: page.url(), url: r.url(), failure: r.failure() ? r.failure().errorText : 'unknown',
  }));
  page.on('response', r => {
    if (r.status() >= 400) report.errors.http.push({
      viewport: viewportName, page: page.url(), url: r.url(), status: r.status(),
    });
  });
}
function urlForFile(base, file) {
  return new URL(file.split('/').map(encodeURIComponent).join('/'), base).href;
}
function decodedFilePath(url) {
  const parsed = new URL(url);
  let pathname = parsed.pathname;
  try { pathname = decodeURIComponent(pathname); } catch {}
  if (pathname === '/') return 'index.html';
  return pathname.startsWith('/') ? pathname.slice(1) : pathname;
}
async function openAndMeasure(page, url) {
  const response = await page.goto(url, { waitUntil: 'load', timeout: 30000 });
  await page.evaluate(() => {
    for (const img of document.images) img.loading = 'eager';
  });
  await page.waitForFunction(() => Array.from(document.images).every(img => img.complete), { timeout: 15000 }).catch(() => {});
  const metrics = await page.evaluate(() => {
    const d = document.documentElement;
    const b = document.body;
    const ids = new Set(Array.from(document.querySelectorAll('[id]')).map(e => e.id));
    const names = new Set(Array.from(document.querySelectorAll('a[name]')).map(e => e.getAttribute('name')));
    const links = Array.from(document.querySelectorAll('a[href]')).map(a => a.href);
    const fragments = links.map(href => {
      let u;
      try { u = new URL(href, location.href); } catch { return null; }
      if (!u.hash || u.origin !== location.origin) return null;
      let id = u.hash.slice(1);
      try { id = decodeURIComponent(id); } catch {}
      if (!id) return null;
      let targetId = true;
      if (u.pathname === location.pathname) targetId = ids.has(id) || names.has(id);
      return { href: href, path: u.pathname, fragment: id, ok: targetId };
    }).filter(Boolean);
    const images = Array.from(document.images).map(i => ({
      src: i.currentSrc || i.src, complete: i.complete, naturalWidth: i.naturalWidth,
    }));
    return {
      title: document.title,
      documentWidth: Math.max(d.scrollWidth, b ? b.scrollWidth : 0),
      viewportWidth: window.innerWidth,
      overflow: Math.max(0, Math.max(d.scrollWidth, b ? b.scrollWidth : 0) - window.innerWidth),
      imageCount: images.length,
      badImages: images.filter(i => !i.complete || i.naturalWidth === 0),
      localPageFragmentCount: fragments.length,
      brokenLocalPageFragments: fragments.filter(f => !f.ok),
      ids: Array.from(ids),
      names: Array.from(names),
      links: links,
    };
  });
  return { response: response, metrics: metrics };
}
async function auditSameSiteLinks(snapshots, context, base, viewportName) {
  const origin = new URL(base).origin;
  const pagesByPath = new Map(snapshots.map(row => [row.file, row]));
  const routes = new Map();
  const fragments = [];
  for (const source of snapshots) {
    for (const href of source.links) {
      let u;
      try { u = new URL(href, source.url); } catch { continue; }
      if (u.origin !== origin) continue;
      const file = decodedFilePath(u.href);
      const routeUrl = new URL(u.pathname + u.search, origin).href;
      routes.set(routeUrl, file);
      if (u.hash) {
        let fragment = u.hash.slice(1);
        try { fragment = decodeURIComponent(fragment); } catch {}
        if (fragment && pagesByPath.has(file)) {
          fragments.push({ source: source.file, target: file, fragment: fragment });
        }
      }
    }
  }
  const routeRows = [];
  for (const [url, file] of routes.entries()) {
    let status = null, error = null;
    try {
      const response = await context.request.get(url, { timeout: 15000 });
      status = response.status();
    } catch (e) {
      error = String(e);
    }
    const row = { url: url, file: file, status: status, error: error, ok: status === 200 && !error };
    routeRows.push(row);
    if (!row.ok) report.errors.http.push({ viewport: viewportName, url: url, status: status, error: error });
  }
  const fragmentRows = fragments.map(f => {
    const target = pagesByPath.get(f.target);
    const ok = Boolean(target && (target.ids.includes(f.fragment) || target.names.includes(f.fragment)));
    return { source: f.source, target: f.target, fragment: f.fragment, ok: ok };
  });
  report.current_site.link_routes.push({
    viewport: viewportName, count: routeRows.length, failures: routeRows.filter(r => !r.ok),
  });
  report.current_site.link_fragments.push({
    viewport: viewportName, count: fragmentRows.length, failures: fragmentRows.filter(r => !r.ok),
  });
  check('all same-origin linked routes return HTTP 200 (' + viewportName + ')',
    routeRows.every(r => r.ok), { count: routeRows.length, failures: routeRows.filter(r => !r.ok) });
  check('all same-origin page anchors resolve (' + viewportName + ')',
    fragmentRows.every(r => r.ok), { count: fragmentRows.length, failures: fragmentRows.filter(r => !r.ok) });
}
async function clickFromIndex(page, base, target, viewportName, surface) {
  const indexUrl = urlForFile(base, 'index.html');
  await page.goto(indexUrl, { waitUntil: 'load', timeout: 30000 });
  if (surface === 'navbar' && viewports.find(v => v.name === viewportName).width < 992) {
    const expanded = await page.locator('#navbar-collapse').evaluate(el => el.classList.contains('show')).catch(() => false);
    if (!expanded) {
      await page.locator('button.navbar-toggler[data-bs-target="#navbar-collapse"]').click();
      await page.waitForFunction(() => document.querySelector('#navbar-collapse')?.classList.contains('show'), { timeout: 5000 });
    }
  }
  const selector = surface === 'body' ? '.col-md-9 a[href]' : '#navbar-collapse a[href]';
  const linkIndex = await page.locator(selector).evaluateAll((anchors, targetPath) => {
    function pathOf(href) {
      let path = new URL(href, location.href).pathname;
      try { path = decodeURIComponent(path); } catch {}
      return path.startsWith('/') ? path.slice(1) : path;
    }
    return anchors.findIndex(a => pathOf(a.href) === targetPath);
  }, target);
  if (linkIndex < 0) {
    return { ok: false, reason: 'target link not found on index ' + surface, landed_url: page.url() };
  }
  const link = page.locator(selector).nth(linkIndex);
  if (surface === 'navbar') {
    const dropdownIndex = await link.evaluate(a => {
      const li = a.closest('li.nav-item.dropdown');
      if (!li) return -1;
      return Array.from(document.querySelectorAll('#navbar-collapse li.nav-item.dropdown')).indexOf(li);
    });
    if (dropdownIndex >= 0) {
      const toggle = page.locator('#navbar-collapse li.nav-item.dropdown').nth(dropdownIndex).locator('a.nav-link.dropdown-toggle');
      const expanded = await toggle.getAttribute('aria-expanded');
      if (expanded !== 'true') await toggle.click();
      await page.waitForFunction(i => Boolean(document.querySelectorAll('#navbar-collapse li.nav-item.dropdown')[i]?.querySelector('.dropdown-menu.show')), dropdownIndex, { timeout: 5000 });
    }
  }
  await link.scrollIntoViewIfNeeded();
  const linkText = (await link.innerText()).trim();
  let clickError = null;
  try {
    await Promise.all([
      page.waitForNavigation({ waitUntil: 'load', timeout: 15000 }),
      link.click({ timeout: 10000 }),
    ]);
  } catch (e) {
    clickError = String(e);
  }
  const landed = decodedFilePath(page.url());
  const ok = !clickError && landed === target;
  return { ok: ok, reason: clickError, landed_path: landed, target: target, link_text: linkText };
}
async function portRefused(port) {
  return await new Promise(resolve => {
    const socket = net.createConnection({ host: '127.0.0.1', port: port });
    let finished = false;
    const done = value => {
      if (finished) return;
      finished = true;
      socket.destroy();
      resolve(value);
    };
    socket.once('connect', () => done(false));
    socket.once('error', e => done(e.code === 'ECONNREFUSED'));
    setTimeout(() => done(false), 2000);
  });
}
async function run() {
  fs.mkdirSync(outDir, { recursive: true });
  if (!fs.existsSync(buildRoot)) throw new Error('Built guide missing at ' + buildRoot);
  const siteFiles = walkHtml(buildRoot).filter(file => file !== '404.html').sort();
  report.current_site.page_count = siteFiles.length;
  report.historical_site_browser_reports.before = historicalHashes();
  report.repository_status.before_sha256 = statusHash(statusSnapshot());
  report.repository_status.before_clean = statusSnapshot().length === 0;
  report.browser = { playwright: require('/home/hideki/DIR-Simulator-audits/spec-contract-gaps-20261014/browser-tests/node_modules/playwright/package.json').version };
  check('current build has exactly 39 navigable pages', siteFiles.length === 39, { count: siteFiles.length, includes_404: fs.existsSync(path.join(buildRoot, '404.html')) });
  check('all expected reference chapters exist', expectedReferences.every(f => siteFiles.includes(f)),
    { expected: expectedReferences, missing: expectedReferences.filter(f => !siteFiles.includes(f)) });
  check('all five upstream Ethernet guides exist', expectedNewEthernetGuides.every(f => siteFiles.includes(f)),
    { expected: expectedNewEthernetGuides, missing: expectedNewEthernetGuides.filter(f => !siteFiles.includes(f)) });
  check('21 historical site-browser reports exist', report.historical_site_browser_reports.before.length === 21,
    { count: report.historical_site_browser_reports.before.length, reports: report.historical_site_browser_reports.before });

  const browser = await chromium.launch({ headless: true });
  report.browser.chromium = browser.version();
  let serverInfo;
  try {
    serverInfo = await startStaticServer(buildRoot);
    report.server = { host: '127.0.0.1', port: serverInfo.port, base_url: serverInfo.base, owner: 'this QA process' };
    for (const viewport of viewports) {
      const context = await browser.newContext({
        viewport: { width: viewport.width, height: viewport.height },
        deviceScaleFactor: 1,
      });
      const page = await context.newPage();
      attachErrors(page, viewport.name);
      const snapshots = [];
      for (const file of siteFiles) {
        let status = null, metrics = null, error = null;
        const url = urlForFile(serverInfo.base, file);
        try {
          const opened = await openAndMeasure(page, url);
          status = opened.response ? opened.response.status() : null;
          metrics = opened.metrics;
        } catch (e) {
          error = String(e);
        }
        const pass = status === 200 && metrics &&
          metrics.badImages.length === 0 &&
          metrics.overflow === 0 &&
          metrics.brokenLocalPageFragments.length === 0;
        const row = {
          viewport: viewport.name, file: file, status: status, error: error,
          title: metrics ? metrics.title : null,
          width: metrics ? metrics.documentWidth : null,
          viewport_width: metrics ? metrics.viewportWidth : null,
          overflow: metrics ? metrics.overflow : null,
          image_count: metrics ? metrics.imageCount : null,
          bad_images: metrics ? metrics.badImages : null,
          local_page_fragment_count: metrics ? metrics.localPageFragmentCount : null,
          broken_local_page_fragments: metrics ? metrics.brokenLocalPageFragments : null,
          ok: Boolean(pass),
        };
        report.current_site.pages.push(row);
        check('page loads with images, local anchors, and no horizontal overflow (' + viewport.name + ') ' + file,
          Boolean(pass), row);
        if (metrics) snapshots.push({
          file: file, url: url, ids: metrics.ids, names: metrics.names, links: metrics.links,
        });
        const shotName =
          viewport.name === 'desktop' && file === 'index.html' ? '01-index-desktop.png' :
          viewport.name === 'desktop' && file === 'reference/Ethernetの機能・動作仕様.html' ? '02-reference-ethernet-desktop.png' :
          viewport.name === 'mobile' && file === 'Ethernetの動的VID登録と有効期限.html' ? '03-new-ethernet-mobile.png' : null;
        if (shotName && pass) {
          await page.evaluate(() => window.scrollTo(0, 0));
          const shotPath = path.join(outDir, shotName);
          await page.screenshot({ path: shotPath });
          row.screenshot = shotPath;
        }
      }
      await auditSameSiteLinks(snapshots, context, serverInfo.base, viewport.name);
      for (const target of expectedNavigationTargets) {
        const surfaces = expectedBodyTargets.includes(target) ? ['body', 'navbar'] : ['navbar'];
        if (!expectedBodyTargets.includes(target)) {
          report.current_site.navigation_skips.push({
            viewport: viewport.name, surface: 'body', target: target,
            reason: 'reference chapters are exposed in the index navbar dropdown, not in the index article body',
          });
        }
        for (const surface of surfaces) {
          let result;
          try {
            result = await clickFromIndex(page, serverInfo.base, target, viewport.name, surface);
          } catch (e) {
            result = { ok: false, reason: String(e), target: target };
          }
          const navRow = { viewport: viewport.name, surface: surface, target: target, ...result };
          report.current_site.navigation.push(navRow);
          check('index ' + surface + ' navigation reaches ' + target + ' (' + viewport.name + ')', navRow.ok, navRow);
        }
      }
      await context.close();
    }
  } finally {
    if (serverInfo && serverInfo.server) {
      await new Promise(resolve => serverInfo.server.close(resolve));
      const refused = await portRefused(serverInfo.port);
      report.server.closed = !serverInfo.server.listening;
      report.server.port_refused_after_shutdown = refused;
      check('owned HTTP server closed and port refuses new connections',
        report.server.closed && refused, {
          host: report.server.host, port: report.server.port,
          closed: report.server.closed, port_refused_after_shutdown: refused,
        });
    }
    await browser.close();
  }
  report.historical_site_browser_reports.after = historicalHashes();
  const historicalUnchanged =
    JSON.stringify(report.historical_site_browser_reports.before) ===
    JSON.stringify(report.historical_site_browser_reports.after);
  check('all 21 historical site-browser reports are unchanged',
    report.historical_site_browser_reports.before.length === 21 &&
      report.historical_site_browser_reports.after.length === 21 && historicalUnchanged,
    { before_count: report.historical_site_browser_reports.before.length,
      after_count: report.historical_site_browser_reports.after.length, unchanged: historicalUnchanged });
  const afterStatus = statusSnapshot();
  report.repository_status.after_sha256 = statusHash(afterStatus);
  report.repository_status.after_clean = afterStatus.length === 0;
  report.repository_status.unchanged = report.repository_status.before_sha256 === report.repository_status.after_sha256;
  check('candidate repository status unchanged by browser QA', report.repository_status.unchanged,
    { before_sha256: report.repository_status.before_sha256, after_sha256: report.repository_status.after_sha256 });
  for (const kind of ['page', 'console', 'request', 'http']) {
    check('no browser ' + kind + ' errors', report.errors[kind].length === 0,
      { count: report.errors[kind].length, items: report.errors[kind] });
  }
  report.summary = {
    result: report.checks.every(row => row.ok) ? 'PASS' : 'FAIL',
    pass: report.checks.filter(row => row.ok).length,
    fail: report.checks.filter(row => !row.ok).length,
    check_count: report.checks.length,
    browser_page_loads: report.current_site.pages.length,
    expected_browser_page_loads: 78,
    index_navigation_clicks: report.current_site.navigation.length,
    expected_index_navigation_clicks: 38,
    planned_navigation_clicks: expectedNavigationTargets.length * 2 * viewports.length,
    skipped_index_body_links: report.current_site.navigation_skips.length,
    same_origin_routes_per_viewport: report.current_site.link_routes.map(row => ({ viewport: row.viewport, count: row.count, failures: row.failures.length })),
    same_origin_fragments_per_viewport: report.current_site.link_fragments.map(row => ({ viewport: row.viewport, count: row.count, failures: row.failures.length })),
    page_console_request_http_errors: Object.fromEntries(Object.keys(report.errors).map(k => [k, report.errors[k].length])),
    screenshots: report.current_site.pages.filter(row => row.screenshot).map(row => row.screenshot),
  };
  const reportPath = path.join(outDir, 'browser-qa-report.json');
  fs.writeFileSync(reportPath, JSON.stringify(report, null, 2) + '\n');
  const summaryPath = path.join(outDir, 'browser-qa-summary.txt');
  fs.writeFileSync(summaryPath, [
    'Result: ' + report.summary.result,
    'Pages: ' + report.current_site.page_count + ' current pages; ' + report.current_site.pages.length + ' page/viewport loads',
    'Navigation: ' + report.current_site.navigation.length + ' homepage body/navbar clicks across 8 references + 5 upstream Ethernet guides',
    'Errors: ' + JSON.stringify(report.summary.page_console_request_http_errors),
    'Historical site-browser reports: ' + report.historical_site_browser_reports.after.length + ', unchanged=' + historicalUnchanged,
    'HTTP server: port ' + report.server.port + ', closed=' + report.server.closed + ', port_refused=' + report.server.port_refused_after_shutdown,
    'Report: ' + reportPath,
  ].join('\n') + '\n');
  process.stdout.write(JSON.stringify({
    report: reportPath, result: report.summary.result, pass: report.summary.pass, fail: report.summary.fail,
    page_loads: report.summary.browser_page_loads, nav_clicks: report.summary.index_navigation_clicks,
    link_routes: report.summary.same_origin_routes_per_viewport,
    link_fragments: report.summary.same_origin_fragments_per_viewport,
    errors: report.summary.page_console_request_http_errors, screenshots: report.summary.screenshots,
    server: report.server, historical_reports_unchanged: historicalUnchanged,
  }, null, 2) + '\n');
  if (report.summary.result !== 'PASS') process.exitCode = 1;
}
run().catch(e => {
  console.error(e);
  try { fs.mkdirSync(outDir, { recursive: true }); fs.writeFileSync(path.join(outDir, 'fatal.txt'), String(e.stack || e) + '\n'); } catch {}
  process.exitCode = 1;
});
