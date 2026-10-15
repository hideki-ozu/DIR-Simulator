const fs=require('fs'),path=require('path'),assert=require('node:assert/strict');
const {chromium}=require(process.env.GUIDE_PLAYWRIGHT||'playwright'),{loadEvidence}=require('./ethernet_psfp_evidence.cjs');
(async()=>{
 const root=path.resolve(process.argv[2]||'.'),{evidence,inputHashes}=loadEvidence(root,process.argv[3]);const browser=await chromium.launch({headless:true,executablePath:process.env.GUIDE_CHROMIUM||undefined});
 const page=await browser.newPage({viewport:{width:1440,height:1100}}),errors=[],report={generated_by:'scripts/verify_ethernet_psfp_viewer.cjs',browser:browser.version(),input_hashes:inputHashes,conditions:{}};page.on('pageerror',e=>errors.push(e.message));
 try{
  for(const name of ['pass','drop']){
   await page.goto('file://'+path.join(evidence,`repository/${name}-viewer.html`));await page.locator('#eth-network').waitFor({state:'visible'});assert.equal(await page.locator('#eth-error').isVisible(),false);
   const jump=async time=>{await page.locator('#eth-jump-value').fill(String(time));await page.locator('#eth-jump').click();assert((await page.locator('#eth-time').innerText()).includes(time+' ps'));};
   const card=page.locator('[data-tsn-stream="s0"]');await jump(1247999);assert((await card.innerText()).includes('color green'));assert.deepEqual((await page.locator('#eth-counts strong').allTextContents()).map(Number),[3,1,1,1,0,1]);
   await jump(1248000);const state=await card.innerText(),verdict=name==='pass'?'pass':'psfp_meter_yellow';
   assert(state.includes(verdict+' · color yellow · consumed 512 bit')&&state.includes('1248000 ps')&&state.includes('committed 5376000000000 · peak 10752000000000'));
   const counts=await page.locator('#eth-counts strong').allTextContents();assert.deepEqual(counts.map(Number),[3,1,0,2,0,name==='pass'?2:1]);
   await page.locator('#eth-rows tr').filter({hasText:'psfp:1'}).click();const detail=JSON.parse(await page.locator('#eth-details pre').innerText());assert.equal(detail.data.status,'serialized');assert.equal(detail.data.drop_reason,null);assert.equal(detail.data.arrival_ps,'1248000');
   const panel=await page.locator('#eth-details').innerText();assert(panel.includes(name==='pass'?'Net.b: 受信完了':'psfp_meter_yellow'));
   await page.screenshot({path:path.join(evidence,name+'-overview.png'),fullPage:true});await page.setViewportSize({width:390,height:844});await card.screenshot({path:path.join(root,`docs/guide/assets/ethernet-psfp-${name}-1248000ps.png`)});await page.setViewportSize({width:1440,height:1100});
   await jump(1920000);const red=await card.innerText();assert(red.includes('psfp_meter_red · color red · consumed 0 bit'));
   if(name==='pass'){await page.setViewportSize({width:390,height:844});await card.screenshot({path:path.join(root,'docs/guide/assets/ethernet-psfp-red-1920000ps.png')});await page.setViewportSize({width:1440,height:1100});}
   await jump(32576000);const later=await card.innerText();assert(later.includes(verdict+' · color yellow · consumed 512 bit')&&later.includes('committed 256000000000000 · peak 0'));
   await jump(120000000);const final=await card.innerText();assert(final.includes('pass · color green · consumed 1024 bit'));assert.deepEqual((await page.locator('#eth-counts strong').allTextContents()).map(Number),[5,0,0,5,0,name==='pass'?4:2]);
   report.conditions[name]={time_ps:1248000,counts,stream_card:state,selected_transfer:detail,panel,red_at_1920000:red,later_yellow_at_32576000:later,final_card:final,boundary:'1247999ps still green record; arrival1248000ps yellow; final serialized5 and received4/2'};
  }
  assert.deepEqual(errors,[]);Object.assign(report,{status:'passed',errors});const raw=JSON.stringify(report,null,2)+'\n';fs.writeFileSync(path.join(root,'docs/verification/results/ethernet-psfp/viewer-browser.json'),raw);fs.writeFileSync(path.join(evidence,'viewer-browser.json'),raw);console.log('PASS: real PSFP Viewer arrival boundary, yellow pass/drop with identical tokens, red/green and final received4/2; actual390px cards');
 }finally{await browser.close();}
})().catch(e=>{console.error(e);process.exitCode=1;});
