// Capture the actual released DDR Transaction Viewer from explicitly selected CLI output.
const fs=require('fs'),path=require('path'),assert=require('node:assert/strict');
const {chromium}=require(process.env.GUIDE_PLAYWRIGHT||'playwright');
const {loadEvidence}=require('./ddr_evidence.cjs');
(async()=>{
 const root=path.resolve(process.argv[2]||'.'),{evidence,inputHashes}=loadEvidence(root,process.argv[3]);
 const browser=await chromium.launch({headless:true}),page=await browser.newPage({viewport:{width:1440,height:1100}});
 const errors=[],report={browser:browser.version(),input_hashes:inputHashes,conditions:{}};page.on('pageerror',e=>errors.push(e.message));
 try{
  for(const delay of [50,100,200]){
   await page.goto('file://'+path.join(evidence,`repository/refresh${delay}-viewer.html`));
   await page.locator('#transaction-dashboard').waitFor({state:'visible'});assert.equal(await page.locator('#error-banner').isVisible(),false);
   await page.locator('#txn-jump-value').fill('999');await page.locator('#txn-jump').click();assert((await page.locator('#txn-time').innerText()).includes('999 ps'));
   const counts=await page.locator('#txn-counts strong').allTextContents();assert.deepEqual(counts.map(Number),[3,0,0,3,0]);
   assert.equal(await page.locator('#txn-rows tr').count(),3);await page.locator('#txn-rows tr[data-request="read:2"]').click();
   const text=await page.locator('#txn-details').innerText(),detail=JSON.parse(text.slice(text.indexOf('{')));
   assert.equal(detail.data.started_ps,String(550+delay));assert.equal(detail.data.completed_ps,String(750+delay));
   assert.equal(detail.data.generated_ps,'500');assert.equal(detail.data.row_hit,false);assert.equal(detail.data.output_hex,'01020304');
   for(const d of await page.locator('#txn-resources details').all())await d.evaluate(e=>e.open=true);
   const resources=await page.locator('#txn-resources').innerText();assert(resources.includes('"refresh_started_ps": "550"'));assert(resources.includes(`"refresh_ended_ps": "${550+delay}"`));
   const rows=await page.locator('#txn-rows').innerText();await page.screenshot({path:path.join(root,`docs/guide/assets/ddr-refresh${delay}-999ps.png`),fullPage:true});
   report.conditions[delay]={time_ps:999,counts,rows,selected_request:detail,resources};
  }
  assert.deepEqual(errors,[]);Object.assign(report,{status:'passed',errors});const raw=JSON.stringify(report,null,2)+'\n';
  fs.writeFileSync(path.join(root,'docs/verification/results/ddr-refresh/viewer-browser.json'),raw);fs.writeFileSync(path.join(evidence,'viewer-browser.json'),raw);
  console.log('PASS: actual DDR Viewer counts, third read, bytes, row miss, refresh times and PNG captures');
 }finally{await browser.close();}
})().catch(e=>{console.error(e);process.exitCode=1;});
