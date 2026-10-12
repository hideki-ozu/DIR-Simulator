use super::*;

fn publish(control: &Directory, message: &worker::Message) -> (Value, Vec<u8>) {
    let mut wire = Vec::new();
    worker::publish_response(control, &mut wire, message).unwrap();
    let length = u32::from_be_bytes(wire[..4].try_into().unwrap()) as usize;
    assert_eq!(wire.len(), length + 4);
    assert!(length <= worker::FRAME_LIMIT);
    let reply = worker::read_frame::<worker::Message>(&mut wire.as_slice()).unwrap();
    (serde_json::to_value(reply).unwrap(), wire)
}
fn huge_prepare_message() -> worker::Message {
    let input = "network-token-".repeat(worker::FRAME_LIMIT / 8);
    let diagnostic = crate::Diagnostic::prepare(format!("Unknown network type: {input}"))
        .with_reason("unknown_type")
        .with_target(&input)
        .with_detail("actual", &input)
        .with_detail("expected", "declared network type")
        .with_span(
            Path::new("/fixed/project.ini"),
            "network = absent\n",
            10,
            16,
        )
        .wire_value();
    worker::Message::PreparedFailed { diagnostic }
}
#[test]
fn large_prepare_response_preserves_full_bytes_hash_code_position_and_protocol_kind() {
    let fixture = Fixture::new();
    let manager = fixture.manager();
    let id = fixture.create(&manager, "large");
    let control = record_dir(&manager.shared, &id)
        .unwrap()
        .child("control")
        .unwrap();
    let message = huge_prepare_message();
    let full = serde_json::to_vec(&message).unwrap();
    assert!(full.len() > worker::FRAME_LIMIT);
    let (reply, wire) = publish(&control, &message);
    assert_eq!(reply["kind"], "prepared_failed");
    let original = serde_json::to_value(&message).unwrap();
    for key in [
        "code",
        "phase",
        "reason",
        "source",
        "line",
        "column",
        "end_line",
        "end_column",
    ] {
        assert_eq!(reply["diagnostic"][key], original["diagnostic"][key]);
    }
    let summary = &reply["diagnostic"];
    assert!(serde_json::to_vec(summary).unwrap().len() <= worker::DIAGNOSTIC_SUMMARY_LIMIT);
    assert_eq!(summary["details_artifact"], worker::LARGE_RESPONSE_ARTIFACT);
    assert_eq!(summary["details_sha256"], hash(&full));
    assert_eq!(summary["details_bytes"], full.len().to_string());
    assert!(
        summary["message"]
            .as_str()
            .unwrap()
            .contains("Full original data is retained")
    );
    assert_eq!(
        control
            .read(worker::LARGE_RESPONSE_FILE, full.len())
            .unwrap(),
        full
    );
    assert_eq!(
        control
            .read("worker-response.json", worker::FRAME_LIMIT)
            .unwrap(),
        wire[4..]
    );
    // The unbounded original must not be written to the transport/log stream.
    assert!(wire.len() < 32 * 1024);
    assert!(!String::from_utf8_lossy(&wire[4..]).contains(&"network-token-".repeat(1000)));
}
#[test]
fn large_prepare_diagnostic_bounds_every_string_with_json_escaping_and_utf8() {
    let fixture = Fixture::new();
    let manager = fixture.manager();
    let id = fixture.create(&manager, "escaped");
    let control = record_dir(&manager.shared, &id)
        .unwrap()
        .child("control")
        .unwrap();
    let dangerous = "\u{0000}\n\\\"日本語😀".repeat(worker::FRAME_LIMIT / 20);
    let diagnostic = json!({"schema_version":1,"seq":"0","severity":"error","primary":true,"code":"E-0001","phase":"prepare","reason":"unknown_type","line":"12","column":"34","end_line":"12","end_column":"99","source":dangerous,"target":dangerous,"message":dangerous,"details":{"name":dangerous}});
    let (reply, _) = publish(&control, &worker::Message::PreparedFailed { diagnostic });
    assert_eq!(reply["kind"], "prepared_failed");
    assert_eq!(reply["diagnostic"]["code"], "E-0001");
    assert_eq!(reply["diagnostic"]["line"], "12");
    assert!(
        serde_json::to_vec(&reply["diagnostic"]).unwrap().len() <= worker::DIAGNOSTIC_SUMMARY_LIMIT
    );
    assert!(
        reply["diagnostic"]["source"]
            .as_str()
            .unwrap()
            .ends_with("[truncated]")
    );
    assert!(
        reply["diagnostic"]["target"]
            .as_str()
            .unwrap()
            .ends_with("[truncated]")
    );
}
#[test]
fn actual_giant_unknown_network_prepare_failure_remains_prepare_failed_after_restart() {
    let mut fixture = Fixture::new();
    let huge = format!("demo.{}", "x".repeat(3 * 1024 * 1024));
    let config = fixture
        .project
        .files
        .values_mut()
        .find(|file| {
            file.roles
                .contains(&super::super::super::input::FileRole::Config)
        })
        .unwrap();
    config.text = config.text.replace("demo.Main", &huge).into();
    config.hash = hash(config.text.as_bytes());
    fixture.project.refresh_inputs();
    let failure = super::super::super::analysis::prepare(&fixture.project).unwrap_err();
    assert_eq!(failure.reason, "unknown_type");
    let message = worker::Message::PreparedFailed {
        diagnostic: failure.wire_value(),
    };
    let full = serde_json::to_vec(&message).unwrap();
    assert!(full.len() > worker::FRAME_LIMIT);
    let manager = fixture.manager();
    let id = fixture.create(&manager, "real-prepare");
    let control = record_dir(&manager.shared, &id)
        .unwrap()
        .child("control")
        .unwrap();
    publish(&control, &message);
    {
        let mut state = manager.shared.state.lock().unwrap();
        let record = state.records.get_mut(&id).unwrap();
        record.phase = "preparing".into();
        record.started_at_utc = Some(utc());
        persist(&manager.shared, record).unwrap();
    }
    let restarted = fixture.restart(manager);
    let record = restarted.get(&id).unwrap();
    assert_eq!(record["phase"], "prepare_failed");
    assert_eq!(record["artifact_state"], "unpublished");
    assert_eq!(record["worker_response"], "available");
    assert_eq!(
        record["diagnostics"][0]["diagnostic"]["details_artifact"],
        worker::LARGE_RESPONSE_ARTIFACT
    );
    let control = record_dir(&restarted.shared, &id)
        .unwrap()
        .child("control")
        .unwrap();
    assert_eq!(
        hash(
            &control
                .read(worker::LARGE_RESPONSE_FILE, full.len())
                .unwrap()
        ),
        hash(&full)
    );
    assert_eq!(
        restarted.list(None).unwrap()["runs"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
    fixture.create(&restarted, "after-large");
    let listing = restarted.artifacts(&id).unwrap();
    let artifact = listing["artifacts"]
        .as_array()
        .unwrap()
        .iter()
        .find(|a| a["artifact_id"] == worker::LARGE_RESPONSE_ARTIFACT)
        .unwrap();
    assert_eq!(artifact["sha256"], hash(&full));
    let mut download = restarted
        .download(&id, worker::LARGE_RESPONSE_ARTIFACT)
        .unwrap();
    let mut downloaded = Vec::new();
    download.read_to_end(&mut downloaded).unwrap();
    assert_eq!(downloaded, full);
    drop(download);
    fs::write(
        restarted
            .shared
            .root
            .join(&id)
            .join("control")
            .join(worker::LARGE_RESPONSE_FILE),
        b"tampered diagnostic",
    )
    .unwrap();
    assert!(
        restarted
            .download(&id, worker::LARGE_RESPONSE_ARTIFACT)
            .is_err()
    );
    assert!(restarted.artifacts(&id).is_err());
}
#[test]
fn normal_prepare_response_remains_unchanged_and_creates_no_large_artifact() {
    let fixture = Fixture::new();
    let manager = fixture.manager();
    let id = fixture.create(&manager, "small");
    let control = record_dir(&manager.shared, &id)
        .unwrap()
        .child("control")
        .unwrap();
    let message = worker::Message::PreparedFailed {
        diagnostic: crate::Diagnostic::prepare("Missing network")
            .with_reason("missing_value")
            .wire_value(),
    };
    let expected = serde_json::to_value(&message).unwrap();
    let (reply, _) = publish(&control, &message);
    assert_eq!(reply, expected);
    assert!(
        !control
            .names()
            .unwrap()
            .iter()
            .any(|name| name == worker::LARGE_RESPONSE_FILE)
    );
}
