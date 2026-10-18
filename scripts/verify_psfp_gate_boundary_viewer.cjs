// Verify and capture unchanged HTML exported by the measured formal v1.1.6 binary.
'use strict';
const fs=require('node:fs'),path=require('node:path'),crypto=require('node:crypto'),assert=require('node:assert/strict');
const {pathToFileURL}=require('node:url');
const {chromium}=require(process.env.GUIDE_PLAYWRIGHT||'playwright');
const sha=bytes=>crypto.createHash('sha256').update(bytes).digest('hex');
const json=file=>JSON.parse(fs.readFileSync(file,'utf8'));
(async()=>{
 const root=path.resolve(process.argv[2]||'.'),evidence=path.resolve(process.argv[3]||''),output=path.resolve(process.argv[4]||'');
 assert(process.argv[3]&&process.argv[4],'Usage: node scripts/verify_psfp_gate_boundary_viewer.cjs <repo-root> <verified-CLI-evidence> <new-browser-output>');
 assert(output.startsWith(evidence+path.sep),'Browser output must be a new directory under CLI evidence');fs.mkdirSync(output);
 const measurement=json(path.join(root,'docs/verification/results/psfp-gate-boundary/measurements.json'));
 assert.deepEqual(measurement,json(path.join(evidence,'measurements.json')));assert.equal(measurement.status,'passed');assert.equal(measurement.source_tag,'v1.1.6');assert.equal(measurement.source_commit,'397900451015c9bca6a33d24feac0a88b9b7b8fe');
 for(const [file,hash]of Object.entries(measurement.input_sha256))assert.equal(sha(fs.readFileSync(path.join(root,file))),hash);
 const report={generated_by:'scripts/verify_psfp_gate_boundary_viewer.cjs',source_tag:measurement.source_tag,source_commit:measurement.source_commit,measurement_sha256:sha(fs.readFileSync(path.join(evidence,'measurements.json'))),input_sha256:measurement.input_sha256,conditions:{},screenshots:[],errors:{page:[],console:[],request_failed:[],external_requests:[]}};
 const browser=await chromium.launch({headless:true,executablePath:process.env.GUIDE_CHROMIUM||undefined});report.browser=browser.version();
 try{
  for(const source of ['repository','zip'])for(const base of [76000,76001]){
   const name='gate-'+base,condition=source==='repository'?measurement.conditions[name]:measurement.zip_conditions[name],html=path.join(evidence,source,name+'-viewer.html'),result=path.join(evidence,source,name,'results.json'),raw=json(result),closed=base===76000;
   assert.equal(sha(fs.readFileSync(html)),condition.viewer_html_sha256);assert.deepEqual(raw.simulation,condition.simulation);assert.equal(raw.metadata.binary_sha256,condition.metadata.binary_sha256);
   const page=await browser.newPage({viewport:{width:1440,height:1100}}),prefix=source+'/'+name;
   page.on('pageerror',e=>report.errors.page.push({case:prefix,error:e.message}));page.on('console',msg=>{if(msg.type()==='error')report.errors.console.push({case:prefix,error:msg.text()});});page.on('requestfailed',r=>report.errors.request_failed.push({case:prefix,url:r.url(),error:r.failure()?.errorText}));page.on('request',r=>{if(!r.url().startsWith('file:'))report.errors.external_requests.push({case:prefix,url:r.url()});});
   await page.goto(pathToFileURL(html).href);await page.locator('#eth-network').waitFor({state:'visible'});assert.equal(await page.locator('#error-banner').isVisible(),false);
   assert.deepEqual(await page.locator('#embedded-results').evaluate(n=>JSON.parse(n.textContent)),raw);
   const card=page.locator('[data-tsn-stream="s0"]'),jump=async time=>{await page.locator('#eth-jump-value').fill(String(time));await page.locator('#eth-jump').click();assert((await page.locator('#eth-time').innerText()).includes(time+' ps'));assert.equal(await page.locator('#eth-error').isVisible(),false);};
   const capture=async(filename,locator,article)=>{const file=path.join(output,filename);await locator.screenshot({path:file,...(locator===page?{fullPage:true}:{})});const record={file:filename,sha256:sha(fs.readFileSync(file)),source:prefix,...(article?{article_asset:article}:{})};if(article)fs.copyFileSync(file,path.join(root,'docs/guide/assets',article));report.screenshots.push(record);};
   await jump(575999);const before=await card.innerText();assert(before.includes('未到着 · color 未評価 · consumed 0 bit'));assert.deepEqual((await page.locator('#eth-counts strong').allTextContents()).map(Number),[1,0,1,0,0,0]);
   await capture(source+'-'+name+'-575999ps.png',card);
   await jump(576000);await page.locator('#eth-rows tr').click();const state=await card.innerText(),counts=(await page.locator('#eth-counts strong').allTextContents()).map(Number),details=await page.locator('#eth-details').innerText(),transfer=JSON.parse(await page.locator('#eth-details pre').innerText());
   assert.equal(transfer.data.arrival_ps,'576000');assert.equal(transfer.data.eof_ps,'576000');assert.equal(transfer.data.release_ps,'672000');assert.equal(transfer.data.drop_reason,null);assert.equal(transfer.data.status,'serialized');assert.deepEqual(counts,[1,0,0,1,0,closed?0:1]);
   assert(state.includes(closed?'psfp_gate_closed · color 未評価 · consumed 0 bit':'pass · color green · consumed 1024 bit'));
   assert(state.includes(closed?'最後のmeter評価 0 ps · committed 512000000000000 · peak 1024000000000000':'最後のmeter評価 576000 ps · committed 0 · peak 512000000000000'));
   assert(details.includes(closed?'Net.b: フィルタ拒否':'Net.b: 受信完了'));if(closed)assert(details.includes('reason psfp_gate_closed'));
   await capture(source+'-'+name+'-576000ps-desktop.png',page,source==='repository'?'guide-psfp-gate-'+base+'-576000ps-desktop.png':null);
   await page.setViewportSize({width:390,height:844});await capture(source+'-'+name+'-576000ps-mobile.png',card,source==='repository'?'guide-psfp-gate-'+base+'-576000ps-mobile.png':null);
   assert.equal(await card.evaluate(n=>n.getBoundingClientRect().width<=390),true);
   const snapshot=async()=>({card:await card.innerText(),counts:await page.locator('#eth-counts').innerText(),details:await page.locator('#eth-details').innerText(),rows:await page.locator('#eth-rows').innerText()});const atArrival=await snapshot();
   await jump(672000);await jump(575999);assert((await card.innerText()).includes('未到着'));await jump(576000);assert.deepEqual(await snapshot(),atArrival);
   report.conditions[prefix]={viewer_html_sha256:sha(fs.readFileSync(html)),results_sha256:sha(fs.readFileSync(result)),binary_sha256:raw.metadata.binary_sha256,before_time_ps:575999,before_card:before,time_ps:576000,card:state,counts,transfer:transfer.data,reception:condition.reception,policing:condition.policing,rewind_equal:true,desktop:{width:1440,height:1100},mobile:{width:390,height:844}};
   await page.close();
  }
  for(const errors of Object.values(report.errors))assert.deepEqual(errors,[]);report.status='passed';const text=JSON.stringify(report,null,2)+'\n';fs.writeFileSync(path.join(output,'viewer-browser.json'),text);fs.writeFileSync(path.join(root,'docs/verification/results/psfp-gate-boundary/viewer-browser.json'),text);
  console.log('PASS: four formal Viewer exports; arrival576000, pre-arrival575999, identical transfer, closed/no-meter versus pass/green, desktop/mobile captures, rewind, zero browser/network errors');
 }finally{await browser.close();}
})().catch(e=>{console.error(e);process.exitCode=1;});
