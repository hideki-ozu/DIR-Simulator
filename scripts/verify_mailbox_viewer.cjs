// Actual released Transaction Viewer state and pixels for the mailbox guide.
const fs=require('fs'),path=require('path'),assert=require('node:assert/strict');
const {chromium}=require(process.env.GUIDE_PLAYWRIGHT||'playwright');
const {loadEvidence}=require('./mailbox_evidence.cjs');
(async()=>{
 const root=path.resolve(process.argv[2]||'.');
 const {evidence,inputHashes}=loadEvidence(root,process.argv[3]);
 const browser=await chromium.launch({headless:true});
 const page=await browser.newPage({viewport:{width:1440,height:1100}}),errors=[],report={browser:browser.version(),input_hashes:inputHashes,conditions:{}};
 page.on('pageerror',e=>errors.push(e.message));
 try {
  for(const capacity of [1,2,3]){
   await page.goto('file://'+path.join(evidence,`repository/capacity${capacity}-viewer.html`));
   await page.locator('#transaction-dashboard').waitFor({state:'visible'});
   assert.equal(await page.locator('#error-banner').isVisible(),false);
   await page.locator('#txn-jump-value').fill('950');await page.locator('#txn-jump').click();
   assert((await page.locator('#txn-time').innerText()).includes('950 ps'));
   const counts=await page.locator('#txn-counts strong').allTextContents();
   assert.deepEqual(counts.map(Number),[6,0,0,6,0]);
   assert.equal(await page.locator('#txn-rows tr').count(),6);
   const rows=await page.locator('#txn-rows').innerText();
   await page.locator('#txn-rows tr').filter({hasText:'send_c:0'}).click();
   const detailText=await page.locator('#txn-details').innerText();
   const detail=JSON.parse(detailText.slice(detailText.indexOf('{')));
   assert.equal(detail.schema_name,'memory-ipc.request');
   assert.equal(detail.data.status,'completed');
   assert.equal(detail.data.reason,capacity<3?'full':'ok');
   const summary=await page.locator('#txn-summary').innerText();
   assert(summary.includes('memory_ipc.notifications'));
   for(const d of await page.locator('#txn-resources details').all()) await d.evaluate(e=>e.open=true);
   const resources=await page.locator('#txn-resources').innerText();
   assert(resources.includes('memory-ipc.mailbox'));
   await page.screenshot({path:path.join(root,`docs/guide/assets/mailbox-capacity${capacity}-950ps.png`),fullPage:true});
   report.conditions[capacity]={time_ps:950,counts,rows,selected_reason:detail.data.reason,resources};
  }
  assert.deepEqual(errors,[]);report.status='passed';report.errors=errors;
  const raw=JSON.stringify(report,null,2)+'\n';
  fs.writeFileSync(path.join(root,'docs/verification/results/mailbox-capacity/viewer-browser.json'),raw);
  fs.writeFileSync(path.join(evidence,'viewer-browser.json'),raw);
  console.log('PASS: real transaction Viewer counters, rows, full/ok details, resources and PNG captures');
 }finally{await browser.close();}
})().catch(e=>{console.error(e);process.exitCode=1;});
