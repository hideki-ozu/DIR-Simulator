use dir_simulator::runtime::ddr4::*;
use serde_json::{Value, json};
use std::io::Write;
use std::process::{Command as Process, Stdio};
fn actual_trace() -> Value {
    let config = Config {
        mode_id: MODE.into(),
        source_sha256: SOURCE_SHA256.into(),
        base: 0,
        size: CAPACITY,
        queue_capacity: 16,
    };
    let mut service = Service::new(config).unwrap();
    let mut requests = Vec::new();
    for (i, address) in [3, 4 * 1024 + 5, 1024 + 7, 16384 + 1]
        .into_iter()
        .enumerate()
    {
        requests.push(Request {
            id: format!("w{i}"),
            address,
            operation: Operation::Write {
                data: (0..61).map(|b| b + i as u8).collect(),
                byte_enables: (0..61).map(|b| b % 4 != 2).collect(),
            },
        });
        requests.push(Request {
            id: format!("r{i}"),
            address,
            operation: Operation::Read { length: 61 },
        });
    }
    for request in &requests {
        service.enqueue(request.clone()).unwrap();
    }
    let effects = service.advance(750_000_000).unwrap();
    service.validate().unwrap();
    assert!(service.is_empty());
    json!({"source_sha256":SOURCE_SHA256,"end_ps":750_000_000u64,"requests":requests,"effects":effects})
}
fn monitor(value: &Value) -> std::process::Output {
    let checker = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/ddr4_oracle/check.py");
    let mut child = Process::new("python3")
        .arg(checker)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(&serde_json::to_vec(value).unwrap())
        .unwrap();
    child.wait_with_output().unwrap()
}
#[test]
fn independent_all_history_monitor_accepts_actual_ddr_controller_trace() {
    let trace = actual_trace();
    let output = monitor(&trace);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let report: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(report["requests"], 8);
    assert!(report["physical_bytes"].as_u64().unwrap() > 480);
    if let Some(path) = std::env::var_os("DIR_DDR_ORACLE_TRACE") {
        std::fs::write(path, serde_json::to_vec_pretty(&trace).unwrap()).unwrap();
    }
}
#[test]
fn independent_monitor_rejects_early_commands_and_missing_changed_physical_bytes() {
    let trace = actual_trace();
    for mutation in [
        "early_cas",
        "early_byte",
        "missing_byte",
        "changed_DM",
        "early_complete",
        "history_cas",
        "missing_last_refresh",
    ] {
        let mut bad = trace.clone();
        let effects = bad["effects"].as_array_mut().unwrap();
        match mutation {
            "early_cas" => {
                let e = effects
                    .iter_mut()
                    .find(|e| e["kind"] == "command" && e["command"]["kind"] == "write")
                    .unwrap();
                e["at_ps"] = json!(e["at_ps"].as_u64().unwrap() - 834);
            }
            "early_byte" => {
                let e = effects
                    .iter_mut()
                    .find(|e| e["kind"] == "write_byte")
                    .unwrap();
                e["at_ps"] = json!(e["at_ps"].as_u64().unwrap() - 1);
            }
            "missing_byte" => {
                let i = effects
                    .iter()
                    .position(|e| e["kind"] == "write_byte")
                    .unwrap();
                effects.remove(i);
            }
            "changed_DM" => {
                let e = effects
                    .iter_mut()
                    .find(|e| e["kind"] == "write_byte")
                    .unwrap();
                e["byte_enable"] = json!(!e["byte_enable"].as_bool().unwrap());
            }
            "early_complete" => {
                let e = effects
                    .iter_mut()
                    .find(|e| e["kind"] == "request_complete")
                    .unwrap();
                e["at_ps"] = json!(e["at_ps"].as_u64().unwrap() - 1);
            }
            "history_cas" => {
                let e = effects
                    .iter_mut()
                    .find(|e| {
                        e["kind"] == "command"
                            && e["command"]["kind"] == "read"
                            && e["command"]["request"] == "r0"
                    })
                    .unwrap();
                e["at_ps"] = json!(e["at_ps"].as_u64().unwrap() - 834);
            }
            "missing_last_refresh" => {
                let i = effects
                    .iter()
                    .rposition(|e| e["kind"] == "command" && e["command"]["kind"] == "refresh")
                    .unwrap();
                effects.remove(i);
            }
            _ => unreachable!(),
        }
        effects.sort_by_key(|e| e["at_ps"].as_u64().unwrap());
        let result = monitor(&bad);
        assert!(!result.status.success(), "{mutation}");
        let expected = match mutation {
            "early_cas" => "ACT_to_RD_WR",
            "early_complete" => "early_logical_completion",
            "history_cas" => "write_to_read_all_history",
            "missing_last_refresh" => "controller_idle_refresh_deadline",
            _ => "physical_byte_bijection",
        };
        assert!(
            String::from_utf8_lossy(&result.stderr).contains(expected),
            "{mutation}: {}",
            String::from_utf8_lossy(&result.stderr)
        );
    }
}
