//! Closed public wire transition catalog. Private reducer serialization stays private.
use super::rstp;
use serde_json::{Map, Value, json};
use std::collections::BTreeMap;

type Changes = BTreeMap<String, Value>;
const ROLES: &[&str] = &["Disabled", "Root", "Designated", "Alternate", "Backup"];
const INFO: &[&str] = &["Disabled", "Aged", "Mine", "Received"];
const RECEIVED: &[&str] = &[
    "None",
    "SuperiorDesignatedInfo",
    "RepeatedDesignatedInfo",
    "InferiorDesignatedInfo",
    "NotDesignatedInfo",
    "OtherInfo",
];
const KINDS: &[&str] = &["None", "Config", "Tcn", "Rst"];
const TIMES: &[&str] = &[
    "MessageAge_ticks",
    "MaxAge_ticks",
    "HelloTime_ticks",
    "ForwardDelay_ticks",
];
const COUNTERS: &[(&str, &str)] = &[
    ("edge", "edgeDelayWhile"),
    ("hello", "helloWhen"),
    ("migration", "mdelayWhile"),
    ("fd", "fdWhile"),
    ("rr", "rrWhile"),
    ("rb", "rbWhile"),
    ("info", "rcvdInfoWhile"),
    ("tc", "tcWhile"),
    ("detected", "tcDetected"),
    ("tx", "txCount"),
];
const PORT_FIELDS: &[(&str, &str)] = &[
    ("enabled", "portEnabled"),
    ("role", "role"),
    ("selected_role", "selectedRole"),
    ("info", "infoIs"),
    ("priority", "portPriority"),
    ("designated", "designatedPriority"),
    ("times", "portTimes"),
    ("designated_times", "designatedTimes"),
    ("selected", "selected"),
    ("reselect", "reselect"),
    ("update", "updtInfo"),
    ("send_rstp", "sendRSTP"),
    ("rcvd_stp", "rcvdSTP"),
    ("rcvd_rstp", "rcvdRSTP"),
    ("mcheck", "mcheck"),
    ("oper_edge", "operEdge"),
    ("isolate", "isolate"),
    ("proposing", "proposing"),
    ("proposed", "proposed"),
    ("agree", "agree"),
    ("agreed", "agreed"),
    ("disputed", "disputed"),
    ("sync", "sync"),
    ("synced", "synced"),
    ("reroot", "reRoot"),
    ("learn", "learn"),
    ("forward", "forward"),
    ("learning", "learning"),
    ("forwarding", "forwarding"),
    ("new_info", "newInfo"),
    ("tc_ack", "tcAck"),
    ("rcvd_tc", "rcvdTc"),
    ("rcvd_tcn", "rcvdTcn"),
    ("rcvd_tc_ack", "rcvdTcAck"),
    ("tc_prop", "tcProp"),
    ("fdb_flush", "fdbFlush"),
];
const STATES: &[(&str, &str)] = &[
    ("prx", "PRX"),
    ("ppm", "PPM"),
    ("bdm", "BDM"),
    ("ptx", "PTX"),
    ("pim", "PIM"),
    ("prt", "PRT"),
    ("pst", "PST"),
    ("tcm", "TCM"),
];

pub(crate) fn canonical_u64(s: &str) -> bool {
    !s.is_empty()
        && (s.len() == 1 || !s.starts_with('0'))
        && s.bytes().all(|b| b.is_ascii_digit())
        && s.parse::<u64>().is_ok()
}
fn decimal(v: &Value, max: u64) -> bool {
    v.as_str()
        .is_some_and(|s| canonical_u64(s) && s.parse::<u64>().is_ok_and(|n| n <= max))
}
fn exact(v: &Value, fields: &[&str]) -> Result<(), String> {
    let o = v.as_object().ok_or("wire object required")?;
    if o.len() != fields.len() || fields.iter().any(|k| !o.contains_key(*k)) {
        return Err("wire fields mismatch".into());
    }
    Ok(())
}
fn member(v: &Value, values: &[&str]) -> bool {
    v.as_str().is_some_and(|s| values.contains(&s))
}
fn nonempty(v: &Value) -> bool {
    v.as_str().is_some_and(|s| !s.is_empty())
}

