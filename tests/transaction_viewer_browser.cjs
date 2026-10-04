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
 if(m.requests.size){await page.locator('#txn-rows tr').first().click();assert((await page.locator('#txn-details').innerText()).includes('schema_name'));}
 await page.locator('#txn-jump-value').fill(m.end.toString());await page.locator('#txn-jump').click();assert((await page.locator('#txn-time').innerText()).includes(`${m.end} ps`));
 if(m.eventTimes.length>2){await page.locator('#txn-prev').click();const before=await page.locator('#txn-time').innerText(),packets=await page.locator('#txn-network [data-transfer]').evaluateAll(nodes=>nodes.map(n=>n.dataset.transfer));await page.locator('#txn-next').click();await page.locator('#txn-prev').click();assert.equal(await page.locator('#txn-time').innerText(),before);assert.deepEqual(await page.locator('#txn-network [data-transfer]').evaluateAll(nodes=>nodes.map(n=>n.dataset.transfer)),packets);}
 await page.locator('#txn-play').click();await page.waitForTimeout(80);assert.equal(await page.locator('#txn-network [data-transfer]').count(),0);await page.locator('#txn-play').click();
 await page.locator('#txn-jump-value').fill((m.end+1n).toString());await page.locator('#txn-jump').click();assert(await page.locator('#txn-error').isVisible());
 await page.setViewportSize({width:390,height:850});assert(await page.locator('#txn-network').isVisible());assert.deepEqual(errors,[]);await page.close();console.log('Transaction browser smoke passed:',input);
}}finally{if(browser)await browser.close();fs.rmSync(temp,{recursive:true,force:true});}})().catch(error=>{console.error(error);process.exitCode=1;});
