'use strict';
// Native editor diagnostics and published Viewer source strings, plus a Viewer
// emitted by the real CLI from hostile result identifiers. No synthetic DOM
// injection or replacement product renderer is used.
const test=require('node:test');
const assert=require('node:assert/strict');
const fs=require('node:fs');
const os=require('node:os');
const path=require('node:path');
const http=require('node:http');
const crypto=require('node:crypto');
const {spawn,spawnSync}=require('node:child_process');
const {once}=require('node:events');
let playwright;try{playwright=require(process.env.PLAYWRIGHT_MODULE||'playwright');}catch(e){if(process.env.PLAYWRIGHT_MODULE||e.code!=='MODULE_NOT_FOUND')throw e;}
const repo=path.resolve(__dirname,'..');
const binary=path.resolve(process.env.DIR_SIMULATOR_BIN||path.join(repo,'target/debug/dir-simulator'));
const delay=ms=>new Promise(resolve=>setTimeout(resolve,ms));
const sha=bytes=>crypto.createHash('sha256').update(bytes).digest('hex');
const secrets=[];
const redact=value=>secrets.reduce((s,v)=>v?s.split(v).join('[redacted]'):s,String(value)).replace(/(http:\/\/127\.0\.0\.1:\d+)\/[a-f0-9]{64}/gi,'$1/[redacted]');
const hostile='</script><img id="hostile-node" src="https://hostile.invalid/probe" onerror="globalThis.hostileExecuted=1"><script>globalThis.hostileExecuted=1</script><!--';

