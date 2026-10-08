//! Product CAN FD execution compared with independently authored synthetic vectors.
use dir_simulator::{prepare, run};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
static NEXT: AtomicU64 = AtomicU64::new(0);
struct Temp(PathBuf);
impl Temp {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!(
            "dir-canfd-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(&path).unwrap();
        Self(path)
    }
}
impl Drop for Temp {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}
fn fixture() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../docs/verification/fixtures/original-network")
}
fn vectors() -> Value {
    serde_json::from_slice(&fs::read(fixture().join("vectors.json")).unwrap()).unwrap()
}
fn frame(v: &Value) -> Value {
    json!({"format":v["format"],"id":v["id"],"data":v["data"],"brs":v["brs"],"wire":{"nominal_bits":v["nominal_bits"],"data_bits":v["data_bits"],"evidence":v["evidence"],"binding_sha256":v["binding_sha256"]}})
}
fn generator(id: &str, node: &str, frame: Value, times: Value) -> Value {
    json!({"id":id,"node":node,"kind":"can.fd.explicit.v1","times_ps":times,"frame":frame})
}
fn rebind(f: &mut Value, rn: u64, rd: u64) {
    let text = format!(
        "{}|{}|{}|{}|{}|{}|{rn}|{rd}",
        f["format"].as_str().unwrap(),
        f["id"].as_u64().unwrap(),
        f["data"].as_str().unwrap().to_ascii_lowercase(),
        u8::from(f["brs"].as_bool().unwrap()),
        f["wire"]["nominal_bits"].as_u64().unwrap(),
        f["wire"]["data_bits"].as_u64().unwrap()
    );
    f["wire"]["binding_sha256"] = json!(format!("{:x}", Sha256::digest(text.as_bytes())));
}
fn scenario(workload: Value, limit: u64, overrides: &str, rn: u64, rd: u64) -> Temp {
    let temp = Temp::new();
    fs::create_dir_all(temp.0.join("models/fd")).unwrap();
    fs::copy(
        fixture().join("models/fd/Main.ned"),
        temp.0.join("models/fd/Main.ned"),
    )
    .unwrap();
    fs::write(
        temp.0.join("model.json"),
        r#"{"schema_version":1,"profile":"can.fd.precomputed.v1"}"#,
    )
    .unwrap();
    fs::write(temp.0.join("workload.json"), workload.to_string()).unwrap();
    fs::write(temp.0.join("run.ini"),format!("[General]\nnetwork = fd.Main\nned-path = \"models\"\nmodel-profile = \"can.fd.precomputed.v1\"\nmodel-config = \"model.json\"\nworkload = \"workload.json\"\nsim-time-limit = {limit}ps\nMain.bus.nominalBitrate = {rn}bps\nMain.bus.dataBitrate = {rd}bps\n{overrides}")).unwrap();
    temp
}
fn execute(temp: &Temp) -> Value {
    let report = run(
        prepare(&temp.0.join("run.ini")).unwrap(),
        &temp.0.join("results"),
    )
    .unwrap();
    assert_eq!(report.exit_code, 0);
    serde_json::from_slice(&fs::read(temp.0.join("results/results.json")).unwrap()).unwrap()
}
fn rows<'a>(v: &'a Value, schema: &str) -> Vec<&'a Value> {
    v["simulation"]["model_records"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|r| r["schema_name"] == schema)
        .collect()
}
fn metric(v: &Value, target: &str, name: &str) -> u64 {
    v["simulation"]["summary"]
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r["target"] == target && r["metric"] == name)
        .unwrap()["value"]
        .as_str()
        .unwrap()
        .parse()
        .unwrap()
}
fn base() -> Value {
    frame(&vectors()["fd"][0]["input"])
}
fn workload(generators: Vec<Value>) -> Value {
    json!({"schema_version":2,"generators":generators})
}

