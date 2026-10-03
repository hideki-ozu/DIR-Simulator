use super::protocol::*;
use crate::types::Frame;
#[test]
fn independent_bit_vectors() {
    let input: serde_json::Value = serde_json::from_str(include_str!(
        "../../../../../docs/verification/fixtures/can/vectors.json"
    ))
    .unwrap();
    let text = |v: &[u8]| v.iter().map(|b| char::from(b'0' + b)).collect::<String>();
    for v in input["vectors"].as_array().unwrap() {
        let frame = Frame {
            format: v["format"].as_str().unwrap().into(),
            id: v["id"].as_u64().unwrap() as u32,
            data: v["data"].as_str().unwrap().into(),
        };
        let wire = serialize(&frame).unwrap();
        assert_eq!(text(&wire.crc_input), v["crc_input"], "{}", v["name"]);
        assert_eq!(wire.crc15 as u64, v["crc15"].as_u64().unwrap());
        assert_eq!(text(&wire.stuffed_region), v["stuffed_region"]);
        assert_eq!(
            serde_json::json!(wire.stuff_positions),
            v["stuff_positions"]
        );
        assert_eq!(text(&wire.frame), v["frame"]);
    }
}
#[test]
fn priority_uses_arbitration_bits() {
    let key = |format: &str, id| {
        serialize(&Frame {
            format: format.into(),
            id,
            data: String::new(),
        })
        .unwrap()
        .arbitration
    };
    assert!(key("standard", 0x123) < key("extended", 0x048c0000));
    assert!(key("extended", 0x123) < key("standard", 0x123));
}
#[test]
fn timing_rounds_from_sof_and_checks_overflow() {
    assert_eq!(end_time(0, 50, 3).unwrap(), 16_666_666_666_667);
    assert_eq!(end_time(0, 53, 3).unwrap(), 17_666_666_666_667);
    assert!(end_time(u64::MAX, 1, 1).is_err());
}
