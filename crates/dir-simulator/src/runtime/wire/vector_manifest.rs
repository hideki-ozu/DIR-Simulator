//! Test-only execution receipts. A receipt is written only after every expected
//! field is compared with an observation (or explicitly excluded with a basis).
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::path::PathBuf;
pub fn vectors(name: &str) -> Vec<Value> {
    serde_json::from_slice::<Value>(&std::fs::read(path(name)).unwrap()).unwrap()["vectors"]
        .as_array()
        .unwrap()
        .clone()
}
fn path(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../docs/proposals/fixtures")
        .join(format!("{name}.json"))
}
pub fn finish(name: &str, row: &Value, test: &str, actual: Value) {
    finish_with_na(name, row, test, actual, &[])
}
pub fn finish_with_na(name: &str, row: &Value, test: &str, actual: Value, na: &[(&str, &str)]) {
    let expected = row["expected"].as_object().unwrap();
    let mut fields = Vec::new();
    for (key, want) in expected {
        if let Some((_, basis)) = na.iter().find(|(k, _)| *k == key) {
            assert!(!basis.is_empty());
            fields.push(json!({"field":key,"status":"not_applicable","basis":basis}));
        } else {
            let got = actual
                .get(key)
                .unwrap_or_else(|| panic!("{} missing observation {key}", row["id"]));
            assert_eq!(got, want, "{} field {key}", row["id"]);
            fields.push(json!({"field":key,"status":"passed","actual":got}));
        }
    }
    if let Ok(dir) = std::env::var("WIRE_CASE_REPORT_DIR") {
        let dir = PathBuf::from(dir);
        std::fs::create_dir_all(&dir).unwrap();
        let hash = format!("{:x}", Sha256::digest(std::fs::read(path(name)).unwrap()));
        let receipt = json!({"fixture":format!("{name}.json"),"fixture_sha256":hash,"id":row["id"],"kind":row["kind"],"test":test,"status":if fields.iter().all(|v|v["status"]=="not_applicable"){"not_applicable"}else{"passed"},"fields":fields});
        let file = dir.join(format!("{}-{}.json", name, row["id"].as_str().unwrap()));
        let tmp = file.with_extension(format!("{}.tmp", std::process::id()));
        std::fs::write(&tmp, serde_json::to_vec_pretty(&receipt).unwrap()).unwrap();
        std::fs::rename(tmp, file).unwrap();
    }
}
