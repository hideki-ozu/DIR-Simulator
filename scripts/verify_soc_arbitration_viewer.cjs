const fs=require('fs'),path=require('path'),assert=require('node:assert/strict');const {chromium}=require(process.env.GUIDE_PLAYWRIGHT||'playwright'),{loadEvidence}=require('./soc_arbitration_evidence.cjs');
(async()=>{
 const root=path.resolve(process.argv[2]||'.'),{evidence,inputHashes}=loadEvidence(root,process.argv[3]);const browser=await chromium.launch({headless:true,executablePath:process.env.GUIDE_CHROMIUM||undefined}),page=await browser.newPage({viewport:{width:1440,height:1100}}),errors=[],report={browser:browser.version(),input_hashes:inputHashes,conditions:{}};page.on('pageerror',e=>errors.push(e.message));
 try{
  for(const policy of ['round_robin','fixed_priority']){
   await page.goto('file://'+path.join(evidence,`repository/${policy}-viewer.html`));await page.locator('#transaction-dashboard').waitFor({state:'visible'});assert.equal(await page.locator('#error-banner').isVisible(),false);
   await page.locator('#txn-jump-value').fill('95');await page.locator('#txn-jump').click();assert((await page.locator('#txn-time').innerText()).includes('95 ps'));
   const counts=await page.locator('#txn-counts strong').allTextContents();assert.deepEqual(counts.map(Number),[3,0,0,3,0]);assert.equal(await page.locator('#txn-rows tr').count(),3);await page.locator('#txn-rows tr[data-request="b:0"]').click();
   const detail=JSON.parse(await page.locator('#txn-details pre').first().innerText());assert.equal(detail.data.start_ps,policy==='round_robin'?'30':'60');assert.equal(detail.data.completed_ps,policy==='round_robin'?'60':'90');assert.equal(detail.data.response,'OKAY');
   const rows=await page.locator('#txn-rows').innerText();await page.screenshot({path:path.join(root,`docs/guide/assets/soc-${policy}-95ps.png`),fullPage:true});report.conditions[policy]={time_ps:95,counts,rows,selected_request:detail};
  }
  assert.deepEqual(errors,[]);Object.assign(report,{status:'passed',errors});const raw=JSON.stringify(report,null,2)+'\n';fs.writeFileSync(path.join(root,'docs/verification/results/soc-arbitration/viewer-browser.json'),raw);fs.writeFileSync(path.join(evidence,'viewer-browser.json'),raw);console.log('PASS: actual SoC Viewer counters, b:0 wait/time/response and two PNG captures');
 }finally{await browser.close();}
})().catch(e=>{console.error(e);process.exitCode=1;});
