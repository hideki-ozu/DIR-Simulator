// NODE_PATH=<playwright node_modules> node tests/media_fd_viewer_browser.cjs <FD results.json> <media results.json>
'use strict';
const {chromium}=require('playwright'),assert=require('node:assert/strict'),fs=require('node:fs'),path=require('node:path'),os=require('node:os');
const {pathToFileURL}=require('node:url'),{execFileSync}=require('node:child_process');
const [fd,media]=process.argv.slice(2);if(!fd||!media)throw new Error('Pass FD and media result paths');
const temp=fs.mkdtempSync(path.join(os.tmpdir(),'dir-fd-media-browser-')),bin=process.env.DIR_SIMULATOR_BIN||path.resolve(__dirname,'../target/debug/dir-simulator');
(async()=>{let browser;try{
 browser=await chromium.launch({headless:true});const page=await browser.newPage({viewport:{width:1440,height:1000}}),errors=[];page.on('pageerror',e=>errors.push(e.message));
 const files=[];for(const [name,input]of [['fd',fd],['media',media]]){const html=path.join(temp,`${name}.html`);execFileSync(bin,['view','--input',input,'--output',html]);files.push(html);}
 await page.goto(pathToFileURL(files[0]).href);await page.locator('#dashboard').waitFor({state:'visible'});assert.equal(await page.locator('#error-banner').isVisible(),false);await page.locator('#request-rows tr').first().click();
 const detail=await page.locator('#detail-fields').innerText();for(const text of ['CAN FD','externally-precomputed-phase-bits','structural-only','Binding SHA-256'])assert(detail.includes(text),text);assert.equal(await page.locator('#gateway-panel').isVisible(),false);
 await page.locator('#time-unit').selectOption('ps');await page.locator('#jump-time').fill('380000000');await page.locator('#jump').click();assert((await page.locator('#selected-status').innerText()).includes('送信成功'));
 await page.goto(pathToFileURL(files[1]).href);await page.locator('#ethernet-dashboard').waitFor({state:'visible'});assert.equal(await page.locator('#error-banner').isVisible(),false);await page.locator('#eth-rows tr').first().click();assert((await page.locator('#eth-details').innerText()).includes('衝突数'));
 await page.locator('#eth-jump-value').fill('100000');await page.locator('#eth-jump').click();assert((await page.locator('#eth-counts').innerText()).includes('jam送信中'));assert((await page.locator('#eth-rows').innerText()).includes('jam送信中'));
 await page.locator('#eth-next').click();const destination=await page.locator('#eth-time').innerText();await page.locator('#eth-prev').click();await page.locator('#eth-next').click();assert.equal(await page.locator('#eth-time').innerText(),destination);
 await page.locator('#eth-play').click();await page.waitForTimeout(80);assert.equal(await page.locator('#eth-network circle[data-transfer]').count(),0);await page.locator('#eth-play').click();
 await page.setViewportSize({width:390,height:850});assert(await page.locator('#eth-network').isVisible());assert.deepEqual(errors,[]);console.log('FD and media browser checks passed');
}finally{if(browser)await browser.close();fs.rmSync(temp,{recursive:true,force:true});}})().catch(e=>{console.error(e);process.exitCode=1;});
