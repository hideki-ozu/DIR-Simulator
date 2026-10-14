const fs=require('fs'),path=require('path'),assert=require('node:assert/strict');
const {chromium}=require(process.env.GUIDE_PLAYWRIGHT||'playwright'),{loadEvidence}=require('./ethernet_priority_evidence.cjs');
(async()=>{
 const root=path.resolve(process.argv[2]||'.'),{evidence,inputHashes}=loadEvidence(root,process.argv[3]);const browser=await chromium.launch({headless:true,executablePath:process.env.GUIDE_CHROMIUM||undefined}),page=await browser.newPage({viewport:{width:1440,height:1100}}),errors=[],report={browser:browser.version(),input_hashes:inputHashes,conditions:{}};page.on('pageerror',e=>errors.push(e.message));
 try{
  for(const priority of [0,3,7]){
   const name='priority'+priority;await page.goto('file://'+path.join(evidence,`repository/${name}-viewer.html`));await page.locator('#eth-network').waitFor({state:'visible'});assert.equal(await page.locator('#eth-error').isVisible(),false);
   await page.locator('#eth-jump-value').fill('500000');await page.locator('#eth-jump').click();assert((await page.locator('#eth-time').innerText()).includes('500000 ps'));
   const counts=await page.locator('#eth-counts strong').allTextContents();assert.deepEqual(counts.map(Number),[3,2,1,0,0,0]);assert.equal(await page.locator('#eth-rows tr').count(),6);
   const busy=page.locator('#eth-rows tr').filter({hasText:'a_busy:0'}).filter({hasText:'Main.a.tx'});assert((await busy.innerText()).includes('送信中'));
   const row=page.locator('#eth-rows tr').filter({hasText:'b_probe:0'}).filter({hasText:'Main.a.tx'});assert((await row.innerText()).includes('送信待機'));await row.click();
   const detail=JSON.parse(await page.locator('#eth-details pre').innerText());assert.equal(detail.data.priority,String(priority));assert.equal(detail.data.queue_id,'Main.a.tx.queue.'+priority);assert.equal(detail.data.sof_ps,priority===0?'1344000':'672000');assert.equal(detail.data.queued_ps,'100000');assert.equal(detail.data.status,'serialized');
   const queues=(await page.locator('#eth-queues').innerText()).split('Main.b.tx')[0];assert(queues.includes('2 / 64 待機'));assert(queues.includes('P'+priority+': '+(priority===3?'2':'1')+'/64 frame'));
   await page.screenshot({path:path.join(evidence,name+'-overview.png'),fullPage:true});await page.locator('#eth-details').screenshot({path:path.join(root,`docs/guide/assets/ethernet-${name}-500000ps.png`)});
   report.conditions[name]={time_ps:500000,counts,source_queues:queues,selected_state:'送信待機',selected_transfer:detail,detail_semantics:'full-run record; future SOF/receptions are not observations at 500000ps'};
  }
  assert.deepEqual(errors,[]);Object.assign(report,{status:'passed',errors});const raw=JSON.stringify(report,null,2)+'\n';fs.writeFileSync(path.join(root,'docs/verification/results/ethernet-priority/viewer-browser.json'),raw);fs.writeFileSync(path.join(evidence,'viewer-browser.json'),raw);console.log('PASS: real Ethernet Viewer three states/classes, nonpreemption and selected detail panel screenshots');
 }finally{await browser.close();}
})().catch(e=>{console.error(e);process.exitCode=1;});