pub(crate) fn machine_states(protocol: &str, machine: &str) -> Option<&'static [&'static str]> {
    Some(match (protocol, machine) {
        ("rstp", "PTI") => &["ONE_SECOND", "TICK"],
        ("rstp", "PRX") => &["DISCARD", "RECEIVE"],
        ("rstp", "PPM") => &["CHECKING_RSTP", "SELECTING_STP", "SENSING"],
        ("rstp", "BDM") => &["EDGE", "NOT_EDGE", "ISOLATED"],
        ("rstp", "PTX") => &[
            "TRANSMIT_INIT",
            "TRANSMIT_PERIODIC",
            "TRANSMIT_CONFIG",
            "TRANSMIT_TCN",
            "TRANSMIT_RSTP",
            "IDLE",
        ],
        ("rstp", "PIM") => &[
            "DISABLED",
            "AGED",
            "UPDATE",
            "CURRENT",
            "RECEIVE",
            "SUPERIOR_DESIGNATED",
            "REPEATED_DESIGNATED",
            "INFERIOR_DESIGNATED",
            "NOT_DESIGNATED",
            "OTHER",
        ],
        ("rstp", "PRS") => &["INIT_TREE", "ROLE_SELECTION"],
        ("rstp", "PRT") => &[
            "INIT_PORT",
            "DISABLE_PORT",
            "DISABLED_PORT",
            "ROOT_PORT",
            "ROOT_DISCARD",
            "ROOT_PROPOSED",
            "ROOT_AGREED",
            "ROOT_SYNCED",
            "REROOT",
            "REROOTED",
            "ROOT_LEARN",
            "ROOT_FORWARD",
            "DESIGNATED_PORT",
            "DESIGNATED_DISCARD",
            "DESIGNATED_PROPOSE",
            "DESIGNATED_AGREE",
            "DESIGNATED_SYNCED",
            "DESIGNATED_RETIRED",
            "DESIGNATED_LEARN",
            "DESIGNATED_FORWARD",
            "BLOCK_PORT",
            "ALTERNATE_PORT",
            "ALTERNATE_PROPOSED",
            "ALTERNATE_AGREED",
            "BACKUP_PORT",
        ],
        ("rstp", "PST") => &["DISCARDING", "LEARNING", "FORWARDING"],
        ("rstp", "TCM") => &[
            "INACTIVE",
            "LEARNING",
            "DETECTED",
            "ACTIVE",
            "NOTIFIED_TCN",
            "NOTIFIED_TC",
            "PROPAGATING",
            "ACKNOWLEDGED",
        ],
        ("mvrp", "Applicant") => &[
            "VO", "VP", "VN", "AN", "AA", "QA", "LA", "AO", "QO", "AP", "QP", "LO",
        ],
        ("mvrp", "Registrar") => &["IN", "LV", "MT"],
        ("mvrp", "LeaveAll" | "Periodic") => &["Active", "Passive"],
        _ => return None,
    })
}
/// Finite write/observation membership; PRS bridge and port scopes are disjoint.
pub(crate) fn machine_variables(
    protocol: &str,
    machine: &str,
    port: bool,
) -> &'static [&'static str] {
    match (protocol, machine, port) {
        ("rstp", "PTI", true) => &[
            "edgeDelayWhile",
            "helloWhen",
            "mdelayWhile",
            "fdWhile",
            "rrWhile",
            "rbWhile",
            "rcvdInfoWhile",
            "tcWhile",
            "tcDetected",
            "txCount",
        ],
        ("rstp", "PRX", true) => &[
            "portEnabled",
            "rcvdSTP",
            "rcvdRSTP",
            "operEdge",
            "isolate",
            "edgeDelayWhile",
            "rcvdMsg",
            "msgPriority",
            "msgTimes",
            "pendingMessageKind",
            "pendingMessageVersion",
            "pendingMessageFlags",
            "retainedMessagePresent",
            "retainedMessagePriority",
            "retainedMessageTimes",
            "retainedMessageKind",
            "retainedMessageVersion",
            "retainedMessageFlags",
        ],
        ("rstp", "PPM", true) => &["mcheck", "sendRSTP", "mdelayWhile", "rcvdSTP", "rcvdRSTP"],
        ("rstp", "BDM", true) => &["operEdge", "isolate"],
        ("rstp", "PTX", true) => &["helloWhen", "txCount", "newInfo", "tcAck"],
        ("rstp", "PIM", true) => &[
            "infoIs",
            "portPriority",
            "portTimes",
            "updtInfo",
            "proposing",
            "proposed",
            "agree",
            "agreed",
            "synced",
            "disputed",
            "reselect",
            "selected",
            "newInfo",
            "rcvdInfoWhile",
            "rcvdTc",
            "rcvdTcn",
            "rcvdTcAck",
            "rcvdInfo",
            "rcvdMsg",
            "msgPriority",
            "msgTimes",
            "pendingMessageKind",
            "pendingMessageVersion",
            "pendingMessageFlags",
        ],
        ("rstp", "PRS", false) => &["rootPriority", "rootTimes", "rootPortId"],
        ("rstp", "PRS", true) => &[
            "selectedRole",
            "designatedPriority",
            "designatedTimes",
            "updtInfo",
            "selected",
            "reselect",
        ],
        ("rstp", "PRT", true) => &[
            "role",
            "fdWhile",
            "rrWhile",
            "rbWhile",
            "edgeDelayWhile",
            "sync",
            "synced",
            "reRoot",
            "learn",
            "forward",
            "proposing",
            "proposed",
            "agree",
            "agreed",
            "disputed",
            "newInfo",
            "syncPorts",
            "reRootPorts",
        ],
        ("rstp", "PST", true) => &["learning", "forwarding"],
        ("rstp", "TCM", true) => &[
            "tcWhile",
            "tcDetected",
            "tcAck",
            "rcvdTc",
            "rcvdTcn",
            "rcvdTcAck",
            "tcProp",
            "fdbFlush",
            "newInfo",
            "tcPropPorts",
        ],
        ("mvrp", "Applicant", true) => &["attribute", "demand", "request_tx"],
        ("mvrp", "Registrar", true) => &["attribute", "registered"],
        ("mvrp", "LeaveAll", true) => &["restart_timer", "send_leaveall", "request_tx"],
        ("mvrp", "Periodic", true) => &["restart_timer", "periodic"],
        _ => &[],
    }
}
fn typed(variable: &str, v: &Value) -> bool {
    match variable {
        "syncPorts" | "reRootPorts" | "tcPropPorts" => v.as_array().is_some_and(|a| {
            a.iter().all(nonempty) && a.windows(2).all(|w| w[0].as_str() < w[1].as_str())
        }),
        "portPriority"
        | "designatedPriority"
        | "rootPriority"
        | "msgPriority"
        | "retainedMessagePriority" => v
            .as_array()
            .is_some_and(|a| a.len() == 7 && a.iter().all(|v| decimal(v, u64::MAX))),
        "portTimes" | "designatedTimes" | "rootTimes" | "msgTimes" | "retainedMessageTimes" => {
            exact(v, TIMES).is_ok() && TIMES.iter().all(|k| decimal(&v[*k], u16::MAX as u64))
        }
        "rootPortId" | "edgeDelayWhile" | "helloWhen" | "mdelayWhile" | "fdWhile" | "rrWhile"
        | "rbWhile" | "rcvdInfoWhile" | "tcWhile" | "tcDetected" | "txCount" => {
            decimal(v, u64::MAX)
        }
        "pendingMessageVersion"
        | "pendingMessageFlags"
        | "retainedMessageVersion"
        | "retainedMessageFlags" => decimal(v, u8::MAX as u64),
        "role" | "selectedRole" => member(v, ROLES),
        "infoIs" => member(v, INFO),
        "rcvdInfo" => member(v, RECEIVED),
        "pendingMessageKind" | "retainedMessageKind" => member(v, KINDS),
        "attribute" => decimal(v, 4094) && v != "0",
        "portEnabled"
        | "selected"
        | "reselect"
        | "updtInfo"
        | "sendRSTP"
        | "rcvdSTP"
        | "rcvdRSTP"
        | "mcheck"
        | "operEdge"
        | "isolate"
        | "proposing"
        | "proposed"
        | "agree"
        | "agreed"
        | "disputed"
        | "sync"
        | "synced"
        | "reRoot"
        | "learn"
        | "forward"
        | "learning"
        | "forwarding"
        | "newInfo"
        | "tcAck"
        | "rcvdTc"
        | "rcvdTcn"
        | "rcvdTcAck"
        | "tcProp"
        | "fdbFlush"
        | "rcvdMsg"
        | "retainedMessagePresent"
        | "demand"
        | "request_tx"
        | "registered"
        | "restart_timer"
        | "send_leaveall"
        | "periodic" => v.is_boolean(),
        _ => false,
    }
}
fn validate_changes(
    protocol: &str,
    machine: &str,
    port: bool,
    changes: &Value,
) -> Result<(), String> {
    let o = changes.as_object().ok_or("wire changes must be object")?;
    let allowed = machine_variables(protocol, machine, port);
    for (key, pair) in o {
        if !allowed.contains(&key.as_str()) {
            return Err(format!(
                "wire {machine} variable {key} forbidden in this scope"
            ));
        }
        exact(pair, &["before", "after"])?;
        if !typed(key, &pair["before"]) || !typed(key, &pair["after"]) {
            return Err(format!("wire variable {key} type mismatch"));
        }
        if key == "attribute" && pair["before"] != pair["after"] {
            return Err("wire attribute identity changed".into());
        }
    }
    if protocol == "mvrp"
        && matches!(machine, "Applicant" | "Registrar")
        && !o.contains_key("attribute")
    {
        return Err("wire attribute identity required".into());
    }
    Ok(())
}
pub(crate) fn validate_transition(v: &Value) -> Result<(), String> {
    exact(
        v,
        &[
            "schema_version",
            "adapter_id",
            "bridge",
            "port",
            "protocol",
            "time_ps",
            "callback_ordinal",
            "transition_ordinal",
            "machine",
            "from_state",
            "to_state",
            "trigger",
            "changes",
        ],
    )?;
    if v["schema_version"].as_u64() != Some(1)
        || !nonempty(&v["adapter_id"])
        || !nonempty(&v["bridge"])
        || !["time_ps", "callback_ordinal", "transition_ordinal"]
            .iter()
            .all(|k| decimal(&v[*k], u64::MAX))
    {
        return Err("invalid wire transition scalars".into());
    }
    let protocol = v["protocol"]
        .as_str()
        .ok_or("wire protocol string required")?;
    let machine = v["machine"]
        .as_str()
        .ok_or("wire machine string required")?;
    let states = machine_states(protocol, machine).ok_or("unknown wire protocol/machine")?;
    if !member(&v["from_state"], states) || !member(&v["to_state"], states) {
        return Err("unknown wire machine state".into());
    }
    let port = !v["port"].is_null();
    if port && !nonempty(&v["port"]) || !port && (protocol, machine) != ("rstp", "PRS") {
        return Err("invalid wire transition port scope".into());
    }
    if !v["trigger"]
        .as_str()
        .is_some_and(|s| super::guards::valid_trigger(protocol, machine, s))
    {
        return Err("unknown wire trigger".into());
    }
    validate_changes(protocol, machine, port, &v["changes"])
}
fn priority(p: rstp::Priority) -> Value {
    json!(p.0.map(|n| n.to_string()))
}
fn times(t: rstp::Times) -> Value {
    json!({"MessageAge_ticks":t.age.to_string(),"MaxAge_ticks":t.max_age.to_string(),"HelloTime_ticks":t.hello.to_string(),"ForwardDelay_ticks":t.forward_delay.to_string()})
}
fn message(out: &mut Map<String, Value>, m: Option<&rstp::Bpdu>, retained: bool) {
    let keys = if retained {
        [
            "retainedMessagePresent",
            "retainedMessagePriority",
            "retainedMessageTimes",
            "retainedMessageKind",
            "retainedMessageVersion",
            "retainedMessageFlags",
        ]
    } else {
        [
            "rcvdMsg",
            "msgPriority",
            "msgTimes",
            "pendingMessageKind",
            "pendingMessageVersion",
            "pendingMessageFlags",
        ]
    };
    out.insert(keys[0].into(), json!(m.is_some()));
    out.insert(
        keys[1].into(),
        priority(m.map_or(rstp::Priority([0; 7]), |m| m.priority)),
    );
    out.insert(
        keys[2].into(),
        times(m.map_or(
            rstp::Times {
                age: 0,
                max_age: 0,
                hello: 0,
                forward_delay: 0,
            },
            |m| m.times,
        )),
    );
    out.insert(keys[3].into(), m.map_or(json!("None"), |m| json!(m.kind)));
    out.insert(
        keys[4].into(),
        json!(m.map_or(0, |m| m.version).to_string()),
    );
    out.insert(keys[5].into(), json!(m.map_or(0, |m| m.flags).to_string()));
}
fn port_projection(v: &Value) -> Result<Value, String> {
    let mut fields: Vec<_> = PORT_FIELDS.iter().map(|(raw, _)| *raw).collect();
    fields.extend(STATES.iter().map(|(raw, _)| *raw));
    fields.extend([
        "config",
        "counters",
        "message",
        "last_message",
        "received_info",
    ]);
    exact(v, &fields)?;
    exact(
        &v["counters"],
        &COUNTERS.iter().map(|(r, _)| *r).collect::<Vec<_>>(),
    )?;
    exact(
        &v["config"],
        &[
            "id",
            "cost",
            "admin_edge",
            "auto_edge",
            "auto_isolate",
            "restricted_role",
            "restricted_tcn",
        ],
    )?;
    for key in ["message", "last_message"] {
        if !v[key].is_null() {
            exact(&v[key], &["kind", "version", "flags", "priority", "times"])?;
            exact(
                &v[key]["times"],
                &["age", "max_age", "hello", "forward_delay"],
            )?;
        }
    }
    for key in ["times", "designated_times"] {
        exact(&v[key], &["age", "max_age", "hello", "forward_delay"])?;
    }
    let p: rstp::Port = serde_json::from_value(v.clone()).map_err(|e| e.to_string())?;
    let mut out = Map::new();
    for (raw, public) in PORT_FIELDS {
        out.insert(
            (*public).into(),
            match *raw {
                "priority" => priority(p.priority),
                "designated" => priority(p.designated),
                "times" => times(p.times),
                "designated_times" => times(p.designated_times),
                _ => v[*raw].clone(),
            },
        );
    }
    for (raw, public) in COUNTERS {
        out.insert(
            (*public).into(),
            json!(
                v["counters"][*raw]
                    .as_u64()
                    .ok_or("invalid internal counter")?
                    .to_string()
            ),
        );
    }
    out.insert(
        "rcvdInfo".into(),
        p.received_info.map_or(json!("None"), |r| json!(r)),
    );
    message(&mut out, p.message.as_ref(), false);
    message(&mut out, p.last_message.as_ref(), true);
    Ok(Value::Object(out))
}
fn bridge_projection(v: &Value) -> Result<Value, String> {
    if v.get("ports").is_some() {
        exact(
            v,
            &[
                "id",
                "times",
                "root",
                "root_times",
                "root_port",
                "hold",
                "pti",
                "prs",
                "ports",
            ],
        )?;
    } else {
        exact(v, &["root", "root_times", "root_port"])?;
    }
    if v.get("ports").is_some() {
        let bridge: rstp::Bridge = serde_json::from_value(v.clone()).map_err(|e| e.to_string())?;
        exact(&v["times"], &["age", "max_age", "hello", "forward_delay"])?;
        for name in bridge.ports.keys() {
            port_projection(&v["ports"][name])?;
        }
    }
    exact(
        &v["root_times"],
        &["age", "max_age", "hello", "forward_delay"],
    )?;
    let p: rstp::Priority = serde_json::from_value(v["root"].clone()).map_err(|e| e.to_string())?;
    let t: rstp::Times =
        serde_json::from_value(v["root_times"].clone()).map_err(|e| e.to_string())?;
    let port: u16 = serde_json::from_value(v["root_port"].clone()).map_err(|e| e.to_string())?;
    Ok(json!({"rootPriority":priority(p),"rootTimes":times(t),"rootPortId":port.to_string()}))
}
fn diff(
    protocol: &str,
    machine: &str,
    port: bool,
    before: Value,
    after: Value,
) -> Result<Changes, String> {
    let b = before
        .as_object()
        .ok_or("projected state object required")?;
    let a = after.as_object().ok_or("projected state object required")?;
    let changes: Changes = a
        .iter()
        .filter(|(k, v)| b.get(*k) != Some(*v))
        .map(|(k, v)| (k.clone(), json!({"before":b[k],"after":v})))
        .collect();
    validate_changes(
        protocol,
        machine,
        port,
        &serde_json::to_value(&changes).map_err(|e| e.to_string())?,
    )?;
    Ok(changes)
}
pub(crate) fn project_rstp_changes(
    machine: &str,
    port: Option<&str>,
    before: &Value,
    after: &Value,
) -> Result<Changes, String> {
    machine_states("rstp", machine).ok_or("unknown RSTP machine")?;
    if port.is_some() {
        if before["config"] != after["config"] {
            return Err("immutable RSTP configuration changed".into());
        }
        for (raw, owner) in STATES {
            if *owner != machine && before[*raw] != after[*raw] {
                return Err(format!(
                    "unjournaled cross-machine {owner} state write in {machine}"
                ));
            }
        }
        diff(
            "rstp",
            machine,
            true,
            port_projection(before)?,
            port_projection(after)?,
        )
    } else if machine == "PRS" {
        for field in ["id", "times", "hold", "pti"] {
            if before.get(field) != after.get(field) {
                return Err(format!(
                    "immutable/other-machine bridge field {field} changed"
                ));
            }
        }
        diff(
            "rstp",
            machine,
            false,
            bridge_projection(before)?,
            bridge_projection(after)?,
        )
    } else {
        Err("only PRS has bridge scope".into())
    }
}
pub(crate) fn project_mvrp_changes(
    machine: &str,
    before: &Value,
    after: &Value,
    vid: Option<u16>,
) -> Result<Changes, String> {
    machine_states("mvrp", machine).ok_or("unknown MVRP machine")?;
    let mut b = Map::new();
    let mut a = Map::new();
    if matches!(machine, "Applicant" | "Registrar") {
        let vid = vid
            .filter(|v| (1..=4094).contains(v))
            .ok_or("MVRP attribute VID required")?;
        let fields = &[
            "applicant",
            "registrar",
            "registered",
            "demand",
            "request_tx",
        ];
        exact(before, fields)?;
        exact(after, fields)?;
        for side in [before, after] {
            if !member(
                &side["applicant"],
                machine_states("mvrp", "Applicant").unwrap(),
            ) || !member(
                &side["registrar"],
                machine_states("mvrp", "Registrar").unwrap(),
            ) || ["registered", "demand", "request_tx"]
                .iter()
                .any(|k| !side[*k].is_boolean())
            {
                return Err("invalid internal MVRP Attribute".into());
            }
        }
        for key in machine_variables("mvrp", machine, true)
            .iter()
            .filter(|k| **k != "attribute")
        {
            b.insert((*key).into(), before[*key].clone());
            a.insert((*key).into(), after[*key].clone());
        }
        let mut out: Changes = a
            .iter()
            .filter(|(k, v)| b.get(*k) != Some(*v))
            .map(|(k, v)| (k.clone(), json!({"before":b[k],"after":v})))
            .collect();
        out.insert(
            "attribute".into(),
            json!({"before":vid.to_string(),"after":vid.to_string()}),
        );
        validate_changes(
            "mvrp",
            machine,
            true,
            &serde_json::to_value(&out).map_err(|e| e.to_string())?,
        )?;
        return Ok(out);
    }
    if vid.is_some() {
        return Err("auxiliary MVRP attribute forbidden".into());
    }
    // Before action flags are ephemeral observations, explicitly initialized false.
    let flags = ["restart_timer", "send_leaveall", "periodic", "request_tx"];
    exact(before, &["state"])?;
    exact(
        after,
        &[
            "state",
            "restart_timer",
            "send_leaveall",
            "periodic",
            "request_tx",
        ],
    )?;
    for side in [before, after] {
        if !member(&side["state"], machine_states("mvrp", machine).unwrap()) {
            return Err("invalid auxiliary state".into());
        }
    }
    for key in flags {
        let v = &after[key];
        if !v.is_boolean() {
            return Err("invalid auxiliary action flag".into());
        }
        if !machine_variables("mvrp", machine, true).contains(&key) && v != false {
            return Err(format!("{machine} cannot observe {key}"));
        }
        if machine_variables("mvrp", machine, true).contains(&key) {
            b.insert(key.into(), json!(false));
            a.insert(key.into(), v.clone());
        }
    }
    diff("mvrp", machine, true, Value::Object(b), Value::Object(a))
}
/// Project only the known public metadata boundary, never checkpoint state.
pub(crate) fn public_state_projection(snapshot: &Value) -> Result<Value, String> {
    let mut out = snapshot.clone();
    for map in ["generations", "tx_ordinals"] {
        let m = out[map]
            .as_object_mut()
            .ok_or("wire metadata counter map required")?;
        for v in m.values_mut() {
            *v = json!(
                v.as_u64()
                    .ok_or("internal metadata u64 required")?
                    .to_string()
            );
        }
    }
    let bridges = out["rstp"]
        .as_object_mut()
        .ok_or("wire metadata RSTP map required")?;
    for b in bridges.values_mut() {
        // Validate the complete source before applying selected numeric conversions.
        bridge_projection(b)?;
        b["id"] = json!(
            b["id"]
                .as_u64()
                .ok_or("RSTP bridge ID required")?
                .to_string()
        );
        b["root"] = priority(serde_json::from_value(b["root"].clone()).map_err(|e| e.to_string())?);
        b["root_port"] = json!(
            b["root_port"]
                .as_u64()
                .ok_or("RSTP root port required")?
                .to_string()
        );
        let ports = b["ports"].as_object_mut().ok_or("RSTP ports required")?;
        for p in ports.values_mut() {
            port_projection(p)?;
            for key in ["priority", "designated"] {
                p[key] =
                    priority(serde_json::from_value(p[key].clone()).map_err(|e| e.to_string())?);
            }
            for key in ["message", "last_message"] {
                if !p[key].is_null() {
                    p[key]["priority"] = priority(
                        serde_json::from_value(p[key]["priority"].clone())
                            .map_err(|e| e.to_string())?,
                    );
                }
            }
            let counters = p["counters"]
                .as_object_mut()
                .ok_or("RSTP counters required")?;
            for v in counters.values_mut() {
                *v = json!(v.as_u64().ok_or("RSTP counter required")?.to_string());
            }
        }
    }
    Ok(out)
}

