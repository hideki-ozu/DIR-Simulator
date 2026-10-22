//! Pure record export helpers; exporting never advances protocol or media state.
use crate::runtime::wire::transition::{self, canonical_u64 as canonical};
use crate::snapshot::wire::*;
use crate::types::Diagnostic;
use serde_json::Value;
use std::net::IpAddr;
fn error(s: impl Into<String>) -> Diagnostic {
    Diagnostic::execution(s)
}
pub(crate) fn validate_record(kind: &str, value: &Value) -> Result<(), Diagnostic> {
    let kind = kind.strip_suffix("/1").unwrap_or(kind);
    let fields: &[&str] = match kind {
        "ethernet.control.wire.event" => &[
            "schema_version",
            "adapter_id",
            "protocol",
            "protocol_revision",
            "bridge",
            "port",
            "event_id",
            "cause_id",
            "event_kind",
            "time_ps",
            "phase",
            "batch_ordinal",
            "packet_id",
            "bytes_hex",
            "mac_bytes",
            "outcome",
            "reason",
            "policy_epoch_before",
            "policy_epoch_after",
            "protocol_generation_before",
            "protocol_generation_after",
            "timer",
            "effects",
            "external_injection",
        ],
        "ethernet.control.wire.policy" => &[
            "schema_version",
            "time_ps",
            "policy_epoch",
            "topology_generation",
            "ports",
            "registered",
            "mac",
        ],
        "ethernet.control.wire.tree_policy" => &[
            "schema_version",
            "time_ps",
            "policy_epoch",
            "component_id",
            "bridge",
            "mstid",
            "mapped_vids",
            "port",
            "policy",
            "boundary",
            "region_name",
            "region_revision",
            "region_digest_hex",
            "root_port",
        ],
        "ethernet.control.wire.transition" => {
            return transition::validate_transition(value).map_err(error);
        }
        "ethernet.wire.boundary" => &[
            "effect_seq",
            "packet_id",
            "ingress_port",
            "time_ps",
            "event",
            "status",
            "actual_start_ps",
            "actual_arrival_ps",
            "actual_release_ps",
        ],
        "ethernet.wire.membership" => &[
            "effect_seq",
            "packet_id",
            "protocol_revision",
            "role",
            "switch",
            "ingress_port",
            "vid",
            "family",
            "time_ps",
            "epoch_before",
            "epoch_after",
            "outcome",
            "reason",
            "message_kind",
            "byte_length",
            "record_count",
            "query",
            "compatibility_flood",
            "changes",
            "forwarded_ports",
        ],
        "ethernet.wire.membership_timer" => &[
            "effect_seq",
            "switch",
            "time_ps",
            "scheduled_ps",
            "wake_generation",
            "outcome",
            "epoch_before",
            "epoch_after",
            "expired_leases",
            "affected_partitions",
            "projected_controls",
        ],
        _ => return Err(error("unknown wire record kind")),
    };
    exact(value, fields)?;
    match kind {
        "ethernet.control.wire.event" => {
            let r: WireEventRecord =
                serde_json::from_value(value.clone()).map_err(|e| error(e.to_string()))?;
            if r.schema_version != 1
                || r.phase > 2
                || ![
                    "initialize",
                    "receive",
                    "rx_failed",
                    "timer",
                    "link",
                    "local",
                    "tx_queued",
                    "tx_started",
                    "tx_completed",
                ]
                .contains(&r.event_kind.as_str())
                || !["changed", "no_op", "control_drop", "stale"].contains(&r.outcome.as_str())
                || [
                    &r.time_ps,
                    &r.batch_ordinal,
                    &r.policy_epoch_before,
                    &r.policy_epoch_after,
                    &r.protocol_generation_before,
                    &r.protocol_generation_after,
                ]
                .iter()
                .any(|s| !canonical(s))
            {
                return Err(error("invalid wire event scalar"));
            }
            validate_event(value)?;
            for effect in &r.effects {
                validate_effect(effect)?;
            }
            if matches!(r.outcome.as_str(), "control_drop" | "stale") && !r.effects.is_empty() {
                return Err(error("drop/stale event has effects"));
            }
        }
        "ethernet.control.wire.policy" => {
            let r: WirePolicyRecord =
                serde_json::from_value(value.clone()).map_err(|e| error(e.to_string()))?;
            if r.schema_version != 1
                || [&r.time_ps, &r.policy_epoch, &r.topology_generation]
                    .iter()
                    .any(|s| !canonical(s))
                || r.ports.values().any(|p| {
                    p.forwarding && !p.learning
                        || ![
                            "Root",
                            "Designated",
                            "Alternate",
                            "Backup",
                            "Disabled",
                            "Master",
                        ]
                        .contains(&p.role.as_str())
                })
            {
                return Err(error("invalid wire policy"));
            }
            for (name, p) in value["ports"]
                .as_object()
                .ok_or_else(|| error("wire ports object required"))?
            {
                if name.is_empty() {
                    return Err(error("wire policy port ID required"));
                }
                validate_policy(p)?;
            }
            for r in &r.registered {
                exact(r, &["port", "vid", "generation"])?;
                if !text(&r["port"]) || !vid(&r["vid"]) || !dec(&r["generation"]) {
                    return Err(error("invalid registered policy row"));
                }
            }
            let identities: Vec<_> = r
                .registered
                .iter()
                .map(|r| (r["port"].as_str().unwrap(), r["vid"].as_u64().unwrap()))
                .collect();
            if identities.windows(2).any(|w| w[0] >= w[1]) {
                return Err(error("registered policy order"));
            }
            let mut keys = Vec::new();
            for m in &r.mac {
                exact(
                    m,
                    &[
                        "switch",
                        "vid",
                        "mac",
                        "port",
                        "expires_at_ps",
                        "generation",
                    ],
                )?;
                validate_mac_key(m, false)?;
                if !text(&m["port"]) || !dec(&m["expires_at_ps"]) || !dec(&m["generation"]) {
                    return Err(error("invalid wire MAC entry"));
                }
                keys.push((
                    m["switch"].as_str().unwrap(),
                    m["vid"].as_u64().unwrap(),
                    m["mac"].as_str().unwrap(),
                ));
            }
            if keys.windows(2).any(|w| w[0] >= w[1]) {
                return Err(error("wire MAC policy order"));
            }
        }
        "ethernet.control.wire.tree_policy" => validate_tree_policy(value)?,
        "ethernet.wire.boundary" => {
            let r: WireBoundaryRecord =
                serde_json::from_value(value.clone()).map_err(|e| error(e.to_string()))?;
            if !text(&value["packet_id"])
                || !text(&value["ingress_port"])
                || !canonical(&r.effect_seq)
                || !canonical(&r.time_ps)
                || !["start", "arrival", "release"].contains(&r.event.as_str())
                || !["receiving", "received", "released"].contains(&r.status.as_str())
                || [r.actual_start_ps, r.actual_arrival_ps, r.actual_release_ps]
                    .into_iter()
                    .flatten()
                    .any(|s| !canonical(&s))
            {
                return Err(error("invalid boundary record"));
            }
            let expected = match r.event.as_str() {
                "start" => "receiving",
                "arrival" => "received",
                _ => "released",
            };
            if r.status != expected {
                return Err(error("wire boundary event/status mismatch"));
            }
        }
        "ethernet.wire.membership" => {
            let r: WireMembershipRecord =
                serde_json::from_value(value.clone()).map_err(|e| error(e.to_string()))?;
            if !["accepted", "control_drop", "stale_timer", "expired"].contains(&r.outcome.as_str())
                || !["query", "report", "legacy", "timer", "unknown"]
                    .contains(&r.message_kind.as_str())
                || !["ipv4", "ipv6"].contains(&r.family.as_str())
                || !(1..=4094).contains(&r.vid)
                || r.role != "passive_conservative_snooper_v1"
                || [&r.effect_seq, &r.time_ps, &r.epoch_before, &r.epoch_after]
                    .iter()
                    .any(|s| !canonical(s))
            {
                return Err(error("invalid membership record"));
            }
            validate_membership(value)?;
        }
        "ethernet.wire.membership_timer" => {
            let r: WireMembershipTimerRecord =
                serde_json::from_value(value.clone()).map_err(|e| error(e.to_string()))?;
            if !text(&value["switch"])
                || !["drained", "stale_timer"].contains(&r.outcome.as_str())
                || [
                    &r.effect_seq,
                    &r.time_ps,
                    &r.scheduled_ps,
                    &r.wake_generation,
                    &r.epoch_before,
                    &r.epoch_after,
                ]
                .iter()
                .any(|s| !canonical(s))
            {
                return Err(error("invalid membership timer"));
            }
        }
        _ => return Err(error("unknown wire record kind")),
    }
    Ok(())
}
fn exact(v: &Value, fields: &[&str]) -> Result<(), Diagnostic> {
    let o = v
        .as_object()
        .ok_or_else(|| error("wire nested object required"))?;
    if o.len() != fields.len() || fields.iter().any(|f| !o.contains_key(*f)) {
        return Err(error("wire nested fields mismatch"));
    }
    Ok(())
}
fn validate_effect(v: &Value) -> Result<(), Diagnostic> {
    let fields: &[&str] = match v["kind"].as_str() {
        Some("port_policy") => &["kind", "port", "before", "after"],
        Some("registration") => &["kind", "port", "vid", "before", "after", "generation"],
        Some("timer_arm" | "timer_cancel") => &["kind", "key", "token", "deadline_ps"],
        Some("tx") => &["kind", "packet_id", "port", "ordinal"],
        Some("mac_flush") => &["kind", "reason", "port", "vids", "keys", "completed"],
        _ => return Err(error("unknown wire effect")),
    };
    exact(v, fields)?;
    match v["kind"].as_str().unwrap() {
        "port_policy" => {
            if !text(&v["port"]) {
                return Err(error("wire effect port required"));
            }
            for side in ["before", "after"] {
                validate_policy(&v[side])?;
                let p: WirePortPolicy =
                    serde_json::from_value(v[side].clone()).map_err(|e| error(e.to_string()))?;
                if p.forwarding && !p.learning {
                    return Err(error("wire forwarding without learning"));
                }
            }
        }
        "registration" => {
            if !text(&v["port"])
                || v["before"].as_bool().is_none()
                || v["after"].as_bool().is_none()
                || !v["vid"].as_u64().is_some_and(|n| (1..=4094).contains(&n))
                || !v["generation"].as_str().is_some_and(canonical)
            {
                return Err(error("invalid registration effect"));
            }
        }
        "timer_arm" | "timer_cancel" => {
            validate_timer_key(&v["key"])?;
            if !v["token"].as_str().is_some_and(canonical)
                || if v["kind"] == "timer_arm" {
                    !v["deadline_ps"].as_str().is_some_and(canonical)
                } else {
                    !v["deadline_ps"].is_null()
                }
            {
                return Err(error("invalid wire timer effect"));
            }
        }
        "tx" => {
            if !text(&v["packet_id"])
                || !text(&v["port"])
                || !v["ordinal"].as_str().is_some_and(canonical)
            {
                return Err(error("invalid TX ordinal"));
            }
        }
        "mac_flush" => {
            if !text(&v["port"])
                || v["completed"] != true
                || !matches!(
                    v["reason"].as_str(),
                    Some("rstp_topology_change" | "mstp_topology_change" | "mvrp_new")
                )
            {
                return Err(error("invalid flush completion"));
            }
            for key in v["keys"]
                .as_array()
                .ok_or_else(|| error("wire MAC keys array required"))?
            {
                validate_mac_key(key, true)?;
            }
            let keys: Vec<WireMacKey> =
                serde_json::from_value(v["keys"].clone()).map_err(|e| error(e.to_string()))?;
            let vids: Vec<u16> =
                serde_json::from_value(v["vids"].clone()).map_err(|e| error(e.to_string()))?;
            if keys.windows(2).any(|w| w[0] >= w[1])
                || vids.windows(2).any(|w| w[0] >= w[1])
                || vids.iter().any(|n| !(1..=4094).contains(n))
                || keys.iter().any(|k| !vids.contains(&k.vid))
            {
                return Err(error("wire flush scope/order"));
            }
        }
        _ => unreachable!(),
    }
    Ok(())
}