#[test]
fn external_vectors_match_dlc_combined_ceiling_and_committed_metrics() {
    for vector in vectors()["fd"].as_array().unwrap() {
        let v = &vector["input"];
        let rn = v["nominal_rate"].as_u64().unwrap();
        let rd = v["data_rate"].as_u64().unwrap();
        let temp = scenario(
            workload(vec![generator("g", "Main.a", frame(v), json!(["0"]))]),
            1_000_000_000,
            "",
            rn,
            rd,
        );
        let result = execute(&temp);
        let request = &rows(&result, "dir.canfd.request")[0]["data"];
        assert_eq!(
            request["eof_ps"],
            vector["expected"][0].as_u64().unwrap().to_string(),
            "{}",
            vector["name"]
        );
        assert_eq!(
            request["release_ps"],
            vector["expected"][1].as_u64().unwrap().to_string()
        );
        assert_eq!(
            rows(&result, "dir.canfd.frame")[0]["data"]["dlc"],
            vector["expected"][2]
        );
        assert_eq!(
            rows(&result, "dir.canfd.frame")[0]["request_id"],
            Value::Null
        );
        assert_eq!(
            rows(&result, "dir.canfd.frame")[0]["data"]["wire_validation"],
            "structural-only"
        );
        assert_eq!(metric(&result, "$all", "canfd.generated"), 1);
        assert_eq!(metric(&result, "$all", "canfd.serialized"), 1);
        assert_eq!(metric(&result, "$all", "canfd.received"), 2);
        assert_eq!(
            result["simulation"]["summary"].as_array().unwrap().len(),
            16
        );
    }
}
#[test]
fn strict_stop_eof_arrival_and_processing_boundaries() {
    for (limit, state, receptions, arrival, completed) in [
        (100_000_000, "transmitting", 0, false, false),
        (100_000_001, "serialized", 2, false, false),
        (100_000_010, "serialized", 2, false, false),
        (100_000_011, "serialized", 2, true, false),
        (100_000_020, "serialized", 2, true, false),
        (100_000_021, "serialized", 2, true, true),
    ] {
        let temp = scenario(
            workload(vec![generator("g", "Main.a", base(), json!(["0"]))]),
            limit,
            "[Channel Main::bus.rx_b]\ndelay = 10ps\n[Channel Main::bus.rx_c]\ndelay = 10ps\n",
            500_000,
            2_000_000,
        );
        // Add processing before Channels to maintain INI section semantics.
        let path = temp.0.join("run.ini");
        let ini=fs::read_to_string(&path).unwrap().replace("[Channel Main::bus.rx_b]","Main.b.rxProcessingDelay = 10ps\nMain.c.rxProcessingDelay = 10ps\n[Channel Main::bus.rx_b]");
        fs::write(path, ini).unwrap();
        let result = execute(&temp);
        let request = &rows(&result, "dir.canfd.request")[0]["data"];
        assert_eq!(request["state"], state);
        assert_eq!(request["release_ps"], Value::Null);
        let rows = rows(&result, "dir.canfd.reception");
        assert_eq!(rows.len(), receptions);
        for row in rows {
            assert_eq!(!row["data"]["arrival_ps"].is_null(), arrival);
            assert_eq!(!row["data"]["completed_ps"].is_null(), completed);
        }
        assert_eq!(
            metric(&result, "$all", "canfd.received"),
            if completed { 2 } else { 0 }
        );
    }
}
#[test]
fn arbitration_standard_vs_extended_and_capacity_generation_order() {
    let mut ext = base();
    ext["format"] = json!("extended");
    ext["id"] = json!(1u32 << 18);
    rebind(&mut ext, 500_000, 2_000_000);
    let mut std = base();
    std["id"] = json!(1);
    rebind(&mut std, 500_000, 2_000_000);
    let temp = scenario(
        workload(vec![
            generator("a", "Main.a", ext, json!(["0", "0"])),
            generator("b", "Main.b", std, json!(["0"])),
        ]),
        300_000_000,
        "Main.a.queueCapacity = 1\n",
        500_000,
        2_000_000,
    );
    let result = execute(&temp);
    let requests = rows(&result, "dir.canfd.request");
    assert_eq!(
        requests.iter().find(|r| r["record_id"] == "a:1").unwrap()["data"]["state"],
        "dropped"
    );
    assert_eq!(
        requests.iter().find(|r| r["record_id"] == "b:0").unwrap()["data"]["sof_ps"],
        "0"
    );
    assert_eq!(
        requests.iter().find(|r| r["record_id"] == "a:0").unwrap()["data"]["sof_ps"],
        "106000000"
    );
    assert_eq!(metric(&result, "Main.a", "canfd.dropped"), 1);
    assert_eq!(metric(&result, "$all", "canfd.received"), 4);
}
#[test]
fn filter_rejection_keeps_planned_completion_and_no_success_count() {
    let temp = scenario(
        workload(vec![generator("g", "Main.a", base(), json!(["0"]))]),
        200_000_000,
        "Main.b.rxFilter = \"none\"\nMain.b.rxProcessingDelay = 5ps\nMain.c.rxFilter = \"std:0x000\"\n",
        500_000,
        2_000_000,
    );
    let result = execute(&temp);
    let receptions = rows(&result, "dir.canfd.reception");
    let filtered = receptions
        .iter()
        .find(|r| r["subject"] == "Main.b")
        .unwrap();
    assert_eq!(filtered["data"]["state"], "filtered");
    assert_eq!(filtered["data"]["arrival_ps"], "100000000");
    assert_eq!(filtered["data"]["completed_ps"], Value::Null);
    assert_eq!(filtered["data"]["planned_completed_ps"], "100000005");
    assert_eq!(metric(&result, "$all", "canfd.serialized"), 1);
    assert_eq!(metric(&result, "$all", "canfd.received"), 1);
    assert_eq!(metric(&result, "$all", "canfd.dropped"), 0);
}
#[test]
fn frame_binding_schema_and_generator_errors_fail_before_execution() {
    let mut bad = Vec::new();
    for key in ["format", "id", "data", "brs", "wire"] {
        let mut f = base();
        f.as_object_mut().unwrap().remove(key);
        bad.push(f);
    }
    for (key, value) in [
        ("data", json!("000000000000000000")),
        ("id", json!(2048)),
        ("remote", json!(true)),
        ("data", json!("ffffffff")),
    ] {
        let mut f = base();
        f[key] = value;
        bad.push(f);
    }
    for (key, value) in [
        ("nominal_bits", json!(0)),
        ("data_bits", json!(31)),
        ("evidence", json!("")),
        ("binding_sha256", json!("0".repeat(64))),
        ("nominal_bits", json!(30.0)),
        ("extra", json!(1)),
    ] {
        let mut f = base();
        f["wire"][key] = value;
        bad.push(f);
    }
    for f in bad {
        let temp = scenario(
            workload(vec![generator("g", "Main.a", f, json!(["0"]))]),
            1_000_000_000,
            "",
            500_000,
            2_000_000,
        );
        let error = prepare(&temp.0.join("run.ini")).unwrap_err();
        assert_eq!(error.code, "E-0001");
        assert!(error.details.is_some());
    }
    for times in [
        json!(["00"]),
        json!(["2", "1"]),
        json!([0]),
        json!(["18446744073709551616"]),
    ] {
        let temp = scenario(
            workload(vec![generator("g", "Main.a", base(), times)]),
            1_000_000_000,
            "",
            500_000,
            2_000_000,
        );
        assert!(prepare(&temp.0.join("run.ini")).is_err());
    }
    let temp = scenario(
        workload(vec![generator("g", "Main.a", base(), json!(["0"]))]),
        1_000_000_000,
        "",
        500_000,
        2_000_000,
    );
    let text = fs::read_to_string(temp.0.join("workload.json"))
        .unwrap()
        .replace("\"brs\":true", "\"brs\":true,\"brs\":true");
    fs::write(temp.0.join("workload.json"), text).unwrap();
    assert!(prepare(&temp.0.join("run.ini")).is_err());
    let temp = scenario(
        workload(vec![
            generator("a", "Main.a", base(), json!([])),
            generator("b", "Main.b", base(), json!([])),
        ]),
        1_000_000_000,
        "",
        500_000,
        2_000_000,
    );
    assert!(prepare(&temp.0.join("run.ini")).is_err());
}
#[test]
fn uppercase_payload_normalizes_and_rebinding_preserves_structural_boundary() {
    let mut f = base();
    f["data"] = json!("ABCDEF00");
    rebind(&mut f, 500_000, 2_000_000);
    let temp = scenario(
        workload(vec![generator("g", "Main.a", f, json!([]))]),
        0,
        "",
        500_000,
        2_000_000,
    );
    let result = execute(&temp);
    assert_eq!(
        rows(&result, "dir.canfd.frame")[0]["data"]["data"],
        "abcdef00"
    );
    assert!(rows(&result, "dir.canfd.request").is_empty());
    assert_eq!(metric(&result, "$all", "canfd.generated"), 0);
}
#[test]
fn overflow_preserves_previous_commit_and_does_not_fabricate_sof() {
    let temp = scenario(
        workload(vec![generator(
            "g",
            "Main.a",
            base(),
            json!(["18446744073709551600"]),
        )]),
        u64::MAX,
        "",
        500_000,
        2_000_000,
    );
    let report = run(
        prepare(&temp.0.join("run.ini")).unwrap(),
        &temp.0.join("results"),
    )
    .unwrap();
    assert_ne!(report.exit_code, 0);
    let result: Value =
        serde_json::from_slice(&fs::read(temp.0.join("results/results.json")).unwrap()).unwrap();
    let request = &rows(&result, "dir.canfd.request")[0]["data"];
    assert_eq!(request["state"], "queued");
    assert_eq!(request["sof_ps"], Value::Null);
    assert_eq!(metric(&result, "$all", "canfd.generated"), 1);
    assert_eq!(metric(&result, "$all", "canfd.serialized"), 0);
}

