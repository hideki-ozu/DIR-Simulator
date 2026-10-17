#[test]
fn transition_priority_is_lossless_and_catalog_checked() {
    let mut s = WireState::new(Arc::new(prepared(&[WireProtocol::Rstp])));
    let d = s
        .plan(0, WireEvent::Initialize, &WireView::default())
        .unwrap();
    let root = d
        .records
        .iter()
        .find(|r| r.kind == "ethernet.control.wire.transition" && r.data["machine"] == "PRS")
        .unwrap();
    assert!(
        root.data["changes"]
            .as_object()
            .unwrap()
            .values()
            .all(|change| change["after"]
                .as_array()
                .is_none_or(|a| a.iter().all(Value::is_string))),
        "priority integers must be canonical decimal strings: {}",
        root.data
    );
    for row in d
        .records
        .iter()
        .filter(|r| r.kind == "ethernet.control.wire.transition")
    {
        crate::output::wire::validate_record(&row.kind, &row.data).unwrap();
    }
    let mut malformed = root.data.clone();
    malformed["machine"] = json!("InventedMachine");
    assert!(
        crate::output::wire::validate_record("ethernet.control.wire.transition", &malformed)
            .is_err()
    );
    s.apply(d);
}
