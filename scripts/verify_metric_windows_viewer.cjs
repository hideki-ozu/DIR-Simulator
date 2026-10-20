// Inspect real formal-exported Metric V2 HTML, including immutable saved aggregates.
'use strict';
const fs=require('node:fs'),path=require('node:path'),assert=require('node:assert/strict'),crypto=require('node:crypto');
const {pathToFileURL}=require('node:url');
const {chromium}=require(process.env.PLAYWRIGHT_MODULE||'playwright');
const cli=process.argv.slice(2),publishReport=cli.includes('--publish-report'),positional=cli.filter(arg=>arg!=='--publish-report');
assert.deepEqual(cli.filter(arg=>arg.startsWith('--')&&arg!=='--publish-report'),[],'unknown option');
assert(positional.length<=2,'usage: node verify_metric_windows_viewer.cjs [repo-root] [fresh-evidence-dir] [--publish-report]');
const root=path.resolve(positional[0]||'.'),evidence=path.resolve(positional[1]||'.'),output=path.join(evidence,'browser');
fs.mkdirSync(output,{recursive:false});
const report={generated_by:'scripts/verify_metric_windows_viewer.cjs',status:'running',generated_at:new Date().toISOString(),evidence_directory:evidence,conditions:{},errors:[],external:[]};
const hash=p=>crypto.createHash('sha256').update(fs.readFileSync(p)).digest('hex');
const headers=['指標','対象','値','単位','観測時刻 ps'];
function assertCurrentMetricRow(actualHeaders,rows,time,value){
 assert.deepEqual(actualHeaders,headers,`unexpected current-value table columns at ${time} ps`);
 const matches=rows.filter(row=>row[0]==='demo.depth'&&row[1]==='Demo.counter');
 assert.equal(matches.length,1,`expected exactly one demo.depth / Demo.counter row at ${time} ps`);
 const observed=time===2?'初期値':String(time);
 const expected=['demo.depth','Demo.counter',String(value),'items',observed];
 assert.deepEqual(matches[0],expected,`wrong demo.depth / Demo.counter cells at ${time} ps`);
 return matches[0];
}
function checkCounterexamples(){
 const run=(name,rows)=>{assert.throws(()=>assertCurrentMetricRow(headers,rows,7,1),assert.AssertionError,name);return {name,rejected:true};};
 return [
  run('depth=9 while demo.minimum=1',[
   ['demo.depth','Demo.counter','9','items','7'],
   ['demo.minimum','Demo.counter','1','items','7'],
  ]),
  run('matching value on another target',[
   ['demo.depth','Demo.other','1','items','7'],
   ['demo.minimum','Demo.counter','1','items','7'],
  ]),
  run('matching value on another metric',[
   ['demo.minimum','Demo.counter','1','items','7'],
  ]),
  run('matching digit only in observation time',[
   ['demo.depth','Demo.counter','9','items','1'],
   ['demo.minimum','Demo.counter','1','items','7'],
  ]),
  run('duplicate matching metric and target rows',[
   ['demo.depth','Demo.counter','1','items','7'],
   ['demo.depth','Demo.counter','1','items','7'],
  ]),
  run('duplicate matching keys with malformed cells',[
   ['demo.depth','Demo.counter','1','items','7'],
   ['demo.depth','Demo.counter'],
  ]),
 ];
}
(async()=>{
  report.counterexample_checks=checkCounterexamples();
  const measurementPath=path.join(evidence,'measurements.json');
  const measurements=JSON.parse(fs.readFileSync(measurementPath,'utf8'));
  assert.equal(measurements.status,'passed');assert.equal(measurements.source_tag,'v1.1.6');
  assert.equal(measurements.source_commit,'397900451015c9bca6a33d24feac0a88b9b7b8fe');
  const binaryHashes=[...new Set(Object.values(measurements.conditions).map(c=>c.metadata.binary_sha256))];
  assert.equal(binaryHashes.length,1,'all windows must use the same formal v1.1.6 binary');
  report.measurement_evidence={path:'measurements.json',sha256:hash(measurementPath),source_tag:measurements.source_tag,source_commit:measurements.source_commit,binary_sha256:binaryHashes[0]};
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
   const currentHeading=panel.getByRole('heading',{name:'この時刻までの計測値',exact:true});
   const currentContainer=currentHeading.locator('xpath=following-sibling::div[1]');
   const current=async()=>currentContainer.innerText();
   const currentCells=async()=>({
    headers:await currentContainer.locator('thead th').evaluateAll(nodes=>nodes.map(n=>n.textContent.trim())),
    rows:await currentContainer.locator('tbody tr').evaluateAll(nodes=>nodes.map(n=>[...n.children].map(c=>c.textContent.trim()))),
   });
   const verifyAt=async(t,value)=>{const cells=await currentCells();return assertCurrentMetricRow(cells.headers,cells.rows,t,value);};
   await move(3);const at3=await current();const row3=await verifyAt(3,4);
   await move(7);const at7=await current();const row7=await verifyAt(7,1);
   await move(2);const at2=await current();const row2=await verifyAt(2,2);
   assert.deepEqual(await aggregates.locator('tbody tr').evaluateAll(nodes=>nodes.map(n=>[...n.children].map(c=>c.textContent))),rows);
   await move(7);const returned7=await verifyAt(7,1);assert.deepEqual(returned7,row7);assert.equal(await current(),at7);await move(10);
   const desktop=path.join(output,`guide-metric-windows-${w}-desktop.png`);await panel.screenshot({path:desktop});
   await page.setViewportSize({width:390,height:844});assert.equal(await page.evaluate(()=>document.documentElement.scrollWidth>innerWidth+1),false);
   const scroll=aggregates.locator('.table-scroll');const dimensions=await scroll.evaluate(n=>({width:n.clientWidth,scrollWidth:n.scrollWidth}));
   const mobile=path.join(output,`guide-metric-windows-${w}-mobile.png`);await aggregates.screenshot({path:mobile});
   await scroll.evaluate(n=>{n.scrollLeft=n.scrollWidth;});const mobileRight=path.join(output,`guide-metric-windows-${w}-mobile-window-columns.png`);await aggregates.screenshot({path:mobileRight});
   const completed=path.dirname(resultPath),artifactNames=['results.json','manifest.json','events.csv','diagnostics.jsonl','summary.csv'];
   const evidenceFiles=artifactNames.map(file=>({path:path.relative(evidence,path.join(completed,file)),sha256:hash(path.join(completed,file))}));
   report.conditions[name]={result_path:path.relative(evidence,resultPath),result_sha256:hash(resultPath),html_sha256:hash(html),evidence_files:evidenceFiles,saved_rows:rows,cursor3:at3,cursor7:at7,cursor2:at2,current_rows:{'2':row2,'3':row3,'7':row7},rollback_7_2_7:[row7,row2,returned7],saved_summary_unchanged:true,mobile_table:dimensions,images:[desktop,mobile,mobileRight].map(p=>({path:path.relative(evidence,p),sha256:hash(p)}))};
   await page.close();
  }
  assert.deepEqual(report.errors,[]);assert.deepEqual(report.external,[]);report.status='passed';
  const text=JSON.stringify(report,null,2)+'\n';
  let publishedText=null;
  if(publishReport){
   const publishRoot=path.join(root,'docs/verification/results/metric-windows/issue-115');
   assert.equal(hash(path.join(publishRoot,'measurements.json')),hash(measurementPath),'published measurement report does not match this run');
   const published=structuredClone(report);published.evidence_directory='issue-115';published.measurement_evidence.path='issue-115/measurements.json';
   for(const condition of Object.values(published.conditions)){
    for(const file of condition.evidence_files){
     const publishedPath=path.join(publishRoot,file.path);
     assert.equal(hash(publishedPath),file.sha256,`published evidence hash mismatch: ${file.path}`);
     file.path=`issue-115/${file.path}`;
    }
    condition.result_path=`issue-115/${condition.result_path}`;delete condition.images;
   }
   publishedText=JSON.stringify(published,null,2)+'\n';
  }
  fs.writeFileSync(path.join(output,'viewer-browser.json'),text,{flag:'wx'});
  if(publishReport)fs.writeFileSync(path.join(root,'docs/verification/results/metric-windows/issue-115-viewer-browser.json'),publishedText);
  console.log('PASS: three formal Metric V2 Viewers; exact metric/target/value/unit/time cells, counterexample rejection, JSON/table equality, 2/3/7/10ps rollback, saved-summary invariance, isolated PNGs and no external traffic');
 }finally{await browser.close();}
})().catch(e=>{console.error(e);process.exitCode=1;});
