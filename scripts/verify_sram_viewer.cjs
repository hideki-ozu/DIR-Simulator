// Actual released Transaction Viewer state and pixels for the SRAM guide.
const fs=require('fs'),path=require('path'),assert=require('node:assert/strict');
const {chromium}=require(process.env.GUIDE_PLAYWRIGHT||'playwright');
(async()=>{
 const root=path.resolve(process.argv[2]||'.'),browser=await chromium.launch({headless:true});
 const page=await browser.newPage({viewport:{width:1440,height:1100}}),errors=[],report={browser:browser.version(),conditions:{}};
 page.on('pageerror',e=>errors.push(e.message));
 try {
  for(const ports of [1,2,3]){
   await page.goto('file://'+path.join(root,`guide-evidence/sram-final/repository/ports${ports}-viewer.html`));
   await page.locator('#transaction-dashboard').waitFor({state:'visible'});
   assert.equal(await page.locator('#error-banner').isVisible(),false);
   await page.locator('#txn-jump-value').fill('150');await page.locator('#txn-jump').click();
   assert((await page.locator('#txn-time').innerText()).includes('150 ps'));
   const counts=await page.locator('#txn-counts strong').allTextContents();
   assert.deepEqual(counts.map(Number),ports===1?[3,1,1,1,0]:ports===2?[3,0,1,2,0]:[3,0,0,3,0]);
   assert.equal(await page.locator('#txn-rows tr').count(),3);
   const rows=await page.locator('#txn-rows').innerText();
   await page.locator('#txn-rows tr').filter({hasText:'c:0'}).click();
   assert((await page.locator('#txn-details').innerText()).includes('memory-ipc.request'));
   await page.screenshot({path:path.join(root,`docs/guide/assets/sram-ports${ports}-150ps.png`),fullPage:true});
   report.conditions[ports]={time_ps:150,counts,rows};
  }
  assert.deepEqual(errors,[]);report.status='passed';report.errors=errors;
  fs.writeFileSync(path.join(root,'docs/verification/results/sram-ports/viewer-browser.json'),JSON.stringify(report,null,2)+'\n');
  console.log('PASS: real transaction Viewer counters, rows, details and PNG captures');
 }finally{await browser.close();}
})().catch(e=>{console.error(e);process.exitCode=1;});
