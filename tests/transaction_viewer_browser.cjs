// NODE_PATH=<playwright node_modules> node tests/transaction_viewer_browser.cjs <results.json> ...
'use strict';
const{chromium}=require('playwright'),assert=require('node:assert/strict'),fs=require('node:fs'),path=require('node:path'),os=require('node:os');
const{pathToFileURL}=require('node:url'),{execFileSync}=require('node:child_process');
const M=require('../crates/dir-simulator/src/tool/viewer/assets/transaction-model.js');
const inputs=process.argv.slice(2);if(!inputs.length)throw new Error('Pass generated transaction results.json files');
const bin=process.env.DIR_SIMULATOR_BIN||path.resolve(__dirname,'../target/debug/dir-simulator'),temp=fs.mkdtempSync(path.join(os.tmpdir(),'dir-transaction-browser-'));
(async()=>{let browser;try{browser=await chromium.launch({headless:true});for(let i=0;i<inputs.length;i++){
 const input=inputs[i],raw=JSON.parse(fs.readFileSync(input)),m=M.parseResults(raw),html=path.join(temp,`${i}.html`);execFileSync(bin,['view','--input',input,'--output',html]);
 const page=await browser.newPage({viewport:{width:1440,height:1000}}),errors=[];page.on('pageerror',e=>errors.push(e.message));await page.goto(pathToFileURL(html).href);await page.locator('#transaction-dashboard').waitFor({state:'visible'});
 assert.equal(await page.locator('#error-banner').isVisible(),false);assert.equal(await page.locator('#txn-rows tr').count(),Math.min(500,m.requests.size));assert((await page.locator('#txn-resources').innerText()).length>0||!m.resources.length);
 for(const r of [...m.requests.values()].slice(0,500)){if(!['rejected','failed','dropped'].includes(r.state))continue;
  const rendered=await page.locator('#txn-timeline [data-request]').evaluateAll((nodes,id)=>{const row=nodes.find(n=>n.dataset.request===id);return [...row.querySelectorAll('rect')].map(n=>({state:n.dataset.state,start:n.dataset.startPs,end:n.dataset.endPs,fill:n.getAttribute('fill'),width:Number(n.getAttribute('width')),x:Number(n.getAttribute('x'))}));},r.id);
  const terminal=rendered.at(-1),time=r.completed??r.updated;
  assert.equal(terminal.state,r.state);assert.equal(terminal.start,time.toString());assert.equal(terminal.end,time.toString());assert.equal(terminal.fill,'#c54a65');assert.equal(terminal.width,4);
  assert(Math.abs(terminal.x-(230+Number(time-m.start)/Number(m.end-m.start)*845))<0.001);
  assert(rendered.filter(s=>s.state==='pending').every(s=>BigInt(s.end)<=time));
  if(r.start===null&&r.generated===time)assert.equal(rendered.length,1);
 }
 if(m.requests.size){await page.locator('#txn-rows tr').first().click();assert((await page.locator('#txn-details').innerText()).includes('schema_name'));}
 await page.locator('#txn-jump-value').fill(m.end.toString());await page.locator('#txn-jump').click();assert((await page.locator('#txn-time').innerText()).includes(`${m.end} ps`));
 const counts=M.stateAt(m,m.end);assert.deepEqual(await page.locator('#txn-counts strong').allTextContents(),['generated','pending','active','completed','dropped'].map(k=>String(counts[k])));
 for(const r of [...m.requests.values()].slice(0,500)){if(r.state!=='rejected'||r.start!==null)continue;
  const cells=await page.locator('#txn-rows tr').evaluateAll((rows,id)=>[...rows.find(row=>row.dataset.request===id).querySelectorAll('td')].map(c=>c.textContent),r.id);
  assert.equal(cells[3],'\u62d2\u5426');assert.equal(cells[6],r.completed.toString());
 }

 if(m.eventTimes.length>2){await page.locator('#txn-prev').click();const before=await page.locator('#txn-time').innerText(),packets=await page.locator('#txn-network [data-transfer]').evaluateAll(nodes=>nodes.map(n=>n.dataset.transfer));await page.locator('#txn-next').click();await page.locator('#txn-prev').click();assert.equal(await page.locator('#txn-time').innerText(),before);assert.deepEqual(await page.locator('#txn-network [data-transfer]').evaluateAll(nodes=>nodes.map(n=>n.dataset.transfer)),packets);}
 await page.locator('#txn-play').click();await page.waitForTimeout(80);assert.equal(await page.locator('#txn-network [data-transfer]').count(),0);await page.locator('#txn-play').click();
 await page.locator('#txn-jump-value').fill((m.end+1n).toString());await page.locator('#txn-jump').click();assert(await page.locator('#txn-error').isVisible());
 await page.setViewportSize({width:390,height:850});assert(await page.locator('#txn-network').isVisible());assert.deepEqual(errors,[]);await page.close();console.log('Transaction browser smoke passed:',input);
}}finally{if(browser)await browser.close();fs.rmSync(temp,{recursive:true,force:true});}})().catch(error=>{console.error(error);process.exitCode=1;});
