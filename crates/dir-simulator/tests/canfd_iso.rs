//! Public input/run/output path checked against independently generated wire data.
use dir_simulator::{prepare, run};
use serde_json::{Value, json};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::{fmt::Write as _, fs};

static NEXT: AtomicU64 = AtomicU64::new(0);
struct Temp(PathBuf);
impl Temp {
    fn new(workload: &Value, limit: u64) -> Self {
        let dir = std::env::temp_dir().join(format!(
            "dir-canfd-iso-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(dir.join("models/fd")).unwrap();
        let source = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../examples/canfd");
        fs::copy(
            source.join("models/fd/Main.ned"),
            dir.join("models/fd/Main.ned"),
        )
        .unwrap();
        fs::write(
            dir.join("model.json"),
            r#"{"schema_version":1,"profile":"can.fd.iso.v1"}"#,
        )
        .unwrap();
        fs::write(dir.join("workload.json"), workload.to_string()).unwrap();
        fs::write(dir.join("run.ini"), format!("[General]\nnetwork = fd.Main\nned-path = \"models\"\nmodel-profile = \"can.fd.iso.v1\"\nmodel-config = \"model.json\"\nworkload = \"workload.json\"\nsim-time-limit = {limit}ps\nMain.bus.profile = \"can.fd.iso.v1\"\nMain.bus.nominalBitrate = 333333bps\nMain.bus.dataBitrate = 777777bps\nMain.a.queueCapacity = 256\n")).unwrap();
        Self(dir)
    }
    fn execute(&self) -> Value {
        let report = run(
            prepare(&self.0.join("run.ini")).unwrap(),
            &self.0.join("out"),
        )
        .unwrap();
        assert_eq!(report.exit_code, 0);
        serde_json::from_slice(&fs::read(self.0.join("out/results.json")).unwrap()).unwrap()
    }
}
impl Drop for Temp {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}
fn vectors() -> Vec<Value> {
    serde_json::from_str(include_str!("fixtures/canfd_iso_wire_vectors.json")).unwrap()
}
fn generator(v: &Value, index: usize) -> Value {
    let data: String =
        v["data"]
            .as_array()
            .unwrap()
            .iter()
            .fold(String::new(), |mut data, byte| {
                write!(&mut data, "{:02x}", byte.as_u64().unwrap()).unwrap();
                data
            });
    json!({"id":format!("g{index:03}"),"kind":"can.fd.iso.explicit.v1","node":"Main.a","times_ps":["0"],
        "frame":{"format":if v["extended"]==true {"extended"} else {"standard"},"id":v["id"],"data":data,"brs":v["brs"],"esi":v["esi"]}})
}
fn workload(rows: Vec<Value>) -> Value {
    json!({"schema_version":2,"generators":rows})
}
fn rows<'a>(result: &'a Value, schema: &str) -> Vec<&'a Value> {
    result["simulation"]["model_records"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|r| r["schema_name"] == schema)
        .collect()
}

#[test]
fn independent_full_wire_vectors_drive_public_runtime_and_metadata() {
    let vectors = vectors();
    let temp = Temp::new(
        &workload(
            vectors
                .iter()
                .enumerate()
                .map(|(i, v)| generator(v, i))
                .collect(),
        ),
        500_000_000_000,
    );
    let result = temp.execute();
    assert_eq!(result["metadata"]["model_profile"], "can.fd.iso.v1");
    assert_eq!(result["metadata"]["wire_validation"], "crc-and-stuffing");
    let frames = rows(&result, "dir.canfd.frame");
    let requests = rows(&result, "dir.canfd.request");
    assert_eq!(frames.len(), 128);
    assert_eq!(requests.len(), 128);
    for (index, vector) in vectors.iter().enumerate() {
        let id = format!("g{index:03}");
        let frame = frames.iter().find(|r| r["record_id"] == id).unwrap();
        assert_eq!(frame["schema_version"], 2);
        assert_eq!(frame["data"]["wire_bits"], vector["bits"]);
        assert_eq!(frame["data"]["crc"], vector["crc"]);
        assert_eq!(
            frame["data"]["nominal_bits"],
            vector["nominal_bits"].as_u64().unwrap().to_string()
        );
        assert_eq!(
            frame["data"]["data_bits"],
            vector["data_bits"].as_u64().unwrap().to_string()
        );
        assert!(frame["data"].get("evidence").is_none());
        let request = requests
            .iter()
            .find(|r| r["data"]["frame_id"] == id)
            .unwrap();
        let time = |field: &str| {
            request["data"][field]
                .as_str()
                .unwrap()
                .parse::<u128>()
                .unwrap()
        };
        let n = vector["nominal_bits"].as_u64().unwrap() as u128;
        let d = vector["data_bits"].as_u64().unwrap() as u128;
        let numerator = 1_000_000_000_000u128 * (n * 777777 + d * 333333);
        let expected = numerator.div_ceil(333333 * 777777);
        assert_eq!(time("eof_ps") - time("sof_ps"), expected);
        assert_eq!(request["data"]["state"], "serialized");
    }
    assert_eq!(rows(&result, "dir.canfd.reception").len(), 256);
}

#[test]
fn horizon_during_transmission_has_no_actual_eof_or_receiver() {
    let vector = &vectors()[5];
    let temp = Temp::new(&workload(vec![generator(vector, 0)]), 10_000_000);
    let result = temp.execute();
    let request = &rows(&result, "dir.canfd.request")[0]["data"];
    assert_eq!(request["state"], "transmitting");
    assert_eq!(request["sof_ps"], "0");
    assert!(request["eof_ps"].is_null());
    assert!(request["planned_eof_ps"].as_str().is_some());
    assert!(rows(&result, "dir.canfd.reception").is_empty());
    let zero = Temp::new(&workload(vec![generator(vector, 0)]), 0).execute();
    assert!(rows(&zero, "dir.canfd.request").is_empty());
}

#[test]
fn iso_requires_esi_and_rejects_external_wire_and_wrong_bus_profile() {
    let original = generator(&vectors()[0], 0);
    for field in ["esi", "brs"] {
        let mut row = original.clone();
        row["frame"].as_object_mut().unwrap().remove(field);
        let temp = Temp::new(&workload(vec![row]), 1);
        assert!(prepare(&temp.0.join("run.ini")).is_err());
    }
    let mut row = original.clone();
    row["frame"]["wire"] = json!({"nominal_bits":1});
    let temp = Temp::new(&workload(vec![row]), 1);
    assert!(prepare(&temp.0.join("run.ini")).is_err());
    let temp = Temp::new(&workload(vec![original]), 1);
    let ini = fs::read_to_string(temp.0.join("run.ini")).unwrap().replace(
        "Main.bus.profile = \"can.fd.iso.v1\"",
        "Main.bus.profile = \"can.fd.precomputed.v1\"",
    );
    fs::write(temp.0.join("run.ini"), ini).unwrap();
    assert!(prepare(&temp.0.join("run.ini")).is_err());
}
