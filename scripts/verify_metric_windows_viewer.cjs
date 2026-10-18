// Inspect real formal-exported Metric V2 HTML, including immutable saved aggregates.
'use strict';
const fs=require('node:fs'),path=require('node:path'),assert=require('node:assert/strict'),crypto=require('node:crypto');
const {pathToFileURL}=require('node:url');
const {chromium}=require(process.env.PLAYWRIGHT_MODULE||'playwright');
const root=path.resolve(process.argv[2]||'.'),evidence=path.resolve(process.argv[3]||''),output=path.join(evidence,'browser');
fs.mkdirSync(output,{recursive:false});
const report={generated_by:'scripts/verify_metric_windows_viewer.cjs',status:'running',conditions:{},errors:[],external:[]};
const hash=p=>crypto.createHash('sha256').update(fs.readFileSync(p)).digest('hex');
(async()=>{
 const browser=await chromium.launch({headless:true});report.browser=browser.version();
 try{
  for(const w of [4,7,10]){
   const name=`window-${w}`,resultPath=path.join(evidence,'repository',name,'completed/results.json'),html=path.join(evidence,'repository',name+'-viewer.html');
   const raw=JSON.parse(fs.readFileSync(resultPath)),page=await browser.newPage({viewport:{width:1440,height:1100}});
   page.on('pageerror',e=>report.errors.push(e.message));page.on('console',m=>{if(m.type()==='error')report.errors.push(m.text());});page.on('request',r=>{if(!/^(file:|data:)/.test(r.url()))report.external.push(r.url());});
   await page.goto(pathToFileURL(html).href);const panel=page.locator('#controlled-dashboard');await panel.waitFor({state:'visible'});assert.equal(await page.locator('#error-banner').isVisible(),false);
   assert((await panel.innerText()).includes('Completed'));assert((await panel.innerText()).includes('観測終端: 10 ps'));
   const aggregates=panel.locator(':scope > details');await aggregates.locator('summary').click();
   const rows=await aggregates.locator('tbody tr').evaluateAll(nodes=>nodes.map(n=>[...n.children].map(c=>c.textContent)));
   const expected=raw.simulation.summary.map(r=>[r.metric,r.target,r.aggregation,r.value??'—',r.unit,r.sample_count,r.start_ps??'—',r.end_ps??'—'].map(String));assert.deepEqual(rows,expected);
   const slider=panel.locator('input[type=range]'),move=async t=>{await slider.fill(String(t*1000));await slider.dispatchEvent('input');assert((await panel.locator('.transport strong').innerText()).startsWith(`${t} ps /`));};
   const current=async()=>panel.getByRole('heading',{name:'この時刻までの計測値',exact:true}).locator('xpath=following-sibling::div[1]').innerText();
   await move(3);const at3=await current();assert(at3.includes('demo.depth')&&at3.includes('4'));
   await move(7);const at7=await current();assert(at7.includes('demo.depth')&&at7.includes('1'));
   await move(2);const at2=await current();assert(at2.includes('2'));
   assert.deepEqual(await aggregates.locator('tbody tr').evaluateAll(nodes=>nodes.map(n=>[...n.children].map(c=>c.textContent))),rows);
   await move(7);assert.equal(await current(),at7);await move(10);
   const desktop=path.join(root,'docs/guide/assets',`guide-metric-windows-${w}-desktop.png`);await panel.screenshot({path:desktop});
   await page.setViewportSize({width:390,height:844});assert.equal(await page.evaluate(()=>document.documentElement.scrollWidth>innerWidth+1),false);
   const scroll=aggregates.locator('.table-scroll');const dimensions=await scroll.evaluate(n=>({width:n.clientWidth,scrollWidth:n.scrollWidth}));
   const mobile=path.join(root,'docs/guide/assets',`guide-metric-windows-${w}-mobile.png`);await aggregates.screenshot({path:mobile});
   await scroll.evaluate(n=>{n.scrollLeft=n.scrollWidth;});const mobileRight=path.join(output,`guide-metric-windows-${w}-mobile-window-columns.png`);await aggregates.screenshot({path:mobileRight});
   report.conditions[name]={result_sha256:hash(resultPath),html_sha256:hash(html),saved_rows:rows,cursor3:at3,cursor7:at7,cursor2:at2,saved_summary_unchanged:true,mobile_table:dimensions,images:[desktop,mobile,mobileRight].map(p=>({path:p,sha256:hash(p)}))};
   await page.close();
  }
  assert.deepEqual(report.errors,[]);assert.deepEqual(report.external,[]);report.status='passed';
  const text=JSON.stringify(report,null,2)+'\n';fs.writeFileSync(path.join(root,'docs/verification/results/metric-windows/viewer-browser.json'),text);fs.writeFileSync(path.join(output,'viewer-browser.json'),text);
  console.log('PASS: three formal Metric V2 Viewers; JSON/table equality, 2/3/7/10ps rollback, saved-summary invariance, desktop/mobile actual PNGs, horizontal table scroll and no external traffic');
 }finally{await browser.close();}
})().catch(e=>{console.error(e);process.exitCode=1;});
