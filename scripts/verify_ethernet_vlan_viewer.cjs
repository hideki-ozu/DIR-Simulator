const fs=require('fs'),path=require('path'),assert=require('node:assert/strict');
const {chromium}=require(process.env.GUIDE_PLAYWRIGHT||'playwright'),{loadEvidence}=require('./ethernet_vlan_evidence.cjs');
(async()=>{
 const root=path.resolve(process.argv[2]||'.'),{evidence,inputHashes}=loadEvidence(root,process.argv[3]);const browser=await chromium.launch({headless:true,executablePath:process.env.GUIDE_CHROMIUM||undefined}),page=await browser.newPage({viewport:{width:1440,height:1100}}),errors=[],report={browser:browser.version(),input_hashes:inputHashes,conditions:{}};page.on('pageerror',e=>errors.push(e.message));
 try{
  for(const name of ['all','tagged','untagged']){
   const rejected=name==='untagged';await page.goto('file://'+path.join(evidence,`repository/${name}-viewer.html`));await page.locator('#eth-network').waitFor({state:'visible'});assert.equal(await page.locator('#eth-error').isVisible(),false);
   await page.locator('#eth-jump-value').fill('2000000');await page.locator('#eth-jump').click();assert((await page.locator('#eth-time').innerText()).includes('2000000 ps'));
   const counts=await page.locator('#eth-counts strong').allTextContents();assert.deepEqual(counts.map(Number),[1,0,0,rejected?1:2,0,rejected?0:1]);assert.equal(await page.locator('#eth-rows tr').count(),rejected?1:2);
   const row=page.locator('#eth-rows tr').filter({hasText:'probe:0'}).filter({hasText:'Main.a.tx'});await row.click();
   const detail=JSON.parse(await page.locator('#eth-details pre').innerText());assert.equal(detail.schema_version,3);assert.equal(detail.data.vlan_id,'10');assert.equal(detail.data.wire.tag.vid,'10');assert.equal(detail.data.wire.mac_bytes,'68');assert.equal(detail.data.arrival_ps,'609000');assert.equal(detail.data.status,'serialized');
   const panel=await page.locator('#eth-details').innerText();assert(panel.includes('Main.sw')&&panel.includes('609000'));assert.equal(panel.includes('ingress_frame_type'),rejected);assert.equal(panel.includes('1220000'),!rejected);
   await page.screenshot({path:path.join(evidence,name+'-overview.png'),fullPage:true});await page.locator('#eth-details').screenshot({path:path.join(root,`docs/guide/assets/ethernet-vlan-${name}-2000000ps.png`)});
   report.conditions[name]={time_ps:2000000,counts,selected_transfer:detail,panel,detail_semantics:'full-run record and actual reception at final selected time'};
  }
  assert.deepEqual(errors,[]);Object.assign(report,{status:'passed',errors});const raw=JSON.stringify(report,null,2)+'\n';fs.writeFileSync(path.join(root,'docs/verification/results/ethernet-vlan/viewer-browser.json'),raw);fs.writeFileSync(path.join(evidence,'viewer-browser.json'),raw);console.log('PASS: real VLAN Viewer three final states, wire tag and reason; actual selected panels');
 }finally{await browser.close();}
})().catch(e=>{console.error(e);process.exitCode=1;});
