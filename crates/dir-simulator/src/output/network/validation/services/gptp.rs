//! Public timestamp proofs bind the selected plane to actual producer records.
use super::*;

fn value(v: &Value) -> Result<u64, Diagnostic> {
    v.as_u64().map(Ok).unwrap_or_else(|| number(v))
}
fn same(v: &Value, expected: u64) -> Result<(), Diagnostic> {
    if value(v)? != expected {
        return Err(fail("gPTP physical proof value differs"));
    }
    Ok(())
}
fn direction(
    n: &PreparedNetwork,
    transfer: &crate::registry::ModelRecord,
) -> Result<usize, Diagnostic> {
    n.ethernet
        .directions
        .iter()
        .position(|d| Some(d.from_port.as_str()) == transfer.data["from_port"].as_str())
        .ok_or_else(|| fail("gPTP source output is absent"))
}

pub(super) fn validate(
    n: &PreparedNetwork,
    snapshot: &Snapshot,
    record: &crate::registry::ModelRecord,
) -> Result<(), Diagnostic> {
    let data = &record.data;
    let tx = data["kind"] == "tx_capture";
    if !tx && data["kind"] != "rx_capture" {
        return Ok(());
    }
    let details = &data["details"];
    let proofs = ["sampled", "configured", "express"]
        .iter()
        .filter(|key| !details[**key].is_null())
        .count();
    if proofs > 1 {
        return Err(fail("gPTP timestamp has conflicting physical planes"));
    }
    let records = &snapshot.registered.as_ref().unwrap().model_records;
    let transfer = super::super::transfer_reference(n, records, &data["transfer_id"])?;
    let i = direction(n, transfer)?;
    let configured = n.ethernet.media.as_ref().and_then(|m| {
        m.physical_links.iter().enumerate().find_map(|(index, l)| {
            l.directions
                .iter()
                .position(|d| *d == i)
                .map(|side| (index, side, l))
        })
    });
    let abstract_mac = n
        .extensions
        .as_ref()
        .and_then(|e| e.preemption.as_ref())
        .is_some_and(|p| p.participating[i]);
    if configured.is_some() && details["configured"].is_null() {
        return Err(fail("configured gPTP capture lacks its physical proof"));
    }
    if configured.is_none() && abstract_mac && details["express"].is_null() {
        return Err(fail("abstract EXPRESS gPTP capture lacks source proof"));
    }
    let (proof, is_configured) = if !details["configured"].is_null() {
        (&details["configured"], true)
    } else if !details["express"].is_null() {
        (&details["express"], false)
    } else {
        return Ok(());
    };
    exact(
        proof,
        "source endpoint plane reference_ps observed_at_ps sof_ps mdi_sof_ps peer_sof_ps eof_planned_ps arrival_planned_ps bitrate_bps propagation_ps tx_latency_ps peer_rx_latency_ps",
    )?;
    if is_configured != configured.is_some()
        || proof["plane"]
            != if is_configured {
                "MdiMacSfd"
            } else {
                "IdealMacSfd"
            }
    {
        return Err(fail(
            "gPTP capture plane differs from selected physical owner",
        ));
    }
    let link = &n.ethernet.directions[i];
    let (tx_latency, rx_latency) = if let Some((index, side, cfg)) = configured {
        if cfg.duplex != "full" {
            return Err(fail("gPTP configured source is half duplex"));
        }
        let endpoint = ids(&proof["endpoint"])?;
        if endpoint.len() != 2
            || value(&endpoint[0])? != index as u64
            || value(&endpoint[1])? != side as u64
        {
            return Err(fail("gPTP configured endpoint differs"));
        }
        if side == 0 {
            (cfg.a_phy.tx_latency_ps, cfg.b_phy.rx_latency_ps)
        } else {
            (cfg.b_phy.tx_latency_ps, cfg.a_phy.rx_latency_ps)
        }
    } else {
        if !proof["endpoint"].is_null() {
            return Err(fail("ideal gPTP source invented a configured endpoint"));
        }
        (0, 0)
    };
    same(&proof["bitrate_bps"], link.bitrate_bps)?;
    same(&proof["propagation_ps"], link.delay_ps)?;
    same(&proof["tx_latency_ps"], tx_latency)?;
    same(&proof["peer_rx_latency_ps"], rx_latency)?;
    let sof = value(&proof["sof_ps"])?;
    same(&transfer.data["sof_ps"], sof)?;
    let eof = value(&proof["eof_planned_ps"])?;
    let arrival = value(&proof["arrival_planned_ps"])?;
    let mdi = u128::from(sof) + u128::from(tx_latency);
    let peer = mdi + u128::from(link.delay_ps);
    let delta = (64u128 * 1_000_000_000_000).div_ceil(u128::from(link.bitrate_bps));
    let expected_reference = if tx { mdi + delta } else { peer + delta };
    if u128::from(value(&proof["mdi_sof_ps"])?) != mdi
        || u128::from(value(&proof["peer_sof_ps"])?) != peer
        || u128::from(value(&proof["reference_ps"])?) != expected_reference
        || value(&proof["observed_at_ps"])? != record.time_ps
        || expected_reference != u128::from(record.time_ps)
        || u128::from(arrival)
            != u128::from(eof)
                + u128::from(tx_latency)
                + u128::from(link.delay_ps)
                + u128::from(rx_latency)
        || eof < sof
    {
        return Err(fail("gPTP source reference/availability timeline differs"));
    }
    same(&details["reference_ps"], record.time_ps)?;
    let source = proof["source"]
        .as_object()
        .filter(|o| o.len() == 1)
        .ok_or_else(|| fail("gPTP source variant is invalid"))?;
    let (kind, source) = source.iter().next().unwrap();
    for scalar in source
        .as_object()
        .ok_or_else(|| fail("gPTP source payload is not an object"))?
        .values()
    {
        value(scalar)?;
    }
    let start_before = |r: &&crate::registry::ModelRecord| r.time_ps <= record.time_ps;
    match kind.as_str() {
        "OrdinaryAttempt" => {
            exact(source, "index transfer number direction physical_link side")?;
            same(&source["direction"], i as u64)?;
            let Some((index, side, cfg)) = configured else {
                return Err(fail("ordinary configured source has no physical link"));
            };
            same(&source["physical_link"], index as u64)?;
            same(&source["side"], side as u64)?;
            if abstract_mac
                || n.extensions
                    .as_ref()
                    .and_then(|e| e.mac_merge.as_ref())
                    .is_some_and(|m| m.participating[i])
            {
                return Err(fail("ordinary gPTP proof names another MAC owner"));
            }
            let candidates = records
                .values()
                .filter(start_before)
                .filter(|r| {
                    r.schema.name == "dir.network.configured_media"
                        && r.data["kind"] == "start"
                        && r.data["work_id"] == transfer.id
                        && r.data["physical_link"] == cfg.id
                        && value(&r.data["attempt"]["number"]).ok() == value(&source["number"]).ok()
                })
                .collect::<Vec<_>>();
            if candidates.len() != 1 {
                return Err(fail("gPTP configured source has no unique actual attempt"));
            }
            same(
                &candidates[0].data["attempt_index"],
                value(&source["index"])?,
            )?;
            let a = &candidates[0].data["attempt"];
            same(&a["direction"], i as u64)?;
            same(&a["owner"]["Data"], value(&source["transfer"])?)?;
            for (a_key, p_key) in [
                ("sof", "sof_ps"),
                ("eof", "eof_planned_ps"),
                ("arrival", "arrival_planned_ps"),
                ("mdi_sof", "mdi_sof_ps"),
                ("peer_sof", "peer_sof_ps"),
            ] {
                same(&a[a_key], value(&proof[p_key])?)?;
            }
        }
        "MergeExpress" => {
            exact(source, "transfer direction packet_id epoch")?;
            same(&source["direction"], i as u64)?;
            let p = records
                .values()
                .find(|r| {
                    r.schema.name == "dir.mac_merge.packet"
                        && value(&r.data["packet_id"]).ok() == value(&source["packet_id"]).ok()
                })
                .ok_or_else(|| fail("gPTP Merge source packet is absent"))?;
            if p.data["kind"] != "Express"
                || p.data["transfer"] != transfer.id
                || p.data["port"] != link.from_port
            {
                return Err(fail("gPTP Merge source identity differs"));
            }
            same(&p.data["transfer_index"], value(&source["transfer"])?)?;
            same(&p.data["epoch"], value(&source["epoch"])?)?;
            for (key, p_key) in [
                ("sof_ps", "sof_ps"),
                ("planned_eof_ps", "eof_planned_ps"),
                ("planned_arrival_ps", "arrival_planned_ps"),
            ] {
                same(&p.data[key], value(&proof[p_key])?)?;
            }
        }
        "AbstractExpress" => {
            exact(source, "transfer direction epoch")?;
            let source_transfer = value(&source["transfer"])?;
            same(&source["direction"], i as u64)?;
            if !abstract_mac
                || transfer.schema.name != "dir.preemption.transfer"
                || transfer.data["kind"] != "express"
            {
                return Err(fail("gPTP source is not abstract EXPRESS"));
            }
            same(&transfer.data["planned_eof_ps"], eof)?;
            same(&transfer.data["planned_arrival_ps"], arrival)?;
            if !records.values().any(|r| {
                r.schema.name == "dir.preemption.decision"
                    && r.time_ps == sof
                    && r.data["port"] == link.from_port
                    && r.data["active_transfer_id"] == transfer.id
                    && value(&r.data["active_transfer_index"]).ok() == Some(source_transfer)
                    && value(&r.data["tx_epoch"]).ok() == value(&source["epoch"]).ok()
            }) {
                return Err(fail(
                    "abstract EXPRESS source lacks actual serializer token",
                ));
            }
        }
        _ => return Err(fail("unknown gPTP MAC source variant")),
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn actual_configured_capture_rejects_changed_source_plane_and_reference() {
        let input = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../examples/network-composite/gptp-configured/model.ini");
        let prepared = crate::prepare(&input).unwrap();
        let n = prepared
            .registered
            .as_ref()
            .unwrap()
            .network
            .as_ref()
            .unwrap();
        let snapshot = crate::runtime::simulate(&prepared).unwrap();
        assert_ne!(snapshot.common.termination, "execution_failed");
        let source = snapshot
            .registered
            .as_ref()
            .unwrap()
            .model_records
            .values()
            .find(|r| {
                r.schema.name == "dir.network.gptp"
                    && r.data["kind"] == "tx_capture"
                    && !r.data["details"]["configured"].is_null()
            })
            .unwrap()
            .clone();
        validate(n, &snapshot, &source).unwrap();
        for which in 0..8 {
            let mut changed = source.clone();
            let details = &mut changed.data["details"];
            match which {
                0 => details["configured"]["plane"] = json!("IdealMacSfd"),
                1 => {
                    details["configured"]["reference_ps"] = json!(
                        (value(&details["configured"]["reference_ps"]).unwrap() + 1).to_string()
                    )
                }
                2 => {
                    details["configured"]["endpoint"][1] = json!(
                        (1 - value(&details["configured"]["endpoint"][1]).unwrap()).to_string()
                    )
                }
                3 => {
                    details["configured"]["source"]["OrdinaryAttempt"]["number"] = json!(
                        (value(&details["configured"]["source"]["OrdinaryAttempt"]["number"])
                            .unwrap()
                            + 1)
                        .to_string()
                    )
                }
                4 => details["sampled"] = json!({}),
                5 => details["configured"] = Value::Null,
                6 => {
                    details["configured"]["tx_latency_ps"] = json!(
                        (value(&details["configured"]["tx_latency_ps"]).unwrap() + 1).to_string()
                    )
                }
                _ => {
                    details["configured"]["source"]["OrdinaryAttempt"]["index"] = json!(
                        (value(&details["configured"]["source"]["OrdinaryAttempt"]["index"])
                            .unwrap()
                            + 1)
                        .to_string()
                    )
                }
            }
            assert!(
                validate(n, &snapshot, &changed).is_err(),
                "changed source proof {which}"
            );
        }
    }
    #[test]
    fn actual_express_captures_reject_changed_numeric_transfer_identity() {
        for fixture in ["merge.ini", "abstract.ini", "abstract-ideal.ini"] {
            let input = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../../examples/network-composite/gptp-configured")
                .join(fixture);
            let prepared = crate::prepare(&input).unwrap();
            let n = prepared
                .registered
                .as_ref()
                .unwrap()
                .network
                .as_ref()
                .unwrap();
            let snapshot = crate::runtime::simulate(&prepared).unwrap();
            assert_ne!(snapshot.common.termination, "execution_failed", "{fixture}");
            let records = &snapshot.registered.as_ref().unwrap().model_records;
            let source = records
                .values()
                .find(|r| {
                    r.schema.name == "dir.network.gptp"
                        && r.data["kind"] == "tx_capture"
                        && (!r.data["details"]["configured"].is_null()
                            || !r.data["details"]["express"].is_null())
                })
                .unwrap()
                .clone();
            validate(n, &snapshot, &source).unwrap();
            let mut changed = source.clone();
            let proof = if !changed.data["details"]["configured"].is_null() {
                "configured"
            } else {
                "express"
            };
            let kind = if fixture == "merge.ini" {
                "MergeExpress"
            } else {
                "AbstractExpress"
            };
            let identity = &mut changed.data["details"][proof]["source"][kind]["transfer"];
            *identity = json!((value(identity).unwrap() + 1).to_string());
            assert!(
                validate(n, &snapshot, &changed).is_err(),
                "unbound source index {fixture}"
            );
        }
    }
}
