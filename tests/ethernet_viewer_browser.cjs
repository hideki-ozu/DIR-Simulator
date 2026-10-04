// NODE_PATH=<playwright node_modules> node tests/ethernet_viewer_browser.cjs <results.json>
'use strict';
const {chromium}=require('playwright');
const assert=require('node:assert/strict');
const fs=require('node:fs'),path=require('node:path'),os=require('node:os');
const {pathToFileURL}=require('node:url');
const {execFileSync}=require('node:child_process');
const E=require('../crates/dir-simulator/src/tool/viewer/assets/ethernet-model.js');
const result=process.argv[2];if(!result)throw new Error('Pass a generated Ethernet results.json');
const raw=JSON.parse(fs.readFileSync(result,'utf8')),m=E.parseResults(raw);
const temp=fs.mkdtempSync(path.join(os.tmpdir(),'dir-ethernet-browser-'));
const bin=process.env.DIR_SIMULATOR_BIN||path.resolve(__dirname,'../target/debug/dir-simulator');
(async()=>{let browser;try{
 const html=path.join(temp,'viewer.html');execFileSync(bin,['view','--input',result,'--output',html]);
 browser=await chromium.launch({headless:true});const page=await browser.newPage({viewport:{width:1440,height:1000}}),errors=[];page.on('pageerror',e=>errors.push(e.message));
 await page.goto(pathToFileURL(html).href);await page.locator('#ethernet-dashboard').waitFor({state:'visible'});
 assert.equal(await page.locator('#error-banner').isVisible(),false);assert.equal(await page.locator('#eth-network [data-device]').count(),m.devices.size);assert.equal(await page.locator('#eth-network [data-direction]').count(),m.directions.size);
 if(m.transfers.size){await page.locator('#eth-rows tr').first().click();assert((await page.locator('#eth-details').innerText()).includes('FCS'));}
 if(m.qos){assert((await page.locator('#eth-queues').innerText()).includes('P7:'));if(m.transfers.size)assert((await page.locator('#eth-details').innerText()).includes('Priority'));const targets=new Set((raw.simulation.summary||[]).filter(r=>r.target.startsWith('@flow:')).map(r=>r.target));assert.equal(await page.locator('#eth-flow-rows tr').count(),targets.size);if(targets.size){await page.locator('#eth-flow-rows tr').first().click();assert((await page.locator('#eth-details').innerText()).includes('配送遅延'));}}
 await page.locator('#eth-jump-value').fill(m.end.toString());await page.locator('#eth-jump').click();assert((await page.locator('#eth-time').innerText()).includes(`${m.end} ps`));
 if(m.eventTimes.length>2){await page.locator('#eth-prev').click();const first=await page.locator('#eth-time').innerText();const before=await page.locator('#eth-network circle[data-transfer]').evaluateAll(nodes=>nodes.map(n=>n.dataset.transfer));await page.locator('#eth-next').click();await page.locator('#eth-prev').click();assert.equal(await page.locator('#eth-time').innerText(),first);assert.deepEqual(await page.locator('#eth-network circle[data-transfer]').evaluateAll(nodes=>nodes.map(n=>n.dataset.transfer)),before);}
 await page.locator('#eth-play').click();await page.waitForTimeout(80);assert.equal(await page.locator('#eth-network circle[data-transfer]').count(),0);await page.locator('#eth-play').click();
 await page.setViewportSize({width:390,height:850});assert(await page.locator('#eth-network').isVisible());
 assert.deepEqual(errors,[]);console.log('Ethernet browser smoke passed:',result);
}finally{if(browser)await browser.close();fs.rmSync(temp,{recursive:true,force:true});}})().catch(e=>{console.error(e);process.exitCode=1;});
