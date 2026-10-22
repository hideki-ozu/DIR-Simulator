const test=require('node:test');
const assert=require('node:assert/strict');
const M=require('../crates/dir-simulator/src/tool/viewer/assets/controlled-model.js');

const event=(time,kind,details)=>({time:BigInt(time),kind:'dir.network.gptp',data:{kind,details}});

test('gPTP Viewer follows packet correction, holdover and re-selection in replay order',()=>{
  const history=[
    event(1,'selection',{node:'Net.b',grandmaster:'0000000000000001',parent:'Net.b.tx_a',synchronized:false}),
    event(2,'clock_corrected',{node:'Net.b',grandmaster:'0000000000000001',sync_transfer_id:'gptp:sync@Net.a.tx'}),
    event(3,'holdover',{node:'Net.b'}),
    event(4,'selection',{node:'Net.b',grandmaster:'0000000000000002',parent:null,synchronized:true}),
  ];
  const at=time=>M.gptpClocks(history.filter(e=>e.time<=BigInt(time)))[0];
  assert.equal(at(1).status,'unsynchronized');
  assert.equal(at(2).status,'synchronized');
  assert.equal(at(2).sync_transfer_id,'gptp:sync@Net.a.tx');
  assert.equal(at(3).status,'holdover');
  assert.equal(at(4).status,'grandmaster');
  assert.equal(at(4).grandmaster,'0000000000000002');
});
