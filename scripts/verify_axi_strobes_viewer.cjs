const fs=require('fs'),path=require('path'),assert=require('node:assert/strict');
const {chromium}=require(process.env.GUIDE_PLAYWRIGHT||'playwright'),{loadEvidence}=require('./axi_strobes_evidence.cjs');
(async()=>{
 const root=path.resolve(process.argv[2]||'.'),{evidence,inputHashes}=loadEvidence(root,process.argv[3]);const browser=await chromium.launch({headless:true,executablePath:process.env.GUIDE_CHROMIUM||undefined}),page=await browser.newPage({viewport:{width:1440,height:1100}}),errors=[],report={browser:browser.version(),input_hashes:inputHashes,conditions:{}};page.on('pageerror',e=>errors.push(e.message));
 try{
  for(const mask of [0,5,15]){
   const hex={0:'00010203',5:'11013303',15:'11223344'}[mask];await page.goto('file://'+path.join(evidence,`repository/strobe${mask}-viewer.html`));await page.locator('#transaction-dashboard').waitFor({state:'visible'});assert.equal(await page.locator('#error-banner').isVisible(),false);
   await page.locator('#txn-jump-value').fill('90000');await page.locator('#txn-jump').click();assert((await page.locator('#txn-time').innerText()).includes('90000 ps'));
   const counts=await page.locator('#txn-counts strong').allTextContents();assert.deepEqual(counts.map(Number),[2,0,0,2,0]);assert.equal(await page.locator('#txn-rows tr').count(),2);await page.locator('#txn-rows tr[data-request="read:0"]').click();
   const detail=JSON.parse(await page.locator('#txn-details pre').first().innerText());assert.deepEqual(detail.data.read_data,[hex]);assert.equal(detail.data.completed_ps,'70000');assert.equal(detail.data.response,'OKAY');
   for(const d of await page.locator('#txn-resources details').all())await d.evaluate(e=>e.open=true);
   const resources=await page.locator('#txn-resources').innerText();assert(resources.includes(hex+'04050607'+'00'.repeat(8)));
   await page.screenshot({path:path.join(root,`docs/guide/assets/axi-strobe${mask}-90ns.png`),fullPage:true});report.conditions[mask]={time_ps:90000,counts,selected_request:detail,resources};
  }
  assert.deepEqual(errors,[]);Object.assign(report,{status:'passed',errors});const raw=JSON.stringify(report,null,2)+'\n';fs.writeFileSync(path.join(root,'docs/verification/results/axi-strobes/viewer-browser.json'),raw);fs.writeFileSync(path.join(evidence,'viewer-browser.json'),raw);console.log('PASS: actual AXI Viewer read bytes, OKAY responses, counters, final RAM and PNG captures');
 }finally{await browser.close();}
})().catch(e=>{console.error(e);process.exitCode=1;});
