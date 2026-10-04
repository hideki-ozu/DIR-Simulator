/* Offline transaction dashboard. Result strings enter the DOM through textContent. */
'use strict';
(() => {
  const M=window.DIRTransactionModel,T=window.DIRViewerModel;
  let root=null,model=null,current=0n,selected=null,playing=false,animation=0,step=null;
  const $=id=>document.getElementById(`txn-${id}`);
  const node=(tag,text,cls)=>{const n=document.createElement(tag);if(text!==undefined)n.textContent=String(text);if(cls)n.className=cls;return n;};
  const svg=(tag,attrs,text)=>{const n=document.createElementNS('http://www.w3.org/2000/svg',tag);for(const[k,v]of Object.entries(attrs))n.setAttribute(k,String(v));if(text!==undefined)n.textContent=String(text);return n;};
  const labels={not_generated:'生成前',pending:'待機',queued:'待機',awaiting_admission:'受理待ち',active:'処理中',setup:'DMA準備',reading:'DMA読出し',writing:'DMA書込み',notifying:'通知待ち',completed:'完了',dropped:'破棄',rejected:'拒否',failed:'失敗'};
  function pause(){cancelAnimationFrame(animation);animation=0;playing=false;step=null;if($('play'))$('play').textContent='▶ 再生';}
  function unmount(){pause();if(root)root.hidden=true;model=null;}
  function mount(raw,filename){
    const parsed=M.parseResults(raw);unmount();model=parsed;current=model.start;selected=null;
    if(!root){root=node('section');root.id='transaction-dashboard';document.querySelector('main').append(root);}root.hidden=false;
    root.innerHTML=`<section class="run-heading"><div><p class="eyebrow">TRANSACTION / MEMORY / IPC</p><h1 id="txn-title"></h1><p id="txn-file"></p></div><span id="txn-termination" class="badge"></span></section>
      <section id="txn-counts" class="summary-grid" aria-label="現在の要求状態"></section>
      <section class="panel eth-controls"><button id="txn-prev" class="button" aria-label="前のイベント時刻">│◀</button><button id="txn-play" class="button primary">▶ 再生</button><button id="txn-next" class="button" aria-label="次のイベント時刻">▶│</button><strong id="txn-time"></strong><label>時刻（ps） <input id="txn-jump-value" inputmode="numeric" value="0"></label><button id="txn-jump" class="button">移動</button><p id="txn-error" role="alert" hidden></p><input id="txn-slider" class="time-slider" type="range" min="0" max="10000" value="0" aria-label="要求の観測時刻"><small>ステップは移動先の直前区間を表示します。連続再生は転送の線を強調します。</small></section>
      <section class="panel network-panel"><div class="panel-heading"><div><h2>要求と転送の経路</h2><p>要求元と対象、記録されたポート間転送を表示します。</p></div></div><div class="network-wrap"><svg id="txn-network" role="group" aria-label="要求と転送の経路"></svg></div></section>
      <div class="analysis-grid"><section class="panel"><div class="panel-heading"><h2>要求タイムライン</h2></div><div class="timeline-wrap"><svg id="txn-timeline" role="group" aria-label="要求の待機と実行"></svg></div><p class="panel-note">黄: 待機、青: 実行、赤: 破棄・拒否。未完了の実行は観測終端まで破線で表示します。</p></section><aside class="panel inspector"><div class="panel-heading"><h2>選択した要求・転送</h2></div><div id="txn-details"></div></aside></div>
      <section class="panel request-panel"><div class="panel-heading"><h2>要求一覧</h2><input id="txn-search" type="search" aria-label="要求を検索" placeholder="要求ID / 資源 / 操作で検索"></div><div class="table-scroll"><table><thead><tr><th>要求ID</th><th>要求元</th><th>操作</th><th>状態</th><th>生成（ps）</th><th>開始（ps）</th><th>完了（ps）</th></tr></thead><tbody id="txn-rows"></tbody></table></div><p id="txn-row-note" class="panel-note"></p></section>
      <section class="panel"><div class="panel-heading"><h2>実行全体の集計</h2></div><div class="table-scroll"><table><thead><tr><th>対象</th><th>指標</th><th>値</th><th>単位</th></tr></thead><tbody id="txn-summary"></tbody></table></div></section>
      <section class="panel"><div class="panel-heading"><div><h2>実行終了時の資源</h2><p>メモリ内容、slot所有者、通知などの最終確定状態です。</p></div></div><div id="txn-resources"></div></section>`;
    $('title').textContent={'axi4.transaction.v1':'AXIトランザクション','soc.shared.v1':'SoC共有バス','ahb.transaction.v1':'AHBトランザクション','noc.xy.v1':'NoC XY mesh','memory.ipc.transaction.v1':'メモリ・IPC'}[model.profile];
    $('file').textContent=`${filename} · ${model.profile}`;$('termination').textContent=`${raw.simulation.termination}${raw.simulation.partial?' · 部分結果':''}`;
    $('prev').onclick=()=>eventStep(-1);$('next').onclick=()=>eventStep(1);$('play').onclick=()=>playing?pause():play();
    $('slider').oninput=()=>{pause();current=T.timeFromFraction(model.start,model.end,Number($('slider').value));render();};
    $('jump').onclick=()=>{pause();try{const t=T.parseTime($('jump-value').value,'ps');if(t>model.end)throw new Error('観測期間内の時刻を指定してください');current=t;$('error').hidden=true;render();}catch(error){$('error').textContent=error.message;$('error').hidden=false;}};
    $('jump-value').onkeydown=e=>{if(e.key==='Enter')$('jump').click();};$('search').oninput=render;
    for(const r of raw.simulation.summary||[]){const tr=node('tr');for(const value of [r.target,r.metric,r.value??'—',r.unit])tr.append(node('td',value));$('summary').append(tr);}
    for(const r of model.resources){const item=node('details');item.append(node('summary',`${r.subject} · ${r.raw.schema_name} · 更新 ${r.updated} ps`),node('pre',JSON.stringify(r.data,null,2)));$('resources').append(item);}
    root.onkeydown=e=>{if(['INPUT','BUTTON'].includes(e.target.tagName))return;if(e.key==='ArrowLeft'){e.preventDefault();eventStep(-1);}if(e.key==='ArrowRight'){e.preventDefault();eventStep(1);}};render();
  }
  function selectable(element,id){element.tabIndex=0;element.setAttribute('role','button');element.onclick=()=>{selected=id;render();};element.onkeydown=e=>{if(e.key==='Enter'||e.key===' '){e.preventDefault();selected=id;render();}};}
  function eventStep(direction){pause();let i=model.eventTimes.findIndex(t=>t>=current);if(i<0)i=model.eventTimes.length;if(direction>0&&model.eventTimes[i]===current)i++;if(direction<0)i--;if(i<0||i>=model.eventTimes.length)return;current=model.eventTimes[i];step={items:M.stepTransfers(model,current),started:performance.now()};render();const tick=now=>{if(!step||!model)return;network(Math.min(1,(now-step.started)/700));if(now-step.started<700)animation=requestAnimationFrame(tick);else{step=null;network();}};animation=requestAnimationFrame(tick);}
  function play(){pause();if(current>=model.end)current=model.start;playing=true;$('play').textContent='❚❚ 停止';const anchor=current,wall=performance.now();const tick=now=>{if(!playing||!model)return;current=anchor+T.timeFromFraction(0n,model.end-model.start,Math.min(1000000,Math.floor((now-wall)/8000*1000000)),1000000);if(current>=model.end){current=model.end;pause();}render();if(playing)animation=requestAnimationFrame(tick);};animation=requestAnimationFrame(tick);}
  function network(progress=0){
    const graph=$('network');graph.replaceChildren();const ids=new Set(),edges=new Map();
    const add=(from,to)=>{if(typeof from!=='string'||typeof to!=='string')return;ids.add(from);ids.add(to);edges.set(JSON.stringify([from,to]),[from,to]);};
    for(const r of model.requests.values())add(r.source,r.target);for(const t of model.transfers)add(t.from,t.to);
    for(const r of model.resources)ids.add(r.subject);
    const list=[...ids].sort().slice(0,80),width=Math.max(760,Math.min(6,list.length)*160),height=Math.max(140,Math.ceil(list.length/6)*110+40),positions=new Map();
    graph.setAttribute('viewBox',`0 0 ${width} ${height}`);graph.style.minWidth=`${width}px`;
    list.forEach((id,i)=>positions.set(id,[90+(i%6)*160,65+Math.floor(i/6)*110]));
    const defs=svg('defs',{}),marker=svg('marker',{id:'txn-arrow',viewBox:'0 0 10 10',refX:9,refY:5,markerWidth:6,markerHeight:6,orient:'auto'});marker.append(svg('path',{d:'M0 0 L10 5 L0 10 z',fill:'context-stroke'}));defs.append(marker);graph.append(defs);
    for(const[from,to]of edges.values()){
      const a=positions.get(from),b=positions.get(to);if(!a||!b||from===to)continue;
      const active=model.transfers.some(t=>t.from===from&&t.to===to&&t.start<=current&&current<t.end),line=svg('line',{x1:a[0],y1:a[1],x2:b[0],y2:b[1],stroke:active?'#2563eb':'#b9c6d4','stroke-width':active?4:2,'marker-end':'url(#txn-arrow)','data-direction':JSON.stringify([from,to])});graph.append(line);
    }
    for(const id of list){const[x,y]=positions.get(id),g=svg('g',{'data-node':id});g.append(svg('rect',{x:x-68,y:y-22,width:136,height:44,rx:7,fill:'#f1f5fa',stroke:'#9fb2c7'}),svg('text',{x,y:y+5,'text-anchor':'middle','font-size':10},id));graph.append(g);}
    if(step)for(const t of step.items){const a=positions.get(t.from),b=positions.get(t.to);if(!a||!b)continue;graph.append(svg('circle',{cx:a[0]+(b[0]-a[0])*progress,cy:a[1]+(b[1]-a[1])*progress,r:6,fill:'#187f88','data-transfer':t.id}));}
  }
  function timeline(){
    const graph=$('timeline');graph.replaceChildren();const rows=[...model.requests.values()].slice(0,500),width=1100,left=230,height=Math.max(80,rows.length*26+45),x=t=>left+T.fraction(t,model.start,model.end)*(width-left-25);graph.setAttribute('viewBox',`0 0 ${width} ${height}`);graph.style.minWidth=`${width}px`;
    for(let i=0;i<rows.length;i++){const r=rows[i],y=i*26+30,g=svg('g',{'data-request':r.id});selectable(g,r.id);g.append(svg('text',{x:6,y:y+5,'font-size':10},r.id));const draw=(a,b,color,dashed)=>g.append(svg('rect',{x:x(a),y:y-7,width:Math.max(2,x(b)-x(a)),height:14,fill:color,...(dashed?{'fill-opacity':.4,stroke:color,'stroke-dasharray':'4 3'}:{})}));if(r.start===null)draw(r.generated,r.state==='dropped'?r.generated:model.end,r.state==='dropped'?'#c54a65':'#bd8c2d',false);else{draw(r.generated,r.start,'#bd8c2d',false);draw(r.start,r.completed??model.end,'#2563eb',r.completed===null);}graph.append(g);}
    graph.append(svg('line',{x1:x(current),y1:0,x2:x(current),y2:height,stroke:'#be3850','stroke-width':1.5}));
  }
  function render(){if(!model)return;const counts=M.stateAt(model,current);$('time').textContent=`${T.formatTime(current,'us')} µs · ${current} ps`;$('slider').value=String(Math.round(T.fraction(current,model.start,model.end)*10000));$('jump-value').value=current.toString();$('prev').disabled=current<=model.start;$('next').disabled=current>=model.end;
    $('counts').replaceChildren();for(const[key,label]of [['generated','生成'],['pending','待機'],['active','処理中'],['completed','完了'],['dropped','破棄・拒否']]){const card=node('article',undefined,'summary-card');card.append(node('span',label),node('strong',counts[key]));$('counts').append(card);}
    const query=$('search').value.toLowerCase(),rows=[...model.requests.values()].filter(r=>`${r.id} ${r.subject} ${r.data.operation??r.data.op}`.toLowerCase().includes(query));$('rows').replaceChildren();for(const r of rows.slice(0,500)){const tr=node('tr');selectable(tr,r.id);tr.dataset.request=r.id;for(const text of [r.id,r.subject,r.data.operation??r.data.op??'packet',labels[M.requestStateAt(r,current)],r.generated,r.start??'—',r.completed??'—'])tr.append(node('td',text));$('rows').append(tr);}$('row-note').textContent=`全 ${rows.length} 件${rows.length>500?'（先頭500件。検索で絞り込み）':''}`;
    $('details').replaceChildren();if(selected){const r=model.requests.get(selected);$('details').append(node('strong',r.id),node('p',labels[M.requestStateAt(r,current)]),node('p','以下は実行全体の最終記録です。'),node('pre',JSON.stringify(r.raw,null,2)));for(const t of model.transfers.filter(t=>t.request===selected)){const detail=node('details');detail.append(node('summary',`${t.id} · ${t.end} ps`),node('pre',JSON.stringify(t.raw.data,null,2)));$('details').append(detail);}}else $('details').append(node('p','一覧またはタイムラインから要求を選択してください。'));
    network();timeline();
  }
  document.addEventListener('visibilitychange',()=>{if(document.hidden)pause();});window.DIRTransactionApp={mount,unmount};
})();