#[test]
fn rates_filters_capacities_and_cross_protocol_types_are_strict() {
    for (rn, rd, overrides) in [
        (0, 2_000_000, ""),
        (1_000_001, 2_000_000, ""),
        (500_000, 499_999, ""),
        (500_000, 8_000_001, ""),
        (500_000, 2_000_000, "Main.a.queueCapacity = -1\n"),
        (500_000, 2_000_000, "Main.a.queueCapacity = 4294967296\n"),
        (500_000, 2_000_000, "Main.b.rxFilter = \"std:0x800\"\n"),
        (
            500_000,
            2_000_000,
            "Main.b.rxFilter = \"std:0x1,std:0x01\"\n",
        ),
    ] {
        let temp = scenario(workload(vec![]), 1, overrides, rn, rd);
        let error = prepare(&temp.0.join("run.ini")).unwrap_err();
        assert_eq!(error.code, "E-0001");
        assert!(error.details.is_some());
    }
    let temp = scenario(workload(vec![]), 1, "", 500_000, 2_000_000);
    let path = temp.0.join("models/fd/Main.ned");
    let text = fs::read_to_string(&path)
        .unwrap()
        .replace("dir.canfd.ControllerV1", "dir.can.Controller");
    fs::write(path, text).unwrap();
    assert!(prepare(&temp.0.join("run.ini")).is_err());
}

#[test]
fn zero_capacity_drops_ready_and_exact_limit_generation_never_commits() {
    let temp = scenario(
        workload(vec![generator("g", "Main.a", base(), json!(["0", "1"]))]),
        1,
        "Main.a.queueCapacity = 0\n",
        500_000,
        2_000_000,
    );
    let result = execute(&temp);
    let requests = rows(&result, "dir.canfd.request");
    assert_eq!(requests.len(), 1);
    assert_eq!(requests[0]["data"]["state"], "dropped");
    assert_eq!(requests[0]["data"]["ready_ps"], "0");
    assert_eq!(requests[0]["data"]["sof_ps"], Value::Null);
    assert_eq!(metric(&result, "$all", "canfd.generated"), 1);
    assert_eq!(metric(&result, "$all", "canfd.dropped"), 1);
    assert_eq!(metric(&result, "$all", "canfd.serialized"), 0);
}
