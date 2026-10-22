//! Exact compile-time validation of independent node clocks and adjustments.
use super::{Result, error, exact};
use crate::runtime::network::clock::NodeClock;
use crate::types::network::{
    PreparedClockAction, PreparedClockAdjustment, PreparedClocks, PreparedNetwork,
    PreparedNodeClock,
};
use serde_json::Value;
use std::collections::BTreeSet;

fn decimal(v: &Value, target: &str) -> Result<u64> {
    let s = v
        .as_str()
        .ok_or_else(|| error("expected canonical u64 decimal").with_target(target))?;
    let n: u64 = s
        .parse()
        .map_err(|_| error("expected canonical u64 decimal").with_target(target))?;
    if s != n.to_string() {
        return Err(error("noncanonical u64 decimal").with_target(target));
    }
    Ok(n)
}
fn signed(v: &Value, target: &str) -> Result<i128> {
    let s = v
        .as_str()
        .ok_or_else(|| error("expected canonical signed decimal").with_target(target))?;
    let n: i128 = s
        .parse()
        .map_err(|_| error("expected canonical signed decimal").with_target(target))?;
    if s != n.to_string() {
        return Err(error("noncanonical signed decimal").with_target(target));
    }
    Ok(n)
}
fn rows<'a>(v: &'a Value, target: &str) -> Result<&'a Vec<Value>> {
    v.as_array()
        .ok_or_else(|| error("expected array").with_target(target))
}
pub(super) fn prepare(v: &Value, n: &PreparedNetwork) -> Result<PreparedClocks> {
    exact(v, &["nodes", "adjustments"], "clocks")?;
    let mut initial = vec![
        PreparedNodeClock {
            offset_ps: 0,
            rate_num: 1,
            rate_den: 1
        };
        n.ethernet.devices.len()
    ];
    let mut seen = BTreeSet::new();
    for (i, item) in rows(&v["nodes"], "/clocks/nodes")?.iter().enumerate() {
        let target = format!("/clocks/nodes/{i}");
        exact(
            item,
            &["node", "offset_ps", "rate_num", "rate_den"],
            "node clock",
        )?;
        let node = item["node"]
            .as_str()
            .ok_or_else(|| error("expected node ID").with_target(format!("{target}/node")))?;
        let index = n
            .ethernet
            .devices
            .iter()
            .position(|d| d.id == node)
            .ok_or_else(|| error("unknown clock node").with_target(format!("{target}/node")))?;
        if !seen.insert(index) {
            return Err(error("duplicate clock node").with_target(format!("{target}/node")));
        }
        let offset_ps = signed(&item["offset_ps"], &format!("{target}/offset_ps"))?;
        let rate_num = decimal(&item["rate_num"], &format!("{target}/rate_num"))?;
        let rate_den = decimal(&item["rate_den"], &format!("{target}/rate_den"))?;
        NodeClock::new(0, offset_ps, rate_num, rate_den)
            .map_err(|e| error(e.to_string()).with_target(&target))?;
        initial[index] = PreparedNodeClock {
            offset_ps,
            rate_num,
            rate_den,
        };
    }
    let mut adjustments = Vec::new();
    let mut keys = BTreeSet::new();
    for (i, item) in rows(&v["adjustments"], "/clocks/adjustments")?
        .iter()
        .enumerate()
    {
        let target = format!("/clocks/adjustments/{i}");
        let action = match item["kind"].as_str() {
            Some("step") => {
                exact(item, &["node", "at_ps", "kind", "offset_ps"], "clock step")?;
                PreparedClockAction::StepBy {
                    offset_ps: signed(&item["offset_ps"], &format!("{target}/offset_ps"))?,
                }
            }
            Some("slew") => {
                exact(
                    item,
                    &["node", "at_ps", "kind", "rate_num", "rate_den"],
                    "clock slew",
                )?;
                PreparedClockAction::Slew {
                    rate_num: decimal(&item["rate_num"], &format!("{target}/rate_num"))?,
                    rate_den: decimal(&item["rate_den"], &format!("{target}/rate_den"))?,
                }
            }
            _ => {
                return Err(
                    error("clock kind must be step or slew").with_target(format!("{target}/kind"))
                );
            }
        };
        let node = item["node"]
            .as_str()
            .ok_or_else(|| error("expected node ID").with_target(format!("{target}/node")))?;
        let index = n
            .ethernet
            .devices
            .iter()
            .position(|d| d.id == node)
            .ok_or_else(|| error("unknown clock node").with_target(format!("{target}/node")))?;
        let at_ps = decimal(&item["at_ps"], &format!("{target}/at_ps"))?;
        if !keys.insert((at_ps, index)) {
            return Err(error("duplicate clock adjustment time/node").with_target(&target));
        }
        adjustments.push(PreparedClockAdjustment {
            node: index,
            at_ps,
            action,
        });
    }
    adjustments.sort_by_key(|a| (a.at_ps, a.node));
    let mut projected = initial
        .iter()
        .map(|c| NodeClock::new(0, c.offset_ps, c.rate_num, c.rate_den).unwrap())
        .collect::<Vec<_>>();
    for (i, adjustment) in adjustments.iter().enumerate() {
        let clock = &mut projected[adjustment.node];
        match adjustment.action {
            PreparedClockAction::StepBy { offset_ps } => clock.step_by(adjustment.at_ps, offset_ps),
            PreparedClockAction::Slew { rate_num, rate_den } => {
                clock.slew(adjustment.at_ps, rate_num, rate_den)
            }
        }
        .map_err(|e| error(e.to_string()).with_target(format!("/clocks/adjustments/{i}")))?;
    }
    Ok(PreparedClocks {
        initial,
        adjustments,
    })
}
