// Capture the actual released shared-memory Viewer from explicit verified CLI outputs.
const fs=require('fs'),path=require('path'),assert=require('node:assert/strict');
const {chromium}=require(process.env.GUIDE_PLAYWRIGHT||'playwright');const {loadEvidence}=require('./shared_evidence.cjs');
(async()=>{
 const root=path.resolve(process.argv[2]||'.'),{evidence,inputHashes}=loadEvidence(root,process.argv[3]);
 const browser=await chromium.launch({headless:true}),page=await browser.newPage({viewport:{width:1440,height:1100}});
 const errors=[],report={browser:browser.version(),input_hashes:inputHashes,conditions:{}};page.on('pageerror',e=>errors.push(e.message));
 try{
  for(const actor of ['reader','writer','guest']){
   const allowed=actor==='reader';await page.goto('file://'+path.join(evidence,`repository/${actor}-viewer.html`));
   await page.locator('#transaction-dashboard').waitFor({state:'visible'});assert.equal(await page.locator('#error-banner').isVisible(),false);
   await page.locator('#txn-jump-value').fill('300');await page.locator('#txn-jump').click();assert((await page.locator('#txn-time').innerText()).includes('300 ps'));
   const counts=await page.locator('#txn-counts strong').allTextContents();assert.deepEqual(counts.map(Number),allowed?[2,0,0,2,0]:[2,0,0,1,1]);
   assert.equal(await page.locator('#txn-rows tr').count(),2);await page.locator('#txn-rows tr[data-request="consume:0"]').click();
   const text=await page.locator('#txn-details').innerText(),detail=JSON.parse(text.slice(text.indexOf('{')));
   assert.equal(detail.data.actor,actor);assert.equal(detail.data.reason,allowed?'ok':'access_denied');assert.equal(detail.data.status,allowed?'completed':'rejected');
   assert.equal(detail.data.started_ps,allowed?'150':null);assert.equal(detail.data.completed_ps,allowed?'250':'150');assert.equal(detail.data.output_hex,allowed?'01020304':null);
   for(const d of await page.locator('#txn-resources details').all())await d.evaluate(e=>e.open=true);
   const resources=await page.locator('#txn-resources').innerText();assert(resources.includes(`"state": "${allowed?'free':'ready'}"`));assert(resources.includes('"owner": null'));assert(allowed||resources.includes('01020304'));
   const rows=await page.locator('#txn-rows').innerText();await page.screenshot({path:path.join(root,`docs/guide/assets/shared-${actor}-300ps.png`),fullPage:true});report.conditions[actor]={time_ps:300,counts,rows,selected_request:detail,resources};
  }
  assert.deepEqual(errors,[]);Object.assign(report,{status:'passed',errors});const raw=JSON.stringify(report,null,2)+'\n';fs.writeFileSync(path.join(root,'docs/verification/results/shared-roles/viewer-browser.json'),raw);fs.writeFileSync(path.join(evidence,'viewer-browser.json'),raw);
  console.log('PASS: actual shared Viewer counts, consumer role/status/bytes, slot state and PNG captures');
 }finally{await browser.close();}
})().catch(e=>{console.error(e);process.exitCode=1;});
