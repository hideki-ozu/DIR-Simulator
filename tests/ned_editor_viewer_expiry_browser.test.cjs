'use strict';
const test=require('node:test'),assert=require('node:assert/strict'),fs=require('node:fs'),path=require('node:path');
const {chromium}=require('playwright');
const {execFileSync}=require('node:child_process');
test('real NED run cards expire at the boundary and reject stale clicks without navigation',async()=>{
  const browser=await chromium.launch({headless:true,executablePath:process.env.PLAYWRIGHT_CHROMIUM_EXECUTABLE});
  const page=await browser.newPage(),errors=[];
  page.on('pageerror',error=>errors.push(error.message));
  const assets=path.join(__dirname,'../crates/dir-simulator/src/tool/ned-editor/assets');
  try {
    await page.clock.install({time:new Date('2026-10-12T00:00:00Z')});
    await page.clock.pauseAt(new Date('2026-10-12T00:00:01Z'));
    await page.setContent(fs.readFileSync(path.join(assets,'index.html'),'utf8').replace(/<script[\s\S]*?<\/script>/g,''));
    for(const file of ['input.js','view.js']) {
      const content=process.env.NED_EXPIRY_BASELINE ? execFileSync('git',['show',`c23855ab3f74e71de197f1e457676a6f48a438c0:crates/dir-simulator/src/tool/ned-editor/assets/${file}`],{cwd:path.join(__dirname,'..'),encoding:'utf8'}) : fs.readFileSync(path.join(assets,file),'utf8');
      await page.addScriptTag({content});
    }
    await page.evaluate(()=>{
      window.expiry=Date.now()+3600000;window.prepares=0;
      window.app={buffers:new Map(),breadcrumbs:[],scopeGeneration:'1',typeKey:null,fileId:null,view:{files:[],outputs:[]},runsReadonly:false,
        runs:[{run_id:'run',phase:'completed',artifact_state:'published',viewer_state:'ready'}],
        viewerURLs:new Map([['run',{url:'http://127.0.0.1:65530/original',expires:String(window.expiry)}]])};
      window.client={writer:true};
      window.callbacks={prepareViewer(){window.prepares++;app.expiredViewers?.delete('run');app.viewerURLs.set('run',{url:'http://127.0.0.1:65530/new',expires:String(Date.now()+3600000)});window.redraw();}};
      window.redraw=()=>NEDEditorView.render(app,client,callbacks);redraw();
    });
    const card=page.locator('[data-run-id="run"]'),open=card.getByRole('link',{name:'Viewerを開く'});
    await page.clock.fastForward(3599999);await page.evaluate(()=>redraw());assert.equal(await open.count(),1);
    await page.clock.fastForward(1);await page.evaluate(()=>redraw());assert.equal(await open.count(),0);
    await card.getByRole('button',{name:'Viewerを準備',exact:true}).click();assert.equal(await page.evaluate(()=>prepares),1);
    assert.equal(await open.getAttribute('href'),'http://127.0.0.1:65530/new');
    // No redraw between time passing and clicking an old anchor, as with a
    // background tab whose polling is suspended. Click must prevent navigation.
    await page.clock.fastForward(3600000);
    let prevented=false;
    prevented=await open.evaluate(link=>!link.dispatchEvent(new MouseEvent('click',{bubbles:true,cancelable:true})));
    assert.equal(prevented,true);assert.equal(await open.count(),0);
    await card.getByRole('button',{name:'Viewerを準備',exact:true}).click();assert.equal(await page.evaluate(()=>prepares),2);
    await page.evaluate(()=>{app.runsReadonly=true;redraw();});await page.clock.fastForward(3600000);await page.evaluate(()=>redraw());
    assert.equal(await card.getByRole('button',{name:'Viewerを準備',exact:true}).isDisabled(),true);
    assert.equal(browser.contexts()[0].pages().length,1);assert.deepEqual(errors,[]);
  } finally {await browser.close();}
});