/// Source-entry observations of tree-wide actions, not target-port FSM entries.
pub(crate) fn project_rstp_tree_observations(
    machine: &str,
    before: &Value,
    after: &Value,
) -> Result<Changes, String> {
    let fields: &[(&str, &str)] = match machine {
        "PRT" => &[("sync", "syncPorts"), ("reroot", "reRootPorts")],
        "TCM" => &[("tc_prop", "tcPropPorts")],
        _ => return Err("tree observations only supported for PRT/TCM".into()),
    };
    let mut sides = Vec::new();
    for side in [before, after] {
        let ports = side["ports"]
            .as_object()
            .ok_or("full Bridge ports required for tree observation")?;
        let mut projected = Map::new();
        for (raw, public) in fields {
            let mut set = Vec::new();
            for (name, p) in ports {
                if !nonempty(&json!(name)) {
                    return Err("empty tree port ID".into());
                }
                if p[*raw].as_bool().ok_or("tree flag required")? {
                    set.push(name.clone());
                }
            }
            set.sort();
            projected.insert((*public).into(), json!(set));
        }
        sides.push(Value::Object(projected));
    }
    diff("rstp", machine, true, sides.remove(0), sides.remove(0))
}

#[cfg(test)]
mod tests {
    use super::*;
    const MACHINES: &[(&str, &str)] = &[
        ("rstp", "PTI"),
        ("rstp", "PRX"),
        ("rstp", "PPM"),
        ("rstp", "BDM"),
        ("rstp", "PTX"),
        ("rstp", "PIM"),
        ("rstp", "PRS"),
        ("rstp", "PRT"),
        ("rstp", "PST"),
        ("rstp", "TCM"),
        ("mvrp", "Applicant"),
        ("mvrp", "Registrar"),
        ("mvrp", "LeaveAll"),
        ("mvrp", "Periodic"),
    ];
    fn sample(key: &str) -> Value {
        match key {
            "portPriority"
            | "designatedPriority"
            | "rootPriority"
            | "msgPriority"
            | "retainedMessagePriority" => json!([
                "9223374235878031361",
                "9007199254740993",
                "18446744073709551615",
                "0",
                "1",
                "65535",
                "2"
            ]),
            "portTimes" | "designatedTimes" | "rootTimes" | "msgTimes" | "retainedMessageTimes" => {
                times(rstp::Times {
                    age: 0,
                    max_age: 65535,
                    hello: 512,
                    forward_delay: 3840,
                })
            }
            "rootPortId" | "edgeDelayWhile" | "helloWhen" | "mdelayWhile" | "fdWhile"
            | "rrWhile" | "rbWhile" | "rcvdInfoWhile" | "tcWhile" | "tcDetected" | "txCount" => {
                json!(u64::MAX.to_string())
            }
            "pendingMessageVersion"
            | "pendingMessageFlags"
            | "retainedMessageVersion"
            | "retainedMessageFlags" => json!("255"),
            "role" | "selectedRole" => json!("Root"),
            "infoIs" => json!("Received"),
            "rcvdInfo" => json!("None"),
            "pendingMessageKind" | "retainedMessageKind" => json!("None"),
            "attribute" => json!("4094"),
            "syncPorts" | "reRootPorts" | "tcPropPorts" => json!(["a.p", "z.p"]),
            _ => json!(false),
        }
    }
    fn record(protocol: &str, machine: &str) -> Value {
        let state = machine_states(protocol, machine).unwrap()[0];
        json!({"schema_version":1,"adapter_id":"a","bridge":"b","port":if machine=="PRS" {Value::Null}else{json!("p")},"protocol":protocol,"time_ps":"0","callback_ordinal":"0","transition_ordinal":"0","machine":machine,"from_state":state,"to_state":state,"trigger":"BEGIN","changes":if protocol=="mvrp" && matches!(machine,"Applicant"|"Registrar") {json!({"attribute":{"before":"1","after":"1"}})}else{json!({})}})
    }
    fn bridge() -> rstp::Bridge {
        rstp::Bridge::new(
            9223374235878031361,
            rstp::Times::default(),
            6,
            BTreeMap::from([(
                "p".into(),
                (
                    rstp::PortConfig {
                        id: 32769,
                        cost: 20000,
                        admin_edge: false,
                        auto_edge: true,
                        auto_isolate: false,
                        restricted_role: false,
                        restricted_tcn: false,
                    },
                    true,
                ),
            )]),
        )
    }
    #[test]
    fn every_machine_state_and_catalog_type_is_closed() {
        for (protocol, machine) in MACHINES {
            let mut r = record(protocol, machine);
            for state in machine_states(protocol, machine).unwrap() {
                r["from_state"] = json!(state);
                r["to_state"] = json!(state);
                validate_transition(&r)
                    .unwrap_or_else(|e| panic!("{protocol}/{machine}/{state}: {e}"));
            }
            for port in [false, true] {
                if !port && *machine != "PRS" {
                    continue;
                }
                r["port"] = if port { json!("p") } else { Value::Null };
                let mut changes = Map::new();
                for key in machine_variables(protocol, machine, port) {
                    let v = sample(key);
                    assert!(typed(key, &v), "{key}");
                    changes.insert((*key).into(), json!({"before":v,"after":v}));
                }
                r["changes"] = Value::Object(changes);
                validate_transition(&r).unwrap_or_else(|e| panic!("{machine}: {e}"));
            }
        }
    }
    #[test]
    fn strict_outer_machine_state_scope_and_guard_rejections() {
        let r = record("rstp", "PRX");
        for field in r.as_object().unwrap().keys() {
            let mut v = r.clone();
            v.as_object_mut().unwrap().remove(field);
            assert!(validate_transition(&v).is_err(), "missing {field}");
        }
        for (field, value) in [
            ("extra", json!(true)),
            ("machine", json!("PRX2")),
            ("protocol", json!("mvrp")),
            ("from_state", json!("EDGE")),
            ("to_state", Value::Null),
            ("port", Value::Null),
            ("port", json!("")),
            ("trigger", json!("PRX/rule/999/999")),
            ("trigger", json!("P-RSTP-T01-V219")),
            ("time_ps", json!(1)),
            ("callback_ordinal", json!("01")),
            ("schema_version", json!(2)),
        ] {
            let mut v = r.clone();
            v[field] = value;
            assert!(validate_transition(&v).is_err(), "{field}: {v}");
        }
        let mut v = record("rstp", "PRS");
        v["changes"] = json!({"selectedRole":{"before":"Root","after":"Alternate"}});
        assert!(validate_transition(&v).is_err());
        v["port"] = json!("p");
        validate_transition(&v).unwrap();
        v["changes"] = json!({"rootPortId":{"before":"0","after":"1"}});
        assert!(validate_transition(&v).is_err());
    }
    #[test]
    fn negative_nested_types_keys_decimals_and_machine_writes() {
        let mut r = record("rstp", "PIM");
        for bad in [
            json!({"before":true}),
            json!({"before":true,"after":false,"extra":0}),
            json!({"before":null,"after":true}),
            json!({"before":"true","after":true}),
        ] {
            r["changes"] = json!({"agree":bad});
            assert!(validate_transition(&r).is_err());
        }
        for key in [
            "counters",
            "message",
            "config",
            "pim",
            "unknown",
            "learning",
            "rootPriority",
        ] {
            r["changes"] = json!({key:{"before":false,"after":true}});
            assert!(validate_transition(&r).is_err(), "{key}");
        }
        r["changes"] = json!({"infoIs":{"before":"Received","after":"UNKNOWN"}});
        assert!(validate_transition(&r).is_err());
        for bad in [
            json!(null),
            json!([]),
            json!(vec!["0"; 6]),
            json!(vec!["0"; 8]),
            json!([0, 0, 0, 0, 0, 0, 0]),
            json!(["0", "0", "0", "0", "0", "0", null]),
        ] {
            r["changes"] = json!({"portPriority":{"before":sample("portPriority"),"after":bad}});
            assert!(validate_transition(&r).is_err());
        }
        for bad in [
            "",
            "00",
            "01",
            "+1",
            "-1",
            "1.0",
            "1e0",
            " 1",
            "1 ",
            "18446744073709551616",
        ] {
            r["changes"] = json!({"rcvdInfoWhile":{"before":"0","after":bad}});
            assert!(validate_transition(&r).is_err(), "{bad}");
            let mut tuple = sample("portPriority");
            tuple[6] = json!(bad);
            r["changes"] = json!({"portPriority":{"before":sample("portPriority"),"after":tuple}});
            assert!(validate_transition(&r).is_err());
        }
        for key in TIMES {
            let mut t = sample("portTimes");
            t.as_object_mut().unwrap().remove(*key);
            r["changes"] = json!({"portTimes":{"before":sample("portTimes"),"after":t}});
            assert!(validate_transition(&r).is_err());
        }
        for (key, value) in [
            ("extra", json!("0")),
            ("HelloTime_ticks", json!(1)),
            ("HelloTime_ticks", json!(null)),
            ("MaxAge_ticks", json!("65536")),
            ("MaxAge_ticks", json!("01")),
        ] {
            let mut t = sample("portTimes");
            t[key] = value;
            r["changes"] = json!({"portTimes":{"before":sample("portTimes"),"after":t}});
            assert!(validate_transition(&r).is_err());
        }
        r = record("rstp", "PRT");
        for bad in [
            json!(["z", "a"]),
            json!(["a", "a"]),
            json!([1]),
            json!([{}]),
            json!([""]),
        ] {
            r["changes"] = json!({"syncPorts":{"before":[],"after":bad}});
            assert!(validate_transition(&r).is_err());
        }
    }
    #[test]
    fn mvrp_projects_only_own_variables_and_typed_auxiliary_actions() {
        let b = json!({"applicant":"VO","registrar":"MT","registered":false,"demand":false,"request_tx":false});
        let a = json!({"applicant":"VN","registrar":"IN","registered":true,"demand":true,"request_tx":true});
        let app = project_mvrp_changes("Applicant", &b, &a, Some(20)).unwrap();
        assert_eq!(
            app.keys().map(String::as_str).collect::<Vec<_>>(),
            vec!["attribute", "demand", "request_tx"]
        );
        let reg = project_mvrp_changes("Registrar", &b, &a, Some(20)).unwrap();
        assert_eq!(
            reg.keys().map(String::as_str).collect::<Vec<_>>(),
            vec!["attribute", "registered"]
        );
        for vid in [None, Some(0), Some(4095)] {
            assert!(project_mvrp_changes("Applicant", &b, &a, vid).is_err());
        }
        let before = json!({"state":"Passive"});
        let after = json!({"state":"Active","restart_timer":true,"send_leaveall":false,"periodic":false,"request_tx":true});
        let aux = project_mvrp_changes("LeaveAll", &before, &after, None).unwrap();
        assert_eq!(aux["restart_timer"], json!({"before":false,"after":true}));
        assert!(!aux.contains_key("attribute"));
        assert!(project_mvrp_changes("Periodic", &before, &after, None).is_err());
        assert!(project_mvrp_changes("LeaveAll", &before, &after, Some(1)).is_err());
        for machine in ["Applicant", "Registrar"] {
            let mut r = record("mvrp", machine);
            for bad in [
                Value::Null,
                json!("0"),
                json!("4095"),
                json!("01"),
                json!(1),
            ] {
                r["changes"] = json!({"attribute":{"before":bad,"after":bad}});
                assert!(validate_transition(&r).is_err());
            }
            r["changes"] = json!({});
            assert!(validate_transition(&r).is_err());
        }
        let mut r = record("mvrp", "LeaveAll");
        r["changes"] = json!({"attribute":{"before":"1","after":"1"}});
        assert!(validate_transition(&r).is_err());
    }
    #[test]
    fn full_source_projection_preserves_precision_buffers_and_real_initial_root() {
        let b = bridge();
        let before = serde_json::to_value(&b.ports["p"]).unwrap();
        let mut a = b.ports["p"].clone();
        a.last_message = Some(rstp::Bpdu {
            kind: rstp::Kind::Rst,
            version: 2,
            flags: 12,
            priority: rstp::Priority([
                9223374235878031361,
                9007199254740993,
                u64::MAX,
                0,
                1,
                65535,
                32769,
            ]),
            times: rstp::Times::default(),
        });
        a.message = a.last_message.clone();
        let after = serde_json::to_value(a).unwrap();
        let changes = project_rstp_changes("PRX", Some("p"), &before, &after).unwrap();
        assert_eq!(changes["msgPriority"]["before"], json!(vec!["0"; 7]));
        assert_eq!(
            changes["msgPriority"]["after"],
            json!([
                "9223374235878031361",
                "9007199254740993",
                "18446744073709551615",
                "0",
                "1",
                "65535",
                "32769"
            ])
        );
        assert_eq!(
            changes["msgTimes"]["before"],
            json!({"MessageAge_ticks":"0","MaxAge_ticks":"0","HelloTime_ticks":"0","ForwardDelay_ticks":"0"})
        );
        assert_eq!(
            changes["pendingMessageKind"],
            json!({"before":"None","after":"Rst"})
        );
        assert_eq!(
            changes["retainedMessagePresent"],
            json!({"before":false,"after":true})
        );
        let mut replacement = after.clone();
        replacement["message"]["priority"][0] = json!(9007199254740993u64);
        let replaced = project_rstp_changes("PRX", Some("p"), &after, &replacement).unwrap();
        assert_eq!(replaced["msgPriority"]["before"][0], "9223374235878031361");
        assert_eq!(replaced["msgPriority"]["after"][0], "9007199254740993");
        let mut ticked = before.clone();
        ticked["counters"]["tx"] = json!(u32::MAX);
        let counter = project_rstp_changes("PTI", Some("p"), &before, &ticked).unwrap();
        assert_eq!(
            counter["txCount"],
            json!({"before":"0","after":"4294967295"})
        );
        let mut consumed = after.clone();
        consumed["message"] = Value::Null;
        let c = project_rstp_changes("PIM", Some("p"), &after, &consumed).unwrap();
        assert!(!c.contains_key("retainedMessagePriority"));
        assert_eq!(c["rcvdMsg"], json!({"before":true,"after":false}));
        let root = json!({"root":b.root,"root_times":b.root_times,"root_port":b.root_port});
        assert!(
            project_rstp_changes("PRS", None, &root, &root)
                .unwrap()
                .is_empty()
        );
        assert!(project_rstp_changes("PRS", None, &json!({}), &root).is_err());
        let mut unknown = before.clone();
        unknown["surprise"] = json!(true);
        assert!(project_rstp_changes("PRX", Some("p"), &before, &unknown).is_err());
        let mut cross = before.clone();
        cross["pim"] = json!("AGED");
        assert!(project_rstp_changes("PRX", Some("p"), &before, &cross).is_err());
        let mut r = record("rstp", "PRX");
        r["changes"] = serde_json::to_value(changes).unwrap();
        validate_transition(&r).unwrap();
        let serialized = serde_json::to_string(&r).unwrap();
        assert!(serialized.contains("\"18446744073709551615\""));
        assert_eq!(serde_json::from_str::<Value>(&serialized).unwrap(), r);
        if let Ok(path) = std::env::var("WIRE_PROJECTION_EVIDENCE") {
            std::fs::write(path, serde_json::to_vec_pretty(&r).unwrap()).unwrap();
        }
    }
    #[test]
    fn metadata_boundary_is_lossless_and_keeps_private_source_intact() {
        let b = bridge();
        let raw = json!({"rstp":{"a":b},"generations":{"a":u64::MAX},"tx_ordinals":{"p":9007199254740993u64},"mvrp":[]});
        let copy = raw.clone();
        let public = public_state_projection(&raw).unwrap();
        assert_eq!(raw, copy);
        assert_eq!(public["rstp"]["a"]["id"], "9223374235878031361");
        assert_eq!(public["rstp"]["a"]["root"][0], "9223374235878031361");
        assert_eq!(public["generations"]["a"], u64::MAX.to_string());
        assert_eq!(public["tx_ordinals"]["p"], "9007199254740993");
        assert!(public["rstp"]["a"]["ports"]["p"]["config"]["id"].is_number());
        if let Ok(path) = std::env::var("WIRE_PROJECTION_EVIDENCE") {
            std::fs::write(
                std::path::Path::new(&path).with_file_name("precision-metadata.json"),
                serde_json::to_vec_pretty(&public).unwrap(),
            )
            .unwrap();
        }
    }
    #[test]
    fn tree_action_observations_derive_sorted_actual_sets() {
        let before = json!({"ports":{"z":{"sync":false,"reroot":false,"tc_prop":false},"a":{"sync":true,"reroot":false,"tc_prop":false}}});
        let after = json!({"ports":{"z":{"sync":true,"reroot":true,"tc_prop":true},"a":{"sync":true,"reroot":true,"tc_prop":false}}});
        let c = project_rstp_tree_observations("PRT", &before, &after).unwrap();
        assert_eq!(c["syncPorts"], json!({"before":["a"],"after":["a","z"]}));
        assert_eq!(c["reRootPorts"]["after"], json!(["a", "z"]));
        let c = project_rstp_tree_observations("TCM", &before, &after).unwrap();
        assert_eq!(c["tcPropPorts"]["after"], json!(["z"]));
        assert!(project_rstp_tree_observations("PRX", &before, &after).is_err());
    }
    #[test]
    fn actual_rstp_reducer_journals_project_without_losing_typed_changes() {
        let mut state = bridge();
        let packet = rstp::Bpdu {
            kind: rstp::Kind::Rst,
            version: 2,
            flags: 12,
            priority: rstp::Priority([
                0x1000000000000001,
                0,
                0x1000000000000002,
                0,
                0x1000000000000002,
                32769,
                32769,
            ]),
            times: rstp::Times::default(),
        };
        let events = [
            rstp::Input::Initialize,
            rstp::Input::Receive("p".into(), packet),
            rstp::Input::Tick,
            rstp::Input::Link("p".into(), false),
            rstp::Input::Link("p".into(), true),
        ];
        let mut count = 0;
        let mut seen = std::collections::BTreeSet::new();
        for event in events {
            let d = state.plan(event, 1000).unwrap();
            for t in &d.transitions {
                let mut changes =
                    project_rstp_changes(&t.machine, t.port.as_deref(), &t.before, &t.after)
                        .unwrap_or_else(|e| {
                            panic!(
                                "{} {}->{} {}: {e}",
                                t.machine, t.from_state, t.to_state, t.guard
                            )
                        });
                if let (Some(b), Some(a)) = (&t.tree_before, &t.tree_after) {
                    changes.extend(project_rstp_tree_observations(&t.machine, b, a).unwrap());
                }
                let mut r = record("rstp", &t.machine);
                r["port"] = json!(t.port);
                r["from_state"] = json!(t.from_state);
                r["to_state"] = json!(t.to_state);
                r["changes"] = json!(changes);
                // Guard producer identity is tested by the wire owner; this test isolates projection.
                validate_transition(&r).unwrap();
                count += 1;
                seen.insert(t.machine.clone());
            }
            state = d.state;
        }
        assert!(count > 20);
        for name in [
            "PTI", "PRX", "PPM", "BDM", "PTX", "PIM", "PRS", "PRT", "PST", "TCM",
        ] {
            assert!(seen.contains(name), "missing real journal {name}");
        }
    }
    #[test]
    fn every_catalog_value_rejects_null_and_other_machine_membership() {
        let all: std::collections::BTreeSet<_> = MACHINES
            .iter()
            .flat_map(|(p, m)| machine_variables(p, m, true).iter().copied())
            .collect();
        for (protocol, machine) in MACHINES {
            let mut r = record(protocol, machine);
            let port = *machine != "PRS";
            let allowed = machine_variables(protocol, machine, port);
            for key in allowed {
                for side in ["before", "after"] {
                    let mut pair = json!({"before":sample(key),"after":sample(key)});
                    pair[side] = Value::Null;
                    r["changes"] = json!({(*key):pair});
                    if *protocol == "mvrp"
                        && matches!(*machine, "Applicant" | "Registrar")
                        && *key != "attribute"
                    {
                        r["changes"]["attribute"] = json!({"before":"1","after":"1"});
                    }
                    assert!(validate_transition(&r).is_err(), "{machine}/{key}/{side}");
                }
            }
            for key in all.iter().filter(|key| !allowed.contains(key)) {
                r["changes"] = json!({(*key):{"before":sample(key),"after":sample(key)}});
                assert!(validate_transition(&r).is_err(), "{machine} accepted {key}");
            }
        }
    }
}
