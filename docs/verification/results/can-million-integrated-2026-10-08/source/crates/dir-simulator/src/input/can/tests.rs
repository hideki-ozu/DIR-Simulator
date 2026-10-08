use super::*;
use std::path::Path;

const MODEL: &str =
    include_str!("../../../../../docs/verification/fixtures/can/models/demo/Main.ned");
fn declarations(model: &str) -> BTreeMap<String, Declaration> {
    ned::parse(model, Path::new("demo/Main.ned"), "demo")
        .unwrap()
        .into_iter()
        .map(|d| (d.name.clone(), d))
        .collect()
}

#[test]
fn selected_instance_profile_mismatch_precedes_later_instance_values() {
    let types = declarations(MODEL);
    let overrides = BTreeMap::from([("Main.b.queueCapacity".into(), "bad".into())]);
    let error = resolve(
        &types,
        "demo.Main",
        &overrides,
        &BTreeMap::new(),
        "can.cc.multibus.v1",
    )
    .err()
    .unwrap();
    assert_eq!(
        error.message,
        "profile/class mismatch at Main.a: dir.can.Controller"
    );
}

#[test]
fn can_instance_counts_precede_channel_override_validation() {
    let types = declarations("package demo; network Main {}");
    let channels = BTreeMap::from([(
        "Main::missing.tx".into(),
        BTreeMap::from([("delay".into(), "bad".into())]),
    )]);
    let error = resolve(
        &types,
        "demo.Main",
        &BTreeMap::new(),
        &channels,
        "can.cc.ideal.v1",
    )
    .err()
    .unwrap();
    assert_eq!(
        error.message,
        "can.cc.ideal.v1 requires exactly one Bus and at least two Controllers"
    );
}
