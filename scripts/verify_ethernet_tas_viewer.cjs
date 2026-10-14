const fs=require('fs'),path=require('path'),assert=require('node:assert/strict');
const {chromium}=require(process.env.GUIDE_PLAYWRIGHT||'playwright'),{loadEvidence}=require('./ethernet_tas_evidence.cjs');
(async()=>{
 const root=path.resolve(process.argv[2]||'.'),{evidence,inputHashes}=loadEvidence(root,process.argv[3]);const browser=await chromium.launch({headless:true,executablePath:process.env.GUIDE_CHROMIUM||undefined}),page=await browser.newPage({viewport:{width:1440,height:1100}}),errors=[],report={browser:browser.version(),input_hashes:inputHashes,conditions:{}};page.on('pageerror',e=>errors.push(e.message));
 try{
  for(const time of [127999,128000,128001]){
   const name='time'+time,late=time===128001;await page.goto('file://'+path.join(evidence,`repository/${name}-viewer.html`));await page.locator('#eth-network').waitFor({state:'visible'});assert.equal(await page.locator('#eth-error').isVisible(),false);
   await page.locator('#eth-jump-value').fill('500000');await page.locator('#eth-jump').click();assert((await page.locator('#eth-time').innerText()).includes('500000 ps'));
   const counts=await page.locator('#eth-counts strong').allTextContents();assert.deepEqual(counts.map(Number),[1,late?1:0,late?0:1,0,0,0]);assert.equal(await page.locator('#eth-rows tr').count(),1);
   const row=page.locator('#eth-rows tr').filter({hasText:'tas:0'});assert((await row.innerText()).includes(late?'送信待機':'送信中'));await row.click();
   const detail=JSON.parse(await page.locator('#eth-details pre').innerText());assert.equal(detail.data.sof_ps,late?'2000000':String(time));assert.equal(detail.data.queued_ps,String(time));assert.equal(detail.data.status,'serialized');
   const card=page.locator('[data-tsn-port="Net.a.tx"]'),state=await card.innerText();assert(state.includes('open P7')&&state.includes('次境界 800000 ps'));assert(state.includes('状態 '+(late?'guard_blocked':'busy')));assert(state.includes('次wake '+(late?'2000000':String(time+672000))+' ps'));
   await page.screenshot({path:path.join(evidence,name+'-overview.png'),fullPage:true});await page.setViewportSize({width:390,height:844});await card.screenshot({path:path.join(root,`docs/guide/assets/ethernet-tas-${time}-500000ps.png`)});await page.setViewportSize({width:1440,height:1100});
   report.conditions[name]={time_ps:500000,counts,selected_state:late?'送信待機':'送信中',tsn_card:state,selected_transfer:detail,detail_semantics:'full-run record; future SOF/EOF is not an observation at 500000ps'};
  }
  assert.deepEqual(errors,[]);Object.assign(report,{status:'passed',errors});const raw=JSON.stringify(report,null,2)+'\n';fs.writeFileSync(path.join(root,'docs/verification/results/ethernet-tas/viewer-browser.json'),raw);fs.writeFileSync(path.join(evidence,'viewer-browser.json'),raw);console.log('PASS: real TAS Viewer three 500000ps states, gate open/guard vs busy, separate boundary/wake, actual gate card screenshots');
 }finally{await browser.close();}
})().catch(e=>{console.error(e);process.exitCode=1;});
