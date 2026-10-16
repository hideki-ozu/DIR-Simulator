const fs=require('fs'),path=require('path'),assert=require('node:assert/strict');
const {chromium}=require(process.env.GUIDE_PLAYWRIGHT||'playwright'),{loadEvidence}=require('./ethernet_learning_evidence.cjs');
(async()=>{
 const root=path.resolve(process.argv[2]||'.'),{evidence,inputHashes}=loadEvidence(root,process.argv[3]);const b=await chromium.launch({headless:true,executablePath:process.env.GUIDE_CHROMIUM||undefined}),page=await b.newPage({viewport:{width:1440,height:1100}}),errors=[];
 const report={generated_by:'scripts/verify_ethernet_learning_viewer.cjs',browser:b.version(),input_hashes:inputHashes,conditions:{}};page.on('pageerror',e=>errors.push(e.message));
 try{for(const [name,age] of [['short',2000000],['long',10000000]]){
  await page.goto('file://'+path.join(evidence,`repository/${name}-viewer.html`));await page.locator('#eth-network').waitFor();assert.equal(await page.locator('#eth-error').isVisible(),false);
  const jump=async t=>{await page.locator('#eth-jump-value').fill(String(t));await page.locator('#eth-jump').click();assert((await page.locator('#eth-time').innerText()).includes(t+' ps'));};
  const state=page.locator('#eth-policy-state');const mac=()=>state.locator('details').filter({has:page.locator('summary').filter({hasText:/^mac:/})});
  const table=async()=>JSON.parse(await mac().locator('pre').textContent());
  await jump(609000+age-1);const before=await table();assert(before.some(r=>r.key.mac==='02:00:00:00:00:01'&&r.expires_at===String(609000+age)));
  await jump(609000+age);assert(!(await table()).some(r=>r.key.mac==='02:00:00:00:00:01'));
  await jump(609000+age-1);assert.deepEqual(await table(),before);
  await jump(5611000);const snapshot=await table();assert.equal(snapshot.length,name==='short'?1:2);assert.equal(snapshot.some(r=>r.key.mac==='02:00:00:00:00:01'),name==='long');assert((await state.innerText()).includes('Topology generation 0'));
  await mac().locator('summary').click();await page.setViewportSize({width:390,height:844});await state.screenshot({path:path.join(root,`docs/guide/assets/ethernet-learning-${name}-5611000ps.png`)});await page.setViewportSize({width:1440,height:1100});
  await jump(7000000);assert.deepEqual((await page.locator('#eth-counts strong').allTextContents()).map(Number),[2,0,0,name==='short'?6:5,0,2]);
  const returned=page.locator('#eth-rows tr').filter({hasText:'return:0'});assert.equal(await returned.count(),name==='short'?3:2);assert.equal(await returned.filter({hasText:'Main.sw.tx_c'}).count(),name==='short'?1:0);
  await returned.filter({hasText:'Main.sw.tx_a'}).click();const detail=JSON.parse(await page.locator('#eth-details pre').innerText());assert.equal(detail.data.sof_ps,'5611000');assert.equal(detail.data.arrival_ps,'6220000');const panel=await page.locator('#eth-details').innerText();assert(panel.includes('Main.a: 受信完了'));assert.equal(panel.includes('destination_mismatch'),name==='short');
  await page.screenshot({path:path.join(evidence,name+'-overview.png'),fullPage:true});
  await jump(20000000);assert.deepEqual(await table(),[]);
  report.conditions[name]={at_ps:5611000,mac_table:snapshot,expiry_ps:609000+age,expiry_boundary:'A present at expiry-1; absent at expiry; backward seek matches',return_selected_transfer:detail,return_panel:panel,final_mac_table:[]};
 }assert.deepEqual(errors,[]);Object.assign(report,{status:'passed',errors});const raw=JSON.stringify(report,null,2)+'\n';fs.writeFileSync(path.join(root,'docs/verification/results/ethernet-learning/viewer-browser.json'),raw);fs.writeFileSync(path.join(evidence,'viewer-browser.json'),raw);console.log('PASS: actual MAC table/expiry boundary and reverse seek, short flood vs long learned copy paths, final expiry, real390px policy PNGs');
 }finally{await b.close();}
})().catch(e=>{console.error(e);process.exitCode=1;});
