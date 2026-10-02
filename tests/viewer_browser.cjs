// Optional browser smoke test: NODE_PATH=<playwright node_modules> node tests/viewer_browser.cjs
'use strict';
const {chromium} = require('playwright');
const assert = require('node:assert/strict');
const fs = require('node:fs');
const os = require('node:os');
const path = require('node:path');
const {pathToFileURL} = require('node:url');
const {execFileSync} = require('node:child_process');
const root = path.resolve(__dirname,'..');
const bin = process.env.DIR_SIMULATOR_BIN || path.join(root,'target/debug/dir-simulator');
const temp = fs.mkdtempSync(path.join(os.tmpdir(),'dir-viewer-browser-'));
const run = args => JSON.parse(execFileSync(bin,args,{encoding:'utf8'}));
const assertText = async(page,id,value) => assert.equal((await page.locator(id).innerText()).trim(),value);
async function jumpPs(page,time){
  await page.locator('#time-unit').selectOption('ps');
  await page.locator('#jump-time').fill(String(time));
  await page.locator('#jump').click();
}
async function main(){
  const report=run(['run','--config',path.join(root,'examples/can/baseline.ini'),'--output',path.join(temp,'baseline')]);
  const result=path.join(report.output_path,'results.json');
  const viewer=path.join(temp,'standalone.html');
  run(['view','--input',result,'--output',viewer]);
  const browser=await chromium.launch({headless:true});
  try {
    const page=await browser.newPage({viewport:{width:1440,height:1000}});
    const errors=[],network=[];
    page.on('pageerror',error=>errors.push(error.message));
    page.on('request',req=>{if(/^https?:/.test(req.url()))network.push(req.url());});
    await page.goto(pathToFileURL(viewer).href);
    await page.locator('#dashboard').waitFor({state:'visible'});
    await assertText(page,'#count-generated','3');
    await assertText(page,'#count-success','0');
    await page.locator('#next-time').click();
    assert.notEqual(await page.locator('#current-ps').innerText(),'0 ps');
    await page.locator('#prev-time').click();
    await assertText(page,'#current-ps','0 ps');
    await page.locator('#play').click();
    await page.waitForFunction(()=>document.getElementById('current-ps').textContent!=='0 ps');
    await page.locator('#play').click();
    await page.locator('#time-unit').selectOption('ps');
    await page.locator('#jump-time').fill('20000000000');
    await page.locator('#jump').click();
    await assertText(page,'#current-ps','20000000000 ps');
    await assertText(page,'#count-generated','60');
    await assertText(page,'#count-success','60');
    await assertText(page,'#count-received','120');
    await page.locator('#request-search').fill('a:0');
    assert.equal(await page.locator('#request-rows tr').count(),1);
    await page.locator('#request-rows tr').first().click();
    await assertText(page,'#selected-id','a:0');
    await page.locator('#zoom-in').click();
    await page.locator('#zoom-reset').click();
    await page.locator('#request-search').fill('');
    await page.locator('#time-unit').selectOption('us');
    await page.locator('#jump-time').fill('750');
    await page.locator('#jump').click();
    await page.evaluate(()=>window.scrollTo(0,0));
    if(process.env.VIEWER_SCREENSHOT) await page.screenshot({path:process.env.VIEWER_SCREENSHOT,fullPage:false});
    await page.setViewportSize({width:390,height:844});
    assert.equal(await page.evaluate(()=>document.documentElement.scrollWidth<=innerWidth),true,'mobile document should not overflow horizontally');
    await page.setViewportSize({width:1440,height:1000});

    // Direct file:// assets use a file input, never fetch a neighbouring private file.
    await page.goto(pathToFileURL(path.join(root,'crates/dir-simulator/viewer/index.html')).href);
    await page.locator('#empty-state').waitFor({state:'visible'});
    await page.locator('#file-input').setInputFiles(result);
    await page.locator('#dashboard').waitFor({state:'visible'});
    await assertText(page,'#count-generated','3');

    // Failure invalidates the old run; another valid selection recovers.
    await page.locator('#file-input').setInputFiles({name:'bad.json',mimeType:'application/json',buffer:Buffer.from('{not json')});
    await page.locator('#error-banner').waitFor({state:'visible'});
    assert.equal(await page.locator('#dashboard').isVisible(),false);
    await page.locator('#file-input').setInputFiles(result);
    await page.locator('#dashboard').waitFor({state:'visible'});

    // A hostile identifier and source content remain inert in both embedding and DOM rendering.
    const raw=JSON.parse(fs.readFileSync(result));
    const hostile='</script><img src=x onerror="globalThis.viewerInjected=1"><script>globalThis.viewerInjected=1</script>';
    raw.run_id=hostile;
    const old=raw.simulation.requests[0].request_id;
    raw.simulation.requests[0].request_id=hostile;
    for(const row of [...raw.simulation.receivers,...raw.simulation.records]) if(row.request_id===old)row.request_id=hostile;
    raw.metadata.sources[0].content_utf8=hostile;
    const hostilePath=path.join(temp,'hostile.json'),hostileViewer=path.join(temp,'hostile.html');
    fs.writeFileSync(hostilePath,JSON.stringify(raw));
    run(['view','--input',hostilePath,'--output',hostileViewer]);
    await page.goto(pathToFileURL(hostileViewer).href);
    await page.locator('#dashboard').waitFor({state:'visible'});
    assert.equal(await page.evaluate(()=>globalThis.viewerInjected),undefined);
    assert.equal(await page.locator('img[src="x"]').count(),0);
    assert((await page.locator('#run-id').innerText()).includes('</script>'));

    // EOF exactly at cutoff is still in-flight; reached EOF without release is intermission.
    for (const [scenario, expectedSuccess, busLabel] of [
      ['eof-boundary','0','送信中'], ['after-eof','1','バス間隔']
    ]) {
      const edge=run(['run','--config',path.join(root,`docs/verification/fixtures/can/${scenario}.ini`),'--output',path.join(temp,scenario)]);
      const edgePath=path.join(edge.output_path,'results.json');
      const edgeRaw=JSON.parse(fs.readFileSync(edgePath));
      await page.locator('#file-input').setInputFiles({name:scenario+'.json',mimeType:'application/json',buffer:Buffer.from(JSON.stringify(edgeRaw))});
      await page.waitForFunction(name=>document.getElementById('filename').textContent===name,scenario+'.json');
      await page.locator('#time-unit').selectOption('ps');
      await page.locator('#jump-time').fill(edgeRaw.simulation.end_ps);
      await page.locator('#jump').click();
      await assertText(page,'#count-success',expectedSuccess);
      assert((await page.locator('#bus-states').innerText()).includes(busLabel));
    }

    // Partial results show committed EOF at the failure boundary and pending delivery.
    const partial=JSON.parse(fs.readFileSync(result));
    partial.simulation.requests=partial.simulation.requests.slice(0,1);
    const committed=partial.simulation.requests[0];
    committed.model_fields.release_ps=null;
    partial.simulation.end_ps=committed.eof_ps;
    partial.simulation.termination='execution_failed';partial.simulation.partial=true;
    partial.simulation.records=[];
    partial.simulation.receivers=partial.simulation.receivers.filter(r=>r.request_id===committed.request_id);
    for(const r of partial.simulation.receivers){r.status='pending';r.observed_ps=null;r.received_ps=null;}
    await page.locator('#file-input').setInputFiles({name:'partial.json',mimeType:'application/json',buffer:Buffer.from(JSON.stringify(partial))});
    await page.waitForFunction(()=>document.getElementById('filename').textContent==='partial.json');
    await page.locator('#jump-time').fill(partial.simulation.end_ps);await page.locator('#jump').click();
    assert.equal(await page.locator('#partial-badge').isVisible(),true);
    await assertText(page,'#count-success','1');await assertText(page,'#count-received','0');

    // Distinct adjacent timestamps above 2^53 remain distinct in the browser controls.
    const huge=JSON.parse(fs.readFileSync(result)), base=9007199254740992n;
    huge.simulation.requests=huge.simulation.requests.slice(0,1);huge.simulation.receivers=[];huge.simulation.records=[];
    huge.simulation.end_ps=(base+10n).toString();
    const first=huge.simulation.requests[0];
    for(const [field,offset] of [['generated_ps',0n],['ready_ps',1n],['sof_ps',2n],['eof_ps',3n]])first[field]=(base+offset).toString();
    first.model_fields.planned_eof_ps=first.eof_ps;
    first.model_fields.release_ps=(base+4n).toString();first.model_fields.planned_release_ps=first.model_fields.release_ps;
    await page.locator('#file-input').setInputFiles({name:'huge.json',mimeType:'application/json',buffer:Buffer.from(JSON.stringify(huge))});
    await page.waitForFunction(()=>document.getElementById('filename').textContent==='huge.json');
    await page.locator('#jump-time').fill((base+1n).toString());await page.locator('#jump').click();
    await assertText(page,'#current-ps',`${base+1n} ps`);
    await assertText(page,'#count-pending','1');
    await page.locator('#next-time').click();
    await assertText(page,'#current-ps',`${base+2n} ps`);
    await assertText(page,'#count-in-flight','1');

    // File drop uses the same local loader.
    await page.evaluate(content=>{
      const transfer=new DataTransfer();transfer.items.add(new File([content],'dropped.json',{type:'application/json'}));
      document.dispatchEvent(new DragEvent('drop',{dataTransfer:transfer,bubbles:true,cancelable:true}));
    },fs.readFileSync(result,'utf8'));
    await page.waitForFunction(()=>document.getElementById('filename').textContent==='dropped.json');
    await assertText(page,'#count-generated','3');

    // The topology uses all requests even beyond the timeline's drawing cap.
    const overload=run(['run','--config',path.join(root,'examples/can/overload.ini'),'--output',path.join(temp,'overload')]);
    const overloadPath=path.join(overload.output_path,'results.json');
    const overloadRaw=JSON.parse(fs.readFileSync(overloadPath));
    await page.locator('#file-input').setInputFiles(overloadPath);
    await page.waitForFunction(()=>document.getElementById('request-total').textContent==='1200 件');
    assert.equal(await page.locator('#network [data-node]').count(),3);
    const late=overloadRaw.simulation.requests.filter(r=>r.sof_ps!==null).reduce((a,b)=>BigInt(a.sof_ps)>BigInt(b.sof_ps)?a:b);
    const sof=BigInt(late.sof_ps),eof=BigInt(late.model_fields.planned_eof_ps);
    const tx=page.locator('#network [data-packet-kind="tx"]');
    await jumpPs(page,sof+(eof-sof)/4n);
    assert.equal(await tx.getAttribute('data-request-id'),late.request_id);
    const firstPosition=await tx.getAttribute('transform');
    await jumpPs(page,sof+(eof-sof)/2n);
    assert.notEqual(await tx.getAttribute('transform'),firstPosition,'TX glyph moves with cursor');
    await jumpPs(page,sof+(eof-sof)/4n);
    assert.equal(await tx.getAttribute('transform'),firstPosition,'rewind reconstructs exact glyph');
    const frozen=await page.locator('#network').innerHTML();
    await page.waitForTimeout(120);
    assert.equal(await page.locator('#network').innerHTML(),frozen,'paused diagram stays fixed');
    await page.locator('#request-search').fill('no matching request');
    assert.equal(await tx.getAttribute('data-request-id'),late.request_id,'search cannot hide active TX');
    await tx.click();await assertText(page,'#selected-id',late.request_id);
    await page.locator('#request-search').fill('');
    await jumpPs(page,19800000000n);
    await page.locator('#network').scrollIntoViewIfNeeded();
    if(process.env.VIEWER_SCREENSHOT)await page.screenshot({path:process.env.VIEWER_SCREENSHOT});
    await page.setViewportSize({width:390,height:844});
    assert.equal(await page.evaluate(()=>document.documentElement.scrollWidth<=innerWidth),true);
    assert.equal(await tx.getAttribute('data-request-id'),late.request_id);
    await page.setViewportSize({width:1440,height:1000});
    await jumpPs(page,overloadRaw.simulation.end_ps);
    assert.equal(await tx.count(),1,'cutoff leaves final packet transmitting');
    assert.equal(await page.locator('#network [data-trail-kind="received"]').evaluateAll((rows,id)=>rows.filter(r=>r.dataset.requestId===id).length,late.request_id),0);

    // Actual zero-delay broadcast illuminates both receiver ports at the same EOF.
    const sent=overloadRaw.simulation.requests.find(r=>r.eof_ps!==null);
    await jumpPs(page,sent.eof_ps);
    const received=page.locator('#network [data-trail-kind="received"]');
    assert.equal(await received.count(),2);
    assert.deepEqual(await received.evaluateAll(rows=>rows.map(r=>[r.dataset.requestId,r.dataset.timePs,r.getAttribute('opacity')])),[[sent.request_id,sent.eof_ps,'1'],[sent.request_id,sent.eof_ps,'1']]);
    await jumpPs(page,BigInt(sent.eof_ps)-1n);
    assert.equal(await received.count(),0);

    // Delayed observation moves toward the receiver, then becomes RX processing.
    const delayed=JSON.parse(fs.readFileSync(result));
    const frame=delayed.simulation.requests[0],finish=BigInt(frame.eof_ps);
    delayed.simulation.requests=[frame];delayed.simulation.records=[];
    delayed.simulation.receivers=delayed.simulation.receivers.filter(r=>r.request_id===frame.request_id);
    delayed.simulation.receivers.forEach((r,i)=>Object.assign(r,{status:i?'filtered':'received',observed_ps:String(finish+BigInt(i?150000000:100000000)),received_ps:i?null:String(finish+200000000n)}));
    await page.locator('#file-input').setInputFiles({name:'delayed.json',mimeType:'application/json',buffer:Buffer.from(JSON.stringify(delayed))});
    await page.waitForFunction(()=>document.getElementById('filename').textContent==='delayed.json');
    await jumpPs(page,finish+25000000n);
    const observing=page.locator('#network [data-packet-kind="rx"][data-phase="observation"]');
    assert.equal(await observing.count(),2);
    const rxPosition=await observing.first().getAttribute('transform');
    await jumpPs(page,finish+75000000n);
    assert.notEqual(await observing.first().getAttribute('transform'),rxPosition,'RX observation glyph moves');
    await jumpPs(page,finish+175000000n);
    assert.equal(await page.locator('#network [data-phase="processing"]').count(),1);
    assert.equal(await page.locator('#network [data-trail-kind="filtered"]').count(),1);
    await assertText(page,'#count-success','1');
    await assertText(page,'#count-received','0');
    await jumpPs(page,finish+200000000n);
    assert.equal(await page.locator('#network [data-packet-kind="rx"]').count(),0);
    await assertText(page,'#count-received','1');

    // Zero duration preserves idle nodes and safely disables playback.
    raw.run_id='zero';raw.simulation.requests=[];raw.simulation.receivers=[];raw.simulation.records=[];
    raw.simulation.end_ps='0';raw.simulation.termination='time_limit';
    await page.locator('#file-input').setInputFiles({name:'zero.json',mimeType:'application/json',buffer:Buffer.from(JSON.stringify(raw))});
    await page.waitForFunction(()=>document.getElementById('filename').textContent==='zero.json');
    await assertText(page,'#count-generated','0');
    assert.equal(await page.locator('#play').isDisabled(),true);
    await assertText(page,'#current-ps','0 ps');
    assert.equal(network.length,0,'viewer must not make external requests');
    assert.deepEqual(errors,[],'browser errors');
    console.log('PASS: standalone/file picker, playback, seek, filters/details, responsive layout, invalid files, injection, partial/cutoff, u64 precision, drop, topology movement/rewind/pause, late overload TX, simultaneous RX, zero horizon; no network or browser errors');
  } finally {await browser.close();}
}
main().catch(e=>{console.error(e);process.exitCode=1;}).finally(()=>fs.rmSync(temp,{recursive:true,force:true}));
