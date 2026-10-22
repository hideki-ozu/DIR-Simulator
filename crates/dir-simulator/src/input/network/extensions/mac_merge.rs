use super::*;
use crate::types::network::mac_merge::*;
pub(super) fn prepare(
    v: &Value,
    n: &PreparedNetwork,
    _workload: &Value,
    _allow_gptp_clocks: bool,
) -> Result<PreparedMacMerge> {
    let p = "/mac_merge";
    let mut fields = vec!["standard", "outputs", "capability_adapter", "limits"];
    if v.get("packet_faults").is_some() {
        fields.push("packet_faults");
    }
    exact(v, &fields, "MAC Merge").map_err(|d| d.with_target(p))?;
    if v["standard"] != "ieee8023_2022_clause99" {
        return Err(error("unknown MAC Merge standard").with_target(format!("{p}/standard")));
    }
    // Every composition is checked at its owned output below. Unrelated
    // physical/control resources do not change this serializer's contract.
    exact(
        &v["capability_adapter"],
        &["name", "interval_ms", "ttl_seconds"],
        "capability adapter",
    )
    .map_err(|d| d.with_target(format!("{p}/capability_adapter")))?;
    if v["capability_adapter"]["name"] != "dir_lldp_capability_v1" {
        return Err(
            error("explicit bounded dir_lldp_capability_v1 adapter required")
                .with_target(format!("{p}/capability_adapter/name")),
        );
    }
    let interval = integer(
        &v["capability_adapter"]["interval_ms"],
        1,
        1_000_000,
        &format!("{p}/capability_adapter/interval_ms"),
    )?;
    let ttl = integer(
        &v["capability_adapter"]["ttl_seconds"],
        1,
        65535,
        &format!("{p}/capability_adapter/ttl_seconds"),
    )?;
    if interval >= ttl * 1000 {
        return Err(
            error("LLDP advertisement interval must be shorter than TTL")
                .with_target(format!("{p}/capability_adapter")),
        );
    }
    let mut limit_fields = vec!["max_rx_mac_octets", "max_pending_packets"];
    for key in ["max_accounting_events", "max_accounting_octets"] {
        if v["limits"].get(key).is_some() {
            limit_fields.push(key);
        }
    }
    exact(&v["limits"], &limit_fields, "Merge limits")
        .map_err(|d| d.with_target(format!("{p}/limits")))?;
    let mut out = PreparedMacMerge {
        standard: "ieee8023_2022_clause99".into(),
        ports: vec![None; n.ethernet.directions.len()],
        reverse: vec![0; n.ethernet.directions.len()],
        participating: vec![false; n.ethernet.directions.len()],
        lldp_interval_ps: interval * 1_000_000_000,
        lldp_ttl_seconds: ttl as u16,
        max_rx_mac_octets: integer(
            &v["limits"]["max_rx_mac_octets"],
            64,
            1_000_000,
            &format!("{p}/limits/max_rx_mac_octets"),
        )? as usize,
        max_pending_packets: integer(
            &v["limits"]["max_pending_packets"],
            1,
            1_000_000,
            &format!("{p}/limits/max_pending_packets"),
        )? as usize,
        max_accounting_events: v["limits"]
            .get("max_accounting_events")
            .map(|x| {
                integer(
                    x,
                    1,
                    10_000_000,
                    &format!("{p}/limits/max_accounting_events"),
                )
            })
            .transpose()?
            .unwrap_or(100_000) as usize,
        max_accounting_octets: v["limits"]
            .get("max_accounting_octets")
            .map(|x| {
                integer(
                    x,
                    1,
                    1_000_000_000,
                    &format!("{p}/limits/max_accounting_octets"),
                )
            })
            .transpose()?
            .unwrap_or(67_108_864) as usize,
        faults: vec![],
    };
    for (i, d) in n.ethernet.directions.iter().enumerate() {
        out.reverse[i] = n
            .ethernet
            .directions
            .iter()
            .position(|r| {
                r.source == d.destination
                    && r.destination == d.source
                    && r.from_port == crate::input::ethernet::paired(&d.to_port)
                    && r.to_port == crate::input::ethernet::paired(&d.from_port)
            })
            .ok_or_else(|| {
                error("Clause99 requires paired physical reverse direction").with_target(p)
            })?;
    }
    let rows = array(&v["outputs"], &format!("{p}/outputs"))?;
    if rows.is_empty() {
        return Err(error("MAC Merge requires at least one configured output").with_target(p));
    }
    for (index, row) in rows.iter().enumerate() {
        let at = format!("{p}/outputs/{index}");
        exact(
            row,
            &[
                "port",
                "source_mac",
                "express_classes",
                "preemptable_classes",
                "advertised_add_frag_size",
                "verify_time_ms",
                "admin_enabled",
            ],
            "Merge output",
        )
        .map_err(|d| d.with_target(&at))?;
        let port = string(&row["port"], &format!("{at}/port"))?;
        if port.len() > 255 {
            return Err(error("bounded LLDP adapter port ID exceeds 255 octets").with_target(&at));
        }
        let i = n
            .ethernet
            .directions
            .iter()
            .position(|d| d.from_port == port)
            .ok_or_else(|| error("unknown MAC Merge output").with_target(&at))?;
        if out.ports[i].is_some() {
            return Err(error("duplicate MAC Merge output").with_target(&at));
        }
        if ![100_000_000, 1_000_000_000].contains(&n.ethernet.directions[i].bitrate_bps) {
            return Err(
                error("initial Clause99 adapter supports only 100 Mb/s or 1 Gb/s").with_target(&at),
            );
        }
        let express_mask = mask(&row["express_classes"], &format!("{at}/express_classes"))?;
        let preemptable_mask = mask(
            &row["preemptable_classes"],
            &format!("{at}/preemptable_classes"),
        )?;
        if express_mask & preemptable_mask != 0 || express_mask | preemptable_mask != 255 {
            return Err(error("Merge classes must partition 0..7").with_target(&at));
        }
        let raw = row["source_mac"]
            .as_str()
            .ok_or_else(|| error("source_mac must be configured").with_target(&at))?;
        let bytes = raw
            .split(':')
            .map(|s| {
                if s.len() == 2 {
                    u8::from_str_radix(s, 16).ok()
                } else {
                    None
                }
            })
            .collect::<Option<Vec<_>>>()
            .filter(|b| b.len() == 6 && b[0] & 1 == 0 && b.iter().any(|x| *x != 0))
            .ok_or_else(|| error("source_mac must be a nonzero unicast MAC").with_target(&at))?;
        out.ports[i] = Some(MergePort {
            source_mac: bytes.try_into().unwrap(),
            express_mask,
            preemptable_mask,
            advertised_add_frag_size: integer(&row["advertised_add_frag_size"], 0, 3, &at)? as u8,
            verify_time_ms: integer(&row["verify_time_ms"], 1, 128, &at)? as u8,
            admin_enabled: row["admin_enabled"]
                .as_bool()
                .ok_or_else(|| error("admin_enabled must be boolean").with_target(&at))?,
        });
        out.participating[i] = true;
        out.participating[out.reverse[i]] = true;
    }
    for (i, d) in n.ethernet.directions.iter().enumerate() {
        if out.participating[i]
            && (![100_000_000, 1_000_000_000].contains(&d.bitrate_bps)
                || d.bitrate_bps != n.ethernet.directions[out.reverse[i]].bitrate_bps)
        {
            return Err(error(
                "Clause99 physical pair requires one common supported full-duplex link rate",
            )
            .with_target(p));
        }
    }
    for (i, participating) in out.participating.iter().enumerate() {
        if !participating {
            continue;
        }
        if n.shared_lans.iter().any(|s| s.members.contains(&i)) {
            return Err(
                error("Clause99 requires full-duplex point-to-point physical service")
                    .with_target(p),
            );
        }
        if let Some(a) = n.phy_attachments.iter().find(|a| a.directions.contains(&i)) {
            use crate::runtime::ethernet::phy::medium::PhysicalMode;
            if !a.config.medium.gigabit_raw_merge
                || a.config.medium.physical_mode != PhysicalMode::Ieee1000BaseT1PointToPoint2022
                || a.config.medium.half_duplex
                || a.config.physical_ports.len() != 2
                || !a.directions.contains(&out.reverse[i])
                || n.ethernet.directions[i].bitrate_bps != 1_000_000_000
            {
                return Err(error("Clause99 sampled output requires the explicit continuous 1000BASE-T1 raw xMII adapter").with_target(p));
            }
            if v.get("packet_faults")
                .and_then(Value::as_array)
                .is_some_and(|f| !f.is_empty())
            {
                return Err(
                    error("sampled Clause99 faults must be physical channel faults")
                        .with_target(format!("{p}/packet_faults")),
                );
            }
        }
    }
    if let Some(faults) = v.get("packet_faults") {
        for (j, row) in array(faults, &format!("{p}/packet_faults"))?
            .iter()
            .enumerate()
        {
            let at = format!("{p}/packet_faults/{j}");
            exact(row, &["port", "packet_ordinal", "action"], "Merge fault")
                .map_err(|d| d.with_target(&at))?;
            let i = n
                .ethernet
                .directions
                .iter()
                .position(|d| row["port"] == d.from_port)
                .filter(|i| out.participating[*i])
                .ok_or_else(|| {
                    error("fault port is not Merge physical direction").with_target(&at)
                })?;
            let action = serde_json::from_value(row["action"].clone())
                .map_err(|e| error(format!("invalid Merge fault: {e}")).with_target(&at))?;
            out.faults.push(MergeFault {
                direction: i,
                packet_ordinal: integer(&row["packet_ordinal"], 0, u64::MAX, &at)?,
                action,
            });
        }
    }
    Ok(out)
}
