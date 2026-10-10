use super::*;
use std::sync::atomic::{AtomicU64, Ordering};

const NED: &str = include_str!("../../../../docs/verification/fixtures/can/models/demo/Main.ned");
const INI: &str = "[General]\nnetwork = demo.Main\nned-path = \"models\"\nsim-time-limit = 1ms\nMain.bus.bitrate = 500kbps\n";
static TEMP_ID: AtomicU64 = AtomicU64::new(0);
struct Fixture(PathBuf);
impl Fixture {
    fn new(ned: &str, ini: &str) -> Self {
        let path = std::env::temp_dir().join(format!(
            "dir-input-test-{}-{}",
            std::process::id(),
            TEMP_ID.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(path.join("models/demo")).unwrap();
        fs::write(path.join("models/demo/Main.ned"), ned).unwrap();
        fs::write(path.join("scenario.ini"), ini).unwrap();
        Self(path)
    }
    fn prepare(&self) -> Result<PreparedSimulation> {
        prepare(&self.0.join("scenario.ini"))
    }
    fn workload(&self, text: &str) {
        fs::write(self.0.join("workload.json"), text).unwrap();
        fs::write(
            self.0.join("scenario.ini"),
            format!("{INI}workload = \"workload.json\"\n"),
        )
        .unwrap();
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

#[test]
fn exact_units_and_extreme_decimal_precision() {
    for (literal, expected) in [
        ("0ps", 0),
        ("0.001ns", 1),
        ("1 ms", 1_000_000_000),
        ("18446744073709551615ps", u64::MAX),
        ("0.000000000001s", 1),
        ("1.00000000000000000000000000000000000000000000000ps", 1),
    ] {
        assert_eq!(parse_time(literal).unwrap(), expected, "{literal}");
    }
    for literal in [
        "0.1ps",
        "18446744073709551616ps",
        "-0ps",
        "-1ps",
        "01ps",
        ".1ps",
        "1.ps",
        "1e3ps",
        "1",
        "1Mbps",
        "1PS",
        "1ps#comment",
        "1\u{a0}ps",
    ] {
        assert!(parse_time(literal).is_err(), "{literal}");
    }
    assert_eq!(quantity("0.5Mbps", "bps").unwrap(), 500_000);
    assert_eq!(quantity("1KiB", "B").unwrap(), 1_024);
    assert!(quantity("0.5bps", "bps").is_err());
}

#[test]
fn all_can_fixtures_load_and_preserve_snapshots() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../docs/verification/fixtures/can");
    for name in [
        "competition",
        "queue-full",
        "capacity-zero",
        "eof-boundary",
        "after-eof",
        "delay-filter",
        "release-arrival",
        "mixed-generators",
    ] {
        let prepared = prepare(&root.join(format!("{name}.ini"))).unwrap();
        assert_eq!(prepared.can.bitrate, 500_000);
        assert_eq!(prepared.can.controllers.len(), 3);
        assert_eq!(prepared.common.channel_count, 6);
        assert_eq!(prepared.common.inputs.len(), 3);
        assert_eq!(prepared.can.controllers[0].id, "Main.a");
        assert_eq!(prepared.can.bus_id, "Main.bus");
    }
    let prepared = prepare(&root.join("delay-filter.ini")).unwrap();
    assert_eq!(prepared.can.controllers[0].tx_processing_ps, 3_000_000);
    assert_eq!(prepared.can.controllers[0].tx_channel_ps, 2_000_000);
    assert_eq!(prepared.can.controllers[1].rx_channel_ps, 3_000_000);
    assert_eq!(prepared.can.controllers[2].rx_filter, "none");
}

#[test]
fn defaults_overrides_empty_workload_and_limits() {
    let fixture = Fixture::new(
        NED,
        &format!(
            "{INI}Main.a.queueCapacity = 32\nmax-events = 18446744073709551615\nsim-time-limit-unknown = 1ms\n"
        ),
    );
    assert!(fixture.prepare().is_err());
    fs::write(
        fixture.0.join("scenario.ini"),
        format!("{INI}Main.a.queueCapacity = 32\nmax-events = 18446744073709551615\n"),
    )
    .unwrap();
    let prepared = fixture.prepare().unwrap();
    assert_eq!(prepared.can.controllers[0].queue_capacity, 32);
    assert_eq!(prepared.can.controllers[1].queue_capacity, 64);
    assert_eq!(prepared.common.max_events, u64::MAX);
    assert_eq!(prepared.common.metrics_window_ps, 1_000_000_000);
    assert!(prepared.can.generators.is_empty());
    assert_eq!(prepared.common.inputs.len(), 2);
}

#[test]
fn bus_gate_names_and_same_bus_output_permutations_are_valid() {
    let baseline = Fixture::new(NED, INI).prepare().unwrap();
    let legacy = NED
        .replace("rx_", "legacy_input_")
        .replace("tx_", "rx_")
        .replace("legacy_input_", "tx_");
    let arbitrary = NED
        .replace("rx_a", "rx_decoy")
        .replace("tx_a", "delivery")
        .replace("rx_b", "request")
        .replace("tx_b", "tx_decoy")
        .replace("rx_c", "_incoming")
        .replace("tx_c", "_");
    for model in [&legacy, &arbitrary] {
        let prepared = Fixture::new(model, INI).prepare().unwrap();
        assert_eq!(
            serde_json::to_value(&prepared.can.controllers).unwrap(),
            serde_json::to_value(&baseline.can.controllers).unwrap()
        );
        assert_eq!(prepared.can.controller_buses, baseline.can.controller_buses);
        assert_eq!(prepared.common.channel_count, baseline.common.channel_count);
    }
    let swapped = NED
        .replace("bus.tx_a -->", "bus.swap -->")
        .replace("bus.tx_b -->", "bus.tx_a -->")
        .replace("bus.swap -->", "bus.tx_b -->");
    let prepared = Fixture::new(
        &swapped,
        &format!(
            "{INI}[Channel Main::bus.tx_a]\ndelay = 7ns\n[Channel Main::bus.tx_b]\ndelay = 11ns\n"
        ),
    )
    .prepare()
    .unwrap();
    assert_eq!(prepared.can.controllers[0].rx_channel_ps, 11_000);
    assert_eq!(prepared.can.controllers[1].rx_channel_ps, 7_000);
}

#[test]
fn bus_requires_equal_input_output_counts_and_two_controllers() {
    for model in [
        NED.replace("input rx_a; output tx_a;", "input rx_a; input tx_a;"),
        NED.replace("input rx_b; output tx_b;", "")
            .replace("input rx_c; output tx_c;", ""),
    ] {
        assert!(
            Fixture::new(&model, INI)
                .prepare()
                .unwrap_err()
                .message
                .contains("equal counts of input/output scalar gates")
        );
    }
}

#[test]
fn compound_boundary_channels_are_independent_and_sum() {
    let model = NED
        .replace("rx_a", "rx_decoy")
        .replace("tx_a", "tx_decoy")
        .replace("a: demo.Controller;", "a: demo.Box;")
        .replace("a.tx -->", "a.out -->")
        .replace("--> a.rx;", "--> a.in;")
        + "\nmodule Box { gates: input in; output out; submodules: c: demo.Controller; connections: in --> demo.Wire --> c.rx; c.tx --> demo.Wire --> out; }\n";
    let fixture = Fixture::new(
        &model,
        &format!(
            "{INI}[Channel Main::a.out]\ndelay = 2ns\n[Channel Main.a::c.tx]\ndelay = 3ns\n[Channel Main::bus.tx_decoy]\ndelay = 7ns\n[Channel Main.a::in]\ndelay = 11ns\n"
        ),
    );
    let prepared = fixture.prepare().unwrap();
    assert_eq!(prepared.can.controllers[0].id, "Main.a.c");
    assert_eq!(prepared.can.controllers[0].tx_channel_ps, 5_000);
    assert_eq!(prepared.can.controllers[0].rx_channel_ps, 18_000);
    assert_eq!(prepared.can.controllers[1].tx_channel_ps, 0);
    assert_eq!(prepared.common.channel_count, 8);
    fs::write(fixture.0.join("scenario.ini"), format!("{INI}[Channel Main::a.out]\ndelay = 18446744073709551615ps\n[Channel Main.a::c.tx]\ndelay = 1ps\n")).unwrap();
    assert!(fixture.prepare().unwrap_err().message.contains("overflow"));
}

#[test]
fn invalid_ned_declarations_and_wiring_are_rejected() {
    let variants = [
        NED.replace("package demo;", "package other;"),
        NED.replace("@class(\"dir.can.Controller\")", "@class(\"unknown\")"),
        NED.replace("int queueCapacity", "double queueCapacity"),
        NED.replace("default(64)", "default(-1)"),
        NED.replace("default(64)", "default(64.0)"),
        NED.replace("int queueCapacity", "int queueCapacity @unit(s)"),
        NED.replace("output tx; input rx;", "output tx; input rx; input extra;"),
        NED.replace("input rx_a; output tx_a;", "input rx_a; input tx_a;"),
        NED.replace("input rx_a; output tx_a;", "input rx_a; output rx_a;"),
        NED.replace("input rx_a;", "input rx_a[2];"),
        NED.replace("input rx_a;", "input 1invalid;"),
        NED.replace("input rx_a;", "input network;"),
        NED.replace("input rx_a;", "inout rx_a;"),
        NED.replace(
            "a.tx --> demo.Wire --> bus.rx_a;",
            "a.rx --> demo.Wire --> bus.rx_a;",
        ),
        NED.replace(
            "a.tx --> demo.Wire --> bus.rx_a;",
            "a.tx --> demo.Wire --> bus.tx_a;",
        ),
        NED.replace(
            "a.tx --> demo.Wire --> bus.rx_a;",
            "a.tx --> demo.Wire --> bus.rx_a; a.tx --> demo.Wire --> bus.rx_a;",
        ),
        NED.replace("bus.tx_a --> demo.Wire --> a.rx;", ""),
        NED.replace("a: demo.Controller;", "a: demo.Missing;"),
        NED.replace("network Main {", "network Main { gates: input in;"),
        format!("{NED}\nmodule Loop {{ submodules: x: demo.Loop; }}"),
        format!("{NED}\nnetwork Other {{ submodules: x: demo.Missing; }}"),
        format!("{NED}\nchannel Wire {{}}"),
        NED.replace("default(64)", "64"),
        NED.replace("a: demo.Controller;", "a: Controller;"),
    ];
    for model in variants {
        let fixture = Fixture::new(&model, INI);
        assert!(fixture.prepare().is_err(), "accepted {model}");
    }
    let fixture = Fixture::new(
        &NED.replace("default(64)", "default(-1)"),
        &format!(
            "{INI}Main.a.queueCapacity = 64\nMain.b.queueCapacity = 64\nMain.c.queueCapacity = 64\n"
        ),
    );
    assert!(
        fixture.prepare().is_err(),
        "INI must not hide invalid defaults"
    );
    let fixture = Fixture::new(
        &NED.replace("default(64)", "default(4294967296)"),
        &format!(
            "{INI}Main.a.queueCapacity = 64\nMain.b.queueCapacity = 64\nMain.c.queueCapacity = 64\n"
        ),
    );
    assert!(
        fixture.prepare().is_err(),
        "INI must not hide an excessive queue default"
    );
}

#[test]
fn truncated_ned_returns_diagnostics_without_panicking() {
    for (end, _) in NED.char_indices() {
        let _ = ned::parse(&NED[..end], Path::new("models/demo/Main.ned"), "demo");
    }
}

#[test]
fn ini_rejects_unknown_duplicate_and_malformed_inputs() {
    for addition in [
        "Main.unknown.queueCapacity = 1",
        "Main.a.unknown = 1",
        "Main.a.queueCapacity = \"32\"",
        "Main.a.queueCapacity = -1",
        "Main.a.queueCapacity = 4294967296",
        "Main.a.queueCapacity = 64.0",
        "Main.a.rxFilter = \"std:0x1,std:0x01\"",
        "Main.a.rxFilter = \"std:0x800\"",
        "Main.a.rxFilter = \"none\" # comment",
        "max-events = 0",
        "metrics-window = 0ps",
        "network = demo.Main",
        "[General]",
        "[Config Fast]",
        "[Channel Main::a.tx]\nunknown = 1ps",
        "[Channel Main::missing.tx]\ndelay = 1ps",
        "[Channel  Main::a.tx]\ndelay = 1ps",
        "workload = \"\"",
        "model-profile = \"can.fd.v1\"",
        "model-config = \"anything.json\"",
        "[Channel Main::a.tx]\ndelay = 1ps\n[Channel Main::a.tx]\ndelay = 1ps",
    ] {
        let fixture = Fixture::new(NED, &format!("{INI}{addition}\n"));
        assert!(fixture.prepare().is_err(), "accepted {addition}");
    }
    for ini in [
        "network = demo.Main\n",
        "[General]\r",
        "[General]\nnetwork = demo.Main\rsim-time-limit = 1ps",
    ] {
        assert!(Ini::parse(ini, Path::new("scenario.ini")).is_err());
    }
}

#[test]
fn json_duplicate_unknown_fields_and_inactive_owner_conflicts() {
    let fixture = Fixture::new(NED, INI);
    for text in [
        "{\"schema_version\":1,\"schema_version\":1,\"generators\":[]}",
        "{\"schema_version\":1.0,\"generators\":[]}",
        "{\"schema_version\":1,\"generators\":[],\"seed\":0}",
        r#"{"schema_version":1,"generators":[{"id":"a","kind":"can.explicit.v1","node":"Main.a","times":[],"frame":{"format":"standard","id":0,"data":"","data":""}}]}"#,
        r#"{"schema_version":1,"generators":[{"id":"a","kind":"can.explicit.v1","node":"Main.a","times":[],"frame":{"format":"standard","id":0,"data":""}},{"id":"b","kind":"can.periodic.v1","node":"Main.b","start":"0ps","period":"1ms","count":0,"frame":{"format":"standard","id":0,"data":""}}]}"#,
    ] {
        fixture.workload(text);
        assert!(fixture.prepare().is_err(), "accepted {text}");
    }
}

#[test]
fn workload_validates_schedule_and_frame_beyond_time_limit() {
    let fixture = Fixture::new(NED, INI);
    let base = serde_json::json!({"schema_version":1,"generators":[{"id":"z","kind":"can.explicit.v1","node":"Main.a","times":["0ps","0ps","2ms"],"frame":{"format":"standard","id":291,"data":"00FF"}}]});
    fixture.workload(&base.to_string());
    let prepared = fixture.prepare().unwrap();
    assert_eq!(prepared.can.generators[0].frame.data, "00ff");
    assert_eq!(
        prepared.can.generators[0].schedule.time(2),
        Some(2_000_000_000)
    );
    for (field, value) in [
        ("times", serde_json::json!(["2ms", "1ms"])),
        ("times", serde_json::json!(["2ms", "-1ps"])),
        ("node", serde_json::json!("Main.bus")),
        ("id", serde_json::json!("bad-id")),
        (
            "frame",
            serde_json::json!({"format":"standard","id":2048,"data":""}),
        ),
        (
            "frame",
            serde_json::json!({"format":"standard","id":0,"data":"f"}),
        ),
        (
            "frame",
            serde_json::json!({"format":"standard","id":0,"data":"000000000000000000"}),
        ),
        (
            "frame",
            serde_json::json!({"format":"standard","id":0,"data":"","rtr":true}),
        ),
        ("count", serde_json::json!(0)),
    ] {
        let mut changed = base.clone();
        changed["generators"][0][field] = value;
        fixture.workload(&changed.to_string());
        assert!(fixture.prepare().is_err(), "accepted {changed}");
    }
    let periodic = serde_json::json!({"schema_version":1,"generators":[{"id":"p","kind":"can.periodic.v1","node":"Main.a","start":"1ms","period":"2ms","phase":"1ms","end":"8ms","count":3,"frame":{"format":"extended","id":291,"data":""}}]});
    fixture.workload(&periodic.to_string());
    let prepared = fixture.prepare().unwrap();
    assert_eq!(
        prepared.can.generators[0].schedule.time(0),
        Some(2_000_000_000)
    );
    assert_eq!(
        prepared.can.generators[0].schedule.time(2),
        Some(6_000_000_000)
    );
    assert_eq!(prepared.can.generators[0].schedule.time(3), None);
    for (field, value) in [
        ("period", serde_json::json!("0ps")),
        ("phase", serde_json::json!("2ms")),
        ("end", serde_json::json!("0ps")),
        ("count", serde_json::json!(0.5)),
    ] {
        let mut changed = periodic.clone();
        changed["generators"][0][field] = value;
        fixture.workload(&changed.to_string());
        assert!(fixture.prepare().is_err(), "accepted {changed}");
    }
}

#[test]
fn root_overlap_missing_files_and_symlinks_fail() {
    let fixture = Fixture::new(
        NED,
        &INI.replace("\"models\"", "\"models\";\"models/demo\""),
    );
    assert!(
        fixture
            .prepare()
            .unwrap_err()
            .message
            .contains("overlapping")
    );
    fs::write(
        fixture.0.join("scenario.ini"),
        format!("{INI}workload = \"missing.json\"\n"),
    )
    .unwrap();
    assert!(fixture.prepare().is_err());
    #[cfg(unix)]
    {
        fs::write(fixture.0.join("scenario.ini"), INI).unwrap();
        std::os::unix::fs::symlink("Main.ned", fixture.0.join("models/demo/symlink.txt")).unwrap();
        assert!(fixture.prepare().unwrap_err().message.contains("symlink"));
    }
}
