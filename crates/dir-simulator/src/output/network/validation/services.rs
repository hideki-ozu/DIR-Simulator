//! Service evidence is scoped to a declared MAC/physical owner.
use super::*;
use crate::types::network::PreparedNetwork;
mod gptp;

fn request(value: &Value, at: u64) -> Result<u64, Diagnostic> {
    exact(value, "quanta received_ps evidence")?;
    let q = number(&value["quanta"])?;
    if q > u16::MAX.into()
        || number(&value["received_ps"])? > at
        || value["evidence"].as_str().is_none_or(str::is_empty)
    {
        return Err(fail("invalid PAUSE receive evidence"));
    }
    Ok(q)
}

pub(super) fn validate(n: &PreparedNetwork, snapshot: &Snapshot) -> Result<(), Diagnostic> {
    for record in snapshot.registered.as_ref().unwrap().model_records.values() {
        let data = &record.data;
        match record.schema.name.as_str() {
            "dir.network.gptp" => gptp::validate(n, snapshot, record)?,
            "dir.network.mac_control" => {
                exact(data, "port kind state evidence effect_seq")?;
                number(&data["effect_seq"])?;
                let port = data["port"]
                    .as_str()
                    .ok_or_else(|| fail("missing PAUSE port"))?;
                let direction = n
                    .ethernet
                    .directions
                    .iter()
                    .position(|d| d.from_port == port)
                    .ok_or_else(|| fail("PAUSE port is not an output"))?;
                if n.mac_control
                    .as_ref()
                    .and_then(|p| p.ports[direction].as_ref())
                    .is_none()
                    || data["kind"].as_str().is_none_or(str::is_empty)
                    || !data["evidence"].is_object()
                {
                    return Err(fail("PAUSE record has no configured owner"));
                }
                let state = &data["state"];
                exact(
                    state,
                    "rx_enabled tx_enabled transmit_enabled link_up generation epoch ordinal pending timer",
                )?;
                for key in ["rx_enabled", "tx_enabled", "transmit_enabled", "link_up"] {
                    if !state[key].is_boolean() {
                        return Err(fail("invalid PAUSE enable state"));
                    }
                }
                let generation = number(&state["generation"])?;
                number(&state["epoch"])?;
                number(&state["ordinal"])?;
                if !state["pending"].is_null() {
                    request(&state["pending"], record.time_ps)?;
                }
                let timer = &state["timer"];
                if !timer.is_null() {
                    exact(timer, "generation loaded_ps expires_ps request")?;
                    let q = request(&timer["request"], record.time_ps)?;
                    let loaded = number(&timer["loaded_ps"])?;
                    let expires = number(&timer["expires_ps"])?;
                    let numerical = n.phy_attachments.iter().find(|a| {
                        a.directions.contains(&direction)
                            && a.config.medium.physical_mode
                                == crate::runtime::ethernet::phy::medium::PhysicalMode::ProjectNumerical
                    });
                    let (rate, denominator) = numerical
                        .map_or((n.ethernet.directions[direction].bitrate_bps, 1u64), |a| {
                            (a.config.medium.channel.grid.symbol_rate_hz, 2u64)
                        });
                    let duration =
                        (u128::from(q) * 512 * u128::from(denominator) * 1_000_000_000_000)
                            .div_ceil(u128::from(rate));
                    if q == 0
                        || loaded > record.time_ps
                        || number(&timer["request"]["received_ps"])? > loaded
                        || number(&timer["generation"])? > generation
                        || u128::from(loaded) + duration != u128::from(expires)
                    {
                        return Err(fail("PAUSE timer differs from its received request"));
                    }
                }
            }
            "dir.network.configured_media" => {
                let fields = if data.get("attempt_index").is_some() {
                    number(&data["attempt_index"])?;
                    "kind work_id attempt_index attempt physical_link effect_seq"
                } else {
                    "kind work_id attempt physical_link effect_seq"
                };
                exact(data, fields)?;
                number(&data["effect_seq"])?;
                let link = n
                    .ethernet
                    .media
                    .as_ref()
                    .and_then(|m| {
                        m.physical_links
                            .iter()
                            .find(|l| Some(l.id.as_str()) == data["physical_link"].as_str())
                    })
                    .ok_or_else(|| fail("configured-media record has no physical owner"))?;
                let a = &data["attempt"];
                exact(
                    a,
                    "owner direction number sof eof release arrival mdi_sof peer_sof collision late_collision jam_end slots backoff_until status",
                )?;
                let direction = usize::try_from(number(&a["direction"])?)
                    .map_err(|_| fail("media direction overflow"))?;
                if !link.directions.contains(&direction)
                    || number(&a["sof"])? > record.time_ps
                    || !a["late_collision"].is_boolean()
                    || data["kind"].as_str().is_none_or(str::is_empty)
                    || data["work_id"].as_str().is_none_or(str::is_empty)
                {
                    return Err(fail(
                        "configured-media attempt crossed its owner or observation",
                    ));
                }
                for key in ["number", "eof", "release", "arrival", "mdi_sof", "peer_sof"] {
                    number(&a[key])?;
                }
                for key in ["collision", "jam_end", "slots", "backoff_until"] {
                    if !a[key].is_null() {
                        number(&a[key])?;
                    }
                }
            }
            _ => {}
        }
    }
    Ok(())
}
