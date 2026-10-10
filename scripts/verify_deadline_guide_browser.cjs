// Inspect an already built guide. No CLI runs, downloads or asset rewrites.
const fs=require('fs'),path=require('path'),http=require('http'),assert=require('node:assert/strict');
const {chromium}=require(process.env.GUIDE_PLAYWRIGHT||'playwright');
const root=path.resolve(process.argv[2]||'.');
const site=path.join(root,'build/guide');
const evidence=path.resolve(process.argv[3]||path.join(root,'guide-evidence','deadline-site-'+Date.now()));
const title='Ethernet配送期限と遅延判定の比較', prefix='/DIR-Simulator/';
(async()=>{
 fs.mkdirSync(evidence,{recursive:false}); // A fresh output folder is required.
 const server=http.createServer((req,res)=>{
  const u=new URL(req.url,'http://localhost');
  if(!u.pathname.startsWith(prefix)){res.writeHead(404);return res.end();}
  const p=path.resolve(site,decodeURIComponent(u.pathname.slice(prefix.length))||'index.html');
  if(!p.startsWith(site+path.sep)){res.writeHead(403);return res.end();}
  fs.readFile(p,(err,data)=>{if(err){res.writeHead(404);return res.end();}
   res.setHeader('Content-Type',({'.html':'text/html; charset=utf-8','.js':'application/javascript','.json':'application/json','.css':'text/css','.png':'image/png'})[path.extname(p)]||'application/octet-stream');res.end(data);});
 });
 let browser;
 try{
  await new Promise(r=>server.listen(0,'127.0.0.1',r));
  const base='http://127.0.0.1:'+server.address().port+prefix;
  browser=await chromium.launch({headless:true});
  const page=await browser.newPage({viewport:{width:1440,height:1100}});
  const errors=[],failures=[],external=[];
  page.on('pageerror',e=>errors.push(e.message));
  page.on('console',m=>{if(m.type()==='error')errors.push(m.text());});
  page.on('requestfailed',r=>failures.push(r.url()));
  page.on('request',r=>{if(!r.url().startsWith(base))external.push(r.url());});
  await page.goto(base+encodeURIComponent(title)+'.html');
  await page.waitForLoadState('networkidle');
  assert.equal(await page.locator('h1').innerText(),title);
  assert.equal(await page.locator('img').count(),4);
  assert.equal(await page.locator('img').evaluateAll(xs=>xs.filter(x=>!x.complete||!x.naturalWidth).length),0);
  await page.screenshot({path:path.join(evidence,'deadline-guide-desktop.png'),fullPage:true});
  await page.setViewportSize({width:390,height:844});
  const width=await page.evaluate(()=>({viewport:innerWidth,document:document.documentElement.scrollWidth}));
  assert(width.document<=width.viewport+1,'390px document overflow');
  await page.screenshot({path:path.join(evidence,'deadline-guide-390px.png'),fullPage:true});
  await page.setViewportSize({width:1440,height:1100});
  await page.goto(base);await page.waitForFunction(()=>typeof min_search_length==='number');
  await page.locator('[data-bs-target="#mkdocs_search_modal"]').click();
  const searches={};
  for(const term of ['期限','配送']){
   await page.locator('#mkdocs-search-query').fill('');await page.locator('#mkdocs-search-query').press('End');
   await page.waitForFunction(()=>document.querySelectorAll('#mkdocs-search-results article').length===0);
   await page.locator('#mkdocs-search-query').fill(term);await page.locator('#mkdocs-search-query').press('End');
   await page.waitForFunction(t=>Array.from(document.querySelectorAll('#mkdocs-search-results h3')).some(x=>x.textContent.includes(t)),title);
   searches[term]=await page.locator('#mkdocs-search-results h3').allTextContents();
   await page.screenshot({path:path.join(evidence,'deadline-search-'+term+'.png')});
  }
  assert.equal(errors.length,0,JSON.stringify(errors));assert.equal(failures.length,0,JSON.stringify(failures));assert.equal(external.length,0,JSON.stringify(external));
  fs.writeFileSync(path.join(evidence,'report.json'),JSON.stringify({status:'passed',scope:'Built guide article only; existing measured CLI and Viewer are not rerun',browser:browser.version(),width,searches,errors,failures,external,images:4,visual_review:'Open the four generated PNG files separately; browser assertions do not constitute visual review'},null,2)+'\n',{flag:'wx'});
  console.log('PASS: deadline guide images, 390px overflow and Japanese searches. Open captured PNGs for visual review.');
 }finally{if(browser)await browser.close();await new Promise(r=>server.close(r));}
})().catch(e=>{console.error(e);process.exitCode=1;});
