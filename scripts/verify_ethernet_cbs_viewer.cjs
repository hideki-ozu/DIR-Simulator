const fs=require('fs'),path=require('path'),assert=require('node:assert/strict');
const {chromium}=require(process.env.GUIDE_PLAYWRIGHT||'playwright'),{loadEvidence}=require('./ethernet_cbs_evidence.cjs');
(async()=>{
 const root=path.resolve(process.argv[2]||'.'),{evidence,inputHashes}=loadEvidence(root,process.argv[3]);
 const browser=await chromium.launch({headless:true,executablePath:process.env.GUIDE_CHROMIUM||undefined}),page=await browser.newPage({viewport:{width:1440,height:1100}}),errors=[];
 const report={generated_by:'scripts/verify_ethernet_cbs_viewer.cjs',browser:browser.version(),input_hashes:inputHashes,conditions:{}};page.on('pageerror',e=>errors.push(e.message));
 try{
  for(const [rate,wake,debt] of [[250,2688000,504],[500,1344000,336],[750,896000,168]]){
   const name='slope'+rate;
   await page.goto('file://'+path.join(evidence,`repository/${name}-viewer.html`));await page.locator('#eth-network').waitFor({state:'visible'});assert.equal(await page.locator('#eth-error').isVisible(),false);
   const jump=async time=>{await page.locator('#eth-jump-value').fill(String(time));await page.locator('#eth-jump').click();assert((await page.locator('#eth-time').innerText()).includes(time+' ps'));};
   const card=page.locator('[data-tsn-port="Net.a.tx"]');
   await jump(576000);const atEof=await card.innerText();assert(atEof.includes('busy')&&atEof.includes('672000'));
   await jump(672000);const state=await card.innerText();fs.writeFileSync(path.join(evidence,name+'-card.txt'),state);
   assert(state.includes('credit_negative')&&state.includes(String(wake))&&state.includes(`-${BigInt(debt)*1000000000000n} / 1000000000000 bit`));
   const counts=await page.locator('#eth-counts strong').allTextContents();assert.deepEqual(counts.map(Number),[2,1,0,1,0,1]);
   const row=page.locator('#eth-rows tr').filter({hasText:'cbs:1'});await row.click();const detail=JSON.parse(await page.locator('#eth-details pre').innerText());assert.equal(detail.data.sof_ps,String(wake));assert.equal(detail.data.status,'serialized');
   await page.screenshot({path:path.join(evidence,name+'-overview.png'),fullPage:true});
   await page.setViewportSize({width:390,height:844});await card.screenshot({path:path.join(root,`docs/guide/assets/ethernet-cbs-${rate}-672000ps.png`)});await page.setViewportSize({width:1440,height:1100});
   await jump(wake-1);assert.deepEqual((await page.locator('#eth-counts strong').allTextContents()).map(Number),[2,1,0,1,0,1]);assert((await card.innerText()).includes('credit_negative'));
   await jump(wake);assert.deepEqual((await page.locator('#eth-counts strong').allTextContents()).map(Number),[2,0,1,1,0,1]);assert((await card.innerText()).includes('busy'));
   await jump(120000000);assert.deepEqual((await page.locator('#eth-counts strong').allTextContents()).map(Number),[2,0,0,2,0,2]);
   report.conditions[name]={time_ps:672000,counts,tsn_card:state,at_eof_ps:atEof,selected_transfer:detail,next_wake_ps:wake,boundary:'wake-1 queued; wake transmitting; final serialized2/received2',detail_semantics:'full-run record; future SOF at selected 672000ps is not current observation'};
  }
  assert.deepEqual(errors,[]);Object.assign(report,{status:'passed',errors});const raw=JSON.stringify(report,null,2)+'\n';fs.writeFileSync(path.join(root,'docs/verification/results/ethernet-cbs/viewer-browser.json'),raw);fs.writeFileSync(path.join(evidence,'viewer-browser.json'),raw);
  console.log('PASS: real CBS Viewer 3 release credits, EOF/busy boundary, wake-1/wake, final states and actual 390px cards');
 }finally{await browser.close();}
})().catch(e=>{console.error(e);process.exitCode=1;});
