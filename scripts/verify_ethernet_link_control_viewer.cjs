const fs=require('fs'),path=require('path'),assert=require('node:assert/strict');
const {chromium}=require(process.env.GUIDE_PLAYWRIGHT||'playwright'),{loadEvidence}=require('./ethernet_link_control_evidence.cjs');
(async()=>{
 const root=path.resolve(process.argv[2]||'.'),{evidence,inputHashes}=loadEvidence(root,process.argv[3]);
 const browser=await chromium.launch({headless:true,executablePath:process.env.GUIDE_CHROMIUM||undefined}),page=await browser.newPage({viewport:{width:1440,height:1100}}),errors=[];
 const report={generated_by:'scripts/verify_ethernet_link_control_viewer.cjs',browser:browser.version(),input_hashes:inputHashes,conditions:{}};page.on('pageerror',e=>errors.push(e.message));
 try{for(const name of ['on-time','late']){
  await page.goto('file://'+path.join(evidence,`repository/${name}-viewer.html`));await page.locator('#eth-network').waitFor();assert.equal(await page.locator('#eth-error').isVisible(),false);
  const jump=async t=>{await page.locator('#eth-jump-value').fill(String(t));await page.locator('#eth-jump').click();assert((await page.locator('#eth-time').innerText()).includes(t+' ps'));};
  const state=page.locator('#eth-policy-state'),port=state.locator('[data-policy-port="Main.a.tx"]');
  await jump(499999);assert((await port.innerText()).includes('up'));
  await jump(500000);assert((await port.innerText()).includes('down'));assert((await state.innerText()).includes('Topology generation 1'));
  await jump(550000);await page.locator('#eth-rows tr').filter({hasText:'unicast:0'}).filter({hasText:'Main.a.tx'}).click();const inFlight=JSON.parse(await page.locator('#eth-details pre').innerText());assert.equal(inFlight.data.sof_ps,'0');assert.equal(inFlight.data.eof_ps,'608000');
  await jump(609000);const snapshot=await state.innerText();assert((await port.innerText()).includes(name==='on-time'?'up':'down'));assert(snapshot.includes('Topology generation '+(name==='on-time'?2:1)));
  await page.setViewportSize({width:390,height:844});await state.screenshot({path:path.join(root,`docs/guide/assets/ethernet-link-control-${name}-609000ps.png`)});await page.setViewportSize({width:1440,height:1100});
  await jump(609001);assert((await port.innerText()).includes('up'));assert((await state.innerText()).includes('Topology generation 2'));
  await jump(609000);assert.equal(await state.innerText(),snapshot);
  await jump(1300000);const first=page.locator('#eth-rows tr').filter({hasText:'unicast:0'});assert.equal(await first.count(),name==='on-time'?2:1);
  await first.filter({hasText:'Main.a.tx'}).click();const detail=JSON.parse(await page.locator('#eth-details pre').innerText()),panel=await page.locator('#eth-details').innerText();assert.equal(detail.data.arrival_ps,'609000');assert.equal(panel.includes('link_down'),name==='late');assert.equal(panel.includes('Main.b: 受信完了'),name==='on-time');
  await page.screenshot({path:path.join(evidence,name+'-reception.png'),fullPage:true});
  await jump(4000000);assert.deepEqual((await page.locator('#eth-counts strong').allTextContents()).map(Number),[2,0,0,name==='on-time'?4:3,0,name==='on-time'?2:1]);
  const second=page.locator('#eth-rows tr').filter({hasText:'unicast:1'});assert.equal(await second.count(),2);await second.filter({hasText:'Main.sw.tx_b'}).click();const secondDetail=JSON.parse(await page.locator('#eth-details pre').innerText());assert.equal(secondDetail.data.arrival_ps,'3220000');assert((await page.locator('#eth-details').innerText()).includes('Main.b: 受信完了'));
  report.conditions[name]={at_ps:609000,policy_text:snapshot,in_flight_selected_transfer:inFlight,first_selected_transfer:detail,first_reception_panel:panel,second_selected_transfer:secondDetail,reverse_seek:'609001 -> 609000 policy identical; link at down-1/down verified'};
 }assert.deepEqual(errors,[]);Object.assign(report,{status:'passed',errors});const raw=JSON.stringify(report,null,2)+'\n';fs.writeFileSync(path.join(root,'docs/verification/results/ethernet-link-control/viewer-browser.json'),raw);fs.writeFileSync(path.join(evidence,'viewer-browser.json'),raw);console.log('PASS: actual Viewer down/up one-ps boundary, in-flight copy, reception detail, reverse seek, recovery frame and 390px policy PNGs');
 }finally{await browser.close();}
})().catch(e=>{console.error(e);process.exitCode=1;});
