// NoC has no memory snapshots. Read real counters and final transaction/transfer records.
const fs=require('fs'),path=require('path'),assert=require('node:assert/strict');
const {chromium}=require(process.env.GUIDE_PLAYWRIGHT||'playwright'),{loadEvidence}=require('./noc_input_fifo_evidence.cjs');
(async()=>{
 const root=path.resolve(process.argv[2]||'.'),{evidence,inputHashes}=loadEvidence(root,process.argv[3]);
 const browser=await chromium.launch({headless:true,executablePath:process.env.GUIDE_CHROMIUM||undefined}),page=await browser.newPage({viewport:{width:1440,height:1100}}),errors=[];
 const grants={1:[0,40,50,60],2:[0,10,40,50],3:[0,10,20,40]},report={browser:browser.version(),input_hashes:inputHashes,conditions:{},detail_semantics:'final committed records, not cursor-time observations'};page.on('pageerror',e=>errors.push(e.message));
 try{
  for(const capacity of [1,2,3]){
   const caseName='capacity'+capacity;await page.goto('file://'+path.join(evidence,`repository/${caseName}-viewer.html`));await page.locator('#transaction-dashboard').waitFor({state:'visible'});assert.equal(await page.locator('#error-banner').isVisible(),false);
   await page.locator('#txn-jump-value').fill('35');await page.locator('#txn-jump').click();assert((await page.locator('#txn-time').innerText()).includes('35 ps'));
   const counts=await page.locator('#txn-counts strong').allTextContents();assert.deepEqual(counts.map(Number),[5,4-capacity,capacity+1,0,0]);assert.equal(await page.locator('#txn-rows tr').count(),5);
   await page.locator('#txn-rows tr[data-request="a:1"]').click();const selected=JSON.parse(await page.locator('#txn-details pre').first().innerText());assert.equal(selected.data.start_ps,String(grants[capacity][1]));assert.equal(selected.data.completed_ps,'60');assert.equal(selected.data.status,'completed');assert.equal(selected.data.response,'OKAY');
   const visibleState=await page.locator('#txn-rows tr[data-request="a:1"]').innerText();assert(visibleState.includes(capacity===1?'待機':'処理中'));
   const details=await page.locator('#txn-details pre').allTextContents();assert.equal(details.length,3);const records=details.map(JSON.parse),transfers=records.slice(1);
   assert.deepEqual(transfers,[{bytes:'4',end_ps:String(grants[capacity][1]+10),from:'Main.r00',hop:'0',response:null,start_ps:String(grants[capacity][1]),to:'Main.r10'},{bytes:'4',end_ps:'60',from:'Main.r10',hop:'1',response:'OKAY',start_ps:'50',to:'Main.e10'}]);
   const rows=await page.locator('#txn-rows').innerText();await page.screenshot({path:path.join(root,`docs/guide/assets/noc-${caseName}-35ps.png`),fullPage:true});
   report.conditions[caseName]={time_ps:35,counts,rows,selected_request:selected,selected_transfers:transfers,visible_selected_state:visibleState};
  }
  assert.deepEqual(errors,[]);Object.assign(report,{status:'passed',errors});const raw=JSON.stringify(report,null,2)+'\n';fs.writeFileSync(path.join(root,'docs/verification/results/noc-input-fifo/viewer-browser.json'),raw);fs.writeFileSync(path.join(evidence,'viewer-browser.json'),raw);console.log('PASS: actual NoC Viewer three35ps conditions, pending/active, final details/two hops and three PNGs');
 }finally{await browser.close();}
})().catch(e=>{console.error(e);process.exitCode=1;});
