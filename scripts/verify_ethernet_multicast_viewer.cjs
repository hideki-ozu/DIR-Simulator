const fs=require('fs'),path=require('path'),assert=require('node:assert/strict');
const {chromium}=require(process.env.GUIDE_PLAYWRIGHT||'playwright'),{loadEvidence}=require('./ethernet_multicast_evidence.cjs');
(async()=>{
 const root=path.resolve(process.argv[2]||'.'),{evidence,inputHashes}=loadEvidence(root,process.argv[3]);
 const browser=await chromium.launch({headless:true,executablePath:process.env.GUIDE_CHROMIUM||undefined});
 const page=await browser.newPage({viewport:{width:1440,height:1100}}),errors=[],report={browser:browser.version(),input_hashes:inputHashes,conditions:{}};
 page.on('pageerror',e=>errors.push(e.message));
 try{
  for(const name of ['none','local','branch']){
   await page.goto('file://'+path.join(evidence,`repository/${name}-viewer.html`));await page.locator('#eth-network').waitFor({state:'visible'});
   assert.equal(await page.locator('#eth-error').isVisible(),false);
   await page.locator('#eth-jump-value').fill('4000000');await page.locator('#eth-jump').click();
   assert((await page.locator('#eth-time').innerText()).includes('4000000 ps'));
   const counts=await page.locator('#eth-counts strong').allTextContents();
   assert.deepEqual(counts.map(Number),[1,0,0,{none:1,local:2,branch:7}[name],0,name==='none'?0:1]);
   assert.equal(await page.locator('#eth-rows tr').count(),{none:1,local:2,branch:7}[name]);
   await page.locator('#eth-rows tr').filter({hasText:'VlanDemo.a.tx'}).click();
   const rootTransfer=JSON.parse(await page.locator('#eth-details pre').innerText());
   assert.equal(rootTransfer.data.parent_transfer_id,null);assert.equal(rootTransfer.data.vlan_id,'20');assert.equal(rootTransfer.data.wire.tag.pcp,'4');
   const panel=await page.locator('#eth-details').innerText();
   fs.writeFileSync(path.join(evidence,name+'-selected-panel.txt'),panel);
   assert(panel.includes('VlanDemo.s1'));
   if(name==='none') assert(panel.includes('multicast_no_egress')); else assert(panel.includes('VlanDemo.s1: 転送判断済み')&&panel.includes('VlanDemo.c: 受信完了'));
   if(name==='none') await page.locator('#eth-details').screenshot({path:path.join(root,`docs/guide/assets/ethernet-multicast-${name}-4000000ps.png`)});
   await page.screenshot({path:path.join(evidence,name+'-overview.png'),fullPage:true});
   report.conditions[name]={time_ps:4000000,counts,selected_transfer:rootTransfer,panel};
   if(name==='branch'){
    await page.locator('#eth-rows tr').filter({hasText:'VlanDemo.s1.tx_b'}).click();
    const child=JSON.parse(await page.locator('#eth-details pre').innerText());assert.equal(child.data.parent_transfer_id,rootTransfer.record_id);
    report.conditions[name].selected_child=child;
    await page.locator('#eth-details').screenshot({path:path.join(root,'docs/guide/assets/ethernet-multicast-branch-4000000ps.png')});
    await page.locator('#eth-rows tr').filter({hasText:'VlanDemo.s2.tx_c'}).click();
    const rejected=await page.locator('#eth-details').innerText();assert(rejected.includes('VlanDemo.b')&&rejected.includes('multicast_not_subscribed'));
    await page.locator('#eth-details').screenshot({path:path.join(evidence,'branch-unsubscribed-b.png')});report.conditions[name].unsubscribed_b=rejected;
    await page.locator('#eth-rows tr').filter({hasText:'VlanDemo.s1.tx_c'}).click();
    const accepted=await page.locator('#eth-details').innerText();assert(accepted.includes('VlanDemo.c: 受信完了')&&accepted.includes('1220000'));report.conditions[name].subscribed_c=accepted;
   }
  }
  assert.deepEqual(errors,[]);Object.assign(report,{status:'passed',errors});
  const raw=JSON.stringify(report,null,2)+'\n';fs.writeFileSync(path.join(root,'docs/verification/results/ethernet-multicast/viewer-browser.json'),raw);fs.writeFileSync(path.join(evidence,'viewer-browser.json'),raw);
  console.log('PASS: real multicast Viewer three final states, source/child copies, subscribed c and filtered b; actual PNGs');
 }finally{await browser.close();}
})().catch(e=>{console.error(e);process.exitCode=1;});
