const {chromium}=require(process.env.GUIDE_PLAYWRIGHT || 'playwright');
const fs=require('fs'),path=require('path'),http=require('http'),assert=require('node:assert/strict');
const root=path.resolve(process.argv[2]||'.');
const prefix=process.env.GUIDE_URL_PREFIX || '/DIR-Simulator/';
const site=path.join(root,'build/guide'),evidence=path.join(root,'guide-evidence');
(async()=>{
 const server=http.createServer((req,res)=>{
  const url=new URL(req.url,'http://localhost');
  if(!url.pathname.startsWith(prefix)){res.writeHead(404);return res.end();}
  let rel=decodeURIComponent(url.pathname.slice(prefix.length))||'index.html';
  let file=path.resolve(site,rel);
  if(!file.startsWith(site+path.sep)){res.writeHead(403);return res.end();}
  fs.readFile(file,(err,data)=>{if(err){res.writeHead(404);return res.end();}
   res.setHeader('Content-Type',({'.html':'text/html; charset=utf-8','.js':'application/javascript','.json':'application/json','.css':'text/css','.png':'image/png'})[path.extname(file)]||'application/octet-stream');res.end(data);});
 });
 await new Promise(r=>server.listen(0,'127.0.0.1',r));
 const base='http://127.0.0.1:'+server.address().port+prefix;
 const browser=await chromium.launch({headless:true});
 const page=await browser.newPage({viewport:{width:1440,height:1000}});
 const errors=[],failed=[],external=[];page.on('console',msg=>console.log('browser:',msg.type(),msg.text()));
 page.on('pageerror',e=>errors.push(e.message));page.on('response',r=>{if(r.status()>=400 && !r.url().endsWith('favicon.ico'))failed.push(r.url());});
 page.on('request',r=>{if(!r.url().startsWith('http://127.0.0.1:')&&!r.url().startsWith('file:'))external.push(r.url());});
 const pages=[];
 for(const name of fs.readdirSync(site).filter(n=>n.endsWith('.html')&&n!=='404.html')){
  await page.goto(base+encodeURIComponent(name));await page.waitForLoadState('networkidle');
  assert.equal(await page.locator('h1').count(),1);
  assert.equal(await page.locator('img').evaluateAll(imgs=>imgs.filter(x=>!x.complete||x.naturalWidth===0).length),0);
  await page.setViewportSize({width:390,height:844});
  assert.equal(await page.evaluate(()=>document.documentElement.scrollWidth>window.innerWidth+1),false);
  await page.setViewportSize({width:1440,height:1000});
  pages.push(name);
 }
 await page.goto(base);
 await page.waitForFunction(()=>typeof min_search_length==='number');
 await page.locator('[data-bs-target="#mkdocs_search_modal"]').click();
 const searches={};
 for(const term of ['CAN','調停','Viewer','ビットレート','フィルタ']){
  console.log('search',term);
  await page.locator('#mkdocs-search-query').fill('');await page.locator('#mkdocs-search-query').press('End');
  await page.waitForFunction(()=>document.querySelectorAll('#mkdocs-search-results article').length===0);
  await page.locator('#mkdocs-search-query').fill(term);
  await page.locator('#mkdocs-search-query').press('End');
  await page.waitForFunction(()=>document.querySelectorAll('#mkdocs-search-results article').length>0);
  const titles=await page.locator('#mkdocs-search-results h3').allTextContents();
  assert(titles.length>0);searches[term]=titles;
  if(term==='調停') await page.screenshot({path:path.join(evidence,'guide-search.png')});
 }
 await page.keyboard.press('Escape');
 await page.goto(base+encodeURIComponent('初めてのCAN実行.html'));
 await page.screenshot({path:path.join(evidence,'guide-desktop.png')});
 await page.setViewportSize({width:390,height:844});await page.screenshot({path:path.join(evidence,'guide-mobile.png')});
 const overflow=await page.evaluate(()=>document.documentElement.scrollWidth>window.innerWidth+1);assert(!overflow);
 for(const name of ['minimal','fast','id-swap']){
  await page.goto('file://'+path.join(evidence,name+'-viewer.html'));
  await page.setViewportSize({width:1440,height:1100});
  await page.locator('#jump-time').fill('1000');await page.locator('#jump').click();
  await page.locator('#request-rows tr').first().click();
  await page.screenshot({path:path.join(root,'docs/guide/assets/guide-'+name+'-viewer.png'),fullPage:true});
 }
 await page.goto('file://'+path.join(evidence,'minimal-viewer.html'));
 await page.setViewportSize({width:1440,height:1100});
 await page.locator('#jump-time').fill('238');await page.locator('#jump').click();
 await page.locator('#request-rows tr').first().click();assert.equal(await page.locator('#selected-id').innerText(),'a:0');
 await page.screenshot({path:path.join(root,'docs/guide/assets/guide-viewer-at-238us.png'),fullPage:true});
 await page.locator('#time-unit').selectOption('ns');
 assert.equal((await page.locator('#current-time').innerText()).replaceAll(',',''),'238000');
 assert.equal((await page.locator('#current-ps').innerText()).replaceAll(',',''),'238000000 ps');
 await page.locator('#next-time').click();assert.equal((await page.locator('#current-time').innerText()).replaceAll(',',''),'244000');
 await page.locator('#prev-time').click();assert.equal((await page.locator('#current-time').innerText()).replaceAll(',',''),'238000');
 assert.deepEqual(errors,[]);assert.deepEqual(failed,[]);assert.deepEqual(external,[]);
 fs.writeFileSync(path.join(evidence,'browser-verification.json'),JSON.stringify({status:'passed',pages,searches,viewport_mobile:[390,844],horizontal_overflow:overflow,page_errors:errors,http_failures:failed,external_requests:external,viewer_unit_conversion:'238us = 238000ns = 238000000ps',viewer_step:'238us -> 244us -> 238us'},null,2));
 console.log('PASS: '+pages.length+' pages, '+Object.keys(searches).length+' search terms including フィルタ, images, all-page mobile layout, no external requests, Viewer units and forward/back steps.');
 await browser.close();await new Promise(r=>server.close(r));
})().catch(e=>{console.error(e);process.exit(1)});
