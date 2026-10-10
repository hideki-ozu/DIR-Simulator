//! Mixed topology rules; physical resolution is shared with the native adapters.
use crate::input::{
    Result, error,
    ned::{self, Declaration, ModelRules, TypedValue, Values},
};
use crate::types::{PreparedCan, ethernet::PreparedEthernet};
use std::collections::BTreeMap;

struct Rules;
fn bridge(d: &Declaration) -> bool {
    d.implementation()
        .is_some_and(|class| class.starts_with("dir.bridge."))
}
impl ModelRules for Rules {
    fn validate_schema(&self, d: &Declaration) -> Result<()> {
        match d.implementation() {
            Some("dir.bridge.CanController" | "dir.bridge.EthEndpoint") if d.simple() => {
                if *d.gates() != BTreeMap::from([("tx".into(), true), ("rx".into(), false)]) {
                    return Err(d.fail("bridge endpoint requires input rx/output tx"));
                }
            }
            Some("dir.bridge.CanBus") if d.simple() => {
                let outputs = d.gates().values().filter(|output| **output).count();
                if outputs < 2 || outputs * 2 != d.gates().len() {
                    return Err(d.fail("bridge CAN Bus requires at least two input/output pairs"));
                }
            }
            Some("dir.bridge.EthLink") if d.kind() == "channel" => {}
            Some(class) if class.starts_with("dir.can.") => {
                return super::super::can::CanRules {
                    profile: "can.cc.multibus.v1",
                }
                .validate_schema(d);
            }
            Some(class) if class.starts_with("dir.ethernet.") => {
                return super::super::ethernet::EthernetRules { media: false }.validate_schema(d);
            }
            _ => return Err(d.fail("unsupported composite implementation")),
        }
        for (name, p) in d.parameters() {
            let expected = match (d.implementation(), name.as_str()) {
                (Some("dir.bridge.CanController" | "dir.bridge.EthEndpoint"), "queueCapacity") => {
                    ("int", None)
                }
                (
                    Some("dir.bridge.CanController" | "dir.bridge.EthEndpoint"),
                    "txProcessingDelay" | "rxProcessingDelay",
                ) => ("double", Some("s")),
                (Some("dir.bridge.CanController"), "rxFilter") => ("string", None),
                (Some("dir.bridge.CanBus" | "dir.bridge.EthLink"), "bitrate") => {
                    ("double", Some("bps"))
                }
                (Some("dir.bridge.EthLink"), "delay") => ("double", Some("s")),
                _ => return Err(d.fail(format!("unsupported bridge parameter {name}"))),
            };
            if p.scalar() != expected.0 || p.unit() != expected.1 {
                return Err(d.fail(format!("wrong type/unit for {name}")));
            }
        }
        Ok(())
    }
    fn validate_value(&self, d: &Declaration, name: &str, value: &TypedValue) -> Result<()> {
        if !bridge(d) {
            if d.implementation()
                .is_some_and(|class| class.starts_with("dir.can."))
            {
                return super::super::can::CanRules {
                    profile: "can.cc.multibus.v1",
                }
                .validate_value(d, name, value);
            }
            return super::super::ethernet::EthernetRules { media: false }
                .validate_value(d, name, value);
        }
        let invalid = match (d.implementation(), name, value) {
            (_, "queueCapacity", TypedValue::Integer(n)) => !(0..=u32::MAX as i64).contains(n),
            (Some("dir.bridge.CanBus"), "bitrate", TypedValue::Quantity(n)) => {
                *n == 0 || *n > 1_000_000
            }
            (Some("dir.bridge.EthLink"), "bitrate", TypedValue::Quantity(n)) => {
                ![10_000_000, 100_000_000, 1_000_000_000, 10_000_000_000].contains(n)
            }
            (_, "rxFilter", TypedValue::String(value)) => {
                super::super::can::validate_filter(value)?;
                false
            }
            _ => false,
        };
        if invalid {
            Err(d.fail(format!("unsupported bridge range for {name}")))
        } else {
            Ok(())
        }
    }
    fn payload(&self, d: &Declaration, gate: &str) -> Option<&'static str> {
        match d.implementation() {
            Some(
                "dir.bridge.CanController" | "dir.can.Controller" | "dir.can.MultibusController",
            ) => Some(if d.gates()[gate] {
                "composite.can.tx"
            } else {
                "composite.can.rx"
            }),
            Some("dir.bridge.CanBus" | "dir.can.Bus" | "dir.can.MultibusBus") => {
                Some(if d.gates()[gate] {
                    "composite.can.rx"
                } else {
                    "composite.can.tx"
                })
            }
            Some("dir.bridge.EthEndpoint" | "dir.ethernet.Endpoint" | "dir.ethernet.Switch") => {
                Some("ethernet.l2.frame.v1")
            }
            _ => None,
        }
    }
    fn default_literal(&self, d: &Declaration, name: &str) -> Option<&'static str> {
        if !bridge(d) {
            return None;
        }
        match (d.implementation(), name) {
            (_, "queueCapacity") => Some("64"),
            (_, "txProcessingDelay" | "rxProcessingDelay" | "delay") => Some("0ps"),
            (_, "rxFilter") => Some("\"*\""),
            (Some("dir.bridge.CanBus"), "bitrate") => Some("500kbps"),
            (Some("dir.bridge.EthLink"), "bitrate") => Some("1Gbps"),
            _ => None,
        }
    }
    fn defaults(&self, d: &Declaration) -> Values {
        match d.implementation() {
            Some("dir.bridge.CanController") => BTreeMap::from([
                ("queueCapacity".into(), TypedValue::Integer(64)),
                ("txProcessingDelay".into(), TypedValue::Quantity(0)),
                ("rxProcessingDelay".into(), TypedValue::Quantity(0)),
                ("rxFilter".into(), TypedValue::String("*".into())),
            ]),
            Some("dir.bridge.EthEndpoint") => BTreeMap::from([
                ("queueCapacity".into(), TypedValue::Integer(64)),
                ("txProcessingDelay".into(), TypedValue::Quantity(0)),
                ("rxProcessingDelay".into(), TypedValue::Quantity(0)),
            ]),
            Some("dir.bridge.CanBus") => {
                BTreeMap::from([("bitrate".into(), TypedValue::Quantity(500_000))])
            }
            Some("dir.bridge.EthLink") => BTreeMap::from([
                ("bitrate".into(), TypedValue::Quantity(1_000_000_000)),
                ("delay".into(), TypedValue::Quantity(0)),
            ]),
            _ => BTreeMap::new(),
        }
    }
}
pub(crate) fn resolve(
    types: &BTreeMap<String, Declaration>,
    network: &str,
    overrides: &BTreeMap<String, String>,
    channels: &BTreeMap<String, BTreeMap<String, String>>,
) -> Result<(PreparedCan, PreparedEthernet, Vec<String>, usize)> {
    let resolved = ned::resolve(types, network, overrides, &Rules)?;
    let channels = resolved.resolve_channels(channels, &Rules)?;
    let can = super::super::can::resolve_prepared(&resolved, &channels, "can.cc.multibus.v1")?;
    let (ethernet, _, _) =
        super::super::ethernet::resolve_prepared(&resolved, &channels, "can.ethernet.gateway.v1")?;
    if can.controllers.is_empty() {
        return Err(error("composite requires CAN controllers"));
    }
    Ok((
        PreparedCan {
            bus_id: can.bus_id,
            bitrate: can.bitrate,
            buses: can.buses,
            controller_buses: can.controller_buses,
            controllers: can.controllers,
            generators: Vec::new(),
        },
        ethernet,
        resolved.module_paths(),
        channels.len(),
    ))
}

pub(crate) fn validate_parameter_literal(d: &Declaration, name: &str, value: &str) -> Result<()> {
    let parameter = d
        .parameters()
        .get(name)
        .ok_or_else(|| error("unknown bridge parameter"))?;
    Rules.validate_value(d, name, &ned::typed_value(parameter, value)?)
}