fn text(v: &Value) -> bool {
    v.as_str().is_some_and(|s| !s.is_empty())
}
fn dec(v: &Value) -> bool {
    v.as_str().is_some_and(canonical)
}
fn vid(v: &Value) -> bool {
    v.as_u64().is_some_and(|v| (1..=4094).contains(&v))
}
fn optional_text(v: &Value) -> bool {
    v.is_null() || text(v)
}
fn optional_decimal(v: &Value) -> bool {
    v.is_null() || dec(v)
}
fn member(v: &Value, values: &[&str]) -> bool {
    v.as_str().is_some_and(|s| values.contains(&s))
}
fn sorted_ports(v: &Value) -> bool {
    v.as_array()
        .is_some_and(|a| a.iter().all(text) && a.windows(2).all(|w| w[0].as_str() < w[1].as_str()))
}
fn validate_policy(v: &Value) -> Result<(), Diagnostic> {
    exact(v, &["role", "learning", "forwarding", "link_up"])?;
    if !member(
        &v["role"],
        &[
            "Disabled",
            "Root",
            "Designated",
            "Alternate",
            "Backup",
            "Master",
        ],
    ) || ["learning", "forwarding", "link_up"]
        .iter()
        .any(|k| !v[*k].is_boolean())
        || v["forwarding"] == true && v["learning"] != true
    {
        return Err(error("invalid wire port policy"));
    }
    Ok(())
}
fn validate_tree_policy(v: &Value) -> Result<(), Diagnostic> {
    if v["schema_version"] != 1
        || !dec(&v["time_ps"])
        || !dec(&v["policy_epoch"])
        || !["component_id", "bridge", "port"]
            .iter()
            .all(|k| text(&v[*k]))
        || v["mstid"].as_u64().is_none_or(|n| n > 4094)
        || !v["boundary"].is_boolean()
        || v["region_name"].as_str().is_none_or(|s| s.len() > 32)
        || v["region_revision"]
            .as_u64()
            .is_none_or(|n| n > u16::MAX as u64)
        || !v["region_digest_hex"].as_str().is_some_and(|s| {
            s.len() == 32
                && s.bytes()
                    .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
        })
        || !optional_text(&v["root_port"])
    {
        return Err(error("invalid MSTP tree policy scalar"));
    }
    let vids = v["mapped_vids"]
        .as_array()
        .ok_or_else(|| error("MSTP mapped VID array required"))?;
    if !vids.iter().all(vid)
        || vids.windows(2).any(|w| w[0].as_u64() >= w[1].as_u64())
        || v["mstid"] == 0 && v["policy"]["role"] == "Master"
    {
        return Err(error("invalid MSTP mapped VID scope/order"));
    }
    validate_policy(&v["policy"])
}
fn validate_timer_key(v: &Value) -> Result<(), Diagnostic> {
    exact(v, &["protocol", "bridge", "port", "slot", "attribute"])?;
    if !text(&v["bridge"]) {
        return Err(error("wire timer bridge required"));
    }
    let good = match (v["protocol"].as_str(), v["slot"].as_str()) {
        (Some("rstp" | "mstp"), Some("tick")) | (Some("igmp" | "mld"), Some("membership")) => {
            v["port"] == "" && v["attribute"].is_null()
        }
        (Some("mvrp"), Some("leave")) => text(&v["port"]) && vid(&v["attribute"]),
        (Some("mvrp"), Some("join" | "leaveall" | "periodic")) => {
            text(&v["port"]) && v["attribute"].is_null()
        }
        _ => false,
    };
    if !good {
        return Err(error("invalid wire timer key scope"));
    }
    Ok(())
}
fn validate_mac_key(v: &Value, key_only: bool) -> Result<(), Diagnostic> {
    if key_only {
        exact(v, &["switch", "vid", "mac"])?;
    }
    if !text(&v["switch"])
        || !vid(&v["vid"])
        || !v["mac"].as_str().is_some_and(|s| {
            s.len() == 17
                && s.bytes().enumerate().all(|(i, b)| {
                    if i % 3 == 2 {
                        b == b':'
                    } else {
                        b.is_ascii_digit() || (b'a'..=b'f').contains(&b)
                    }
                })
        })
    {
        return Err(error("invalid wire MAC key"));
    }
    Ok(())
}
const MEMBERSHIP_REASONS: &[&str] = &[
    "ingress_ineligible",
    "ip_length",
    "envelope",
    "ip_checksum",
    "message_length",
    "checksum",
    "record_type",
    "address",
    "source_limit",
    "destination_mac",
    "unsupported_fragment",
    "unsupported_extension",
    "unknown_message",
];
fn validate_event(v: &Value) -> Result<(), Diagnostic> {
    if !["adapter_id", "bridge", "event_id"]
        .iter()
        .all(|k| text(&v[*k]))
        || v["port"].as_str().is_none()
        || !["cause_id", "packet_id"]
            .iter()
            .all(|k| optional_text(&v[*k]))
    {
        return Err(error("wire event identity invalid"));
    }
    let revision = match v["protocol"].as_str() {
        Some("rstp") => "ieee802.1q-2022.rstp.initial.v1",
        Some("mstp") => "ieee802.1q-2022.mstp.initial.v1",
        Some("mvrp") => "ieee802.1q-2022.mvrp.initial.v1",
        Some("igmp") => "igmp-rfc9776-2025",
        Some("mld") => "mld-rfc9777-2025",
        _ => return Err(error("unknown wire event protocol")),
    };
    if v["protocol_revision"] != revision {
        return Err(error("wire event revision mismatch"));
    }
    let reason = &v["reason"];
    match v["outcome"].as_str() {
        Some("changed" | "no_op") if reason.is_null() => {}
        Some("stale") if reason == "timer_token" => {}
        Some("control_drop")
            if member(
                reason,
                &[
                    "link_down",
                    "frame_bounds",
                    "fcs",
                    "protocol_malformed",
                    "unsupported_revision",
                    "unsupported_kind",
                    "link_epoch_reset",
                    "mac_merge_packet_lost",
                    "mac_merge_receive_error",
                    "mac_merge_transport_failure",
                    "service_tree_changed",
                ],
            ) || member(&v["protocol"], &["igmp", "mld"])
                && member(reason, MEMBERSHIP_REASONS) => {}
        _ => return Err(error("wire event outcome/reason mismatch")),
    }
    if !v["bytes_hex"].is_null() {
        let hex = v["bytes_hex"]
            .as_str()
            .ok_or_else(|| error("wire bytes hex string required"))?;
        if hex.len() % 2 != 0
            || !hex
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
            || v["mac_bytes"].as_u64() != Some((hex.len() / 2) as u64)
            || !text(&v["packet_id"])
        {
            return Err(error("invalid wire packet bytes"));
        }
    } else if !v["mac_bytes"].is_null() {
        return Err(error("wire bytes length without bytes"));
    }
    if !v["timer"].is_null() {
        exact(&v["timer"], &["key", "token", "deadline_ps"])?;
        validate_timer_key(&v["timer"]["key"])?;
        if !dec(&v["timer"]["token"]) || !dec(&v["timer"]["deadline_ps"]) {
            return Err(error("invalid wire event timer"));
        }
    }
    Ok(())
}
fn ip(v: &Value, v6: bool, multicast: Option<bool>) -> Option<IpAddr> {
    let s = v.as_str()?;
    let a: IpAddr = s.parse().ok()?;
    (a.is_ipv6() == v6 && a.to_string() == s && multicast.is_none_or(|m| a.is_multicast() == m))
        .then_some(a)
}
fn validate_membership(v: &Value) -> Result<(), Diagnostic> {
    let v6 = v["family"] == "ipv6";
    let revision = if v6 {
        "mld-rfc9777-2025"
    } else {
        "igmp-rfc9776-2025"
    };
    if v["protocol_revision"] != revision
        || !text(&v["switch"])
        || !text(&v["ingress_port"])
        || !optional_text(&v["packet_id"])
        || !sorted_ports(&v["forwarded_ports"])
    {
        return Err(error("invalid membership identity/ports"));
    }
    let drop = v["outcome"] == "control_drop";
    let stale = v["outcome"] == "stale_timer";
    if if drop {
        !member(&v["reason"], MEMBERSHIP_REASONS)
    } else if stale {
        v["reason"] != "timer_token"
    } else {
        !v["reason"].is_null()
    } {
        return Err(error("membership reason mismatch"));
    }
    let changes = v["changes"]
        .as_array()
        .ok_or_else(|| error("membership changes array required"))?;
    if (drop || stale)
        && (!changes.is_empty() || !v["forwarded_ports"].as_array().unwrap().is_empty())
    {
        return Err(error("dropped membership has effects"));
    }
    if !v["query"].is_null() {
        let q = &v["query"];
        exact(
            q,
            &[
                "group",
                "suppress",
                "qrv",
                "qqic",
                "response_code",
                "response_ps",
                "interval_ps",
            ],
        )?;
        if !ip(&q["group"], v6, None).is_some_and(|g| g.is_multicast() || g.is_unspecified())
            || !q["suppress"].is_boolean()
            || q["qrv"].as_u64().is_none_or(|n| n > 7)
            || q["qqic"].as_u64().is_none_or(|n| n > 255)
            || q["response_code"]
                .as_u64()
                .is_none_or(|n| n > if v6 { 65535 } else { 255 })
            || !dec(&q["response_ps"])
            || !dec(&q["interval_ps"])
        {
            return Err(error("invalid membership query"));
        }
    }
    for c in changes {
        exact(
            c,
            &[
                "key",
                "lease_mode",
                "sources",
                "expires_before_ps",
                "expires_after_ps",
                "generation",
                "action",
            ],
        )?;
        let k = &c["key"];
        exact(k, &["switch", "vid", "family", "group", "port"])?;
        if k["switch"] != v["switch"]
            || k["vid"] != v["vid"]
            || k["family"] != v["family"]
            || k["port"] != v["ingress_port"]
        {
            return Err(error("membership change scope mismatch"));
        }
        let mode = c["lease_mode"]
            .as_str()
            .ok_or_else(|| error("membership lease mode required"))?;
        let source_values = c["sources"]
            .as_array()
            .ok_or_else(|| error("membership sources array required"))?;
        let sources = source_values
            .iter()
            .map(|s| ip(s, v6, Some(false)).filter(|a| !a.is_unspecified()))
            .collect::<Option<Vec<_>>>()
            .ok_or_else(|| error("invalid membership source"))?;
        if sources.windows(2).any(|w| w[0] >= w[1])
            || !["expires_before_ps", "expires_after_ps", "generation"]
                .iter()
                .all(|k| optional_decimal(&c[*k]))
        {
            return Err(error("invalid membership change types/order"));
        }
        if mode == "compatibility" {
            if !k["group"].is_null()
                || !sources.is_empty()
                || c["action"] != "enable_flood"
                || !["expires_before_ps", "expires_after_ps", "generation"]
                    .iter()
                    .all(|k| c[*k].is_null())
            {
                return Err(error("invalid compatibility observation"));
            }
        } else {
            if mode == "router" {
                if !k["group"].is_null() || !sources.is_empty() {
                    return Err(error("invalid router lease"));
                }
            } else if !["include", "exclude"].contains(&mode)
                || ip(&k["group"], v6, Some(true)).is_none()
            {
                return Err(error("invalid member lease"));
            }
            if !dec(&c["generation"]) {
                return Err(error("membership generation required"));
            }
            let valid = match c["action"].as_str() {
                Some("insert") => c["expires_before_ps"].is_null() && dec(&c["expires_after_ps"]),
                Some("refresh") => dec(&c["expires_before_ps"]) && dec(&c["expires_after_ps"]),
                Some("expire") => dec(&c["expires_before_ps"]) && c["expires_after_ps"].is_null(),
                _ => false,
            };
            if !valid {
                return Err(error("membership action/lease mismatch"));
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    fn policy() -> Value {
        json!({"role":"Designated","learning":true,"forwarding":true,"link_up":true})
    }
    fn key() -> Value {
        json!({"protocol":"mvrp","bridge":"b","port":"p","slot":"leave","attribute":20})
    }
    fn event() -> Value {
        json!({"schema_version":1,"adapter_id":"a","protocol":"mvrp","protocol_revision":"ieee802.1q-2022.mvrp.initial.v1","bridge":"b","port":"p","event_id":"e","cause_id":null,"event_kind":"timer","time_ps":"0","phase":0,"batch_ordinal":"0","packet_id":null,"bytes_hex":null,"mac_bytes":null,"outcome":"changed","reason":null,"policy_epoch_before":"0","policy_epoch_after":"1","protocol_generation_before":"0","protocol_generation_after":"1","timer":{"key":key(),"token":"1","deadline_ps":"0"},"effects":[],"external_injection":false})
    }
    fn membership() -> Value {
        json!({"effect_seq":"0","packet_id":"packet","protocol_revision":"igmp-rfc9776-2025","role":"passive_conservative_snooper_v1","switch":"b","ingress_port":"p","vid":20,"family":"ipv4","time_ps":"0","epoch_before":"0","epoch_after":"1","outcome":"accepted","reason":null,"message_kind":"query","byte_length":40,"record_count":0,"query":{"group":"0.0.0.0","suppress":false,"qrv":2,"qqic":125,"response_code":100,"response_ps":"1000","interval_ps":"125000"},"compatibility_flood":false,"changes":[{"key":{"switch":"b","vid":20,"family":"ipv4","group":null,"port":"p"},"lease_mode":"router","sources":[],"expires_before_ps":null,"expires_after_ps":"260000","generation":"1","action":"insert"}],"forwarded_ports":["q","r"]})
    }
    #[test]
    fn nested_effect_catalog_is_exact_and_typed() {
        let mac = json!({"switch":"b","vid":20,"mac":"02:00:00:00:00:01"});
        let effects = vec![
            json!({"kind":"port_policy","port":"p","before":policy(),"after":policy()}),
            json!({"kind":"registration","port":"p","vid":20,"before":false,"after":true,"generation":"9007199254740993"}),
            json!({"kind":"timer_arm","key":key(),"token":"1","deadline_ps":"18446744073709551615"}),
            json!({"kind":"timer_cancel","key":key(),"token":"2","deadline_ps":null}),
            json!({"kind":"tx","packet_id":"packet","port":"p","ordinal":"1"}),
            json!({"kind":"mac_flush","reason":"mvrp_new","port":"p","vids":[20],"keys":[mac],"completed":true}),
        ];
        for effect in effects {
            validate_effect(&effect).unwrap();
            for field in effect.as_object().unwrap().keys() {
                let mut bad = effect.clone();
                bad.as_object_mut().unwrap().remove(field);
                assert!(validate_effect(&bad).is_err(), "missing {field}: {effect}");
            }
            let mut bad = effect.clone();
            bad["extra"] = json!(0);
            assert!(validate_effect(&bad).is_err());
        }
        for bad in [
            json!({"kind":"unknown"}),
            json!({"kind":"tx","packet_id":null,"port":"p","ordinal":"0"}),
            json!({"kind":"registration","port":"p","vid":0,"before":false,"after":true,"generation":"1"}),
            json!({"kind":"timer_arm","key":key(),"token":"01","deadline_ps":"1"}),
            json!({"kind":"timer_cancel","key":key(),"token":"1","deadline_ps":"1"}),
            json!({"kind":"mac_flush","reason":"mvrp_new","port":"p","vids":[20,20],"keys":[],"completed":true}),
        ] {
            assert!(validate_effect(&bad).is_err(), "{bad}");
        }
        let mut bad = policy();
        bad["role"] = json!("Unknown");
        assert!(validate_policy(&bad).is_err());
        bad = policy();
        bad["link_up"] = json!("true");
        assert!(validate_policy(&bad).is_err());
        bad = policy();
        bad["learning"] = json!(false);
        assert!(validate_policy(&bad).is_err());
        for (field, value) in [
            ("extra", json!(0)),
            ("attribute", Value::Null),
            ("attribute", json!(4095)),
            ("protocol", json!("rstp")),
            ("slot", json!("unknown")),
            ("port", json!("")),
        ] {
            let mut bad = key();
            bad[field] = value;
            assert!(validate_timer_key(&bad).is_err());
        }
    }
    #[test]
    fn event_timer_and_reason_catalog_is_closed() {
        let good = event();
        validate_record("ethernet.control.wire.event", &good).unwrap();
        for pointer in ["/timer/extra", "/timer/key/extra"] {
            let mut bad = good.clone();
            let (parent, key) = pointer.rsplit_once('/').unwrap();
            bad.pointer_mut(parent)
                .unwrap()
                .as_object_mut()
                .unwrap()
                .insert(key.into(), json!(true));
            assert!(validate_record("ethernet.control.wire.event", &bad).is_err());
        }
        for (pointer, value) in [
            ("/timer/token", json!("01")),
            ("/timer/deadline_ps", json!(0)),
            ("/timer/key/attribute", json!("20")),
            ("/timer/key/slot", json!("unknown")),
            ("/reason", json!("protocol_malformed")),
            ("/protocol_revision", json!("arbitrary")),
        ] {
            let mut bad = good.clone();
            *bad.pointer_mut(pointer).unwrap() = value;
            assert!(validate_record("ethernet.control.wire.event", &bad).is_err());
        }
        let mut drop = good.clone();
        drop["outcome"] = json!("control_drop");
        drop["reason"] = json!("protocol_malformed");
        validate_record("ethernet.control.wire.event", &drop).unwrap();
        drop["reason"] = json!("rst_short");
        assert!(validate_record("ethernet.control.wire.event", &drop).is_err());
        for field in good.as_object().unwrap().keys() {
            let mut bad = good.clone();
            bad.as_object_mut().unwrap().remove(field);
            assert!(
                validate_record("ethernet.control.wire.event", &bad).is_err(),
                "missing {field}"
            );
        }
    }
    #[test]
    fn mstp_events_and_per_instance_scope_are_typed() {
        let mut good = event();
        good["protocol"] = json!("mstp");
        good["protocol_revision"] = json!("ieee802.1q-2022.mstp.initial.v1");
        good["port"] = json!("");
        good["timer"]["key"] = json!({
            "protocol":"mstp","bridge":"b","port":"","slot":"tick","attribute":null
        });
        good["effects"] = json!([
            {"kind":"mac_flush","reason":"mstp_topology_change","port":"p", "vids":[10,20], "keys":[{"switch":"b","vid":20,"mac":"02:00:00:00:00:01"}],"completed":true}
        ]);
        validate_record("ethernet.control.wire.event/1", &good).unwrap();
        let tree = json!({
            "schema_version":1,"time_ps":"9007199254740993","policy_epoch":"2",
            "component_id":"mstp-b","bridge":"b","mstid":1,"mapped_vids":[10,20],
            "port":"p","policy":{"role":"Master","learning":true,"forwarding":true,"link_up":true},
            "boundary":true,"region_name":"region","region_revision":65535,
            "region_digest_hex":"00112233445566778899aabbccddeeff","root_port":"p"
        });
        validate_record("ethernet.control.wire.tree_policy/1", &tree).unwrap();
        for (pointer, value) in [
            ("/mstid", json!(0)),
            ("/mstid", json!(4095)),
            ("/mstid", json!("1")),
            ("/mapped_vids", json!([20, 10])),
            ("/mapped_vids", json!([10, 10])),
            ("/mapped_vids", json!([0])),
            ("/mapped_vids", json!(["10"])),
            ("/policy/learning", json!(false)),
            ("/policy_epoch", json!(2)),
            ("/time_ps", json!("01")),
            ("/boundary", json!("true")),
            ("/region_name", json!("あ".repeat(11))),
            ("/region_revision", json!(65536)),
            (
                "/region_digest_hex",
                json!("00112233445566778899AABBCCDDEEFF"),
            ),
            ("/root_port", json!("")),
        ] {
            let mut bad = tree.clone();
            *bad.pointer_mut(pointer).unwrap() = value;
            assert!(
                validate_record("ethernet.control.wire.tree_policy", &bad).is_err(),
                "{pointer}"
            );
        }
        for field in tree.as_object().unwrap().keys() {
            let mut bad = tree.clone();
            bad.as_object_mut().unwrap().remove(field);
            assert!(
                validate_record("ethernet.control.wire.tree_policy", &bad).is_err(),
                "missing {field}"
            );
        }
        let mut bad = tree;
        bad["extra"] = json!(true);
        assert!(validate_record("ethernet.control.wire.tree_policy", &bad).is_err());
    }
    #[test]
    fn policy_nested_rows_and_required_nullable_keys_are_closed() {
        let good = json!({"schema_version":1,"time_ps":"0","policy_epoch":"1","topology_generation":"2","ports":{"p":policy()},"registered":[{"port":"p","vid":20,"generation":"18446744073709551615"}],"mac":[{"switch":"b","vid":20,"mac":"02:00:00:00:00:01","port":"p","expires_at_ps":"100","generation":"1"}]});
        validate_record("ethernet.control.wire.policy", &good).unwrap();
        for pointer in ["/ports/p", "/registered/0", "/mac/0"] {
            let mut bad = good.clone();
            bad.pointer_mut(pointer)
                .unwrap()
                .as_object_mut()
                .unwrap()
                .insert("extra".into(), json!(false));
            assert!(validate_record("ethernet.control.wire.policy", &bad).is_err());
        }
        for (pointer, value) in [
            ("/registered/0/generation", json!(1)),
            ("/registered/0/vid", json!(4095)),
            ("/mac/0/expires_at_ps", json!("18446744073709551616")),
            ("/mac/0/mac", json!("bad")),
            ("/ports/p/role", json!("Unknown")),
        ] {
            let mut bad = good.clone();
            *bad.pointer_mut(pointer).unwrap() = value;
            assert!(validate_record("ethernet.control.wire.policy", &bad).is_err());
        }
        let mut bad = good.clone();
        bad["registered"]
            .as_array_mut()
            .unwrap()
            .push(good["registered"][0].clone());
        assert!(validate_record("ethernet.control.wire.policy", &bad).is_err());
    }
    #[test]
    fn membership_query_change_key_and_sources_are_strict() {
        let good = membership();
        validate_record("ethernet.wire.membership/1", &good).unwrap();
        for pointer in ["/query", "/changes/0", "/changes/0/key"] {
            let mut bad = good.clone();
            bad.pointer_mut(pointer)
                .unwrap()
                .as_object_mut()
                .unwrap()
                .insert("extra".into(), json!(false));
            assert!(validate_record("ethernet.wire.membership", &bad).is_err());
        }
        for (pointer, value) in [
            ("/query/response_ps", json!(1)),
            ("/query/qrv", json!(8)),
            ("/query/suppress", Value::Null),
            ("/query/group", json!("::")),
            ("/changes/0/key/vid", json!(21)),
            ("/changes/0/generation", json!("01")),
            ("/changes/0/expires_after_ps", Value::Null),
            ("/changes/0/sources", json!(["1.2.3.4"])),
            ("/forwarded_ports", json!(["r", "q"])),
            ("/changes/0/lease_mode", json!("unknown")),
        ] {
            let mut bad = good.clone();
            *bad.pointer_mut(pointer).unwrap() = value;
            assert!(
                validate_record("ethernet.wire.membership", &bad).is_err(),
                "{pointer}"
            );
        }
        let mut report = good.clone();
        report["query"] = Value::Null;
        report["message_kind"] = json!("report");
        report["changes"][0]["key"]["group"] = json!("239.1.1.1");
        report["changes"][0]["lease_mode"] = json!("include");
        report["changes"][0]["sources"] = json!(["1.1.1.1", "2.1.1.1"]);
        validate_record("ethernet.wire.membership", &report).unwrap();
        report["changes"][0]["sources"] = json!(["2.1.1.1", "1.1.1.1"]);
        assert!(validate_record("ethernet.wire.membership", &report).is_err());
        let mut bad = good.clone();
        bad["changes"][0]
            .as_object_mut()
            .unwrap()
            .remove("expires_before_ps");
        assert!(validate_record("ethernet.wire.membership", &bad).is_err());
    }
}
