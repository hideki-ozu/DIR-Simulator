'use strict';
// Actual standalone formal Viewer captures. Requires fresh measurement output.
const fs=require('node:fs'),path=require('node:path'),assert=require('node:assert/strict'),crypto=require('node:crypto');
const {pathToFileURL}=require('node:url');const {chromium}=require('playwright');
const ROOT=path.resolve(__dirname,'..'),evidence=path.resolve(process.argv[2]);
const hash=p=>crypto.createHash('sha256').update(fs.readFileSync(p)).digest('hex');
const configs={
 'ethernet-beb-seed':{names:['seed-1','seed-0'],times:[100000,1200000,6080000,100000000],capture:[6080000],panel:'#eth-rows',prefix:'eth'},
 'axi-outstanding':{names:['capacity-1','capacity-2'],times:[10000,30000,50000],capture:[10000],panel:'#txn-rows',prefix:'txn'},
 'ethernet-cbs-gate':{names:['closed','open'],times:[672000,1000000,1500000,2000000,2687999,2688000,3687999,3688000,120000000],capture:[1500000,2688000],panel:'#eth-tsn-panel',prefix:'eth'},
 'gateway-rx-capacity':{names:['rx-1','rx-2'],times:[429999999,430000000,500000000,523999999,524000000,2000000000],capture:[500000000],panel:'#gateway-panel',prefix:null},
};
(async()=>{const browser=await chromium.launch({headless:true});try{for(const [slug,conf]of Object.entries(configs)){
 const report={status:'passed',browser_version:browser.version(),source_commit:'397900451015c9bca6a33d24feac0a88b9b7b8fe',conditions:{},captures:[]};const measured=JSON.parse(fs.readFileSync(path.join(evidence,slug,'measurements.json')));
 assert.equal(measured.status,'passed');report.binary_sha256=measured.binary_sha256;
 for(const name of conf.names){const html=path.join(evidence,slug,'repository',name,'viewer.html');assert.equal(hash(html),measured.conditions[name].viewer_sha256);
  for(const viewport of [{name:'desktop',width:1440,height:1000},{name:'mobile',width:390,height:844}]){const page=await browser.newPage({viewport:{width:viewport.width,height:viewport.height}}),errors=[],requests=[],failed=[];
   page.on('pageerror',e=>errors.push(e.message));page.on('console',m=>{if(m.type()==='error')errors.push(m.text())});page.on('request',r=>{if(!r.url().startsWith('file:'))requests.push(r.url())});page.on('requestfailed',r=>failed.push(r.url()));
   await page.goto(pathToFileURL(html).href);await page.locator(conf.prefix?'#'+conf.prefix+'-rows':'#request-rows').waitFor({state:'attached'});assert.equal(await page.locator('#error-banner').isVisible(),false);
   const jump=async t=>{if(conf.prefix){await page.locator('#'+conf.prefix+'-jump-value').fill(String(t));await page.locator('#'+conf.prefix+'-jump').click();assert((await page.locator('#'+conf.prefix+'-time').innerText()).includes(t+' ps'));}else{await page.locator('#time-unit').selectOption('ps');await page.locator('#jump-time').fill(String(t));await page.locator('#jump').click();assert.equal((await page.locator('#current-ps').innerText()).replaceAll(',',''),t+' ps');}};
   const state=async()=>({rows:await page.locator(conf.prefix?'#'+conf.prefix+'-rows':'#request-rows').innerText(),counts:conf.prefix?await page.locator('#'+conf.prefix+'-counts').innerText():await page.locator('#gateway-rx-rows').innerText(),tsn:slug==='ethernet-cbs-gate'?await page.locator('[data-tsn-port="Net.a.tx"]').innerText():null});
   const observed=[];for(const t of conf.times){await jump(t);const s=await state();observed.push({time_ps:String(t),...s});
    const parsed=s.rows.split('\n').map(line=>line.split('\t'));
    if(slug==='ethernet-beb-seed'){
     const expected=t===100000?['jam送信中','jam送信中']:t===1200000?(name==='seed-1'?['媒体待ち','再送待ち']:['再送待ち','再送待ち']):t===6080000?(name==='seed-1'?['送信中','媒体待ち']:['送信中','送信中']):['EOF 到達','EOF 到達'];assert.deepEqual(parsed.map(r=>r[2]),expected);
    }else if(slug==='axi-outstanding'){
     const expected=t===10000?['処理中',name==='capacity-1'?'破棄':'待機']:t===30000?['完了',name==='capacity-1'?'破棄':'処理中']:['完了',name==='capacity-1'?'破棄':'完了'];assert.deepEqual(parsed.map(r=>r[3]),expected);
    }else if(slug==='ethernet-cbs-gate'){
     const sof=name==='closed'?3688000:2688000;assert.equal(parsed[1][2],t<sof?'送信待機':t<sof+576000?'送信中':'EOF 到達');
     if(t===1500000)assert(s.tsn.includes(name==='closed'?'gate_closed':'credit_negative')&&s.tsn.includes(name==='closed'?'-422000000000000':'-297000000000000'));
     if(t===2000000)assert(s.tsn.includes('credit_negative')&&s.tsn.includes(name==='closed'?'-422000000000000':'-172000000000000'));
    }else{
     const rx=await page.locator('#gateway-rx-rows tr').first().locator('td').allTextContents();const expectedHolding=t<430000000?1:t<524000000?(name==='rx-1'?1:2):t<2000000000?(name==='rx-1'?0:1):0;
     assert.equal(Number(rx[3]),expectedHolding);if(t===500000000){assert(rx[4].includes('source:2'));assert.equal(rx[4].includes('source:3'),name==='rx-2');assert.equal(Number(rx[5]),name==='rx-1'?1:0);const source3=page.locator('#request-rows tr').filter({hasText:'gw:source:3/Main.gw/fanout/Main.gw.c'});assert.equal(await source3.count(),name==='rx-1'?0:1);if(name==='rx-2')assert((await source3.innerText()).includes('送信成功'));}
    }
    if(conf.capture.includes(t)){
     let panel=page.locator(conf.panel);if(conf.panel.endsWith('-rows'))panel=panel.locator('xpath=ancestor::section[1]');
     // On narrow displays scroll the real table to its state/holding columns; report exact position.
     if(viewport.name==='mobile')await panel.locator('.table-scroll').evaluateAll((xs,slug)=>xs.forEach((x,i)=>x.scrollLeft=slug==='gateway-rx-capacity'?(i===0?220:280):0),slug);
     const filename=`guide-${slug}-${name}-t${t}ps-${viewport.name}.png`,png=path.join(ROOT,'docs/guide/assets',filename);assert(!fs.existsSync(png),'refuse old screenshot '+png);await panel.screenshot({path:png,animations:'disabled'});
     report.captures.push({condition:name,current_time_ps:String(t),viewport,viewer_sha256:hash(html),results_sha256:measured.conditions[name].results_sha256,screenshot:'docs/guide/assets/'+filename,sha256:hash(png),width:fs.readFileSync(png).readUInt32BE(16),height:fs.readFileSync(png).readUInt32BE(20),selector:conf.panel,horizontal_scroll:await panel.locator('.table-scroll').evaluateAll(xs=>xs.map(x=>({left:x.scrollLeft,width:x.clientWidth,content:x.scrollWidth}))),state:s});
    }
   }
   const pivot=conf.capture[0];await jump(pivot);const before=await state();await jump(conf.times[0]);await jump(pivot);assert.deepEqual(await state(),before,'rollback must restore same confirmed state');
   assert.deepEqual(errors,[]);assert.deepEqual(requests,[]);assert.deepEqual(failed,[]);
   report.conditions[name+'-'+viewport.name]={observations:observed,rollback:[String(pivot),String(conf.times[0]),String(pivot)],rollback_equal:true,errors,external_requests:requests,failed_requests:failed,document_width:await page.evaluate(()=>({viewport:innerWidth,document:document.documentElement.scrollWidth}))};await page.close();
  }
 }
 fs.writeFileSync(path.join(evidence,slug,'viewer-browser.json'),JSON.stringify(report,null,2)+'\n',{flag:'wx'});console.log(slug+': exact cursor states, rollback, desktop/mobile actual PNG, offline requests PASS');
 }}finally{await browser.close()}})().catch(e=>{console.error(e);process.exitCode=1});
