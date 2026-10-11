// Actual released Transaction Viewer: finished writes versus pending DMA notification.
const fs=require('fs'),path=require('path'),assert=require('node:assert/strict');
const {chromium}=require(process.env.GUIDE_PLAYWRIGHT||'playwright');
const {loadEvidence}=require('./dma_evidence.cjs');
(async()=>{
 const root=path.resolve(process.argv[2]||'.');
 const {evidence,inputHashes}=loadEvidence(root,process.argv[3]);
 const browser=await chromium.launch({headless:true});
 const page=await browser.newPage({viewport:{width:1440,height:1100}}),errors=[],report={browser:browser.version(),input_hashes:inputHashes,conditions:{}};
 page.on('pageerror',e=>errors.push(e.message));
 try{
  for(const delay of [50,200,600]){
   await page.goto('file://'+path.join(evidence,`repository/notify${delay}-viewer.html`));
   await page.locator('#transaction-dashboard').waitFor({state:'visible'});
   assert.equal(await page.locator('#error-banner').isVisible(),false);
   await page.locator('#txn-jump-value').fill('900');await page.locator('#txn-jump').click();
   assert((await page.locator('#txn-time').innerText()).includes('900 ps'));
   const counts=await page.locator('#txn-counts strong').allTextContents();
   assert.deepEqual(counts.map(Number),delay===600?[6,0,1,5,0]:[6,0,0,6,0]);
   assert.equal(await page.locator('#txn-rows tr').count(),6);
   await page.locator('#txn-rows tr[data-request="copy:0"]').click();
   const detailText=await page.locator('#txn-details').innerText();
   const detail=JSON.parse(detailText.slice(detailText.indexOf('{')));
   assert.equal(detail.data.data_done_ps,'600');assert.equal(detail.data.committed_bytes,'8');
   assert.equal(detail.data.planned_notify_ps,String(600+delay));
   assert.equal(detail.data.status,delay===600?'notifying':'completed');
   assert.equal(detail.data.completed_ps,delay===600?null:String(600+delay));
   assert.equal(detail.data.notified_ps,delay===600?null:String(600+delay));
   const rows=await page.locator('#txn-rows').innerText();
   for(const d of await page.locator('#txn-resources details').all()) await d.evaluate(e=>e.open=true);
   const resources=await page.locator('#txn-resources').innerText();
   assert(resources.includes('01020304050607080000000000000000'));
   await page.screenshot({path:path.join(root,`docs/guide/assets/dma-notify${delay}-900ps.png`),fullPage:true});
   report.conditions[delay]={time_ps:900,counts,rows,selected_status:detail.data.status,selected_completed_ps:detail.data.completed_ps,resources};
  }
  assert.deepEqual(errors,[]);report.status='passed';report.errors=errors;
  const raw=JSON.stringify(report,null,2)+'\n';
  fs.writeFileSync(path.join(root,'docs/verification/results/dma-notification/viewer-browser.json'),raw);
  fs.writeFileSync(path.join(evidence,'viewer-browser.json'),raw);
  console.log('PASS: real Viewer parent state, six requests, committed bytes, notification fields and PNG captures');
 }finally{await browser.close();}
})().catch(e=>{console.error(e);process.exitCode=1;});
