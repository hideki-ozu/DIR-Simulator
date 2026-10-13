// Use actual formal-CLI HTML; detail records describe final commits, not cursor snapshots.
const fs=require('fs'),path=require('path'),assert=require('node:assert/strict');
const {chromium}=require(process.env.GUIDE_PLAYWRIGHT||'playwright'),{loadEvidence}=require('./ahb_wait_error_evidence.cjs');
(async()=>{
 const root=path.resolve(process.argv[2]||'.'),{evidence,inputHashes}=loadEvidence(root,process.argv[3]);
 const browser=await chromium.launch({headless:true,executablePath:process.env.GUIDE_CHROMIUM||undefined}),page=await browser.newPage({viewport:{width:1440,height:1100}}),errors=[];
 const report={browser:browser.version(),input_hashes:inputHashes,conditions:{},detail_semantics:'final committed records, not cursor-time observations'};page.on('pageerror',e=>errors.push(e.message));
 try{
  for(const wait of [0,1,2]){
   const caseName='wait'+wait,first=(2+wait)*10,last=(5+2*wait)*10;
   await page.goto('file://'+path.join(evidence,`repository/${caseName}-viewer.html`));await page.locator('#transaction-dashboard').waitFor({state:'visible'});assert.equal(await page.locator('#error-banner').isVisible(),false);
   await page.locator('#txn-jump-value').fill('95');await page.locator('#txn-jump').click();assert((await page.locator('#txn-time').innerText()).includes('95 ps'));
   const counts=await page.locator('#txn-counts strong').allTextContents();assert.deepEqual(counts.map(Number),[2,0,0,2,0]);assert.equal(await page.locator('#txn-rows tr').count(),2);
   await page.locator('#txn-rows tr[data-request="a:0"]').click();let a=JSON.parse(await page.locator('#txn-details pre').first().innerText());assert.equal(a.data.start_ps,'0');assert.equal(a.data.completed_ps,String(first));assert.equal(a.data.response,'OKAY');
   await page.locator('#txn-rows tr[data-request="b:0"]').click();let b=JSON.parse(await page.locator('#txn-details pre').first().innerText());assert.equal(b.data.start_ps,String(first));assert.equal(b.data.completed_ps,String(last));assert.equal(b.data.response,'ERROR');assert.equal(b.data.status,'completed');
   // At 5ps the counter observes one active and one pending, but selected detail stays final.
   await page.locator('#txn-jump-value').fill('5');await page.locator('#txn-jump').click();const early=await page.locator('#txn-counts strong').allTextContents();assert.deepEqual(early.map(Number),[2,1,1,0,0]);
   const finalAtEarly=JSON.parse(await page.locator('#txn-details pre').first().innerText());assert.deepEqual(finalAtEarly,b);
   await page.locator('#txn-jump-value').fill('95');await page.locator('#txn-jump').click();
   const rows=await page.locator('#txn-rows').innerText();await page.screenshot({path:path.join(root,`docs/guide/assets/ahb-${caseName}-95ps.png`),fullPage:true});
   report.conditions[caseName]={time_ps:95,counts,rows,okay_request:a,error_request:b,early_time_ps:5,early_counts:early,early_detail_matches_final:true};
  }
  assert.deepEqual(errors,[]);Object.assign(report,{status:'passed',errors});const raw=JSON.stringify(report,null,2)+'\n';fs.writeFileSync(path.join(root,'docs/verification/results/ahb-wait-error/viewer-browser.json'),raw);fs.writeFileSync(path.join(evidence,'viewer-browser.json'),raw);console.log('PASS: actual AHB Viewer three conditions, final-detail semantics, counters/OKAY/ERROR and PNGs');
 }finally{await browser.close();}
})().catch(e=>{console.error(e);process.exitCode=1;});
