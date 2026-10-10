//! Strict typed TSN preparation. The parent prepares the wrapped dynamic config.
use crate::types::{
    Diagnostic,
    ethernet::{PreparedEthernet, tsn::*},
};
use serde_json::{Map, Value};
use std::{
    collections::{BTreeMap, BTreeSet},
    sync::Arc,
};
type Result<T> = std::result::Result<T, Diagnostic>;
fn err(path: &str, msg: impl Into<String>) -> Diagnostic {
    Diagnostic::prepare(msg).with_target(path)
}
fn obj<'a>(v: &'a Value, fields: &[&str], p: &str) -> Result<&'a Map<String, Value>> {
    let o = v.as_object().ok_or_else(|| err(p, "expected object"))?;
    if o.len() != fields.len() || fields.iter().any(|k| !o.contains_key(*k)) {
        return Err(err(
            p,
            format!("required exact fields: {}", fields.join(", ")),
        ));
    }
    Ok(o)
}
fn s<'a>(o: &'a Map<String, Value>, k: &str, p: &str) -> Result<&'a str> {
    o[k].as_str()
        .filter(|s| !s.is_empty())
        .ok_or_else(|| err(&format!("{p}.{k}"), "expected nonempty string"))
}
fn id<'a>(o: &'a Map<String, Value>, k: &str, p: &str) -> Result<&'a str> {
    let v = s(o, k, p)?;
    if !v.is_ascii()
        || !v
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"_.-:/@".contains(&b))
    {
        return Err(err(&format!("{p}.{k}"), "expected ASCII identifier"));
    }
    Ok(v)
}
fn d(o: &Map<String, Value>, k: &str, p: &str) -> Result<u64> {
    let v = s(o, k, p)?;
    if !v.bytes().all(|b| b.is_ascii_digit()) || v.len() > 1 && v.starts_with('0') {
        return Err(err(
            &format!("{p}.{k}"),
            "expected canonical u64 decimal string",
        ));
    }
    v.parse()
        .map_err(|_| err(&format!("{p}.{k}"), "u64 overflow"))
}
fn j(o: &Map<String, Value>, k: &str, lo: u64, hi: u64, p: &str) -> Result<u64> {
    o[k].as_u64()
        .filter(|n| (lo..=hi).contains(n))
        .ok_or_else(|| err(&format!("{p}.{k}"), "JSON integer out of range"))
}
fn arr<'a>(o: &'a Map<String, Value>, k: &str, p: &str) -> Result<&'a Vec<Value>> {
    o[k].as_array()
        .ok_or_else(|| err(&format!("{p}.{k}"), "expected array"))
}
fn scale(n: u64, bytes: bool, p: &str) -> Result<u128> {
    (n as u128)
        .checked_mul(if bytes { 8 } else { 1 })
        .and_then(|n| n.checked_mul(Q))
        .ok_or_else(|| err(p, "scaled capacity overflow"))
}
fn schedule(v: &Value, stream: bool, p: &str) -> Result<Schedule> {
    let fields = if stream {
        vec!["base_time_ps", "cycle_time_ps", "entries"]
    } else {
        vec!["id", "base_time_ps", "cycle_time_ps", "entries"]
    };
    let o = obj(v, &fields, p)?;
    let name = if stream {
        String::new()
    } else {
        id(o, "id", p)?.into()
    };
    let base_ps = d(o, "base_time_ps", p)?;
    let cycle_ps = d(o, "cycle_time_ps", p)?;
    if cycle_ps == 0 {
        return Err(err(p, "cycle_time_ps must be positive"));
    }
    let raw = arr(o, "entries", p)?;
    if raw.is_empty() {
        return Err(err(p, "entries must be nonempty"));
    }
    let mut entries = Vec::new();
    let mut prefix_ps = vec![0u64];
    for (i, v) in raw.iter().enumerate() {
        let path = format!("{p}.entries[{i}]");
        let fields = if stream {
            vec!["duration_ps", "open"]
        } else {
            vec!["duration_ps", "open_priorities"]
        };
        let e = obj(v, &fields, &path)?;
        let duration_ps = d(e, "duration_ps", &path)?;
        if duration_ps == 0 {
            return Err(err(&path, "duration_ps must be positive"));
        }
        let mut mask = 0u8;
        if stream {
            mask = if e["open"]
                .as_bool()
                .ok_or_else(|| err(&path, "open must be boolean"))?
            {
                255
            } else {
                0
            };
        } else {
            for v in arr(e, "open_priorities", &path)? {
                let n = v
                    .as_u64()
                    .filter(|n| *n < 8)
                    .ok_or_else(|| err(&path, "priority must be integer 0..7"))?;
                if mask & (1 << n) != 0 {
                    return Err(err(&path, "duplicate open priority"));
                }
                mask |= 1 << n;
            }
        }
        prefix_ps.push(
            prefix_ps
                .last()
                .unwrap()
                .checked_add(duration_ps)
                .ok_or_else(|| err(&path, "GCL prefix overflow"))?,
        );
        entries.push(GateEntry {
            duration_ps,
            open_mask: mask,
        });
    }
    if *prefix_ps.last().unwrap() != cycle_ps {
        return Err(err(p, "duration sum must equal cycle_time_ps"));
    }
    let mut class_open_runs: [Vec<OpenRun>; 8] = std::array::from_fn(|_| Vec::new());
    let mut open_total_ps = [0u64; 8];
    for c in 0..8 {
        for (i, e) in entries.iter().enumerate() {
            if e.open_mask & (1 << c) == 0 {
                continue;
            }
            open_total_ps[c] += e.duration_ps;
            let runs = &mut class_open_runs[c];
            if let Some(last) = runs.last_mut().filter(|r| r.end_ps == prefix_ps[i] as u128) {
                last.end_ps = prefix_ps[i + 1] as u128;
            } else {
                runs.push(OpenRun {
                    start_ps: prefix_ps[i],
                    end_ps: prefix_ps[i + 1] as u128,
                });
            }
        }
        let runs = &mut class_open_runs[c];
        if runs.len() > 1
            && runs[0].start_ps == 0
            && runs.last().unwrap().end_ps == cycle_ps as u128
        {
            let head = runs[0].end_ps;
            runs.last_mut().unwrap().end_ps += head;
        }
    }
    Ok(Schedule {
        id: name,
        base_ps,
        cycle_ps,
        entries,
        prefix_ps,
        class_open_runs,
        open_total_ps,
    })
}
/// Raw duplicate JSON keys must be rejected before serde_json::Value decoding.
pub(crate) fn prepare(config: &Value, base: &PreparedEthernet) -> Result<PreparedTsn> {
    let w = obj(config, &["schema_version", "base", "tsn"], "model-config")?;
    if w["schema_version"].as_u64() != Some(1) {
        return Err(err("schema_version", "TSN requires schema 1"));
    }
    let b = obj(&w["base"], &["profile", "config"], "base")?;
    if s(b, "profile", "base")? != "ethernet.l2.dynamic.v1" || !b["config"].is_object() {
        return Err(err("base", "TSN requires dynamic Ethernet base config"));
    }
    if base.media.is_some() {
        return Err(err(
            "base",
            "TSN media/half-duplex/PAUSE combination is unsupported",
        ));
    }
    let c = obj(
        &w["tsn"],
        &["clock", "outputs", "streams", "gcl_updates"],
        "tsn",
    )?;
    if s(c, "clock", "tsn")? != "ideal_shared" {
        return Err(err("tsn.clock", "only ideal_shared clock is supported"));
    }
    let mut outputs = Vec::new();
    let mut port_index = BTreeMap::new();
    let mut schedule_ids: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    for (i, v) in arr(c, "outputs", "tsn")?.iter().enumerate() {
        let p = format!("tsn.outputs[{i}]");
        let o = obj(v, &["port", "tas", "cbs"], &p)?;
        let port = id(o, "port", &p)?;
        let direction = base
            .directions
            .iter()
            .find(|d| d.from_port == port)
            .ok_or_else(|| err(&p, "unknown connected output port"))?;
        let output = base
            .outputs
            .iter()
            .find(|o| o.port == port)
            .ok_or_else(|| err(&p, "missing base output config"))?;
        if output.scheduler != "strict_priority"
            || output.queues.len() != 8
            || (0..8).any(|c| output.queues.iter().filter(|q| q.priority == c).count() != 1)
        {
            return Err(err(
                &p,
                "TSN requires strict_priority and all 8 unique classes",
            ));
        }
        if port_index.contains_key(port) {
            return Err(err(&p, "duplicate output port"));
        }
        let tas = if o["tas"].is_null() {
            None
        } else {
            Some(Arc::new(schedule(&o["tas"], false, &format!("{p}.tas"))?))
        };
        let mut names = BTreeSet::new();
        if let Some(t) = &tas {
            names.insert(t.id.clone());
        }
        schedule_ids.insert(port.into(), names);
        let mut cbs: [Option<CbsConfig>; 8] = std::array::from_fn(|_| None);
        for (i, v) in arr(o, "cbs", &p)?.iter().enumerate() {
            let p = format!("{p}.cbs[{i}]");
            let r = obj(
                v,
                &[
                    "priority",
                    "idle_slope_bps",
                    "hi_credit_bits",
                    "lo_credit_bits",
                ],
                &p,
            )?;
            let priority = j(r, "priority", 0, 7, &p)? as u8;
            let idle_bps = d(r, "idle_slope_bps", &p)?;
            if idle_bps == 0
                || idle_bps >= direction.bitrate_bps
                || cbs[priority as usize].is_some()
            {
                return Err(err(&p, "CBS slope out of range or duplicate class"));
            }
            cbs[priority as usize] = Some(CbsConfig {
                priority,
                idle_bps,
                link_bps: direction.bitrate_bps,
                hi: scale(d(r, "hi_credit_bits", &p)?, false, &p)?,
                lo: scale(d(r, "lo_credit_bits", &p)?, false, &p)?,
            });
        }
        outputs.push(TsnOutput {
            port: port.into(),
            link_bps: direction.bitrate_bps,
            tas,
            cbs,
        });
        port_index.insert(port.into(), 0);
    }
    if outputs.len() != base.directions.len() {
        return Err(err(
            "tsn.outputs",
            "every connected output must be listed exactly once",
        ));
    }
    outputs.sort_by(|a, b| a.port.cmp(&b.port));
    for (i, o) in outputs.iter().enumerate() {
        port_index.insert(o.port.clone(), i);
    }
    let mut streams = Vec::new();
    let mut stream_index = BTreeMap::new();
    let mut stream_ids = BTreeSet::new();
    for (i, v) in arr(c, "streams", "tsn")?.iter().enumerate() {
        let p = format!("tsn.streams[{i}]");
        let o = obj(
            v,
            &[
                "id",
                "ingress",
                "dst_mac",
                "vid",
                "priority",
                "max_sdu_bytes",
                "gate",
                "meter",
            ],
            &p,
        )?;
        let name = id(o, "id", &p)?;
        let ingress = id(o, "ingress", &p)?;
        if !base.directions.iter().any(|d| d.to_port == ingress)
            || !stream_ids.insert(name.to_owned())
        {
            return Err(err(&p, "unknown ingress or duplicate stream ID"));
        }
        let mac = s(o, "dst_mac", &p)?.to_ascii_lowercase();
        if mac.len() != 17
            || mac.split(':').count() != 6
            || mac
                .split(':')
                .any(|s| s.len() != 2 || !s.bytes().all(|b| b.is_ascii_hexdigit()))
        {
            return Err(err(&p, "invalid MAC address"));
        }
        let key = StreamKey {
            ingress: ingress.into(),
            dst_mac: mac,
            vid: j(o, "vid", 1, 4094, &p)? as u16,
            priority: j(o, "priority", 0, 7, &p)? as u8,
        };
        if stream_index.insert(key.clone(), streams.len()).is_some() {
            return Err(err(&p, "duplicate normalized stream key"));
        }
        let gate = if o["gate"].is_null() {
            None
        } else {
            Some(Arc::new(schedule(&o["gate"], true, &format!("{p}.gate"))?))
        };
        let meter = if o["meter"].is_null() {
            None
        } else {
            let m = obj(
                &o["meter"],
                &[
                    "committed_rate_bps",
                    "peak_rate_bps",
                    "committed_burst_bytes",
                    "peak_burst_bytes",
                    "yellow_action",
                ],
                &format!("{p}.meter"),
            )?;
            let cr = d(m, "committed_rate_bps", &p)?;
            let pr = d(m, "peak_rate_bps", &p)?;
            let cb = d(m, "committed_burst_bytes", &p)?;
            let pb = d(m, "peak_burst_bytes", &p)?;
            if cr == 0 || cr > pr || cb == 0 || cb > pb {
                return Err(err(&p, "invalid two-bucket rate/burst bounds"));
            }
            let yellow_drop = match s(m, "yellow_action", &p)? {
                "drop" => true,
                "pass" => false,
                _ => return Err(err(&p, "unknown yellow_action")),
            };
            Some(MeterConfig {
                committed_rate_bps: cr,
                peak_rate_bps: pr,
                committed_cap: scale(cb, true, &p)?,
                peak_cap: scale(pb, true, &p)?,
                yellow_drop,
            })
        };
        streams.push(StreamConfig {
            id: name.into(),
            key,
            max_sdu_bytes: d(o, "max_sdu_bytes", &p)?,
            gate,
            meter,
        });
    }
    streams.sort_by(|a, b| a.key.cmp(&b.key));
    for (i, s) in streams.iter().enumerate() {
        stream_index.insert(s.key.clone(), i);
    }
    let mut updates = Vec::new();
    let mut ids = BTreeSet::new();
    let mut times = BTreeSet::new();
    for (i, v) in arr(c, "gcl_updates", "tsn")?.iter().enumerate() {
        let p = format!("tsn.gcl_updates[{i}]");
        let o = obj(
            v,
            &[
                "id",
                "submitted_at_ps",
                "effective_at_ps",
                "port",
                "schedule",
            ],
            &p,
        )?;
        let name = id(o, "id", &p)?;
        let port = id(o, "port", &p)?;
        let submitted = d(o, "submitted_at_ps", &p)?;
        let effective = d(o, "effective_at_ps", &p)?;
        let schedule = Arc::new(schedule(&o["schedule"], false, &format!("{p}.schedule"))?);
        if !port_index.contains_key(port)
            || !ids.insert(name.to_owned())
            || !times.insert((effective, port.to_owned()))
            || submitted > effective
            || schedule.base_ps != effective
        {
            return Err(err(&p, "invalid update reference/ID/effective time"));
        }
        if !schedule_ids
            .get_mut(port)
            .unwrap()
            .insert(schedule.id.clone())
        {
            return Err(err(&p, "schedule ID must be unique within port"));
        }
        updates.push(GclUpdate {
            id: name.into(),
            submitted_at_ps: submitted,
            effective_at_ps: effective,
            port: port.into(),
            schedule,
        });
    }
    updates.sort_by(|a, b| {
        (a.effective_at_ps, &a.port, &a.id).cmp(&(b.effective_at_ps, &b.port, &b.id))
    });
    Ok(PreparedTsn {
        outputs,
        port_index,
        streams,
        stream_index,
        updates,
    })
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::types::ethernet::{EthernetDirection, EthernetOutputConfig, EthernetQueueConfig};
    use serde_json::json;
    pub(crate) fn base() -> PreparedEthernet {
        PreparedEthernet {
            devices: vec![],
            directions: vec![EthernetDirection {
                channel_id: "l".into(),
                from_port: "N.a.tx".into(),
                to_port: "N.b.rx".into(),
                source: 0,
                destination: 1,
                bitrate_bps: 1_000_000_000,
                delay_ps: 0,
            }],
            generators: vec![],
            outputs: vec![EthernetOutputConfig {
                port: "N.a.tx".into(),
                scheduler: "strict_priority".into(),
                queues: (0..8)
                    .map(|priority| EthernetQueueConfig {
                        priority,
                        capacity_frames: 2,
                        capacity_bytes: None,
                    })
                    .collect(),
            }],
            port_policies: vec![],
            media: None,
        }
    }
    pub(crate) fn config() -> Value {
        json!({"schema_version":1,"base":{"profile":"ethernet.l2.dynamic.v1","config":{}},"tsn":{"clock":"ideal_shared","outputs":[{"port":"N.a.tx","tas":null,"cbs":[]}],"streams":[],"gcl_updates":[]}})
    }
    pub(crate) fn gate(base: u64, cycle: u64, rows: &[(u64, bool)]) -> Value {
        json!({"id":"g0","base_time_ps":base.to_string(),"cycle_time_ps":cycle.to_string(),"entries":rows.iter().map(|(d,open)|json!({"duration_ps":d.to_string(),"open_priorities":if *open {vec![7]}else{vec![]}})).collect::<Vec<_>>()})
    }
    pub(crate) fn prep(mut v: Value) -> PreparedTsn {
        // Helpers always exercise production prepare, including reference checks.
        v["schema_version"] = json!(1);
        prepare(&v, &base()).unwrap()
    }
    #[test]
    fn rejects_missing_unknown_numeric_bool_and_noncanonical_fields() {
        let v = config();
        assert!(prepare(&v, &base()).is_ok());
        for value in [
            json!(0),
            json!(false),
            json!("00"),
            json!("+1"),
            json!(" 1"),
            json!("18446744073709551616"),
        ] {
            let mut v = config();
            v["tsn"]["outputs"][0]["tas"] = gate(0, 10, &[(10, true)]);
            v["tsn"]["outputs"][0]["tas"]["cycle_time_ps"] = value;
            assert!(prepare(&v, &base()).is_err());
        }
        let mut v = config();
        v["tsn"]["bogus"] = json!(1);
        assert!(prepare(&v, &base()).is_err());
        let mut v = config();
        v["tsn"].as_object_mut().unwrap().remove("clock");
        assert!(prepare(&v, &base()).is_err());
        for priorities in [json!([7, 7]), json!([8]), json!([true])] {
            let mut v = config();
            v["tsn"]["outputs"][0]["tas"] = gate(0, 10, &[(10, true)]);
            v["tsn"]["outputs"][0]["tas"]["entries"][0]["open_priorities"] = priorities;
            assert!(prepare(&v, &base()).is_err());
        }
        for rows in [
            vec![(0, true)],
            vec![(9, true)],
            vec![(u64::MAX, true), (1, false)],
        ] {
            let mut v = config();
            v["tsn"]["outputs"][0]["tas"] = gate(0, 10, &rows);
            assert!(prepare(&v, &base()).is_err());
        }
    }
    #[test]
    fn rejects_output_and_cbs_combinations_and_accepts_zero_bounds() {
        for port in ["unknown.tx", ""] {
            let mut v = config();
            v["tsn"]["outputs"][0]["port"] = json!(port);
            assert!(prepare(&v, &base()).is_err());
        }
        let mut v = config();
        let out = v["tsn"]["outputs"][0].clone();
        v["tsn"]["outputs"].as_array_mut().unwrap().push(out);
        assert!(prepare(&v, &base()).is_err());
        let mut b = base();
        b.outputs[0].scheduler = "fifo".into();
        assert!(prepare(&config(), &b).is_err());
        let mut b = base();
        b.outputs[0].queues.pop();
        assert!(prepare(&config(), &b).is_err());
        let row = json!({"priority":7,"idle_slope_bps":"250000000","hi_credit_bits":"0","lo_credit_bits":"0"});
        let mut v = config();
        v["tsn"]["outputs"][0]["cbs"] = json!([row]);
        assert!(prepare(&v, &base()).is_ok());
        for rate in ["0", "1000000000", "1000000001"] {
            v["tsn"]["outputs"][0]["cbs"][0]["idle_slope_bps"] = json!(rate);
            assert!(prepare(&v, &base()).is_err());
        }
        v["tsn"]["outputs"][0]["cbs"] = json!([row, row]);
        assert!(prepare(&v, &base()).is_err());
    }
    #[test]
    fn validates_every_future_update_and_normalizes_order() {
        let update = |id: &str, time: u64, sid: &str| {
            let mut s = gate(time, 10, &[(10, true)]);
            s["id"] = json!(sid);
            json!({"id":id,"submitted_at_ps":time.to_string(),"effective_at_ps":time.to_string(),"port":"N.a.tx","schedule":s})
        };
        let mut v = config();
        v["tsn"]["gcl_updates"] = json!([update("u2", 200, "g2"), update("u1", 100, "g1")]);
        let p = prep(v.clone());
        assert_eq!(p.updates[0].id, "u1");
        for (field, value) in [
            ("submitted_at_ps", json!("201")),
            ("port", json!("unknown")),
        ] {
            let mut bad = v.clone();
            bad["tsn"]["gcl_updates"][0][field] = value;
            assert!(prepare(&bad, &base()).is_err());
        }
        let mut bad = v.clone();
        bad["tsn"]["gcl_updates"][0]["schedule"]["base_time_ps"] = json!("0");
        assert!(prepare(&bad, &base()).is_err());
        let mut bad = v.clone();
        bad["tsn"]["gcl_updates"][1] = update("u2", 100, "g1");
        assert!(prepare(&bad, &base()).is_err());
        let mut bad = v.clone();
        bad["tsn"]["gcl_updates"][1] = update("u1", 200, "g1");
        assert!(prepare(&bad, &base()).is_err());
        let mut bad = v.clone();
        bad["tsn"]["gcl_updates"][1] = update("u1", 100, "g2");
        assert!(prepare(&bad, &base()).is_err());
    }
    pub(crate) fn stream() -> Value {
        json!({"id":"s","ingress":"N.b.rx","dst_mac":"01:00:5e:00:00:01","vid":1,"priority":7,"max_sdu_bytes":"64","gate":null,"meter":{"committed_rate_bps":"8000000","peak_rate_bps":"16000000","committed_burst_bytes":"64","peak_burst_bytes":"128","yellow_action":"pass"}})
    }
    #[test]
    fn validates_stream_tuple_gate_and_meter() {
        let mut v = config();
        v["tsn"]["streams"] = json!([stream()]);
        assert!(prepare(&v, &base()).is_ok());
        for (field, value) in [
            ("ingress", json!("N.a.tx")),
            ("vid", json!(0)),
            ("priority", json!(true)),
            ("dst_mac", json!("bad")),
            ("id", json!("非ASCII")),
        ] {
            let mut bad = v.clone();
            bad["tsn"]["streams"][0][field] = value;
            assert!(prepare(&bad, &base()).is_err());
        }
        for (field, value) in [
            ("committed_rate_bps", json!("0")),
            ("committed_rate_bps", json!("16000001")),
            ("committed_burst_bytes", json!("0")),
            ("committed_burst_bytes", json!("129")),
            ("yellow_action", json!("other")),
        ] {
            let mut bad = v.clone();
            bad["tsn"]["streams"][0]["meter"][field] = value;
            assert!(prepare(&bad, &base()).is_err());
        }
        let mut other = stream();
        other["id"] = json!("s2");
        other["dst_mac"] = json!("01:00:5E:00:00:01");
        v["tsn"]["streams"] = json!([stream(), other]);
        assert!(prepare(&v, &base()).is_err());
    }
}
