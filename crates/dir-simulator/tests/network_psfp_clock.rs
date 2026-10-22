//! Receiver clock changes real PSFP acceptance while physical transmission stays fixed.
use dir_simulator::{prepare, run};
use serde_json::Value;
use std::{fs, path::PathBuf};

#[test]
fn receiver_local_offset_and_corrections_change_actual_psfp_delivery() {
    let fixture = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../examples/network-composite/psfp-clock");
    let scratch = std::env::temp_dir().join(format!("dir-psfp-clock-{}", std::process::id()));
    fs::create_dir(&scratch).unwrap();
    for (name, expected) in [
        ("identity", vec!["psfp_gate_closed", "psfp_gate_closed"]),
        ("offset", vec!["pass", "pass"]),
        ("wide-offset", vec!["pass", "pass"]),
        ("steps", vec!["psfp_gate_closed", "pass"]),
    ] {
        let output = scratch.join(name);
        run(
            prepare(&fixture.join(format!("{name}.ini"))).unwrap(),
            &output,
        )
        .unwrap();
        let result: Value =
            serde_json::from_slice(&fs::read(output.join("results.json")).unwrap()).unwrap();
        let records = result["simulation"]["model_records"].as_array().unwrap();
        let mut policing: Vec<_> = records
            .iter()
            .filter(|r| {
                r["schema_name"] == "ethernet.tsn.policing" && r["data"]["stream_id"] == "s0"
            })
            .collect();
        policing.sort_by_key(|r| r["time_ps"].as_str().unwrap().parse::<u64>().unwrap());
        assert_eq!(
            policing
                .iter()
                .map(|r| r["data"]["verdict"].as_str().unwrap())
                .collect::<Vec<_>>(),
            expected,
            "{name}"
        );
        assert_eq!(
            policing
                .iter()
                .map(|r| r["time_ps"].as_str().unwrap())
                .collect::<Vec<_>>(),
            ["576000", "2576000"]
        );
        let mut source: Vec<_> = records
            .iter()
            .filter(|r| {
                r["schema_name"] == "ethernet.dynamic.transfer"
                    && r["data"]["from_port"] == "Net.a.tx"
            })
            .map(|r| {
                r["data"]["sof_ps"]
                    .as_str()
                    .unwrap()
                    .parse::<u64>()
                    .unwrap()
            })
            .collect();
        source.sort_unstable();
        assert_eq!(source, [0, 2_000_000], "source timeline is unchanged");
        assert_eq!(
            records
                .iter()
                .filter(|r| r["schema_name"] == "ethernet.dynamic.reception"
                    && r["data"]["ingress"] == "Net.b.rx"
                    && r["data"]["status"] == "received")
                .count(),
            expected.iter().filter(|v| **v == "pass").count()
        );
        if name == "steps" {
            let mut steps: Vec<_> = records
                .iter()
                .filter(|r| r["schema_name"] == "dir.network.clock" && r["data"]["cause"] == "step")
                .map(|r| r["time_ps"].as_str().unwrap().parse::<u64>().unwrap())
                .collect();
            steps.sort_unstable();
            assert_eq!(steps, [400_000, 1_400_000]);
        }
    }
    fs::remove_dir_all(scratch).unwrap();
}
