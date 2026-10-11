'use strict';
function fixture() {
  return {
    schema_version: 1, run_id: 'test-run',
    metadata: { initial_state: ['Main.a','Main.b','Main.c'].map(instance => ({instance,state:'{"queue":[]}'})).concat([{instance:'Main.bus',state:'{"profile":"can.cc.ideal.v1","state":"idle"}'}]) },
    simulation: {
      start_ps:'0',end_ps:'200',termination:'events_exhausted',partial:false,
      requests: [{ request_id:'g:0',source:'Main.a',bus:'Main.bus',status:'success',generated_ps:'10',ready_ps:'20',sof_ps:'30',eof_ps:'100',
        payload_bits:'8',serialized_bits:'52',model_fields:{profile:'can.cc.ideal.v1',schema_version:1,planned_eof_ps:'100',planned_release_ps:'110',release_ps:'110'}}],
      receivers: [
        {request_id:'g:0',receiver:'Main.b',status:'received',observed_ps:'120',received_ps:'130'},
        {request_id:'g:0',receiver:'Main.c',status:'filtered',observed_ps:'125',received_ps:null}
      ],
      records: [
        {seq:'0',time_ps:'0',metric:'queue_length',target:'Main.a.txQueue',value_kind:'integer',value:'0'},
        {seq:'1',time_ps:'20',metric:'queue_length',target:'Main.a.txQueue',value_kind:'integer',value:'1'},
        {seq:'2',time_ps:'30',metric:'queue_length',target:'Main.a.txQueue',value_kind:'integer',value:'0'}
      ],summary:[]
    }
  };
}

function rxHoldingFixture() {
  const profile='can.cc.multibus.v1';
  const data=[];
  function request(id,source,bus,generated,ready,enqueued,sof,eof,release,parent=null) {
    const model_fields={profile,schema_version:1,crc15:'0',stuff_bits:'0',frame_bits:'52',intermission_bits:'3',bitrate_bps:'500000',planned_eof_ps:eof,planned_release_ps:release,release_ps:release,origin_request_id:parent || id,parent_request_id:parent,gw_hops:parent?'1':'0',tx_enqueued_ps:enqueued};
    const row={request_id:id,source,bus,status:sof===null?(enqueued===null?'waiting_tx':'pending'):'success',generated_ps:generated,ready_ps:ready,sof_ps:sof,eof_ps:eof,payload_bits:'8',serialized_bits:'52',model_fields,attempts:'1',retries:'0',drop_reason:null};
    data.push({schema_name:'can.request',schema_version:1,record_id:id,subject:source,request_id:id,origin_request_id:parent||id,time_ps:release??sof??enqueued??ready,data:row});
  }
  function receive(id,at) {
    data.push({schema_name:'can.receiver',schema_version:1,record_id:`${id}/Main.gw.a`,subject:'Main.gw.a',request_id:id,origin_request_id:id,time_ps:at,data:{request_id:id,receiver:'Main.gw.a',status:'received',observed_ps:at,received_ps:at}});
  }
  request('source:0','Main.source','Main.busA','10','20','20','30','100','110');receive('source:0','130');
  request('source:1','Main.source','Main.busA','120','130','130','140','210','220');receive('source:1','240');
  for(const [port,enqueued,sof,eof,release] of [['b','250','260','300','310'],['c','500','510','550','560']]) {
    const id=`gw:source:0/${port}`;
    request(id,`Main.gw.${port}`,`Main.bus${port.toUpperCase()}`,'150','160',enqueued,sof,eof,release,'source:0');
    data.push({schema_name:'gw.forward',schema_version:1,record_id:id,subject:'Main.gw',request_id:'source:0',origin_request_id:'source:0',time_ps:'150',data:{forward_id:id,parent_request_id:'source:0',origin_request_id:'source:0',gateway:'Main.gw',ingress:'Main.gw.a',egress:`Main.gw.${port}`,route_id:'fanout',gw_hops:'1',received_ps:'130',planned_forward_ps:'150',forwarded_ps:'150',child_request_id:id,status:'submitted',reason:null}});
  }
  for(const [id,at,status,released] of [['source:0','130','released','500'],['source:1','240','dropped',null]]) {
    const buffer_id=`rx:${id}/Main.gw/Main.gw.a`;
    data.push({schema_name:'gw.rx_buffer',schema_version:1,record_id:buffer_id,subject:'Main.gw.a',request_id:id,origin_request_id:id,time_ps:released??at,data:{buffer_id,parent_request_id:id,origin_request_id:id,gateway:'Main.gw',ingress:'Main.gw.a',capacity:'1',received_ps:at,released_ps:released,status,reason:status==='dropped'?'rx_queue_full':null,egress:['Main.gw.b','Main.gw.c']}});
  }
  return {schema_version:2,run_id:'rx-test',metadata:{model_profile:profile,topology:{controllers:[['Main.source','Main.busA'],['Main.gw.a','Main.busA'],['Main.gw.b','Main.busB'],['Main.gw.c','Main.busC'],['Idle.Controller','Main.busC']].map(([id,bus])=>({id,bus,tx_channel_delay_ps:'0',rx_channel_delay_ps:'0'}))},config:[{key:`@profile:${profile}:Main.gw`,value:JSON.stringify({node:'Main.gw',ports:['Main.gw.a','Main.gw.b','Main.gw.c'],rx_queue_capacity:'1',routes:[{id:'fanout',ingress:'Main.gw.a',egress:['Main.gw.b','Main.gw.c']}]})}]},simulation:{start_ps:'0',end_ps:'1000',termination:'events_exhausted',partial:false,model_records:data,records:[],summary:[]}};
}
module.exports = {fixture, rxHoldingFixture};
