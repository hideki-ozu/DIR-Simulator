//! Production-path evidence for local-clock TAS composition.
use dir_simulator::{prepare, run, viewer};
use serde_json::Value;
use std::{
    fs,
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
};

static NEXT: AtomicU64 = AtomicU64::new(0);

fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../examples/network-composite")
}

fn temp() -> PathBuf {
    let path = std::env::temp_dir().join(format!(
        "dir-network-composite-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ));
    fs::create_dir_all(&path).unwrap();
    path
}

fn isolated_ini(out: &Path) {
    let ini = fs::read_to_string(root().join("local-clock.ini"))
        .unwrap()
        .replace(
            "../ethernet/dynamic/models",
            &root()
                .join("../ethernet/dynamic/models")
                .canonicalize()
                .unwrap()
                .display()
                .to_string(),
        )
        .replace(
            "workload.json",
            &root().join("workload.json").display().to_string(),
        );
    fs::write(out.join("model.ini"), ini).unwrap();
}

fn run_example(name: &str, output: &Path) -> Value {
    run(prepare(&root().join(name)).unwrap(), output).unwrap();
    serde_json::from_slice(&fs::read(output.join("results.json")).unwrap()).unwrap()
}

fn source_sofs(result: &Value) -> Vec<u64> {
    result["simulation"]["model_records"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|row| {
            row["schema_name"] == "ethernet.dynamic.transfer"
                && row["data"]["from_port"] == "Net.a.tx"
                && row["data"]["status"] == "serialized"
        })
        .map(|row| row["data"]["sof_ps"].as_str().unwrap().parse().unwrap())
        .collect()
}

#[test]
fn local_clock_offset_and_backward_step_change_physical_tas_departure() {
    let out = temp();
    let identity = run_example("model.ini", &out.join("identity"));
    let offset = run_example("local-clock-no-step.ini", &out.join("offset"));
    let step = run_example("local-clock.ini", &out.join("step"));
    assert_eq!(source_sofs(&identity), [0, 752_000]);
    assert_eq!(source_sofs(&offset), [900_000, 2_900_000]);
    assert_eq!(source_sofs(&step), [1_900_000, 3_900_000]);
    let records = step["simulation"]["model_records"].as_array().unwrap();
    assert!(
        records
            .iter()
            .any(|row| row["schema_name"] == "dir.network.clock"
                && row["data"]["node"] == "Net.a"
                && row["data"]["cause"] == "step"
                && row["time_ps"] == "500000")
    );
    assert!(
        records
            .iter()
            .any(|row| row["schema_name"] == "dir.network.clock"
                && row["data"]["node"] == "Net.s1"
                && row["data"]["cause"] == "slew"
                && row["time_ps"] == "900000")
    );
    let html = out.join("clock-viewer.html");
    viewer::write(&out.join("step/results.json"), &html).unwrap();
    assert!(
        fs::read_to_string(&html)
            .unwrap()
            .contains("ノード時計の変更")
    );
    fs::remove_dir_all(out).unwrap();
}

#[test]
fn invalid_clock_configurations_fail_prepare() {
    let out = temp();
    let mut model: Value =
        serde_json::from_slice(&fs::read(root().join("local-clock-model.json")).unwrap()).unwrap();
    let baseline = model.clone();
    isolated_ini(&out);
    type Mutation = (&'static str, Box<dyn Fn(&mut Value)>);
    let cases: Vec<Mutation> = vec![
        (
            "zero rate",
            Box::new(|m| m["clocks"]["nodes"][0]["rate_num"] = "0".into()),
        ),
        (
            "negative zero",
            Box::new(|m| m["clocks"]["nodes"][0]["offset_ps"] = "-0".into()),
        ),
        (
            "unknown node",
            Box::new(|m| m["clocks"]["nodes"][0]["node"] = "missing".into()),
        ),
        (
            "duplicate adjustment",
            Box::new(|m| {
                let row = m["clocks"]["adjustments"][0].clone();
                m["clocks"]["adjustments"].as_array_mut().unwrap().push(row);
            }),
        ),
        (
            "overflow step",
            Box::new(|m| m["clocks"]["adjustments"][0]["offset_ps"] = i128::MAX.to_string().into()),
        ),
    ];
    for (name, edit) in cases {
        model = baseline.clone();
        edit(&mut model);
        fs::write(
            out.join("local-clock-model.json"),
            serde_json::to_vec(&model).unwrap(),
        )
        .unwrap();
        assert!(prepare(&out.join("model.ini")).is_err(), "{name}");
    }
    fs::remove_dir_all(out).unwrap();
}

#[test]
fn forward_step_replans_old_gate_wake_at_the_step_time() {
    let out = temp();
    isolated_ini(&out);
    let mut model: Value =
        serde_json::from_slice(&fs::read(root().join("local-clock-model.json")).unwrap()).unwrap();
    model["clocks"]["adjustments"][0]["offset_ps"] = "2500000".into();
    fs::write(
        out.join("local-clock-model.json"),
        serde_json::to_vec(&model).unwrap(),
    )
    .unwrap();
    let result = run(prepare(&out.join("model.ini")).unwrap(), &out.join("run")).unwrap();
    let result: Value =
        serde_json::from_slice(&fs::read(result.output_path.join("results.json")).unwrap())
            .unwrap();
    assert_eq!(source_sofs(&result)[0], 500_000);
    fs::remove_dir_all(out).unwrap();
}

#[test]
fn sampled_phy_requires_an_observed_frame_before_network_reception() {
    let good = temp();
    let good_result = run_example("sampled-phy-short.ini", &good.join("run"));
    let observed = good_result["simulation"]["model_records"]
        .as_array()
        .unwrap();
    assert!(
        observed
            .iter()
            .any(|row| row["schema_name"] == "dir.network.phy" && row["data"]["kind"] == "RxFrame")
    );
    assert!(
        observed
            .iter()
            .any(|row| row["schema_name"] == "ethernet.dynamic.reception"
                && row["data"]["ingress"] == "Net.s1.rx_a")
    );

    let muted = temp();
    let mut model: Value =
        serde_json::from_slice(&fs::read(root().join("sampled-phy-model.json")).unwrap()).unwrap();
    model["phy_attachments"][0]["medium"]["channel"]["paths"][1][0]["taps_q30"] =
        serde_json::json!([107_374]);
    fs::write(
        muted.join("model.json"),
        serde_json::to_vec(&model).unwrap(),
    )
    .unwrap();
    let ini = fs::read_to_string(root().join("sampled-phy-short.ini"))
        .unwrap()
        .replace(
            "../ethernet/dynamic/models",
            &root()
                .join("../ethernet/dynamic/models")
                .canonicalize()
                .unwrap()
                .display()
                .to_string(),
        )
        .replace(
            "sampled-phy-model.json",
            &muted.join("model.json").display().to_string(),
        )
        .replace(
            "workload.json",
            &root().join("workload.json").display().to_string(),
        );
    fs::write(muted.join("model.ini"), ini).unwrap();
    run(
        prepare(&muted.join("model.ini")).unwrap(),
        &muted.join("run"),
    )
    .unwrap();
    let muted_result: Value =
        serde_json::from_slice(&fs::read(muted.join("run/results.json")).unwrap()).unwrap();
    let rows = muted_result["simulation"]["model_records"]
        .as_array()
        .unwrap();
    assert!(
        !rows
            .iter()
            .any(|row| row["schema_name"] == "dir.network.phy" && row["data"]["kind"] == "RxFrame")
    );
    assert!(
        !rows
            .iter()
            .any(|row| row["schema_name"] == "ethernet.dynamic.reception")
    );
    assert!(
        !rows
            .iter()
            .any(|row| row["schema_name"] == "dir.redundancy.recovery")
    );
    assert!(
        rows.iter()
            .any(|row| row["schema_name"] == "ethernet.dynamic.transfer"
                && row["data"]["drop_reason"] == "phy:no_valid_received_frame")
    );
    fs::remove_dir_all(good).unwrap();
    fs::remove_dir_all(muted).unwrap();
}

#[test]
fn sampled_phy_rejects_unbound_and_duplicate_directions() {
    let out = temp();
    let ini = fs::read_to_string(root().join("sampled-phy-short.ini"))
        .unwrap()
        .replace(
            "../ethernet/dynamic/models",
            &root()
                .join("../ethernet/dynamic/models")
                .canonicalize()
                .unwrap()
                .display()
                .to_string(),
        )
        .replace(
            "sampled-phy-model.json",
            &out.join("model.json").display().to_string(),
        )
        .replace(
            "workload.json",
            &root().join("workload.json").display().to_string(),
        );
    fs::write(out.join("model.ini"), ini).unwrap();
    let baseline: Value =
        serde_json::from_slice(&fs::read(root().join("sampled-phy-model.json")).unwrap()).unwrap();
    let mut model = baseline.clone();
    model["phy_attachments"][0]["bindings"][0]["to_port"] = "Net.s2.rx_a".into();
    fs::write(out.join("model.json"), serde_json::to_vec(&model).unwrap()).unwrap();
    assert!(prepare(&out.join("model.ini")).is_err());

    let mut model = baseline;
    let attachment = model["phy_attachments"][0].clone();
    model["phy_attachments"]
        .as_array_mut()
        .unwrap()
        .push(attachment);
    fs::write(out.join("model.json"), serde_json::to_vec(&model).unwrap()).unwrap();
    assert!(prepare(&out.join("model.ini")).is_err());
    fs::remove_dir_all(out).unwrap();
}
