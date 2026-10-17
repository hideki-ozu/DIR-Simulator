/* Read-only prefix replay; all input text is inserted with textContent. */
(function(){'use strict';let panel=null;const M=window.DIRControlledModel;const node=(tag,text,cls)=>{const n=document.createElement(tag);if(text!==undefined)n.textContent=String(text);if(cls)n.className=cls;return n;};
function unmount(){panel?.remove();panel=null;}
function table(headers,rows){const box=node('div',undefined,'table-scroll'),t=node('table'),head=node('thead'),tr=node('tr');for(const h of headers)tr.append(node('th',h));head.append(tr);const body=node('tbody');for(const row of rows){const tr=node('tr');for(const v of row)tr.append(node('td',v??'—'));body.append(tr);}t.append(head,body);box.append(t);return box;}
function mount(raw,filename){const model=M.parseResults(raw);unmount();panel=node('section',undefined,'panel');panel.id='controlled-dashboard';panel.style.margin='24px';
panel.append(node('h1',model.extension?'転送・制御と計測結果':'実行状態と計測結果'),node('p',`${filename} · ${model.metadata.model_profile}`));
const control=model.control;panel.append(node('p',`保存時の状態: ${control?.state??model.simulation.termination} · 成功イベント: ${model.committed} · 観測終端: ${model.end} ps · 再開: ${control?.resume_supported?'可能（元session又はcheckpointが必要）':'対象外'}`));
panel.append(node('p','この画面は保存された成功記録を再生します。再生位置を変えても、シミュレーションの実行状態は変わりません。'));
const controls=node('div',undefined,'transport'),previous=node('button','前の記録','button'),next=node('button','次の記録','button'),label=node('strong'),slider=node('input');slider.type='range';slider.min='0';slider.max='10000';slider.value='10000';slider.setAttribute('aria-label','記録の再生時刻');controls.append(previous,next,label);panel.append(controls,slider);
const details=node('details'),title=node('summary','保存時点の集計');details.append(title,table(['指標','対象','集計方法','値','単位','標本数','窓開始 ps','窓終了 ps'],model.summary.map(r=>[r.metric,r.target,r.aggregation??'—',r.value,r.unit,r.sample_count,r.start_ps,r.end_ps])));panel.append(details);
const content=node('div');panel.append(content);const times=M.times(model);let current=model.end;
function render(time){current=time;label.textContent=`${time} ps / ${model.end} ps`;slider.value=model.end===0n?'0':String(Number(time*10000n/model.end));const state=M.at(model,time);content.replaceChildren(node('h2','この時刻までの計測値'),table(['指標','対象','値','単位','観測時刻 ps'],state.metrics.map(p=>[p.metric,p.target,p.value,p.unit,p.initial?'初期値':p.time_ps])),node('h2',`確定記録 ${state.events.length}件`));if(model.extension){
 const branches=state.events.filter(e=>e.kind==='dir.redundancy.branch').map(e=>[e.data.stream_id,e.data.path_id,e.data.branch_id,e.data.status,e.data.first_sof_ps,e.data.completed_ps,e.data.reason]);
 if(branches.length)content.append(node('h2','冗長配送の枝'),table(['stream','経路','枝ID','状態','初回送信 ps','完了 ps','理由'],branches));
 const fragments=state.events.filter(e=>e.kind==='dir.preemption.fragment'||e.kind==='dir.preemption.gap').map(e=>[e.kind.endsWith('gap')?'再開待ち':'fragment',e.data.port,e.data.transfer_id,e.data.offset_bytes,e.data.payload_bytes,e.data.start_ps,e.data.end_ps,e.data.status]);
 if(fragments.length)content.append(node('h2','分割送信と再開待ち'),table(['種類','port','転送','offset byte','MAC byte','開始 ps','実完了 ps','状態'],fragments));
 const policy=state.events.filter(e=>e.kind==='ethernet.control.wire.policy').slice(-1)[0];
 if(policy)content.append(node('h2','port・VLAN・MACの状態'),node('pre',JSON.stringify(policy.data,null,2)));
 }
 const visible=state.events.slice(-500);if(state.events.length>500)content.append(node('p','直近500件を表示しています。全記録は結果JSONを参照してください。'));for(const e of visible){const d=node('details');d.append(node('summary',`${e.time} ps · ${e.kind} · ${e.id}`),node('pre',JSON.stringify(e.data,null,2)));content.append(d);}}
slider.addEventListener('input',()=>render(model.end*BigInt(slider.value)/10000n));previous.addEventListener('click',()=>render(times.filter(t=>t<current).pop()??0n));next.addEventListener('click',()=>render(times.find(t=>t>current)??model.end));document.querySelector('main').append(panel);render(current);}
window.DIRControlledApp={mount,unmount};})();
