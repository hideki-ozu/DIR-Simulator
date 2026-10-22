//! Strict prepared MAC Control policy. No peer timer is mutated by a producer.
use super::*;
use crate::types::network::mac_control::*;
pub(super) fn prepare(v: &Value, network: &PreparedNetwork) -> Result<PreparedMacControl> {
    exact(v, &["ports", "controls"], "MAC Control").map_err(|d| d.with_target("/mac_control"))?;
    let mut ports = vec![None; network.ethernet.directions.len()];
    for (i, row) in array(&v["ports"], "/mac_control/ports")?.iter().enumerate() {
        let ptr = format!("/mac_control/ports/{i}");
        exact(
            row,
            &["port", "station_mac", "rx_enabled", "tx_enabled", "policy"],
            "PAUSE port",
        )
        .map_err(|d| d.with_target(&ptr))?;
        let port = string(&row["port"], &format!("{ptr}/port"))?;
        let direction = network
            .ethernet
            .directions
            .iter()
            .position(|d| d.from_port == port)
            .ok_or_else(|| {
                error("PAUSE port must name an output MAC").with_target(format!("{ptr}/port"))
            })?;
        if ports[direction].is_some() {
            return Err(error("duplicate PAUSE MAC").with_target(ptr));
        }
        let mac = row["station_mac"].as_str().ok_or_else(|| {
            error("expected MAC address").with_target(format!("{ptr}/station_mac"))
        })?;
        let bytes = mac
            .split(':')
            .map(|s| {
                if s.len() == 2 {
                    u8::from_str_radix(s, 16).ok()
                } else {
                    None
                }
            })
            .collect::<Option<Vec<_>>>();
        let station_mac: [u8; 6] = bytes
            .and_then(|b| b.try_into().ok())
            .filter(|b: &[u8; 6]| b[0] & 1 == 0 && *b != [0; 6])
            .ok_or_else(|| {
                error("station_mac must be an individual nonzero 48-bit address")
                    .with_target(format!("{ptr}/station_mac"))
            })?;
        let boolean = |key: &str| {
            row[key]
                .as_bool()
                .ok_or_else(|| error("expected boolean").with_target(format!("{ptr}/{key}")))
        };
        let (experimental_merge, experimental_project_generation) = match row["policy"].as_str() {
            Some("ieee802.3.pause.v1") => (false, false),
            Some("dir.experimental.merge_pause.v1") => (true, false),
            Some("dir.experimental.project_preemption_pause.v1") => (true, true),
            _ => {
                return Err(
                    error("unknown MAC Control capability/policy; PFC is not implemented")
                        .with_target(format!("{ptr}/policy")),
                );
            }
        };
        ports[direction] = Some(PausePort {
            station_mac,
            rx_enabled: boolean("rx_enabled")?,
            tx_enabled: boolean("tx_enabled")?,
            experimental_merge,
            experimental_project_generation,
        });
    }
    let mut controls = Vec::new();
    for (i, row) in array(&v["controls"], "/mac_control/controls")?
        .iter()
        .enumerate()
    {
        let ptr = format!("/mac_control/controls/{i}");
        let kind = row["kind"].as_str().unwrap_or("");
        let field = if kind == "pause_request" {
            "quanta"
        } else {
            "enabled"
        };
        exact(
            row,
            &["at_ps", "port", "kind", field],
            "MAC Control administration",
        )
        .map_err(|d| d.with_target(&ptr))?;
        let direction = network
            .ethernet
            .directions
            .iter()
            .position(|d| row["port"] == d.from_port)
            .filter(|&d| ports[d].is_some())
            .ok_or_else(|| {
                error("administration requires a configured MAC Control port")
                    .with_target(format!("{ptr}/port"))
            })?;
        let action = match kind {
            "pause_request" => {
                Action::PauseRequest(
                    integer(&row["quanta"], 0, 65535, &format!("{ptr}/quanta"))? as u16
                )
            }
            "rx_enable" | "tx_enable" | "transmit_enable" => {
                let enabled = row["enabled"].as_bool().ok_or_else(|| {
                    error("expected boolean").with_target(format!("{ptr}/enabled"))
                })?;
                match kind {
                    "rx_enable" => Action::RxEnable(enabled),
                    "tx_enable" => Action::TxEnable(enabled),
                    _ => Action::TransmitEnable(enabled),
                }
            }
            _ => {
                return Err(error("unknown MAC Control administration kind")
                    .with_target(format!("{ptr}/kind")));
            }
        };
        controls.push(Control {
            direction,
            at_ps: decimal(&row["at_ps"], &format!("{ptr}/at_ps"))?,
            action,
        });
    }
    controls.sort_by_key(|c| c.at_ps); // stable: configuration order is receive-independent administration order
    Ok(PreparedMacControl { ports, controls })
}
/// Reuse the existing control FIFO even when no bridge protocol was selected.
pub(super) fn ensure_fifo(network: &mut PreparedNetwork) {
    network
        .wire
        .get_or_insert_with(|| crate::types::wire::PreparedWire {
            adapters: vec![],
            ports: BTreeMap::new(),
            packets: vec![],
            svre_exists: BTreeSet::new(),
            registrar_modes: BTreeMap::new(),
        });
}
