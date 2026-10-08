//! Media-specific schema projection and clipped signal observations.
use super::*;
use crate::types::ethernet::{EthernetPhyEnd, EthernetPhysicalLink};
pub(crate) const METRICS: &str = "ethernet.media.attempts ethernet.media.collisions ethernet.media.retry_exhausted ethernet.media.queue_length ethernet.media.queue_mean ethernet.media.tx_utilization ethernet.media.jam_ps ethernet.media.backoff_slots ethernet.media.delivered";
fn phy(end: &EthernetPhyEnd) -> Value {
    json!({"role":end.role,"tx_latency_ps":end.tx_latency_ps.to_string(),"rx_latency_ps":end.rx_latency_ps.to_string()})
}
fn link_json(prepared: &PreparedSimulation, link: &EthernetPhysicalLink) -> Value {
    let ethernet = prepared.ethernet.as_ref().unwrap();
    let direction = &ethernet.directions[link.directions[0]];
    let half = link.duplex == "half";
    json!({"a":link.a,"b":link.b,"phy_mode":link.phy_mode,"duplex":link.duplex,"bitrate_bps":direction.bitrate_bps.to_string(),"propagation_ps":direction.delay_ps.to_string(),"a_phy":phy(&link.a_phy),"b_phy":phy(&link.b_phy),"link_state":"up","seed":ethernet.media.as_ref().unwrap().seed.to_string(),"deference_policy":if half {"continuous-idle-96.v1"} else {"not-applicable"},"backoff_policy":if half {"sha256-beb.v1"} else {"not-applicable"}})
}
pub(super) fn model_rows(
    prepared: &PreparedSimulation,
    state: &EthernetSnapshot,
    rows: &mut Vec<Value>,
) {
    for (row, transfer) in rows
        .iter_mut()
        .filter(|r| r["schema_name"] == "ethernet.transfer")
        .zip(&state.transfers)
    {
        let media = transfer.media.as_ref().unwrap();
        row["schema_version"] = json!(2);
        row["data"]["physical_link"] = json!(media.physical_link);
        row["data"]["attempt_count"] = json!(media.attempt_count.to_string());
        row["data"]["collision_count"] = json!(media.collision_count.to_string());
        row["data"]["last_attempt_id"] = json!(media.last_attempt_id);
        row["data"]["backoff_until_ps"] = opt_d(media.backoff_until_ps);
    }
    for attempt in &state.attempts {
        let t = &state.transfers[attempt.transfer];
        let m = t.media.as_ref().unwrap();
        rows.push(json!({"schema_name":"ethernet.attempt","schema_version":1,"record_id":attempt.attempt_id,"subject":t.from_port,"request_id":t.frame_id,"origin_request_id":null,"time_ps":attempt.time_ps.to_string(),"data":{
            "transfer_id":t.transfer_id,"physical_link":m.physical_link,"from_port":t.from_port,"to_port":t.to_port,"number":attempt.number.to_string(),"sof_ps":attempt.sof_ps.to_string(),"planned_eof_ps":attempt.planned_eof_ps.to_string(),"planned_release_ps":attempt.planned_release_ps.to_string(),"planned_arrival_ps":attempt.planned_arrival_ps.to_string(),"collision_ps":opt_d(attempt.collision_ps),"planned_jam_start_ps":opt_d(attempt.planned_jam_start_ps),"planned_jam_end_ps":opt_d(attempt.planned_jam_end_ps),"jam_end_ps":opt_d(attempt.jam_end_ps),"eof_ps":opt_d(attempt.eof_ps),"release_ps":opt_d(attempt.release_ps),"arrival_ps":opt_d(attempt.arrival_ps),"backoff_slots":opt_d(attempt.backoff_slots),"backoff_until_ps":opt_d(attempt.backoff_until_ps),"status":attempt.status,"planned_mdi_sof_ps":attempt.planned_mdi_sof_ps.to_string(),"planned_mdi_eof_ps":opt_d(attempt.planned_mdi_eof_ps),"planned_peer_mdi_sof_ps":attempt.planned_peer_mdi_sof_ps.to_string(),"planned_peer_mdi_eof_ps":opt_d(attempt.planned_peer_mdi_eof_ps)
        }}));
    }
    for link in &prepared
        .ethernet
        .as_ref()
        .unwrap()
        .media
        .as_ref()
        .unwrap()
        .physical_links
    {
        rows.push(json!({"schema_name":"ethernet.phy_link","schema_version":1,"record_id":link.id,"subject":format!("@media:{}",link.id),"request_id":null,"origin_request_id":null,"time_ps":"0","data":link_json(prepared,link)}));
    }
}
fn interval(start: u64, end: u64, left: u64, right: u64) -> u128 {
    end.min(right).saturating_sub(start.max(left)) as u128
}
fn windows(
    prepared: &PreparedSimulation,
    snapshot: &Snapshot,
    start: u64,
    end: u64,
) -> Result<Vec<Record>, Diagnostic> {
    let eth = prepared.ethernet.as_ref().unwrap();
    let state = snapshot.ethernet.as_ref().unwrap();
    let h = snapshot.common.end_ps;
    let mut rows = Vec::new();
    for d in &eth.directions {
        let mut signal = 0u128;
        let mut jam = 0u128;
        for a in state
            .attempts
            .iter()
            .filter(|a| state.transfers[a.transfer].from_port == d.from_port)
        {
            let stop = if a.collision_ps.is_some() {
                a.jam_end_ps.unwrap_or(h)
            } else {
                a.eof_ps.unwrap_or(h)
            };
            signal = checked_add(signal, interval(a.sof_ps, stop, start, end))?;
            if let Some(jam_start) = a.planned_jam_start_ps {
                jam = checked_add(
                    jam,
                    interval(
                        jam_start,
                        a.jam_end_ps.or(a.planned_jam_end_ps).unwrap_or(h).min(h),
                        start,
                        end,
                    ),
                )?;
            }
        }
        rows.push(Record::aggregate(
            &d.from_port,
            "ethernet.media.tx_utilization",
            ratio(signal, (end - start) as u128)?,
            start,
            end,
        ));
        rows.push(Record::aggregate(
            &d.from_port,
            "ethernet.media.jam_ps",
            MetricValue::Integer(jam),
            start,
            end,
        ));
    }
    Ok(rows)
}
pub(super) fn records(
    prepared: &PreparedSimulation,
    snapshot: &Snapshot,
) -> Result<(Vec<Record>, Vec<Record>), Diagnostic> {
    let eth = prepared.ethernet.as_ref().unwrap();
    let state = snapshot.ethernet.as_ref().unwrap();
    let h = snapshot.common.end_ps;
    let mut points = Vec::new();
    for (i, point) in snapshot.common.iter_points()?.enumerate() {
        let point = point?;
        points.push(Record::point(
            &point,
            &point.metric,
            MetricValue::Integer(point.value as u128),
            i,
        ));
    }
    let mut summary = Vec::new();
    for target in eth
        .directions
        .iter()
        .map(|d| d.from_port.as_str())
        .chain(std::iter::once("$all"))
    {
        let attempts: Vec<_> = state
            .attempts
            .iter()
            .filter(|a| target == "$all" || state.transfers[a.transfer].from_port == target)
            .collect();
        for (metric, value) in [
            ("ethernet.media.attempts", attempts.len()),
            (
                "ethernet.media.collisions",
                attempts.iter().filter(|a| a.collision_ps.is_some()).count(),
            ),
            (
                "ethernet.media.retry_exhausted",
                state
                    .transfers
                    .iter()
                    .filter(|t| {
                        (target == "$all" || t.from_port == target)
                            && t.drop_reason.as_deref() == Some("attempt_limit")
                    })
                    .count(),
            ),
        ] {
            summary.push(Record::aggregate(
                target,
                metric,
                MetricValue::Integer(value as u128),
                0,
                h,
            ));
        }
    }
    for target in eth
        .devices
        .iter()
        .filter(|d| d.kind == "endpoint")
        .map(|d| d.id.as_str())
        .chain(std::iter::once("$all"))
    {
        let n = state
            .receptions
            .iter()
            .filter(|r| r.status == "received" && (target == "$all" || r.device == target))
            .count();
        summary.push(Record::aggregate(
            target,
            "ethernet.media.delivered",
            MetricValue::Integer(n as u128),
            0,
            h,
        ));
    }
    for d in &eth.directions {
        let queue = format!("{}.queue", d.from_port);
        let mut integral = 0u128;
        let mut previous = 0;
        let mut length = 0u64;
        let changes = snapshot
            .common
            .select_points(|p| p.target == queue && p.metric == "ethernet.media.queue_length")?;
        for p in &changes {
            integral = checked_add(integral, (p.time_ps - previous) as u128 * length as u128)?;
            previous = p.time_ps;
            length = p.value;
        }
        integral = checked_add(integral, (h - previous) as u128 * length as u128)?;
        summary.push(Record::aggregate(
            &queue,
            "ethernet.media.queue_mean",
            ratio(integral, h as u128)?,
            0,
            h,
        ));
    }
    let mut start = 0;
    while start < h {
        let end = start
            .saturating_add(prepared.common.metrics_window_ps)
            .min(h);
        points.extend(windows(prepared, snapshot, start, end)?);
        start = end;
    }
    summary.extend(windows(prepared, snapshot, 0, h)?);
    if h == 0 && !snapshot.common.partial {
        points.clear();
    }
    Ok(finalize(points, summary))
}
pub(super) fn metadata(prepared: &PreparedSimulation, result: &mut Value) {
    let eth = prepared.ethernet.as_ref().unwrap();
    let config = eth.media.as_ref().unwrap();
    result["model_schemas"] = json!([{"schema_name":"ethernet.attempt","schema_version":1},{"schema_name":"ethernet.frame","schema_version":1},{"schema_name":"ethernet.phy_link","schema_version":1},{"schema_name":"ethernet.reception","schema_version":1},{"schema_name":"ethernet.transfer","schema_version":2}]);
    result["seed"] = json!(config.seed.to_string());
    result["models"][0]["assumptions"] = json!([
        "two MACs per physical pair",
        "continuous-idle-96.v1 half-duplex deference",
        "sha256-beb.v1 deterministic backoff",
        "fixed up links",
        "fixed calibrated TX/RX PHY latency",
        "static FDB connected tree",
        "MAC time and computed MDI planned time are distinct",
        "IEEE certification and PHY bitstreams excluded"
    ]);
    let pairs:Vec<_>=config.physical_links.iter().map(|p|json!({"id":p.id,"current_a":null,"current_b":null,"local_busy_a":false,"local_busy_b":false,"ifg_ready_a":true,"ifg_ready_b":true,"next_generation":"0"})).collect();
    result["initial_state"].as_array_mut().unwrap().push(json!({"instance":format!("@profile:{}:{}",prepared.common.profile,prepared.common.network),"state":canonical(&json!({"profile":prepared.common.profile,"deference_policy":"continuous-idle-96.v1","backoff_policy":"sha256-beb.v1","seed":config.seed.to_string(),"pairs":pairs,"attempts":[]}))}));
    result["initial_state"]
        .as_array_mut()
        .unwrap()
        .sort_by(|a, b| a["instance"].as_str().cmp(&b["instance"].as_str()));
    let links: Vec<_> = config
        .physical_links
        .iter()
        .map(|p| {
            let mut v = link_json(prepared, p);
            v["id"] = json!(p.id);
            v
        })
        .collect();
    result["topology"]["physical_links"] = json!(links);
    result["ethernet_topology"]["physical_links"] = json!(links);
}