test('XC40 hostile NED diagnostics and real rendered Viewer remain inert in Chromium',{skip:playwright?false:'Set PLAYWRIGHT_MODULE',timeout:180000},async()=>{
  const root=fs.mkdtempSync(path.join(os.tmpdir(),'ned-hostile-browser-')),input=path.join(root,'input'),state=path.join(root,'state'),exports=path.join(root,'exports'),evidenceDir=path.resolve(process.env.DIR_NED_HOSTILE_EVIDENCE||path.join(root,'evidence'));
  fs.mkdirSync(path.join(input,'models/demo'),{recursive:true});fs.mkdirSync(exports);fs.mkdirSync(evidenceDir,{recursive:true});
  for(const [src,dest]of [['examples/can/baseline.ini','project.ini'],['examples/can/baseline.json','baseline.json'],['examples/can/models/demo/Main.ned','models/demo/Main.ned']])fs.copyFileSync(path.join(repo,src),path.join(input,dest));
  const nedPath=path.join(input,'models/demo/Main.ned'),validSource=fs.readFileSync(nedPath,'utf8')+'\n// hostile source text: '+hostile+'\n';fs.writeFileSync(nedPath,validSource);
  const originalNedHash=sha(fs.readFileSync(nedPath));
  const editor=spawn(binary,['ned-editor','--config',path.join(input,'project.ini'),'--export-root',exports,'--state-root',state],{cwd:root,stdio:['ignore','pipe','pipe']});
  const closed=once(editor,'close');let browser,server;
  const evidence={root,binary_sha256:sha(fs.readFileSync(binary)),screenshots:[],external_requests:[],page_errors:[],csp_errors:[]};
  const screenshot=async(page,name)=>{const file=path.join(evidenceDir,name);await page.screenshot({path:file,fullPage:true});evidence.screenshots.push({path:file,sha256:sha(fs.readFileSync(file))});};
  try{
    const startup=await new Promise((resolve,reject)=>{let out='';const timer=setTimeout(()=>reject(new Error('native startup timeout')),20000);editor.on('error',()=>{clearTimeout(timer);reject(new Error('native startup failed'));});editor.stdout.on('data',bytes=>{out+=bytes;const end=out.indexOf('\n');if(end>=0){clearTimeout(timer);resolve(JSON.parse(out.slice(0,end)));}});});
    const startupURL=new URL(startup.url),origin=startupURL.origin,secret=new URLSearchParams(startupURL.hash.slice(1)).get('session');secrets.push(secret);
    const api=async route=>{const r=await fetch(origin+route,{headers:{'X-Editor-Session':secret}});assert.equal(r.status,200,'authenticated observation');return r.json();};
    const waitView=async predicate=>{const end=Date.now()+30000;for(;;){const view=await api('/api/session');if(!view.busy&&predicate(view))return view;assert.ok(Date.now()<end,'expected editor state timed out');await delay(40);}};
    browser=await playwright.chromium.launch({headless:true,...(process.env.PLAYWRIGHT_CHROMIUM_EXECUTABLE?{executablePath:process.env.PLAYWRIGHT_CHROMIUM_EXECUTABLE}:{})});evidence.browser_version=browser.version();
    const context=await browser.newContext({viewport:{width:1500,height:1050}});
    await context.addInitScript(()=>{globalThis.hostileExecuted=0;globalThis.observedCsp=[];document.addEventListener('securitypolicyviolation',event=>globalThis.observedCsp.push(event.violatedDirective));});
    await context.route('https://hostile.invalid/**',route=>{evidence.external_requests.push('hostile.invalid');return route.abort();});
    context.on('page',page=>{page.on('pageerror',error=>evidence.page_errors.push(redact(error.message)));page.on('console',message=>{if(message.type()==='error'&&/content security policy|\bcsp\b/i.test(message.text()))evidence.csp_errors.push(redact(message.text()));});});
    const page=await context.newPage();page.setDefaultTimeout(20000);await page.goto(startup.url);await page.waitForFunction(()=>!document.getElementById('source-editor').disabled);
    assert.equal(await page.locator('#source-editor').inputValue(),validSource);
    const before=await api('/api/session'),main=before.files.find(file=>file.role==='ned');
    // The common lexer rejects this hostile declaration name. The actual source
    // and resulting diagnostic must display as text, including closing tags.
    const invalidName=validSource+'\nmodule '+JSON.stringify(hostile)+' {}\n';
    await page.locator('#source-editor').fill(invalidName);
    const invalid=await waitView(view=>view.files.find(file=>file.id===main.id)?.hash===sha(Buffer.from(invalidName))&&view.diagnostics.length>0);
    await page.waitForFunction(()=>document.getElementById('diagnostics').textContent.includes('</script>'));
    assert.ok((await page.locator('#diagnostics').innerText()).includes('</script>'));
    assert.equal(await page.locator('#source-editor').inputValue(),invalidName);
    assert.equal(await page.locator('#hostile-node').count(),0);assert.equal(await page.evaluate(()=>globalThis.hostileExecuted),0);
    assert.deepEqual(await page.evaluate(()=>globalThis.observedCsp),[]);
    await screenshot(page,'hostile-ned-name-diagnostic-as-text.png');
    evidence.editor={hostile_source_sha256:sha(Buffer.from(invalidName)),diagnostic_count:invalid.diagnostics.length,diagnostic_contains_closing_tag:true,no_injected_nodes:true};
    await page.locator('#source-editor').fill(validSource);
    await waitView(view=>view.files.find(file=>file.id===main.id)?.hash===sha(Buffer.from(validSource))&&view.files.find(file=>file.id===main.id)?.syntax==='parsed');
    await page.locator('#validate').click();await waitView(view=>view.analysis==='ready'&&view.diagnostics.length===0);
    const runsBefore=new Set((await api('/api/runs')).runs.map(run=>run.run_id));
    await page.locator('#run-simulation').click();await page.waitForFunction(()=>document.getElementById('editor-dialog').open&&document.getElementById('dialog-title').textContent==='現在の編集内容を実行');await page.locator('#dialog-submit').click();
    let run;const deadline=Date.now()+60000;for(;;){const runs=(await api('/api/runs')).runs;run=runs.find(item=>!runsBefore.has(item.run_id));if(run&&run.phase==='completed')break;assert.ok(Date.now()<deadline,'hostile source run completion');await delay(50);}
    const card=page.locator('#runs article.run-card[data-run-id="'+run.run_id+'"]');await card.getByRole('button',{name:'Viewerを準備',exact:true}).click();const link=card.getByRole('link',{name:'Viewerを開く'});await link.waitFor();const capability=new URL(await link.getAttribute('href'));secrets.push(capability.href,capability.pathname.slice(1));
    const popupPromise=context.waitForEvent('page');await link.click();const nativeViewer=await popupPromise;await nativeViewer.waitForFunction(()=>document.getElementById('dashboard')&&!document.getElementById('dashboard').hidden);
    assert.equal(await nativeViewer.evaluate(()=>window.opener===null),true);
    const embedded=await nativeViewer.locator('#embedded-results').textContent();assert.ok(embedded.includes('\\u003c/script'));
    const decoded=JSON.parse(embedded);assert.ok(decoded.metadata.sources.some(source=>String(source.content_utf8).includes(hostile)));
    assert.equal(await nativeViewer.evaluate(()=>globalThis.hostileExecuted),0);assert.equal(await nativeViewer.locator('#hostile-node').count(),0);assert.deepEqual(await nativeViewer.evaluate(()=>globalThis.observedCsp),[]);
    const previous=await nativeViewer.locator('#current-ps').innerText();await nativeViewer.locator('#next-time').click();await nativeViewer.waitForFunction(value=>document.getElementById('current-ps').textContent!==value,previous);
    await screenshot(nativeViewer,'native-hostile-source-viewer-after-step.png');
    evidence.native_viewer={run_id:run.run_id,source_contains_hostile_text:true,opener_null:true,normal_step_succeeded:true,csp_violations:0};
    await nativeViewer.close();
    // Real CLI rendering also accepts hostile result identifiers. Serve its
    // emitted bytes with the same hash-only script/CSP policy as ViewerService;
    // the fixture server supplies transport, not HTML or product JavaScript.
    const originalResult=path.join(state,'runs',run.run_id,'result/results.json');
    const raw=JSON.parse(fs.readFileSync(originalResult,'utf8'));raw.run_id=hostile;
    const old=raw.simulation.requests[0].request_id;raw.simulation.requests[0].request_id=hostile;
    for(const row of [...raw.simulation.receivers,...raw.simulation.records])if(row.request_id===old)row.request_id=hostile;
    const hostileJSON=path.join(evidenceDir,'hostile-results.json'),hostileHTML=path.join(evidenceDir,'hostile-viewer.html');fs.writeFileSync(hostileJSON,JSON.stringify(raw));
    const rendered=spawnSync(binary,['view','--input',hostileJSON,'--output',hostileHTML],{cwd:root,encoding:'utf8',timeout:20000});assert.equal(rendered.status,0,'actual CLI Viewer render');
    const html=fs.readFileSync(hostileHTML),scripts=[...html.toString().matchAll(/<script>([\s\S]*?)<\/script>/g)].map(match=>"'sha256-"+crypto.createHash('sha256').update(match[1]).digest('base64')+"'");assert.equal(scripts.length,8); // Six legacy assets plus controlled model/app; every emitted script remains hash-allowed.
    const csp="default-src 'none'; script-src "+scripts.join(' ')+"; style-src 'unsafe-inline'; img-src data: blob:; connect-src 'none'; object-src 'none'; base-uri 'none'; frame-ancestors 'none'; form-action 'none'";
    server=http.createServer((request,response)=>{response.writeHead(200,{'Content-Type':'text/html; charset=utf-8','Content-Security-Policy':csp,'Referrer-Policy':'no-referrer','X-Content-Type-Options':'nosniff'});response.end(html);});server.listen(0,'127.0.0.1');await once(server,'listening');
    const cliViewer=await context.newPage();await cliViewer.goto('http://127.0.0.1:'+server.address().port+'/');await cliViewer.locator('#dashboard').waitFor({state:'visible'});
    assert.equal(await cliViewer.evaluate(()=>globalThis.hostileExecuted),0);assert.equal(await cliViewer.locator('#hostile-node').count(),0);assert.ok((await cliViewer.locator('#run-id').innerText()).includes('</script>'));
    await cliViewer.locator('#request-search').fill('</script>');assert.ok(await cliViewer.locator('#request-rows tr').count()>0);await cliViewer.locator('#request-rows tr').first().click();assert.ok((await cliViewer.locator('#selected-id').innerText()).includes('</script>'));
    await cliViewer.locator('#request-search').fill('');const cliPrevious=await cliViewer.locator('#current-ps').innerText();await cliViewer.locator('#next-time').click();await cliViewer.waitForFunction(value=>document.getElementById('current-ps').textContent!==value,cliPrevious);
    assert.deepEqual(await cliViewer.evaluate(()=>globalThis.observedCsp),[]);await screenshot(cliViewer,'cli-hostile-identifiers-viewer-after-step.png');
    evidence.cli_viewer={input:hostileJSON,input_sha256:sha(fs.readFileSync(hostileJSON)),html:hostileHTML,html_sha256:sha(html),renderer:'product CLI view',transport:'fixture HTTP server with native ViewerService CSP form and emitted script hashes',script_hashes:scripts.length,normal_search_select_step:true,csp_violations:0};
    assert.deepEqual(evidence.external_requests,[]);assert.deepEqual(evidence.page_errors,[]);assert.deepEqual(evidence.csp_errors,[]);assert.equal(sha(fs.readFileSync(nedPath)),originalNedHash);evidence.original_source_disk_unchanged=true;
  }catch(error){evidence.failure=redact(error.stack||error.message);throw new Error(evidence.failure);}
  finally{if(browser)await browser.close();if(server)await new Promise(resolve=>server.close(resolve));if(editor.exitCode===null&&editor.signalCode===null)editor.kill('SIGTERM');await Promise.race([closed,delay(5000)]);const text=JSON.stringify(evidence,null,2);assert.ok(secrets.every(secret=>!text.includes(secret)),'evidence excludes secrets');fs.writeFileSync(path.join(evidenceDir,'hostile-browser-evidence.json'),text+'\n');console.log('Hostile browser evidence: '+path.join(evidenceDir,'hostile-browser-evidence.json'));}
});
