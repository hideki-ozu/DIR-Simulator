use dir_simulator::registry::{RegisteredSnapshot, Registry};
use serde_json::{Value, json};
use std::{
    path::PathBuf,
    sync::atomic::{AtomicU64, Ordering},
};
static NEXT: AtomicU64 = AtomicU64::new(0);
pub struct Fixture(pub PathBuf);
impl Fixture {
    pub fn new(config: Value, workload: Value, profile: &str, horizon: &str) -> Self {
        let root = std::env::temp_dir().join(format!(
            "dir-can-compound-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir_all(root.join("ned/demo")).unwrap();
        std::fs::write(root.join("ned/demo/compound.ned"),"package demo; simple Compound { parameters: @class(\"dir.can.CompoundProtocolV1\"); } network Demo { submodules: bus: demo.Compound; }").unwrap();
        std::fs::write(root.join("config.json"), config.to_string()).unwrap();
        std::fs::write(root.join("workload.json"), workload.to_string()).unwrap();
        std::fs::write(root.join("compound.ini"),format!("[General]\nnetwork=demo.Demo\nned-path=\"ned\"\nmodel-profile=\"{profile}\"\nmodel-config=\"config.json\"\nworkload=\"workload.json\"\nsim-time-limit={horizon}\n")).unwrap();
        Self(root)
    }
    pub fn registry() -> Registry {
        let mut r = Registry::empty();
        dir_simulator::runtime::can_protocol::register_can_protocol(&mut r).unwrap();
        r
    }
    pub fn prepared(&self) -> dir_simulator::PreparedSimulation {
        dir_simulator::prepare_with_registry(&self.0.join("compound.ini"), Self::registry())
            .unwrap()
    }
    pub fn run(&self) -> RegisteredSnapshot {
        let snapshot = dir_simulator::runtime::simulate(&self.prepared()).unwrap();
        assert!(
            !snapshot.common.partial,
            "{:?}",
            snapshot.common.diagnostics
        );
        snapshot.registered.unwrap()
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}
pub fn bus(id: &str) -> Value {
    json!({"id":id,"nominal_bps":500000,"data_bps":2000000,"nominal_sample":{"num":3,"den":4},"data_sample":{"num":4,"den":5},"controllers":[{"id":"a","capability":"fd_capable"},{"id":"b","capability":"fd_capable"}]})
}
pub fn config() -> Value {
    json!({"schema_version":1,"execution_contract":"ideal-digital-can.v1","standard_edition":"ISO11898-1:2015-wire/Bosch2.0B-cc","buses":[bus("bus")],"max_gateway_hops":4})
}
pub fn cc(id: u32) -> Value {
    json!({"protocol":"cc","id":id,"extended":false,"remote":false,"dlc":2,"data":[18,52]})
}
pub fn fd(id: u32, brs: bool, length: usize) -> Value {
    json!({"protocol":"iso_fd","id":id,"extended":false,"brs":brs,"data":vec![0x5a;length]})
}
pub fn workload(frame: Value) -> Value {
    json!({"schema_version":1,"generators":[{"id":"g","bus":"bus","controller":"a","frame":frame,"schedule":{"kind":"explicit.v1","times_ps":[0]}}]})
}
pub fn rows(snapshot: &RegisteredSnapshot, kind: &str) -> Vec<Value> {
    snapshot
        .model_records
        .values()
        .filter(|r| r.data["kind"] == kind)
        .map(|r| r.data.clone())
        .collect()
}
