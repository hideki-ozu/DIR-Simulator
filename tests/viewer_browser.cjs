// Optional browser smoke test: NODE_PATH=<playwright node_modules> node tests/viewer_browser.cjs
'use strict';
const {chromium} = require('playwright');
const assert = require('node:assert/strict');
const fs = require('node:fs');
const os = require('node:os');
const path = require('node:path');
const {pathToFileURL} = require('node:url');
const {execFileSync} = require('node:child_process');
const root = path.resolve(__dirname,'..');
const bin = process.env.DIR_SIMULATOR_BIN || path.join(root,'target/debug/dir-simulator');
const temp = fs.mkdtempSync(path.join(os.tmpdir(),'dir-viewer-browser-'));
const run = args => JSON.parse(execFileSync(bin,args,{encoding:'utf8'}));
const assertText = async(page,id,value) => assert.equal((await page.locator(id).innerText()).trim(),value);
async function assertGatewayLayout(page, expectedPorts, orientation) {
  const layout=await page.locator('#network').evaluate(svg=>{
    const gateway=Array.from(svg.querySelectorAll('g[data-gateway]')).find(group=>group.dataset.gateway==='Main.gw');
    const frame=gateway?.querySelector('rect[data-gateway-frame]');
    const box=element=>({x:Number(element.getAttribute('x')),y:Number(element.getAttribute('y')),
      width:Number(element.getAttribute('width')),height:Number(element.getAttribute('height'))});
    const frameBox=box(frame);
    const nodes=Array.from(gateway.children).filter(element=>element.matches('g[data-node]')).map(group=>{
      const rect=Array.from(group.children).find(element=>element.matches('rect'));
      return {id:group.dataset.node,...box(rect)};
    });
    const buses=Array.from(svg.querySelectorAll('g.network-selectable[data-bus]')).map(group=>{
      const rect=Array.from(group.children).find(element=>element.matches('rect'));
      const bounds=rect.getBoundingClientRect();
      return {id:group.dataset.bus,...box(rect),visible:bounds.width>0&&bounds.height>0,
        separate:group.closest('g[data-gateway]')===null};
    });
    return {frame:frameBox,nodes,buses};
  });
  assert.deepEqual(layout.nodes.map(node=>node.id).sort(),[...expectedPorts].sort());
  for(const node of layout.nodes) {
    assert(node.x>=layout.frame.x&&node.y>=layout.frame.y,'Gateway Controller starts inside its frame');
    assert(node.x+node.width<=layout.frame.x+layout.frame.width,'Gateway Controller fits horizontally inside its frame');
    assert(node.y+node.height<=layout.frame.y+layout.frame.height,'Gateway Controller fits vertically inside its frame');
  }
  for(let i=0;i<layout.nodes.length;i++)for(let j=i+1;j<layout.nodes.length;j++) {
    const a=layout.nodes[i],b=layout.nodes[j];
    assert(!(a.x<b.x+b.width&&b.x<a.x+a.width&&a.y<b.y+b.height&&b.y<a.y+a.height),'Gateway Controllers do not overlap');
  }
  if(orientation==='horizontal') {
    assert(layout.nodes.every(node=>node.y===layout.nodes[0].y),'desktop Gateway Controllers share a row');
    assert(layout.nodes.every((node,index)=>index===0||node.x>layout.nodes[index-1].x),'desktop Gateway Controllers are ordered horizontally');
  } else {
    assert(layout.nodes.every(node=>node.x===layout.nodes[0].x),'mobile Gateway Controllers share a column');
    assert(layout.nodes.every((node,index)=>index===0||node.y>layout.nodes[index-1].y),'mobile Gateway Controllers are ordered vertically');
  }
  assert.deepEqual(layout.buses.map(bus=>bus.id).sort(),['Main.busA','Main.busB']);
  assert(layout.buses.every(bus=>bus.visible&&bus.separate),'bus labels stay visible outside Gateway groups');
  assert(layout.buses.every(bus=>bus.y>=layout.frame.y+layout.frame.height),'bus labels stay below the Gateway frame');
  for(let i=0;i<layout.buses.length;i++)for(let j=i+1;j<layout.buses.length;j++) {
    const a=layout.buses[i],b=layout.buses[j];
    assert(!(a.x<b.x+b.width&&b.x<a.x+a.width&&a.y<b.y+b.height&&b.y<a.y+a.height),'bus labels do not overlap');
  }
}
async function jumpPs(page,time){
  await page.locator('#time-unit').selectOption('ps');
  await page.locator('#jump-time').fill(String(time));
  await page.locator('#jump').click();
}
async function assertStepPlayback(browser, viewer, result) {
  const page=await browser.newPage({viewport:{width:1440,height:1000}});
  const errors=[];page.on('pageerror',error=>errors.push(error.message));
  try {
    await page.clock.install({time:new Date('2026-10-03T00:00:00Z')});
    await page.goto(pathToFileURL(viewer).href);
    await page.clock.pauseAt(new Date('2026-10-03T01:00:00Z'));
    const raw=JSON.parse(fs.readFileSync(result));
    const request=raw.simulation.requests[0];
    raw.simulation.requests=[request];raw.simulation.records=[];
    raw.simulation.receivers=raw.simulation.receivers.filter(r=>r.request_id===request.request_id);
    const load=async()=>{
      await page.locator('#file-input').setInputFiles({name:'step.json',mimeType:'application/json',buffer:Buffer.from(JSON.stringify(raw))});
      await page.waitForFunction(()=>document.getElementById('filename').textContent==='step.json');
      await page.locator('#dashboard').waitFor({state:'visible'});
    };
    await load();
    const step=page.locator('#network [data-step-transfer]');
    const stepTx=page.locator('#network [data-step-transfer][data-packet-kind="tx"]');
    const stepRx=page.locator('#network [data-step-transfer][data-packet-kind="rx"]');
    const click=async id=>page.locator(id).dispatchEvent('click');
    await click('#next-time');
    await assertText(page,'#current-ps',`${request.eof_ps} ps`);
    await assertText(page,'#count-success','1');await assertText(page,'#count-received','2');
    assert.equal(await step.count(),3,'EOF step shows TX and both instantaneous RX transfers');
    assert.equal(await stepTx.isVisible(),true);
    assert.equal(await stepRx.filter({visible:true}).count(),0,'receivers stay hidden until sender reaches bus');
    assert.equal(await page.locator('#network [data-highlight-kind="rx"]').count(),0,'receive lines stay idle during the send phase');
    assert.deepEqual(await page.locator('#network line[data-connection-node]:not([data-highlight-kind])').evaluateAll(rows=>rows.map(r=>r.getAttribute('stroke'))),['#94a3b8','#94a3b8']);
    const start=await step.first().getAttribute('transform');
    await page.clock.runFor(240);
    assert.notEqual(await step.first().getAttribute('transform'),start,'message moves with wall time while simulation cursor is fixed');
    await assertText(page,'#current-ps',`${request.eof_ps} ps`);
    await step.first().dispatchEvent('click');await assertText(page,'#selected-id',request.request_id);
    assert(Number(await step.first().getAttribute('data-progress'))>.3,'selection preserves animation progress');
    await page.setViewportSize({width:390,height:844});
    await page.clock.runFor(160);
    assert(Number(await step.first().getAttribute('data-progress'))>.5,'resize preserves animation progress');
    await page.clock.runFor(400);
    assert.equal(await stepTx.isVisible(),false,'completed sender is removed when receiving starts');
    assert.equal(await stepRx.filter({visible:true}).count(),2,'both broadcast receivers start after the send phase');
    assert.equal(await page.locator('#network [data-highlight-kind="tx"]').count(),0);
    assert.equal(await page.locator('#network [data-highlight-kind="rx"]').count(),2);
    assert(Number(await stepRx.first().getAttribute('data-progress'))>0&&Number(await stepRx.first().getAttribute('data-progress'))<.3);
    await stepRx.first().dispatchEvent('click');
    await page.setViewportSize({width:1440,height:1000});
    await page.clock.runFor(100);
    assert.equal(await stepTx.isVisible(),false,'selection and resize cannot bring the completed sender back');
    assert.equal(await stepRx.filter({visible:true}).count(),2);
    await page.setViewportSize({width:390,height:844});
    await page.clock.runFor(600);
    const endpoints=await step.evaluateAll(rows=>rows.map(g=>{
      const d=g.dataset,tx=d.packetKind==='tx';
      return {progress:Number(d.progress),atEnd:g.getAttribute('transform')===`translate(${d.toX} ${d.toY})`,
        correctDirection:tx?Number(d.toX)>Number(d.fromX):Number(d.toX)<Number(d.fromX)};
    }));
    assert(endpoints.every(g=>g.progress===1&&g.atEnd&&g.correctDirection),'mobile TX goes node->bus; RX goes bus->node');
    await click('#prev-time');await assertText(page,'#current-ps','0 ps');
    assert.equal(await step.count(),0,'the first event has no earlier interval to replay');
    assert.equal(await page.locator('#network').getAttribute('data-display-mode'),'paused');
    await page.clock.runFor(100);
    await click('#next-time');
    assert(Number(await step.first().getAttribute('data-progress'))<.05,'rapid next click replaces the previous animation');
    await click('#play');
    assert.equal(await page.locator('#network').getAttribute('data-display-mode'),'playback');
    assert.equal(await page.locator('#network [data-packet-kind]').count(),0,'play immediately removes message glyphs');
    assert.equal(await page.locator('#network [data-highlight-kind="tx"]').count(),1);
    assert.equal(await page.locator('#network [data-highlight-kind="rx"]').count(),2);
    const directions=await page.locator('#network [data-highlight-kind]').evaluateAll(rows=>rows.map(r=>({kind:r.dataset.highlightKind,
      stroke:r.getAttribute('stroke'),start:r.getAttribute('marker-start'),end:r.getAttribute('marker-end')})));
    assert.deepEqual(directions,[
      {kind:'tx',stroke:'#2563eb',start:null,end:'url(#communication-tx-arrow)'},
      {kind:'rx',stroke:'#c2410c',start:'url(#communication-rx-arrow)',end:null},
      {kind:'rx',stroke:'#c2410c',start:'url(#communication-rx-arrow)',end:null}
    ],'TX and RX have distinct colors and oppositely directed arrowheads');
    await page.clock.runFor(64);
    assert.equal(await page.locator('#network [data-packet-kind]').count(),0,'continuous replay uses lines throughout');
    await click('#play');
    assert.equal(await page.locator('#network').getAttribute('data-display-mode'),'paused','pause changes mode immediately');
    await page.clock.runFor(800);
    assert.equal(await step.count(),0,'canceled step cannot return after pause');
    await load();await click('#next-time');await page.clock.runFor(100);await load();
    assert.equal(await step.count(),0,'file replacement clears the prior step');
    await page.clock.runFor(800);assert.equal(await step.count(),0);
    await page.setViewportSize({width:1440,height:1000});
    await click('#play');
    assert.equal(await page.locator('#network [data-highlight-kind="tx"]').count(),1,'active TX has a highlighted edge');
    assert.equal(await page.locator('#network [data-highlight-kind="rx"]').count(),2,'active RX has both highlighted edges');
    await page.clock.runFor(1000);
    assert.equal(await page.locator('#network [data-highlight-kind]').count(),0,'idle edges lose their highlights');
    assert(await page.locator('#network line[data-connection-node]').evaluateAll(rows=>rows.every(r=>r.getAttribute('stroke')==='#94a3b8'&&!r.hasAttribute('marker-start')&&!r.hasAttribute('marker-end'))),'idle connections use gray without directional highlights');
    assert.equal(await page.locator('#network [data-packet-kind]').count(),0);
    await page.clock.runFor(7100);
    assert.equal(await page.locator('#network').getAttribute('data-display-mode'),'paused','automatic end also changes mode');
    await load();await click('#next-time');await page.clock.runFor(1600);
    assert.equal(await stepTx.isVisible(),false);
    assert.equal(await stepRx.filter({visible:true}).count(),2);
    assert.equal(await stepRx.first().getAttribute('data-progress'),'1','a clock advance past both phases finishes RX');
    raw.simulation.receivers.forEach(r=>Object.assign(r,{observed_ps:String(BigInt(request.eof_ps)+100000000n),received_ps:String(BigInt(request.eof_ps)+200000000n)}));
    await load();
    await page.locator('#time-unit').selectOption('ps');await page.locator('#jump-time').fill(request.eof_ps);await click('#jump');
    await click('#next-time');
    assert.equal(await stepTx.count(),0,'an RX-only interval does not replay an older TX');
    assert.equal(await stepRx.filter({visible:true}).count(),2,'RX-only intervals start immediately');
    await page.clock.runFor(350);
    assert(Number(await stepRx.first().getAttribute('data-progress'))>.45);
    assert.deepEqual(errors,[]);
  } finally { await page.close(); }
}
async function assertGatewayRouteReplay(browser, viewer, result) {
  const page=await browser.newPage({viewport:{width:1440,height:1000}});
  try {
    await page.clock.install({time:new Date('2026-10-03T00:00:00Z')});
    await page.goto(pathToFileURL(viewer).href);
    await page.clock.pauseAt(new Date('2026-10-03T01:00:00Z'));
    await page.locator('#dashboard').waitFor({state:'visible'});
    const raw=JSON.parse(fs.readFileSync(result));
    const futureChildren=new Set(raw.simulation.model_records.filter(row=>row.schema_name==='can.request'&&
      row.data.model_fields.parent_request_id!==null&&row.data.model_fields.tx_enqueued_ps==='248000000').map(row=>row.data.request_id));
    const futureForwards=new Set(raw.simulation.model_records.filter(row=>row.schema_name==='gw.forward'&&
      futureChildren.has(row.data.child_request_id)).map(row=>row.data.forward_id));
    assert.equal(futureChildren.size,2,'fanout children are admitted at 248us');
    assert.equal(futureForwards.size,2,'both future child admissions belong to the two fanout forwards');
    await page.locator('#time-unit').selectOption('ps');
    await page.locator('#jump-time').fill('244000000');
    await page.locator('#jump').dispatchEvent('click');
    await assertText(page,'#current-ps','244000000 ps');

    const expectedKeys=[
      JSON.stringify(['Main.gw','fanout','Main.gw.a','Main.gw.b']),
      JSON.stringify(['Main.gw','fanout','Main.gw.a','Main.gw.c'])
    ].sort();
    const assertGlyphGeometry=async(selector)=>{
      const glyphs=await page.locator('#network').evaluate((svg,query)=>{
        const cards=Array.from(svg.querySelectorAll('g[data-node] > rect')).map(rect=>({
          x:Number(rect.getAttribute('x')),y:Number(rect.getAttribute('y')),
          width:Number(rect.getAttribute('width')),height:Number(rect.getAttribute('height'))
        }));
        const distance=(point,points)=>Math.min(...points.slice(1).map((to,index)=>{
          const from=points[index],dx=to.x-from.x,dy=to.y-from.y;
          const t=Math.max(0,Math.min(1,((point.x-from.x)*dx+(point.y-from.y)*dy)/(dx*dx+dy*dy)));
          return Math.hypot(point.x-(from.x+t*dx),point.y-(from.y+t*dy));
        }));
        return Array.from(svg.querySelectorAll(query)).map(glyph=>{
          const match=glyph.getAttribute('transform').match(/translate\(([-+\d.eE]+)[ ,]+([-+\d.eE]+)\)/);
          const point={x:Number(match[1]),y:Number(match[2])};
          const route=Array.from(svg.querySelectorAll('g[data-route-id]')).find(row=>row.dataset.routeKey===glyph.dataset.routeKey);
          const routePoints=JSON.parse(route.dataset.routePoints);
          return {key:glyph.dataset.routeKey,leg:glyph.dataset.gatewayLeg,forward:glyph.dataset.forwardId,
            onRoute:distance(point,routePoints)<.001,
            outsideCards:cards.every(card=>point.x<=card.x||point.x>=card.x+card.width||point.y<=card.y||point.y>=card.y+card.height)};
        });
      },selector);
      assert.deepEqual(glyphs.map(glyph=>glyph.key).sort(),expectedKeys);
      assert(glyphs.every(glyph=>glyph.onRoute&&glyph.outsideCards),'Gateway glyph centers lie on their declared route and outside Controller cards');
      return glyphs;
    };
    const assertRouteStyles=async activeLeg=>{
      const rows=await page.locator('#network [data-route-id]').evaluateAll(groups=>groups.flatMap(group=>
        Array.from(group.querySelectorAll('path[data-route-leg]')).map(path=>({key:group.dataset.routeKey,leg:path.dataset.routeLeg,
          stroke:path.getAttribute('stroke'),end:path.getAttribute('marker-end'),start:path.getAttribute('marker-start')}))));
      assert.deepEqual([...new Set(rows.map(row=>row.key))].sort(),expectedKeys);
      assert.equal(rows.length,4);
      for(const row of rows) {
        const active=row.leg===activeLeg;
        assert.equal(row.stroke,active?(activeLeg==='tx'?'#2563eb':'#c2410c'):'#94a3b8');
        assert.equal(row.end,active?`url(#communication-${row.leg}-arrow)`:'url(#gateway-route-arrow)');
        assert.equal(row.start,null,'route arrows point toward the declared egress');
      }
    };
    const staticGlyphs=page.locator('#network g[data-packet-kind="gw"]');
    assert.equal(await staticGlyphs.count(),2,'244us shows both active fanout Gateway transfers');
    assert((await assertGlyphGeometry('#network g[data-packet-kind="gw"]')).every(glyph=>glyph.leg==='rx'));
    await assertRouteStyles('rx');
    await page.setViewportSize({width:390,height:844});
    await assertGlyphGeometry('#network g[data-packet-kind="gw"]');
    await page.setViewportSize({width:1440,height:1000});

    const replaySnapshot=async()=>page.locator('#network').evaluate(svg=>{
      const glyphs=Array.from(svg.querySelectorAll('[data-step-transfer]')).map(glyph=>{
        const data=glyph.dataset,match=glyph.getAttribute('transform').match(/translate\(([-+\d.eE]+)[ ,]+([-+\d.eE]+)\)/);
        const routeKey=data.routeKey||'';
        return {medium:data.medium,kind:data.gatewayLeg||data.packetKind,source:data.source,receiver:data.receiver,
          request:data.requestId,forward:data.forwardId||'',parent:data.parentRequestId||'',bus:data.bus||'',routeKey,
          delay:data.stepDelay,until:data.stepUntil,glyphPath:data.glyphPath,hidden:glyph.hidden,
          visible:!glyph.hidden&&getComputedStyle(glyph).display!=='none',progress:Number(data.progress),
          x:Number(match[1]),y:Number(match[2])};
      }).sort((a,b)=>JSON.stringify([a.medium,a.kind,a.source,a.receiver,a.request,a.forward]).localeCompare(JSON.stringify([b.medium,b.kind,b.source,b.receiver,b.request,b.forward])));
      const routes=Array.from(svg.querySelectorAll('g[data-route-id]')).flatMap(group=>
        Array.from(group.querySelectorAll('path[data-route-leg]')).map(path=>({key:group.dataset.routeKey,leg:path.dataset.routeLeg,
          d:path.getAttribute('d'),stroke:path.getAttribute('stroke'),start:path.getAttribute('marker-start'),end:path.getAttribute('marker-end'),
          highlight:path.dataset.highlightKind||'',request:path.dataset.requestId||''})))
        .sort((a,b)=>JSON.stringify([a.key,a.leg]).localeCompare(JSON.stringify([b.key,b.leg])));
      const highlights=Array.from(svg.querySelectorAll('[data-highlight-kind]')).map(element=>({kind:element.dataset.highlightKind,
        routeKey:element.dataset.routeKey||'',node:element.dataset.connectionNode||'',bus:element.dataset.connectionBus||'',
        request:element.dataset.requestId||'',stroke:element.getAttribute('stroke'),start:element.getAttribute('marker-start'),
        end:element.getAttribute('marker-end')}))
        .sort((a,b)=>JSON.stringify([a.routeKey,a.node,a.bus,a.kind,a.request]).localeCompare(JSON.stringify([b.routeKey,b.node,b.bus,b.kind,b.request])));
      return {glyphs,routes,highlights};
    });
    const replayAt=async(startPs,direction)=>{
      await jumpPs(page,startPs);
      await page.locator(direction>0?'#next-time':'#prev-time').dispatchEvent('click');
      await assertText(page,'#current-ps','244000000 ps');
      const samples=[];let previous=0;
      for(const offset of [80,350,650,750,1000]) {
        await page.clock.runFor(offset-previous);previous=offset;
        samples.push(await replaySnapshot());
      }
      return samples;
    };
    const from238=await replayAt('238000000',1);
    const from248=await replayAt('248000000',-1);
    for(const [index,offset] of [80,350,650,750,1000].entries()) {
      const next=from238[index],previous=from248[index];
      assert.equal(next.glyphs.length,2,`238→244 shows both Gateway TX glyphs at ${offset}ms`);
      assert(next.glyphs.every(glyph=>glyph.medium==='gateway'&&glyph.kind==='tx'&&glyph.source==='Main.gw.a'&&
        glyph.receiver.startsWith('Main.gw.')&&glyph.request&&glyph.forward),`244us replays Gateway TX only at ${offset}ms`);
      assert.deepEqual(next.glyphs.map(glyph=>[glyph.source,glyph.receiver]).sort(),[
        ['Main.gw.a','Main.gw.b'],['Main.gw.a','Main.gw.c']
      ],'both fanout branches are present');
      assert(next.glyphs.every(glyph=>!(glyph.medium==='can'&&futureChildren.has(glyph.request))&&
        !(glyph.medium==='gateway'&&glyph.kind==='rx'&&futureForwards.has(glyph.forward))),
        '248us child CAN admission and Gateway RX do not leak into the 244us replay');
      assert.deepEqual(next.glyphs.map(({progress,x,y,...identity})=>identity),
        previous.glyphs.map(({progress,x,y,...identity})=>identity),`transfer identities, delay, routes and visibility match at ${offset}ms`);
      assert.deepEqual(next.routes,previous.routes,`Gateway routes match at ${offset}ms`);
      assert.deepEqual(next.highlights,previous.highlights,`Gateway and CAN highlights match at ${offset}ms`);
      for(let glyph=0;glyph<next.glyphs.length;glyph++) {
        assert(Math.abs(next.glyphs[glyph].progress-previous.glyphs[glyph].progress)<=.05,
          `sampled progress matches within RAF tolerance at ${offset}ms`);
        assert(Math.abs(next.glyphs[glyph].x-previous.glyphs[glyph].x)<=18&&Math.abs(next.glyphs[glyph].y-previous.glyphs[glyph].y)<=18,
          `sampled position matches within RAF tolerance at ${offset}ms`);
      }
    }
    await jumpPs(page,'244000000');

    await page.locator('#play').dispatchEvent('click');
    assert.equal(await page.locator('#network').getAttribute('data-display-mode'),'playback');
    assert.equal(await page.locator('#network [data-packet-kind]').count(),0,'continuous Gateway playback draws paths without message glyphs');
    await assertRouteStyles('rx');
    await page.locator('#play').dispatchEvent('click');
    await page.locator('#next-time').dispatchEvent('click');
    await assertText(page,'#current-ps','248000000 ps');

    const gwTx=page.locator('#network [data-step-transfer][data-medium="gateway"][data-gateway-leg="tx"]');
    const gwRx=page.locator('#network [data-step-transfer][data-medium="gateway"][data-gateway-leg="rx"]');
    const canTx=page.locator('#network [data-step-transfer][data-medium="can"][data-packet-kind="tx"]');
    const canRx=page.locator('#network [data-step-transfer][data-medium="can"][data-packet-kind="rx"]');
    assert.equal(await gwTx.filter({visible:true}).count(),2,'first replay stage shows internal TX on both branches');
    assert.equal(await gwRx.filter({visible:true}).count(),0);
    assert.equal(await canTx.filter({visible:true}).count(),0);
    await assertRouteStyles('tx');
    await page.clock.runFor(250);
    const beforeSelect=Number(await gwTx.first().getAttribute('data-progress'));
    const selectedId=await gwTx.first().getAttribute('data-request-id');
    await gwTx.first().dispatchEvent('click');
    await assertText(page,'#selected-id',selectedId);
    const afterSelect=Number(await gwTx.first().getAttribute('data-progress'));
    assert(afterSelect>=beforeSelect&&afterSelect-beforeSelect<.05,'selection preserves Gateway replay progress');
    await page.setViewportSize({width:390,height:844});
    await page.clock.runFor(100);
    assert(Number(await gwTx.first().getAttribute('data-progress'))>afterSelect,'resize preserves and advances replay progress');
    await assertGlyphGeometry('#network [data-step-transfer][data-medium="gateway"][data-gateway-leg="tx"]');
    await page.clock.runFor(300);
    assert.equal(await gwTx.filter({visible:true}).count(),2,'internal TX remains visible before 700ms');
    await page.clock.runFor(60);
    assert.equal(await gwTx.filter({visible:true}).count(),0);
    assert.equal(await gwRx.filter({visible:true}).count(),2,'internal RX starts after 700ms');
    await assertRouteStyles('rx');
    await assertGlyphGeometry('#network [data-step-transfer][data-medium="gateway"][data-gateway-leg="rx"]');
    await page.clock.runFor(700);
    assert.equal(await canTx.filter({visible:true}).count(),2,'both child CAN TX transfers start after 1400ms');
    assert.equal(await canRx.filter({visible:true}).count(),0);
    await page.clock.runFor(700);
    await page.clock.runFor(50);
    assert.equal(await canTx.filter({visible:true}).count(),0);
    assert.equal(await canRx.filter({visible:true}).count(),2,'both child CAN RX transfers start after 2100ms');
    await page.clock.runFor(1000);
    assert(await canRx.evaluateAll(rows=>rows.every(g=>g.dataset.progress==='1'&&g.getAttribute('transform')===`translate(${g.dataset.toX} ${g.dataset.toY})`)),'one clock advance past all four stages completes both child RX glyphs');
  } finally { await page.close(); }
}
async function main(){
  if (process.argv.includes('--recorded-events') || process.argv.includes('--interval-comparison')) {
    const browser=await chromium.launch({headless:true,executablePath:process.env.PLAYWRIGHT_CHROMIUM_EXECUTABLE});
    try { if(process.argv.includes('--interval-comparison')) await assertIntervalComparison(browser); else await assertRecordedEvents(browser); }
    finally { await browser.close(); }
    return;
  }
  const report=run(['run','--config',path.join(root,'examples/can/baseline.ini'),'--output',path.join(temp,'baseline')]);
  const result=path.join(report.output_path,'results.json');
  const viewer=path.join(temp,'standalone.html');
  run(['view','--input',result,'--output',viewer]);
  const browser=await chromium.launch({headless:true});
  try {
    await assertRecordedEvents(browser);
    await assertIntervalComparison(browser);
    await assertStepPlayback(browser,viewer,result);
    const fanout=run(['run','--config',path.join(root,'examples/gateway/fanout.ini'),'--output',path.join(temp,'gateway-fanout')]);
    const fanoutResult=path.join(fanout.output_path,'results.json'),fanoutViewer=path.join(temp,'gateway-fanout.html');
    run(['view','--input',fanoutResult,'--output',fanoutViewer]);
    await assertGatewayRouteReplay(browser,fanoutViewer,fanoutResult);
    const page=await browser.newPage({viewport:{width:1440,height:1000}});
    const errors=[],network=[];
    page.on('pageerror',error=>errors.push(error.message));
    page.on('request',req=>{if(/^https?:/.test(req.url()))network.push(req.url());});
    await page.goto(pathToFileURL(viewer).href);
    await page.locator('#dashboard').waitFor({state:'visible'});
    await assertText(page,'#count-generated','3');
    await assertText(page,'#count-success','0');
    await page.locator('#next-time').click();
    assert.notEqual(await page.locator('#current-ps').innerText(),'0 ps');
    await page.locator('#prev-time').click();
    await assertText(page,'#current-ps','0 ps');
    await page.locator('#play').click();
    await page.waitForFunction(()=>document.getElementById('current-ps').textContent!=='0 ps');
    await page.locator('#play').click();
    await page.locator('#time-unit').selectOption('ps');
    await page.locator('#jump-time').fill('20000000000');
    await page.locator('#jump').click();
    await assertText(page,'#current-ps','20000000000 ps');
    await assertText(page,'#count-generated','60');
    await assertText(page,'#count-success','60');
    await assertText(page,'#count-received','120');
    await page.locator('#request-search').fill('a:0');
    assert.equal(await page.locator('#request-rows tr').count(),1);
    await page.locator('#request-rows tr').first().click();
    await assertText(page,'#selected-id','a:0');
    await page.locator('#zoom-in').click();
    await page.locator('#zoom-reset').click();
    await page.locator('#request-search').fill('');
    await page.locator('#time-unit').selectOption('us');
    await page.locator('#jump-time').fill('750');
    await page.locator('#jump').click();
    await page.evaluate(()=>window.scrollTo(0,0));
    if(process.env.VIEWER_SCREENSHOT) await page.screenshot({path:process.env.VIEWER_SCREENSHOT,fullPage:false});
    await page.setViewportSize({width:390,height:844});
    assert.equal(await page.evaluate(()=>document.documentElement.scrollWidth<=innerWidth),true,'mobile document should not overflow horizontally');
    await page.setViewportSize({width:1440,height:1000});

    // Direct file:// assets use a file input, never fetch a neighbouring private file.
    await page.goto(pathToFileURL(path.join(root,'crates/dir-simulator/src/tool/viewer/assets/index.html')).href);
    await page.locator('#empty-state').waitFor({state:'visible'});
    await page.locator('#file-input').setInputFiles(result);
    await page.locator('#dashboard').waitFor({state:'visible'});
    await assertText(page,'#count-generated','3');

    // Failure invalidates the old run; another valid selection recovers.
    await page.locator('#file-input').setInputFiles({name:'bad.json',mimeType:'application/json',buffer:Buffer.from('{not json')});
    await page.locator('#error-banner').waitFor({state:'visible'});
    assert.equal(await page.locator('#dashboard').isVisible(),false);
    await page.locator('#file-input').setInputFiles(result);
    await page.locator('#dashboard').waitFor({state:'visible'});

    // A hostile identifier and source content remain inert in both embedding and DOM rendering.
    const raw=JSON.parse(fs.readFileSync(result));
    const hostile='</script><img src=x onerror="globalThis.viewerInjected=1"><script>globalThis.viewerInjected=1</script>';
    raw.run_id=hostile;
    const old=raw.simulation.requests[0].request_id;
    raw.simulation.requests[0].request_id=hostile;
    for(const row of [...raw.simulation.receivers,...raw.simulation.records]) if(row.request_id===old)row.request_id=hostile;
    raw.metadata.sources[0].content_utf8=hostile;
    const hostilePath=path.join(temp,'hostile.json'),hostileViewer=path.join(temp,'hostile.html');
    fs.writeFileSync(hostilePath,JSON.stringify(raw));
    run(['view','--input',hostilePath,'--output',hostileViewer]);
    await page.goto(pathToFileURL(hostileViewer).href);
    await page.locator('#dashboard').waitFor({state:'visible'});
    assert.equal(await page.evaluate(()=>globalThis.viewerInjected),undefined);
    assert.equal(await page.locator('img[src="x"]').count(),0);
    assert((await page.locator('#run-id').innerText()).includes('</script>'));

    // EOF exactly at cutoff is still in-flight; reached EOF without release is intermission.
    for (const [scenario, expectedSuccess, busLabel] of [
      ['eof-boundary','0','送信中'], ['after-eof','1','バス間隔']
    ]) {
      const edge=run(['run','--config',path.join(root,`docs/verification/fixtures/can/${scenario}.ini`),'--output',path.join(temp,scenario)]);
      const edgePath=path.join(edge.output_path,'results.json');
      const edgeRaw=JSON.parse(fs.readFileSync(edgePath));
      await page.locator('#file-input').setInputFiles({name:scenario+'.json',mimeType:'application/json',buffer:Buffer.from(JSON.stringify(edgeRaw))});
      await page.waitForFunction(name=>document.getElementById('filename').textContent===name,scenario+'.json');
      await page.locator('#time-unit').selectOption('ps');
      await page.locator('#jump-time').fill(edgeRaw.simulation.end_ps);
      await page.locator('#jump').click();
      await assertText(page,'#count-success',expectedSuccess);
      assert((await page.locator('#bus-states').innerText()).includes(busLabel));
    }

    // Partial results show committed EOF at the failure boundary and pending delivery.
    const partial=JSON.parse(fs.readFileSync(result));
    partial.simulation.requests=partial.simulation.requests.slice(0,1);
    const committed=partial.simulation.requests[0];
    committed.model_fields.release_ps=null;
    partial.simulation.end_ps=committed.eof_ps;
    partial.simulation.termination='execution_failed';partial.simulation.partial=true;
    partial.simulation.records=[];
    partial.simulation.receivers=partial.simulation.receivers.filter(r=>r.request_id===committed.request_id);
    for(const r of partial.simulation.receivers){r.status='pending';r.observed_ps=null;r.received_ps=null;}
    await page.locator('#file-input').setInputFiles({name:'partial.json',mimeType:'application/json',buffer:Buffer.from(JSON.stringify(partial))});
    await page.waitForFunction(()=>document.getElementById('filename').textContent==='partial.json');
    await page.locator('#jump-time').fill(partial.simulation.end_ps);await page.locator('#jump').click();
    assert.equal(await page.locator('#partial-badge').isVisible(),true);
    await assertText(page,'#count-success','1');await assertText(page,'#count-received','0');

    // Distinct adjacent timestamps above 2^53 remain distinct in the browser controls.
    const huge=JSON.parse(fs.readFileSync(result)), base=9007199254740992n;
    huge.simulation.requests=huge.simulation.requests.slice(0,1);huge.simulation.receivers=[];huge.simulation.records=[];
    huge.simulation.end_ps=(base+10n).toString();
    const first=huge.simulation.requests[0];
    for(const [field,offset] of [['generated_ps',0n],['ready_ps',1n],['sof_ps',2n],['eof_ps',3n]])first[field]=(base+offset).toString();
    first.model_fields.planned_eof_ps=first.eof_ps;
    first.model_fields.release_ps=(base+4n).toString();first.model_fields.planned_release_ps=first.model_fields.release_ps;
    await page.locator('#file-input').setInputFiles({name:'huge.json',mimeType:'application/json',buffer:Buffer.from(JSON.stringify(huge))});
    await page.waitForFunction(()=>document.getElementById('filename').textContent==='huge.json');
    await page.locator('#jump-time').fill((base+1n).toString());await page.locator('#jump').click();
    await assertText(page,'#current-ps',`${base+1n} ps`);
    await assertText(page,'#count-pending','1');
    await page.locator('#next-time').click();
    await assertText(page,'#current-ps',`${base+2n} ps`);
    await assertText(page,'#count-in-flight','1');

    // File drop uses the same local loader.
    await page.evaluate(content=>{
      const transfer=new DataTransfer();transfer.items.add(new File([content],'dropped.json',{type:'application/json'}));
      document.dispatchEvent(new DragEvent('drop',{dataTransfer:transfer,bubbles:true,cancelable:true}));
    },fs.readFileSync(result,'utf8'));
    await page.waitForFunction(()=>document.getElementById('filename').textContent==='dropped.json');
    await assertText(page,'#count-generated','3');

    // The topology uses all requests even beyond the timeline's drawing cap.
    const overload=run(['run','--config',path.join(root,'examples/can/overload.ini'),'--output',path.join(temp,'overload')]);
    const overloadPath=path.join(overload.output_path,'results.json');
    const overloadRaw=JSON.parse(fs.readFileSync(overloadPath));
    await page.locator('#file-input').setInputFiles(overloadPath);
    await page.waitForFunction(()=>document.getElementById('request-total').textContent==='1200 件');
    assert.equal(await page.locator('#network [data-node]').count(),3);
    const late=overloadRaw.simulation.requests.filter(r=>r.sof_ps!==null).reduce((a,b)=>BigInt(a.sof_ps)>BigInt(b.sof_ps)?a:b);
    const sof=BigInt(late.sof_ps),eof=BigInt(late.model_fields.planned_eof_ps);
    const tx=page.locator('#network [data-packet-kind="tx"]');
    await jumpPs(page,sof+(eof-sof)/4n);
    assert.equal(await tx.getAttribute('data-request-id'),late.request_id);
    const firstPosition=await tx.getAttribute('transform');
    await jumpPs(page,sof+(eof-sof)/2n);
    assert.notEqual(await tx.getAttribute('transform'),firstPosition,'TX glyph moves with cursor');
    await jumpPs(page,sof+(eof-sof)/4n);
    assert.equal(await tx.getAttribute('transform'),firstPosition,'rewind reconstructs exact glyph');
    const frozen=await page.locator('#network').innerHTML();
    await page.waitForTimeout(120);
    assert.equal(await page.locator('#network').innerHTML(),frozen,'paused diagram stays fixed');
    await page.locator('#request-search').fill('no matching request');
    assert.equal(await tx.getAttribute('data-request-id'),late.request_id,'search cannot hide active TX');
    await tx.click();await assertText(page,'#selected-id',late.request_id);
    await page.locator('#request-search').fill('');
    await jumpPs(page,19800000000n);
    await page.locator('#network').scrollIntoViewIfNeeded();
    if(process.env.VIEWER_SCREENSHOT)await page.screenshot({path:process.env.VIEWER_SCREENSHOT});
    await page.setViewportSize({width:390,height:844});
    assert.equal(await page.evaluate(()=>document.documentElement.scrollWidth<=innerWidth),true);
    assert.equal(await tx.getAttribute('data-request-id'),late.request_id);
    await page.setViewportSize({width:1440,height:1000});
    await jumpPs(page,overloadRaw.simulation.end_ps);
    assert.equal(await tx.count(),1,'cutoff leaves final packet transmitting');
    assert.equal(await page.locator('#network [data-trail-kind="received"]').evaluateAll((rows,id)=>rows.filter(r=>r.dataset.requestId===id).length,late.request_id),0);

    // Actual zero-delay broadcast illuminates both receiver ports at the same EOF.
    const sent=overloadRaw.simulation.requests.find(r=>r.eof_ps!==null);
    await jumpPs(page,sent.eof_ps);
    const received=page.locator('#network [data-trail-kind="received"]');
    assert.equal(await received.count(),2);
    assert.deepEqual(await received.evaluateAll(rows=>rows.map(r=>[r.dataset.requestId,r.dataset.timePs,r.getAttribute('opacity')])),[[sent.request_id,sent.eof_ps,'1'],[sent.request_id,sent.eof_ps,'1']]);
    await jumpPs(page,BigInt(sent.eof_ps)-1n);
    assert.equal(await received.count(),0);

    // Delayed observation moves toward the receiver, then becomes RX processing.
    const delayed=JSON.parse(fs.readFileSync(result));
    const frame=delayed.simulation.requests[0],finish=BigInt(frame.eof_ps);
    delayed.simulation.requests=[frame];delayed.simulation.records=[];
    delayed.simulation.receivers=delayed.simulation.receivers.filter(r=>r.request_id===frame.request_id);
    delayed.simulation.receivers.forEach((r,i)=>Object.assign(r,{status:i?'filtered':'received',observed_ps:String(finish+BigInt(i?150000000:100000000)),received_ps:i?null:String(finish+200000000n)}));
    await page.locator('#file-input').setInputFiles({name:'delayed.json',mimeType:'application/json',buffer:Buffer.from(JSON.stringify(delayed))});
    await page.waitForFunction(()=>document.getElementById('filename').textContent==='delayed.json');
    await jumpPs(page,finish+25000000n);
    const observing=page.locator('#network [data-packet-kind="rx"][data-phase="observation"]');
    assert.equal(await observing.count(),2);
    const rxPosition=await observing.first().getAttribute('transform');
    await jumpPs(page,finish+75000000n);
    assert.notEqual(await observing.first().getAttribute('transform'),rxPosition,'RX observation glyph moves');
    await jumpPs(page,finish+175000000n);
    assert.equal(await page.locator('#network [data-phase="processing"]').count(),1);
    assert.equal(await page.locator('#network [data-trail-kind="filtered"]').count(),1);
    await assertText(page,'#count-success','1');
    await assertText(page,'#count-received','0');
    await jumpPs(page,finish+200000000n);
    assert.equal(await page.locator('#network [data-packet-kind="rx"]').count(),0);
    await assertText(page,'#count-received','1');

    // Schema2: independent bus activity, GW processing and references to native/copy requests.
    const gateway=run(['run','--config',path.join(root,'docs/verification/fixtures/gw/delay.ini'),'--output',path.join(temp,'gateway')]);
    const gatewayResult=path.join(gateway.output_path,'results.json');
    const gatewayRaw=JSON.parse(fs.readFileSync(gatewayResult));
    const gatewayEntry=gatewayRaw.metadata.config.find(entry=>entry.key==='@profile:can.cc.multibus.v1:Main.gw');
    assert(gatewayEntry,'fixture exports normalized Gateway configuration');
    const actualPorts=JSON.parse(gatewayEntry.value).ports;
    await page.locator('#file-input').setInputFiles(gatewayResult);
    await page.waitForFunction(()=>document.getElementById('gateway-panel').hidden===false);
    await jumpPs(page,0n);
    const declared=gatewayRaw.metadata.topology?.controllers;
    if(declared) {
      const links=await page.locator('#network [data-connection-node]').evaluateAll(rows=>rows.map(r=>[r.dataset.connectionNode,r.dataset.connectionBus,r.dataset.inferred]));
      for(const c of declared)assert(links.some(link=>link[0]===c.id&&link[1]===c.bus&&link[2]==='false'),'declared wiring exists at idle');
    }
    const routes=JSON.parse(gatewayEntry.value).routes.flatMap(r=>r.egress.map(egress=>[r.ingress,egress]));
    assert.deepEqual(await page.locator('#network [data-route-id]').evaluateAll(rows=>rows.map(r=>[r.dataset.routeIngress,r.dataset.routeEgress])),routes);
    assert.equal(await page.locator('#network [data-rx-buffer]').count(),actualPorts.length);
    assert.equal(await page.locator('#network [data-rx-buffer="Main.source"]').count(),0);
    await jumpPs(page,50000000n);
    assert.equal(await page.locator('#network [data-packet-kind="tx"]').count(),2);
    await assertGatewayLayout(page,actualPorts,'horizontal');
    const frameRx=page.locator('#network [data-packet-kind="rx"][data-phase="frame"][data-receiver="Main.gw.a"]');
    assert.equal(await frameRx.count(),1);
    assert.equal(await frameRx.locator('rect').getAttribute('fill'),'#c2410c');
    await assertText(page,'#count-received','0');
    await jumpPs(page,120000000n);
    assert.match(await page.locator('#gateway-rows').innerText(),/GW 処理中/);
    await jumpPs(page,126000000n);
    assert.match(await page.locator('#gateway-rows').innerText(),/コピー生成済み/);
    await page.locator('#gateway-rows button').filter({hasText:'要求を見る'}).click();
    assert.match(await page.locator('#selected-id').innerText(),/^gw:source:0/);
    assert.match(await page.locator('#detail-fields').innerText(),/元要求/);
    await jumpPs(page,412000000n);
    assert.match(await page.locator('#detail-fields').innerText(),/経路遅延 Main.sink/);
    assert.match(await page.locator('#detail-fields').innerText(),/412000000 ps/);
    // Real finite-TX fixture retains its copy in ingress RX until a TX slot is freed.
    const queueRun=run(['run','--config',path.join(root,'docs/verification/fixtures/gw/queue.ini'),'--output',path.join(temp,'gateway-queue')]);
    const queuePath=path.join(queueRun.output_path,'results.json'),queueRaw=JSON.parse(fs.readFileSync(queuePath));
    const waitingCopy=queueRaw.simulation.model_records.find(r=>r.schema_name==='can.request'&&r.data.model_fields.tx_enqueued_ps!==null&&BigInt(r.data.model_fields.tx_enqueued_ps)>BigInt(r.data.ready_ps));
    assert(waitingCopy,'real fixture records TX-capacity waiting');
    const waitingForward=queueRaw.simulation.model_records.find(r=>r.schema_name==='gw.forward'&&r.data.parent_request_id===waitingCopy.data.model_fields.parent_request_id&&r.data.egress===waitingCopy.data.source);
    assert(waitingForward,'real fixture links the retained copy to its Gateway forward');
    const queuedBuffer=queueRaw.simulation.model_records.find(r=>r.schema_name==='gw.rx_buffer'&&r.data.parent_request_id===waitingCopy.data.model_fields.parent_request_id);
    await page.locator('#file-input').setInputFiles(queuePath);
    await page.waitForFunction(()=>document.getElementById('filename').textContent==='results.json');
    const beforeAdmission=BigInt(waitingCopy.data.model_fields.tx_enqueued_ps)-1n;
    await jumpPs(page,beforeAdmission);
    const retained=page.locator('#gateway-rx-rows').getByRole('button',{name:waitingCopy.data.model_fields.parent_request_id,exact:true});
    assert.equal(await retained.count(),1);
    await retained.click();
    assert.match(await page.locator('#detail-fields').innerText(),/GW RX Main.gw.a/);
    assert.match(await page.locator('#detail-fields').innerText(),/保持中/);
    assert.match(await page.locator('#gateway-rows').innerText(),/RX保持 \/ TX 容量待機/);
    const realWait=page.locator('#network [data-packet-kind="gw"][data-phase="waiting_tx"]');
    assert.equal(await realWait.count(),1);
    assert.equal(await realWait.getAttribute('data-request-id'),waitingCopy.record_id);
    const waitStepTime=queueRaw.simulation.model_records.map(row=>BigInt(row.time_ps)).filter(time=>time>BigInt(waitingCopy.data.ready_ps)&&time<BigInt(waitingCopy.data.model_fields.tx_enqueued_ps)).sort((a,b)=>a<b?-1:a>b?1:0)[0];
    assert(waitStepTime,'fixture has an event while the copy remains in the ingress RX buffer');
    await jumpPs(page,waitStepTime);
    await page.locator('#next-time').dispatchEvent('click');
    const waitReplay=page.locator(`#network [data-step-transfer][data-forward-id="${waitingForward.data.forward_id}"]`);
    const retainedWait=page.locator(`#network [data-packet-kind="gw"][data-forward-id="${waitingForward.data.forward_id}"]`);
    assert.equal(await waitReplay.count(),0,'unrelated event step has no replay for the still-waiting forward');
    assert.equal(await retainedWait.getAttribute('data-gateway-leg'),'waiting_tx','step keeps its static RX-buffer wait badge at ingress');
    assert(BigInt((await page.locator('#current-ps').innerText()).replace(' ps',''))<BigInt(waitingCopy.data.model_fields.tx_enqueued_ps));
    await jumpPs(page,beforeAdmission);
    assert((await page.locator('#timeline title').allTextContents()).some(text=>text.includes('Gateway RX保持')),'timeline shows capacity wait in ingress RX');
    await page.locator('#network').scrollIntoViewIfNeeded();
    if(process.env.VIEWER_RX_SCREENSHOT)await page.screenshot({path:process.env.VIEWER_RX_SCREENSHOT});
    await page.setViewportSize({width:390,height:844});
    assert.equal(await page.evaluate(()=>document.documentElement.scrollWidth<=innerWidth),true,'mobile RX wait diagram fits page');
    assert.equal(await realWait.count(),1);
    await page.setViewportSize({width:1440,height:1000});
    await page.waitForFunction(()=>Number(document.querySelector('#network').getAttribute('viewBox').split(' ')[2])>=650);
    const retainedSnapshot=await page.locator('#network').innerHTML();
    await jumpPs(page,waitingCopy.data.model_fields.tx_enqueued_ps);
    assert.equal(await realWait.count(),0);
    assert.equal(await retained.count(),0);
    await jumpPs(page,beforeAdmission);
    assert.equal(await page.locator('#network').innerHTML(),retainedSnapshot);
    assert.equal(queuedBuffer.data.released_ps,waitingCopy.data.model_fields.tx_enqueued_ps);
    // Replay a fanout where one sibling is admitted while the other waits for TX capacity.
    const waiting=structuredClone(gatewayRaw);
    waiting.simulation.records=[];
    waiting.simulation.model_records=waiting.simulation.model_records.filter(r=>r.schema_name!=='gw.rx_buffer');
    const forward=waiting.simulation.model_records.find(r=>r.schema_name==='gw.forward'&&r.data.child_request_id);
    const child=waiting.simulation.model_records.find(r=>r.schema_name==='can.request'&&r.data.request_id===forward.data.child_request_id);
    const ready=child.data.ready_ps;
    const sibling=structuredClone(child),siblingForward=structuredClone(forward);
    const siblingId=child.record_id+'-sibling';
    Object.assign(sibling,{record_id:siblingId,subject:'Main.gw.c',request_id:siblingId,time_ps:ready});
    Object.assign(sibling.data,{request_id:siblingId,source:'Main.gw.c',bus:'Main.busC',status:'pending',sof_ps:null,eof_ps:null});
    Object.assign(sibling.data.model_fields,{tx_enqueued_ps:ready,planned_eof_ps:null,planned_release_ps:null,release_ps:null});
    siblingForward.record_id=siblingId;
    Object.assign(siblingForward.data,{forward_id:siblingId,egress:'Main.gw.c',child_request_id:siblingId});
    const waitedId=child.record_id;
    waiting.simulation.model_records=waiting.simulation.model_records.filter(r=>r.schema_name!=='can.receiver'||r.request_id!==waitedId);
    Object.assign(child,{time_ps:ready});
    Object.assign(child.data,{status:'waiting_tx',sof_ps:null,eof_ps:null});
    Object.assign(child.data.model_fields,{tx_enqueued_ps:null,planned_eof_ps:null,planned_release_ps:null,release_ps:null});
    waiting.simulation.model_records.push(sibling,siblingForward);
    const normalized=JSON.parse(waiting.metadata.config.find(e=>e.key===gatewayEntry.key).value);
    normalized.ports.push('Main.gw.c');normalized.rx_queue_capacity='1';
    normalized.routes.find(r=>r.id===forward.data.route_id).egress.push('Main.gw.c');
    waiting.metadata.config.find(e=>e.key===gatewayEntry.key).value=JSON.stringify(normalized);
    if(waiting.metadata.topology)waiting.metadata.topology.controllers.push({id:'Main.gw.c',bus:'Main.busC',tx_channel_delay_ps:'0',rx_channel_delay_ps:'0'});
    const bufferId=`rx:${forward.data.parent_request_id}/${forward.data.gateway}/${forward.data.ingress}`;
    waiting.simulation.model_records.push({schema_name:'gw.rx_buffer',schema_version:1,record_id:bufferId,subject:forward.data.ingress,request_id:forward.data.parent_request_id,origin_request_id:forward.origin_request_id,time_ps:forward.data.received_ps,data:{buffer_id:bufferId,parent_request_id:forward.data.parent_request_id,origin_request_id:forward.origin_request_id,gateway:forward.data.gateway,ingress:forward.data.ingress,capacity:'1',received_ps:forward.data.received_ps,released_ps:null,status:'holding',reason:null,egress:[forward.data.egress,'Main.gw.c']}});
    await page.locator('#file-input').setInputFiles({name:'gateway-wait.json',mimeType:'application/json',buffer:Buffer.from(JSON.stringify(waiting))});
    await page.waitForFunction(()=>document.getElementById('filename').textContent==='gateway-wait.json');
    await jumpPs(page,ready);
    const held=page.locator('#gateway-rx-rows tr[data-rx-ingress="Main.gw.a"]');
    assert.match(await held.innerText(),/Main.gw.b: TX 容量待機/);
    assert.match(await held.innerText(),/Main.gw.c: TX受付済み/);
    const rxBuffer=page.locator('#network [data-rx-buffer="Main.gw.a"]');
    assert.equal(await rxBuffer.getAttribute('data-occupancy'),'1');
    const waitPacket=page.locator('#network [data-packet-kind="gw"][data-phase="waiting_tx"]');
    assert.equal(await waitPacket.count(),1);
    assert.equal(await waitPacket.getAttribute('data-parent-request-id'),forward.data.parent_request_id);
    const waitingSnapshot=await page.locator('#network').innerHTML();
    await jumpPs(page,BigInt(ready)-1n);
    assert.equal(await waitPacket.count(),0);
    await jumpPs(page,ready);
    assert.equal(await page.locator('#network').innerHTML(),waitingSnapshot,'rewind restores Gateway wait and retained RX exactly');
    await page.locator('#request-search').fill(waitedId);
    assert.match(await page.locator('#request-rows').innerText(),/TX 容量待機/);
    await page.locator('#request-search').fill('');
    // Schema sorting puts ordinal 99 after 599; the visible tail must follow reception time.
    const many=JSON.parse(fs.readFileSync(gatewayResult));
    const templates=many.simulation.model_records;
    many.simulation.model_records=[];many.simulation.records=[];many.simulation.end_ps='600000000000';
    for(let i=0;i<600;i++) for(const template of templates) {
      const row=structuredClone(template),offset=BigInt(i)*1000000000n;
      const identifier=value=>typeof value==='string'?value.replace(/source:0/g,`source:${i}`).replace(/other:0/g,`other:${i}`):value;
      for(const key of ['record_id','request_id','origin_request_id'])row[key]=identifier(row[key]);
      for(const key of ['request_id','origin_request_id','parent_request_id','forward_id','child_request_id','buffer_id'])if(key in row.data)row.data[key]=identifier(row.data[key]);
      if(row.data.model_fields)for(const key of ['origin_request_id','parent_request_id'])row.data.model_fields[key]=identifier(row.data.model_fields[key]);
      for(const target of [row,row.data,row.data.model_fields].filter(Boolean))for(const key of Object.keys(target))if(key.endsWith('_ps')&&target[key]!==null)target[key]=String(BigInt(target[key])+offset);
      many.simulation.model_records.push(row);
    }
    many.simulation.model_records.sort((a,b)=>a.schema_name.localeCompare(b.schema_name)||a.subject.localeCompare(b.subject)||a.record_id.localeCompare(b.record_id));
    await page.locator('#file-input').setInputFiles({name:'many-gateway.json',mimeType:'application/json',buffer:Buffer.from(JSON.stringify(many))});
    await page.waitForFunction(()=>document.getElementById('filename').textContent==='many-gateway.json');
    await jumpPs(page,600000000000n);
    assert.equal(await page.locator('#gateway-rows tr').count(),500);
    assert.match(await page.locator('#gateway-count').innerText(),/記録 1200 \/ 1200/);
    assert.equal(await page.locator('#gateway-rows tr').first().locator('button').first().innerText(),'source:350');
    assert.equal(await page.locator('#gateway-rows tr').last().locator('button').first().innerText(),'other:599');
    const boundary=run(['run','--config',path.join(root,'docs/verification/fixtures/gw/forward-boundary.ini'),'--output',path.join(temp,'gateway-boundary')]);
    await page.locator('#file-input').setInputFiles(path.join(boundary.output_path,'results.json'));
    await page.waitForFunction(()=>document.getElementById('request-total').textContent.includes('1'));
    await jumpPs(page,120000000n);
    assert.match(await page.locator('#gateway-rows').innerText(),/GW 処理中/);
    assert.equal(await page.locator('#gateway-rows button').filter({hasText:'要求を見る'}).count(),0);

    // Normalized metadata retains an idle, non-prefixed third Controller in the Gateway layout.
    const gatewayLayout=structuredClone(gatewayRaw);
    const layoutEntry=gatewayLayout.metadata.config.find(entry=>entry.key==='@profile:can.cc.multibus.v1:Main.gw');
    const normalizedGateway=JSON.parse(layoutEntry.value);
    normalizedGateway.ports.push('Idle.Controller');
    layoutEntry.value=JSON.stringify(normalizedGateway);
    const layoutPath=path.join(temp,'gateway-layout.json');
    fs.writeFileSync(layoutPath,JSON.stringify(gatewayLayout));
    await page.locator('#file-input').setInputFiles(layoutPath);
    await page.waitForFunction(()=>document.getElementById('filename').textContent==='gateway-layout.json');
    const layoutPorts=[...normalizedGateway.ports].sort();
    await page.setViewportSize({width:1440,height:1000});
    await assertGatewayLayout(page,layoutPorts,'horizontal');
    await page.setViewportSize({width:390,height:844});
    await page.waitForFunction(()=>{
      const group=document.querySelector('#network g[data-gateway="Main.gw"]');
      const nodes=group?Array.from(group.children).filter(element=>element.matches('g[data-node]')):[];
      const boxes=nodes.map(node=>node.querySelector(':scope > rect')).map(rect=>({x:Number(rect.getAttribute('x')),y:Number(rect.getAttribute('y'))}));
      return boxes.length===3&&boxes.every(box=>box.x===boxes[0].x)&&boxes[1].y>boxes[0].y&&boxes[2].y>boxes[1].y;
    });
    await assertGatewayLayout(page,layoutPorts,'vertical');
    assert.equal(await page.evaluate(()=>document.documentElement.scrollWidth<=innerWidth),true,'mobile Gateway layout does not overflow the page');

    // Zero duration preserves idle nodes and safely disables playback.
    raw.run_id='zero';raw.simulation.requests=[];raw.simulation.receivers=[];raw.simulation.records=[];
    raw.simulation.end_ps='0';raw.simulation.termination='time_limit';
    await page.locator('#file-input').setInputFiles({name:'zero.json',mimeType:'application/json',buffer:Buffer.from(JSON.stringify(raw))});
    await page.waitForFunction(()=>document.getElementById('filename').textContent==='zero.json');
    await assertText(page,'#count-generated','0');
    assert.equal(await page.locator('#play').isDisabled(),true);
    await assertText(page,'#current-ps','0 ps');
    assert.equal(network.length,0,'viewer must not make external requests');
    assert.deepEqual(errors,[],'browser errors');
    console.log('PASS: sequential step TX/RX animation, directional blue/orange/gray lines, cancellation/resize/rewind; viewer playback, seek, filters/details, responsive layout, validation, exact times, topology animation, schema2 independent buses/Gateway processing/copy/origin/boundary, RX capacity/fanout/TX wait/rewind, zero horizon; no network or browser errors');
  } finally {await browser.close();}
}
async function assertRecordedEvents(browser) {
  const {fixture,rxHoldingFixture}=require('./viewer_fixtures.cjs');
  const API=require('../crates/dir-simulator/src/tool/viewer/assets/model.js');
  const page=await browser.newPage({viewport:{width:1440,height:1000}});
  const errors=[],network=[];
  page.on('pageerror',e=>errors.push(e.message));
  page.on('request',r=>{if(/^https?:/.test(r.url()))network.push(r.url());});
  const load=async(raw,name)=>{
    await page.locator('#file-input').setInputFiles({name,mimeType:'application/json',buffer:Buffer.from(JSON.stringify(raw))});
    await page.waitForFunction(name=>document.getElementById('filename').textContent===name,name);
  };
  try {
    await page.clock.install({time:new Date('2026-10-11T00:00:00Z')});
    await page.clock.pauseAt(new Date('2026-10-11T01:00:00Z'));
    await page.goto(pathToFileURL(path.join(root,'crates/dir-simulator/src/tool/viewer/assets/index.html')).href);
    const publicDir=path.join(root,'docs/verification/results/acceptance-2026-10-08/output-exports/dir_test_0072_input_a_complete_ledgers_per_receiver_and_conservation-1226012-0');
    const publicRaw=JSON.parse(fs.readFileSync(path.join(publicDir,'results.json')));
    for(const [raw,id,name] of [[fixture(),'g:0','schema1.json'],[rxHoldingFixture(),'source:0','schema2.json'],[publicRaw,publicRaw.simulation.requests[0].request_id,'public-recorded-can.json']]) {
      await load(raw,name);
      await page.locator('#request-rows tr').filter({hasText:id}).first().click();
      const groups=API.requestEventGroups(API.parseResults(raw),id);
      assert.equal(await page.locator('#request-events button').count(),groups.length);
      assert.equal(await page.locator('#request-total').innerText(),`${API.parseResults(raw).requests.length} 件`);
      for(const group of [...groups].reverse()) {
        await page.locator(`#request-events [data-event-time="${group.time}"]`).click();
        await assertText(page,'#current-ps',`${group.time} ps`);
        await assertText(page,'#selected-id',id);
        const eventState=await page.locator('#node-states').innerText();
        await jumpPs(page,group.time);
        assert.equal(await page.locator('#node-states').innerText(),eventState);
      }
      // Enter activates a standard button and focus survives its rerender.
      const first=page.locator('#request-events button').first();
      await first.focus();await page.keyboard.press('Enter');
      assert.equal(await page.locator('#request-events button').first().evaluate(e=>e===document.activeElement),true);
      const chosen=API.parseResults(raw).requests.find(r=>r.id===id);
      await jumpPs(page,chosen.sof-1n);
      await page.locator('#next-time').click();
      assert(await page.locator('#network [data-step-transfer]').count()>0,'positive control: step animation exists before event jump');
      await page.locator('#request-events button').first().click();
      assert.equal(await page.locator('#network [data-step-transfer]').count(),0);
      await page.locator('#play').click();
      await page.locator('#request-events button').first().focus();
      const control=await page.locator('#request-events button').first().elementHandle();
      await page.clock.runFor(100);
      assert.equal(await control.evaluate(e=>e.isConnected),true,'playback preserves control identity for slow pointer gestures');
      assert.equal(await page.locator('#request-events button').first().evaluate(e=>e===document.activeElement),true,'playback rerenders preserve event control focus');
      await page.keyboard.press('Space');
      await assertText(page,'#current-ps',`${groups[0].time} ps`);
      assert.equal(await page.locator('#play').getAttribute('aria-pressed'),'false');
      await page.locator('#play').click();await page.locator('#request-events button').first().focus();
      await page.clock.runFor(100);await page.keyboard.press('Enter');
      await assertText(page,'#current-ps',`${groups[0].time} ps`);
      assert.equal(await page.locator('#play').getAttribute('aria-pressed'),'false');
      await page.setViewportSize({width:390,height:844});
      assert.equal(await page.evaluate(()=>document.documentElement.scrollWidth<=innerWidth),true);
      if(process.env.VIEWER_EVENTS_SCREENSHOT)await page.screenshot({path:process.env.VIEWER_EVENTS_SCREENSHOT,fullPage:true});
      if(name==='schema2.json') {
        await page.locator('#request-rows tr').filter({hasText:'source:1'}).click();
        await assertText(page,'#selected-id','source:1');
        assert.deepEqual(await page.locator('#request-events button').evaluateAll(rows=>rows.map(e=>e.dataset.eventTime)),API.requestEventGroups(API.parseResults(raw),'source:1').map(g=>String(g.time)));
      }
      await page.locator('#clear-selection').click();
      assert.equal(await page.locator('#request-events button').count(),0);
      await page.setViewportSize({width:1440,height:1000});
    }
    const partial=fixture();partial.simulation.partial=true;partial.simulation.end_ps='40';
    Object.assign(partial.simulation.requests[0],{status:'in_flight',eof_ps:null});
    partial.simulation.requests[0].model_fields.release_ps=null;partial.simulation.receivers=[];
    await load(partial,'partial.json');await page.locator('#request-rows tr').first().click();
    assert.deepEqual(await page.locator('#request-events button').evaluateAll(rows=>rows.map(e=>e.dataset.eventTime)),['10','20','30']);
    const empty=fixture();empty.simulation.requests=[];empty.simulation.receivers=[];empty.simulation.records=[];empty.simulation.end_ps='0';
    await load(empty,'empty.json');assert.equal(await page.locator('#request-events button').count(),0);
    const high=fixture(),base=(1n<<64n)-6n,r=high.simulation.requests[0];
    high.simulation.end_ps=((1n<<64n)-1n).toString();high.simulation.receivers=[];high.simulation.records=[];
    for(const [field,delta] of [['generated_ps',0n],['ready_ps',1n],['sof_ps',2n],['eof_ps',3n]])r[field]=String(base+delta);
    Object.assign(r.model_fields,{planned_eof_ps:String(base+3n),release_ps:String(base+4n),planned_release_ps:String(base+4n)});
    await load(high,'adjacent-u64.json');await page.locator('#request-rows tr').first().click();
    for(let delta=0n;delta<5n;delta++) {
      await page.locator(`#request-events [data-event-time="${base+delta}"]`).click();
      await assertText(page,'#current-ps',`${base+delta} ps`);
    }
    await page.setViewportSize({width:390,height:844});
    assert.equal(await page.evaluate(()=>document.documentElement.scrollWidth<=innerWidth),true,'large u64 buttons fit on mobile');
    if(process.env.VIEWER_U64_SCREENSHOT)await page.screenshot({path:process.env.VIEWER_U64_SCREENSHOT,fullPage:true});
    // Public saved products exercise successful cross-profile mounts, rather
    // than merely clearing the controls through the invalid-input path.
    for(const [product,panel] of [['bridge-complete','#ethernet-dashboard'],['axi-complete','#transaction-dashboard']]) {
      await load(fixture(),`before-${product}.json`);await page.locator('#request-rows tr').first().click();
      assert(await page.locator('#request-events button').count()>0);
      await page.locator('#file-input').setInputFiles(path.join(root,`docs/verification/results/v1.1.4-pr-2026-10-08/non-rust-gates/results/verified/${product}/results.json`));
      await page.locator(panel).waitFor({state:'visible'});
      assert.equal(await page.locator('#error-banner').isVisible(),false);
      assert.equal(await page.locator('#request-events button').count(),0);
    }
    await load(fixture(),'before-invalid.json');await page.locator('#request-rows tr').first().click();
    await page.locator('#file-input').setInputFiles({name:'invalid.json',mimeType:'application/json',buffer:Buffer.from('{}')});
    await page.locator('#error-banner').waitFor({state:'visible'});
    assert.equal(await page.locator('#request-events button').count(),0);
    assert.deepEqual(errors,[]);assert.deepEqual(network,[]);
    console.log('PASS recorded-events: schema1/schema2/public CAN, adjacent u64, rewind/manual equivalence, playback 100ms focus/identity and Space/Enter, positive step cancellation, narrow layout, partial/empty, different request, file/profile/invalid reset, no network');
  } finally {await page.close();}
}
async function assertIntervalComparison(browser) {
  const {fixture,rxHoldingFixture}=require('./viewer_fixtures.cjs');
  const page=await browser.newPage({viewport:{width:1440,height:1000}}), errors=[],network=[];
  page.on('pageerror',e=>errors.push(e.message));page.on('request',r=>{if(/^https?:/.test(r.url()))network.push(r.url());});
  const load=async(raw,name)=>{
    await page.locator('#file-input').setInputFiles({name,mimeType:'application/json',buffer:Buffer.from(JSON.stringify(raw))});
    await page.waitForFunction(name=>document.getElementById('filename').textContent===name,name);
  };
  const cells=async(selector)=>page.locator(selector).evaluateAll(rows=>rows.map(row=>[...row.cells].map(cell=>cell.textContent)));
  try {
    await page.goto(pathToFileURL(path.join(root,'crates/dir-simulator/src/tool/viewer/assets/index.html')).href);
    const raw=fixture(),source=JSON.stringify(raw);
    raw.simulation.records.splice(2,0,{time_ps:'20',metric:'queue_length',target:'Main.a.txQueue',value_kind:'integer',value:'2'},
      {time_ps:'20',metric:'queue_length',target:'Main.a.txQueue',value_kind:'integer',value:'1'});
    await load(raw,'comparison-can.json');
    assert.deepEqual((await cells('#comparison-buses tr'))[0].slice(0,5),['Main.bus','70','35.000%','10','0 / 0']);
    const tx=(await cells('#comparison-tx tr'))[0];assert.deepEqual(tx,['Main.a','1','20','10','10','20']);
    await page.locator('#comparison-tx [data-comparison-time="20"]').first().click();await assertText(page,'#current-ps','20 ps');
    const linked=await page.locator('#node-states').innerText();
    await page.locator('#time-unit').selectOption('ps');await page.locator('#jump-time').fill('20');await page.locator('#jump').click();
    assert.equal(await page.locator('#node-states').innerText(),linked);
    await page.locator('#comparison-buses button').click();await assertText(page,'#current-ps','30 ps');await assertText(page,'#selected-id','g:0');
    await page.locator('#comparison-tx button').first().click();await assertText(page,'#current-ps','20 ps');
    const original=await cells('#comparison-buses tr');
    await page.locator('#request-search').fill('no matching requests');assert.deepEqual(await cells('#comparison-buses tr'),original);
    await page.locator('#request-search').fill('');
    await page.locator('#comparison-scope').selectOption('viewport');await page.locator('#zoom-in').click();
    assert.notEqual(await page.locator('#comparison-range').innerText(),'[0, 200) ps / 200 ps');
    await page.locator('#zoom-reset').click();assert.deepEqual(await cells('#comparison-buses tr'),original);
    await page.locator('#comparison-tx-sort').selectOption('id');assert.equal((await cells('#comparison-tx tr'))[0][0],'Main.a');
    const gw=rxHoldingFixture();
    gw.simulation.records=[['queue_length','Main.gw.b.txQueue','0','0'],['queue_length','Main.gw.b.txQueue','250','1'],['queue_length','Main.gw.b.txQueue','260','0'],['gw_rx_queue_length','Main.gw.a.rxQueue','0','0'],['gw_rx_queue_length','Main.gw.a.rxQueue','130','1'],['gw_rx_queue_length','Main.gw.a.rxQueue','500','0']].map(([metric,target,time_ps,value])=>({metric,target,time_ps,value,value_kind:'integer'}));
    await load(gw,'comparison-gateway.json');assert.equal(await page.locator('#comparison-buses tr').count(),3);
    assert.equal(await page.locator('#comparison-rx tr').count(),3);
    assert.equal((await cells('#comparison-rx tr'))[0][3],'370');
    assert.equal((await cells('#comparison-tx tr')).find(row=>row[0]==='Main.gw.b')[3],'10');
    await page.locator('#comparison-bus-sort').selectOption('id');
    assert.deepEqual((await cells('#comparison-buses tr')).map(row=>row[0]),['Main.busA','Main.busB','Main.busC']);
    await page.locator('#comparison-rx-sort').selectOption('longest');
    assert.equal((await cells('#comparison-rx tr'))[0][0],'Main.gw.a');
    await page.locator('#comparison-rx button').first().click();await assertText(page,'#current-ps','130 ps');
    await page.setViewportSize({width:390,height:844});
    assert.equal(await page.evaluate(()=>document.documentElement.scrollWidth<=innerWidth),true);
    if(process.env.VIEWER_COMPARISON_SCREENSHOT)await page.screenshot({path:process.env.VIEWER_COMPARISON_SCREENSHOT,fullPage:true});
    const many=fixture();many.simulation.receivers=[];many.simulation.requests=[];many.simulation.end_ps='6200';
    for(let i=0;i<620;i++)for(const [bus,length] of [['A',5],['B',2]]) {
      const request=structuredClone(fixture().simulation.requests[0]),sof=i*10;
      Object.assign(request,{request_id:`${bus}:${i}`,source:`Node.${bus}`,bus:`Bus.${bus}`,generated_ps:String(sof),ready_ps:String(sof),sof_ps:String(sof),eof_ps:String(sof+length)});
      Object.assign(request.model_fields,{planned_eof_ps:String(sof+length),planned_release_ps:String(sof+length+2),release_ps:String(sof+length+2)});
      many.simulation.requests.push(request);
    }
    await load(many,'many-requests.json');
    const totals=await cells('#comparison-buses tr');
    assert.equal(totals.find(row=>row[0]==='Bus.A')[1],'3100');assert.equal(totals.find(row=>row[0]==='Bus.B')[1],'1240');
    await page.locator('#page-next').click();assert.deepEqual(await cells('#comparison-buses tr'),totals);
    await page.locator('#request-search').fill('B:619');assert.deepEqual(await cells('#comparison-buses tr'),totals);
    const empty=fixture();Object.assign(empty.simulation,{end_ps:'0',requests:[],receivers:[],records:[]});
    await load(empty,'zero-duration.json');assert((await cells('#comparison-buses tr')).every(row=>row[2]==='未定義'));
    assert((await cells('#comparison-tx tr')).every(row=>row[1]==='N/A'));
    const publicPath=path.join(root,'docs/verification/results/acceptance-2026-10-08/output-exports/dir_test_0072_input_a_complete_ledgers_per_receiver_and_conservation-1226012-0/results.json');
    const publicBytes=fs.readFileSync(publicPath);
    const manifestPath=path.join(path.dirname(publicPath),'manifest.json'),manifestBytes=fs.readFileSync(manifestPath);
    await page.locator('#file-input').setInputFiles(publicPath);
    await page.waitForFunction(()=>document.getElementById('filename').textContent==='results.json');
    assert(await page.locator('#comparison-buses tr').count()>0);assert.deepEqual(fs.readFileSync(publicPath),publicBytes);
    assert.deepEqual(fs.readFileSync(manifestPath),manifestBytes);
    await load(fixture(),'reset.json');assert.equal(await page.locator('#comparison-scope').inputValue(),'all');
    assert.equal(JSON.stringify(fixture()),source);
    await page.locator('#file-input').setInputFiles({name:'invalid.json',mimeType:'application/json',buffer:Buffer.from('{}')});
    await page.locator('#error-banner').waitFor({state:'visible'});assert.equal(await page.locator('#comparison-buses tr').count(),0);
    assert.deepEqual(errors,[]);assert.deepEqual(network,[]);
    console.log('PASS interval-comparison: schema1/schema2/public CAN, same-time replay peak, TX ratio, seek/manual/rewind, viewport/sort/search, narrow screen, file/reset, unchanged public input, no network');
  } finally {await page.close();}
}
main().catch(e=>{console.error(e);process.exitCode=1;}).finally(()=>fs.rmSync(temp,{recursive:true,force:true}));
