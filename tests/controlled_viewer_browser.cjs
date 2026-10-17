'use strict';
// Exercise the actual standalone HTML emitted by the Rust Viewer exporter.
const {chromium}=require(process.env.PLAYWRIGHT_MODULE||'playwright');
const assert=require('node:assert/strict'),fs=require('node:fs'),path=require('node:path');
const {pathToFileURL}=require('node:url');
const inputs=process.argv.slice(2);
if(!inputs.length||inputs.length%2)throw new Error('Pass results.json / exported-viewer.html pairs');
(async()=>{
 const browser=await chromium.launch({headless:true,executablePath:process.env.PLAYWRIGHT_CHROMIUM_EXECUTABLE});
 try {
  for(let i=0;i<inputs.length;i+=2){
   const result=JSON.parse(fs.readFileSync(inputs[i],'utf8')),page=await browser.newPage({viewport:{width:1440,height:1000}}),errors=[],requests=[];
   page.on('pageerror',e=>errors.push(e.message));page.on('console',m=>{if(m.type()==='error')errors.push(m.text());});page.on('request',r=>{if(!r.url().startsWith('file:')&&!r.url().startsWith('data:'))requests.push(r.url());});
   await page.goto(pathToFileURL(path.resolve(inputs[i+1])).href);await page.locator('#controlled-dashboard').waitFor({state:'visible'});
   assert.equal(await page.locator('#error-banner').isVisible(),false);
   const text=await page.locator('#controlled-dashboard').innerText();assert(text.includes(result.metadata.model_profile));assert(text.includes(`${result.simulation.end_ps} ps`));
   const previous=page.getByRole('button',{name:'前の記録',exact:true}),next=page.getByRole('button',{name:'次の記録',exact:true});
   await previous.click();const before=await page.locator('#controlled-dashboard').innerText();await next.click();await previous.click();assert.equal(await page.locator('#controlled-dashboard').innerText(),before);
   const slider=page.locator('#controlled-dashboard input[type=range]');
   await slider.fill('0');await slider.dispatchEvent('input');const zero=await page.locator('#controlled-dashboard').innerText();await slider.fill('10000');await slider.dispatchEvent('input');await slider.fill('0');await slider.dispatchEvent('input');assert.equal(await page.locator('#controlled-dashboard').innerText(),zero);
   for(const viewport of [{width:1440,height:1000},{width:390,height:844}]){
    await page.setViewportSize(viewport);assert.equal(await page.evaluate(()=>document.documentElement.scrollWidth>innerWidth+1),false);
   }
   assert.deepEqual(errors,[]);assert.deepEqual(requests,[]);await page.close();console.log(`PASS ${result.metadata.model_profile} (${result.control?.state??result.simulation.termination}) exported HTML, both viewports, reversible cursor, no browser errors/external requests`);
  }
 }finally{await browser.close();}
})().catch(error=>{console.error(error);process.exitCode=1;});
