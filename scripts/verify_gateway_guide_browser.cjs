// Verify actual generated Viewer pixels, or the built public guide without rewriting old assets.
const fs=require('fs'),path=require('path'),http=require('http'),assert=require('node:assert/strict');
const {chromium}=require(process.env.GUIDE_PLAYWRIGHT||'playwright');
const root=path.resolve(process.argv[2]||'.'), mode=process.argv[3]||'viewer';
const evidence=path.join(root,'guide-evidence/gateway-verified');
const title='Gateway処理遅延と複数CANバスの中継';
(async()=>{
 const browser=await chromium.launch({headless:true});
 const page=await browser.newPage({viewport:{width:1440,height:1100}});
 const errors=[],failures=[],external=[];
 page.on('pageerror',e=>errors.push(e.message));
 page.on('response',r=>{if(r.status()>=400&&!r.url().endsWith('favicon.ico'))failures.push(r.url());});
 page.on('requestfailed',r=>failures.push(r.url()));
 page.on('request',r=>{if(!r.url().startsWith('file:')&&!r.url().startsWith('http://127.0.0.1:'))external.push(r.url());});
 let server;
 const report={browser:browser.version(),mode};
 try {
  if(mode==='viewer'){
   report.conditions={};
   for(const name of ['delay0','delay100','delay300']){
    await page.goto('file://'+path.join(evidence,'repository',name+'-viewer.html'));
    await page.locator('#jump-time').fill('400');await page.locator('#jump').click();
    await page.locator('#request-rows tr').filter({hasText:'Main.src'}).click();
    assert.equal(await page.locator('#selected-id').innerText(),'source:0');
    assert.equal((await page.locator('#current-time').innerText()).replaceAll(',',''),'400');
    const rows=await page.locator('#request-rows').innerText();
    assert(rows.includes('Main.busA'));
    assert.equal(await page.locator('#request-rows tr').count(),3);
    if(name==='delay300') assert.equal((rows.match(/生成前/g)||[]).length,2);
    else assert.equal((rows.match(/生成前/g)||[]).length,0);
    const bState=await page.locator('#request-rows tr').filter({hasText:'Main.gw.b'}).innerText();
    const cState=await page.locator('#request-rows tr').filter({hasText:'Main.gw.c'}).innerText();
    assert(bState.includes(name==='delay300'?'生成前':'送信中'));
    assert(cState.includes(name==='delay300'?'生成前':name==='delay0'?'送信成功':'送信中'));
    report.conditions[name]={time_us:400,request_rows:rows};
    await page.screenshot({path:path.join(root,'docs/guide/assets/gateway-'+name+'-400us.png'),fullPage:true});
   }
  } else {
   const site=path.join(root,'build/guide'),prefix='/DIR-Simulator/';
   server=http.createServer((req,res)=>{
    const u=new URL(req.url,'http://localhost');
    if(!u.pathname.startsWith(prefix)){res.writeHead(404);return res.end();}
    const p=path.resolve(site,decodeURIComponent(u.pathname.slice(prefix.length))||'index.html');
    if(!p.startsWith(site+path.sep)){res.writeHead(403);return res.end();}
    fs.readFile(p,(e,d)=>{if(e){res.writeHead(404);return res.end();}
     res.setHeader('Content-Type',({'.html':'text/html; charset=utf-8','.js':'application/javascript','.json':'application/json','.css':'text/css','.png':'image/png'})[path.extname(p)]||'application/octet-stream');res.end(d);});
   });
   await new Promise(r=>server.listen(0,'127.0.0.1',r));
   const base='http://127.0.0.1:'+server.address().port+prefix;
   report.pages=[];
   for(const name of fs.readdirSync(site).filter(n=>n.endsWith('.html')&&n!=='404.html')){
    await page.goto(base+encodeURIComponent(name));await page.waitForLoadState('networkidle');
    assert.equal(await page.locator('h1').count(),1);
    assert.equal(await page.locator('img').evaluateAll(xs=>xs.filter(x=>!x.complete||!x.naturalWidth).length),0);
    await page.setViewportSize({width:390,height:844});
    assert(await page.evaluate(()=>document.documentElement.scrollWidth<=innerWidth+1));
    await page.setViewportSize({width:1440,height:1100});report.pages.push(name);
   }
   await page.goto(base+encodeURIComponent(title)+'.html');
   assert.equal(await page.locator('img').count(),3);
   await page.screenshot({path:path.join(evidence,'article-desktop.png'),fullPage:true});
   await page.screenshot({path:path.join(evidence,'article-top-desktop.png')});
   await page.setViewportSize({width:390,height:844});
   report.width=await page.evaluate(()=>({viewport:innerWidth,document:document.documentElement.scrollWidth}));
   await page.screenshot({path:path.join(evidence,'article-390px.png'),fullPage:true});
   await page.screenshot({path:path.join(evidence,'article-top-390px.png')});
   await page.locator('h2').filter({hasText:'実測：'}).scrollIntoViewIfNeeded();
   await page.screenshot({path:path.join(evidence,'article-measurements-390px.png')});
   await page.setViewportSize({width:1440,height:1100});await page.goto(base);
   await page.waitForFunction(()=>typeof min_search_length==='number');
   await page.locator('[data-bs-target="#mkdocs_search_modal"]').click();report.searches={};
   for(const term of ['中継','遅延']){
    await page.locator('#mkdocs-search-query').fill('');await page.locator('#mkdocs-search-query').press('End');
    await page.waitForFunction(()=>document.querySelectorAll('#mkdocs-search-results article').length===0);
    await page.locator('#mkdocs-search-query').fill(term);await page.locator('#mkdocs-search-query').press('End');
    await page.waitForFunction(()=>document.querySelectorAll('#mkdocs-search-results article').length>0);
    report.searches[term]=await page.locator('#mkdocs-search-results h3').allTextContents();
    console.log(term,JSON.stringify(report.searches[term]));
    assert(await page.locator('#mkdocs-search-results a').evaluateAll((xs,t)=>xs.some(x=>decodeURIComponent(x.getAttribute('href')).includes(t)),title));
    await page.screenshot({path:path.join(evidence,'search-'+term+'.png')});
   }
  }
  assert.deepEqual(errors,[]);assert.deepEqual(failures,[]);assert.deepEqual(external,[]);
  Object.assign(report,{status:'passed',errors,failures,external,visual_review:'Requires opening actual PNG pixels separately'});
  fs.writeFileSync(path.join(evidence,mode+'-browser.json'),JSON.stringify(report,null,2)+'\n');
  console.log('PASS: '+mode+' actual Chromium assertions and PNG captures.');
 }finally{await browser.close();if(server)await new Promise(r=>server.close(r));}
})().catch(e=>{console.error(e);process.exitCode=1;});
