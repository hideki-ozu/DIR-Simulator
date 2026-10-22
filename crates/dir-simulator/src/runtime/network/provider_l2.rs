//! Provider/customer L2 decisions for the composite network owner.
//! This module does not own physical queues, time, or protocol state.
use crate::runtime::ethernet::retag_wire_frame;
use crate::types::Diagnostic;
use crate::types::ethernet::{EthernetTag, EthernetVlanTag, EthernetWireFrame};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

type Result<T> = std::result::Result<T, Diagnostic>;
fn invalid(message: &str) -> Diagnostic {
    Diagnostic::prepare(message)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PortRole {
    CustomerEdge,    // C-component CEP
    ProviderEdge,    // C-component PEP, internal service handoff
    CustomerNetwork, // S-component CNP
    ProviderNetwork, // S-component PNP
}
impl PortRole {
    pub fn is_customer(self) -> bool {
        matches!(self, Self::CustomerEdge | Self::ProviderEdge)
    }
    pub fn is_provider(self) -> bool {
        !self.is_customer()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ComponentPort {
    pub port: String,
    pub component: String,
    pub role: PortRole,
    pub pvid: u16,
    pub member_vids: BTreeSet<u16>,
    pub tagged_egress: bool,
    pub default_priority: u8,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ServiceMapping {
    pub c_component: String,
    pub pep: String,
    pub c_vid: u16,
    pub s_component: String,
    pub cnp: String,
    pub s_vid: u16,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProviderTopology {
    pub ports: BTreeMap<String, ComponentPort>,
    pub mappings: Vec<ServiceMapping>,
}

#[derive(Debug, Clone)]
pub struct ClassifiedFrame {
    pub component: String,
    pub ingress_port: String,
    pub vid: u16,
    pub priority: u8,
    pub dei: u8,
    pub wire: EthernetWireFrame,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ControlDisposition {
    Consume,
    Drop,
    Forward,
    Ordinary,
}

/// Table 8-1/8-2 component-specific reserved group-address handling. A
/// component consumes only its own BPDU address; other reserved addresses
/// are filtered and must not be handed to that component's protocol.
pub fn control_disposition(role: PortRole, dst_mac: &str) -> ControlDisposition {
    let name = dst_mac.to_ascii_lowercase();
    let Some(last) = name
        .strip_prefix("01:80:c2:00:00:")
        .and_then(|s| u8::from_str_radix(s, 16).ok())
    else {
        return ControlDisposition::Ordinary;
    };
    if last > 15 {
        return ControlDisposition::Ordinary;
    }
    if role.is_provider() {
        match last {
            0 => ControlDisposition::Forward,
            8 => ControlDisposition::Consume,
            1..=10 | 14 => ControlDisposition::Drop,
            _ => ControlDisposition::Forward,
        }
    } else {
        match last {
            0 => ControlDisposition::Consume,
            _ => ControlDisposition::Drop,
        }
    }
}

impl ProviderTopology {
    pub fn validate(&self) -> Result<()> {
        for (name, port) in &self.ports {
            if name != &port.port
                || name.is_empty()
                || port.component.is_empty()
                || !(1..=4094).contains(&port.pvid)
                || port.default_priority > 7
                || !port.member_vids.contains(&port.pvid)
                || port.member_vids.iter().any(|v| !(1..=4094).contains(v))
            {
                return Err(invalid("invalid provider component port"));
            }
        }
        let mut c_keys = BTreeSet::new();
        let mut pep_services = BTreeMap::new();
        let mut cnp_peers = BTreeMap::new();
        for map in &self.mappings {
            let pep = self
                .ports
                .get(&map.pep)
                .ok_or_else(|| invalid("missing PEP"))?;
            let cnp = self
                .ports
                .get(&map.cnp)
                .ok_or_else(|| invalid("missing CNP"))?;
            if pep.role != PortRole::ProviderEdge
                || cnp.role != PortRole::CustomerNetwork
                || pep.component != map.c_component
                || cnp.component != map.s_component
                || !pep.member_vids.contains(&map.c_vid)
                || !cnp.member_vids.contains(&map.s_vid)
                || !c_keys.insert((map.c_component.clone(), map.c_vid))
            {
                return Err(invalid("invalid or ambiguous service mapping"));
            }
            let service = (map.s_component.clone(), map.cnp.clone(), map.s_vid);
            if pep_services
                .insert(map.pep.clone(), service.clone())
                .is_some_and(|old| old != service)
                || cnp_peers
                    .insert(map.cnp.clone(), map.pep.clone())
                    .is_some_and(|old| old != map.pep)
            {
                return Err(invalid(
                    "PEP/CNP internal attachment is not one service peer",
                ));
            }
        }
        Ok(())
    }

    /// Ingress authorization occurs before learning or protocol handling.
    /// VID 0 retains PCP/DEI but resolves to the port PVID; VID 4095 is invalid.
    pub fn classify_ingress(
        &self,
        port_name: &str,
        wire: &EthernetWireFrame,
    ) -> Result<ClassifiedFrame> {
        let port = self
            .ports
            .get(port_name)
            .ok_or_else(|| invalid("unknown component port"))?;
        let tags = wire.canonical_tags().map_err(invalid)?;
        let outer = tags.first().copied();
        let selected = match port.role {
            PortRole::CustomerEdge | PortRole::ProviderEdge => {
                if outer.is_some_and(|t| t.tpid != EthernetTag::C_TPID) || tags.len() > 1 {
                    return Err(invalid("S-tag on customer component ingress"));
                }
                outer
            }
            PortRole::CustomerNetwork => {
                // A C-tag on a port-based customer service is encapsulated
                // data, not the S-component's service selector.
                outer.filter(|t| t.tpid == EthernetTag::S_TPID)
            }
            PortRole::ProviderNetwork => {
                // Provider BPDUs are link-local control frames and may be
                // untagged; their address is interpreted before data VLANs.
                if outer.is_none()
                    && control_disposition(port.role, &wire.dst_mac) == ControlDisposition::Consume
                {
                    None
                } else if outer.is_none_or(|t| t.tpid != EthernetTag::S_TPID) {
                    return Err(invalid("provider network ingress requires S-tag"));
                } else {
                    outer
                }
            }
        };
        let vid = selected.map_or(port.pvid, |t| {
            if t.tci.vid == 0 { port.pvid } else { t.tci.vid }
        });
        if !port.member_vids.contains(&vid) {
            return Err(invalid("VLAN not admitted by component port"));
        }
        Ok(ClassifiedFrame {
            component: port.component.clone(),
            ingress_port: port.port.clone(),
            vid,
            priority: selected.map_or(port.default_priority, |t| t.tci.pcp),
            dei: selected.map_or(0, |t| t.tci.dei),
            wire: wire.clone(),
        })
    }

    /// Return an egress frame for an ordinary physical port. The S-component
    /// changes only its outer S-tag; any customer C-tag stays byte-identical.
    pub fn encode_egress(
        &self,
        port_name: &str,
        class: &ClassifiedFrame,
    ) -> Result<EthernetWireFrame> {
        let port = self
            .ports
            .get(port_name)
            .ok_or_else(|| invalid("unknown component port"))?;
        if port.component != class.component || !port.member_vids.contains(&class.vid) {
            return Err(invalid("egress component/VLAN mismatch"));
        }
        let old = class.wire.canonical_tags().map_err(invalid)?;
        let inner = if port.role.is_provider() {
            if old.first().is_some_and(|t| t.tpid == EthernetTag::S_TPID) {
                old[1..].to_vec()
            } else if old.first().is_some_and(|t| t.tpid == EthernetTag::C_TPID) {
                old.clone()
            } else {
                Vec::new()
            }
        } else {
            Vec::new()
        };
        let mut tags = Vec::new();
        if port.tagged_egress || port.role == PortRole::ProviderNetwork {
            tags.push(EthernetTag {
                tpid: if port.role.is_provider() {
                    EthernetTag::S_TPID
                } else {
                    EthernetTag::C_TPID
                },
                tci: EthernetVlanTag {
                    vid: class.vid,
                    pcp: class.priority,
                    dei: class.dei,
                },
            });
        }
        tags.extend(inner);
        retag_wire_frame(&class.wire, &tags)
    }

    /// Internal C↔S service transition. Caller posts this as a logical owner
    /// event, then routes the returned frame through ordinary physical egress.
    pub fn handoff_all(
        &self,
        from_port: &str,
        class: &ClassifiedFrame,
    ) -> Result<Vec<ClassifiedFrame>> {
        let source = self
            .ports
            .get(from_port)
            .ok_or_else(|| invalid("unknown handoff port"))?;
        if source.component != class.component
            || source.role != PortRole::ProviderEdge && source.role != PortRole::CustomerNetwork
        {
            return Err(invalid("invalid handoff source"));
        }
        let mappings: Vec<_> = if source.role == PortRole::ProviderEdge {
            self.mappings
                .iter()
                .filter(|m| m.pep == from_port && m.c_vid == class.vid)
                .collect()
        } else {
            self.mappings
                .iter()
                .filter(|m| m.cnp == from_port && m.s_vid == class.vid)
                .collect()
        };
        if mappings.is_empty() {
            return Err(invalid("unmapped service handoff"));
        }
        let mut results = Vec::with_capacity(mappings.len());
        for map in mappings {
            let (target, vid, tags) = if source.role == PortRole::ProviderEdge {
                let old = class.wire.canonical_tags().map_err(invalid)?;
                // The internal ISS carries customer MAC data without an S-tag.
                // The S-component selects its service from the CNP PVID and
                // inserts the S-tag only on a physical PNP egress.
                (map.cnp.as_str(), map.s_vid, old)
            } else {
                let old = class.wire.canonical_tags().map_err(invalid)?;
                let inner = if old.first().is_some_and(|t| t.tpid == EthernetTag::S_TPID) {
                    old[1..].to_vec()
                } else {
                    old
                };
                (map.pep.as_str(), map.c_vid, inner)
            };
            let wire = retag_wire_frame(&class.wire, &tags)?;
            let result = self.classify_ingress(target, &wire)?;
            // One S service may reach several C-components. A tagged customer
            // frame is delivered only to PEPs for its C-VID.
            if result.vid == vid {
                results.push(result);
            }
        }
        Ok(results)
    }

    #[cfg(test)]
    pub fn handoff(&self, from_port: &str, class: &ClassifiedFrame) -> Result<ClassifiedFrame> {
        let mut results = self.handoff_all(from_port, class)?;
        if results.len() != 1 {
            return Err(invalid("service handoff has multiple or no recipients"));
        }
        Ok(results.remove(0))
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ComponentFdb {
    /// MAC lookup is isolated by component and VID/FID.
    #[serde(with = "super::checkpoint::map")]
    entries: BTreeMap<(String, u16, String), FdbEntry>,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FdbEntry {
    pub port: String,
    pub learned_ps: u64,
    pub expires_ps: u64,
}
pub const FDB_MAX_ENTRIES: usize = 4096;
pub const FDB_AGE_PS: u64 = 300_000_000_000_000;
impl ComponentFdb {
    pub fn entries(&self) -> &BTreeMap<(String, u16, String), FdbEntry> {
        &self.entries
    }
    #[cfg(test)]
    pub fn learn(&mut self, component: &str, vid: u16, mac: &str, port: &str) {
        self.learn_at(component, vid, mac, port, 0);
    }
    #[cfg(test)]
    pub fn learn_at(&mut self, component: &str, vid: u16, mac: &str, port: &str, now: u64) {
        self.learn_with_limits(component, vid, mac, port, now, FDB_MAX_ENTRIES, FDB_AGE_PS);
    }
    #[allow(clippy::too_many_arguments)]
    pub fn learn_with_limits(
        &mut self,
        component: &str,
        vid: u16,
        mac: &str,
        port: &str,
        now: u64,
        capacity: usize,
        age_ps: u64,
    ) {
        self.entries.retain(|_, entry| entry.expires_ps > now);
        let key = (component.to_owned(), vid, mac.to_ascii_lowercase());
        if !self.entries.contains_key(&key) && self.entries.len() >= capacity {
            if let Some(oldest) = self
                .entries
                .iter()
                .min_by_key(|(key, entry)| (entry.learned_ps, *key))
                .map(|(key, _)| key.clone())
            {
                self.entries.remove(&oldest);
            }
        }
        self.entries.insert(
            key,
            FdbEntry {
                port: port.to_owned(),
                learned_ps: now,
                expires_ps: now.saturating_add(age_ps),
            },
        );
    }
    #[cfg(test)]
    pub fn lookup(&self, component: &str, vid: u16, mac: &str) -> Option<&str> {
        self.entries
            .get(&(component.to_owned(), vid, mac.to_ascii_lowercase()))
            .map(|entry| entry.port.as_str())
    }
    pub fn lookup_at(&self, component: &str, vid: u16, mac: &str, now: u64) -> Option<&str> {
        self.entries
            .get(&(component.to_owned(), vid, mac.to_ascii_lowercase()))
            .filter(|entry| entry.expires_ps > now)
            .map(|entry| entry.port.as_str())
    }
    pub fn flush_port(
        &mut self,
        component: &str,
        port: &str,
        vids: &BTreeSet<u16>,
    ) -> Vec<(String, u16, String)> {
        let keys: Vec<_> = self
            .entries
            .iter()
            .filter(|((c, vid, _), entry)| {
                c == component && entry.port == port && vids.contains(vid)
            })
            .map(|(key, _)| key.clone())
            .collect();
        for key in &keys {
            self.entries.remove(key);
        }
        keys
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runtime::ethernet::{l2, serialize_tag_stack_frame, serialize_vlan_frame};

    fn c(vid: u16) -> EthernetTag {
        EthernetTag {
            tpid: EthernetTag::C_TPID,
            tci: EthernetVlanTag {
                vid,
                pcp: 3,
                dei: 0,
            },
        }
    }
    fn s(vid: u16) -> EthernetTag {
        EthernetTag {
            tpid: EthernetTag::S_TPID,
            tci: EthernetVlanTag {
                vid,
                pcp: 5,
                dei: 1,
            },
        }
    }
    fn wire() -> EthernetWireFrame {
        serialize_vlan_frame(
            "02:00:00:00:00:01",
            "02:00:00:00:00:02",
            0x0800,
            "01020304",
            Some(c(10).tci),
        )
        .unwrap()
    }
    fn port(name: &str, component: &str, role: PortRole, vid: u16, tagged: bool) -> ComponentPort {
        ComponentPort {
            port: name.into(),
            component: component.into(),
            role,
            pvid: vid,
            member_vids: [vid].into(),
            tagged_egress: tagged,
            default_priority: 0,
        }
    }
    fn topology() -> ProviderTopology {
        let ports = [
            port("c.cep", "c", PortRole::CustomerEdge, 10, true),
            port("c.pep", "c", PortRole::ProviderEdge, 10, true),
            port("s.cnp", "s", PortRole::CustomerNetwork, 100, false),
            port("s.pnp", "s", PortRole::ProviderNetwork, 100, true),
        ]
        .into_iter()
        .map(|p| (p.port.clone(), p))
        .collect();
        ProviderTopology {
            ports,
            mappings: vec![ServiceMapping {
                c_component: "c".into(),
                pep: "c.pep".into(),
                c_vid: 10,
                s_component: "s".into(),
                cnp: "s.cnp".into(),
                s_vid: 100,
            }],
        }
    }

    #[test]
    fn literal_stack_fcs_padding_duration_and_legacy_json() {
        let data = (0..46).fold(String::with_capacity(92), |mut data, n| {
            use std::fmt::Write as _;
            write!(&mut data, "{n:02x}").unwrap();
            data
        });
        let stacked = serialize_tag_stack_frame(
            "02:00:00:00:00:01",
            "02:00:00:00:00:02",
            0x0800,
            &data,
            &[s(100), c(10)],
        )
        .unwrap();
        assert_eq!(
            stacked.mac_hex,
            "02000000000202000000000188a8b0648100600a0800000102030405060708090a0b0c0d0e0f101112131415161718191a1b1c1d1e1f202122232425262728292a2b2c2d0ad92a3a"
        );
        assert_eq!(
            (
                stacked.pad_bytes,
                stacked.mac_bytes,
                stacked.fcs_hex.as_str()
            ),
            (0, 72, "0ad92a3a")
        );
        assert_eq!(stacked.tag, None);
        assert_eq!(stacked.canonical_tags().unwrap(), vec![s(100), c(10)]);
        let raw = (0..stacked.mac_hex.len())
            .step_by(2)
            .map(|i| u8::from_str_radix(&stacked.mac_hex[i..i + 2], 16).unwrap())
            .collect::<Vec<_>>();
        let phy = crate::runtime::ethernet::phy::attachment::decode_mac(&raw, 2048).unwrap();
        assert_eq!(phy.wire.canonical_tags().unwrap(), vec![s(100), c(10)]);
        assert_eq!(
            crate::runtime::ethernet::phy::attachment::canonical_bytes(&stacked, 2048).unwrap(),
            raw
        );
        let single = serialize_vlan_frame(
            "02:00:00:00:00:01",
            "02:00:00:00:00:02",
            0x0800,
            &data,
            Some(c(10).tci),
        )
        .unwrap();
        assert_eq!(single.mac_bytes, 68);
        assert_eq!(serde_json::to_value(&single).unwrap().get("tags"), None);
        let old: EthernetWireFrame =
            serde_json::from_value(serde_json::to_value(&single).unwrap()).unwrap();
        assert_eq!(old.mac_hex, single.mac_hex);
        let short = serialize_tag_stack_frame(
            "02:00:00:00:00:01",
            "02:00:00:00:00:02",
            0x0800,
            "01020304",
            &[s(100), c(10)],
        )
        .unwrap();
        assert_eq!(
            (short.pad_bytes, short.mac_bytes, short.fcs_hex.as_str()),
            (34, 64, "54c7b127")
        );
        assert_eq!(
            l2::timing(0, stacked.mac_bytes, 1_000_000_000, 0)
                .unwrap()
                .0,
            l2::timing(0, single.mac_bytes, 1_000_000_000, 0).unwrap().0 + 32_000
        );
    }

    #[test]
    fn service_handoff_outer_rewrite_and_isolated_learning() {
        let topo = topology();
        topo.validate().unwrap();
        let first = topo.classify_ingress("c.cep", &wire()).unwrap();
        let provider = topo.handoff("c.pep", &first).unwrap();
        assert_eq!((provider.component.as_str(), provider.vid), ("s", 100));
        // Q2022 15.4: no S-tag on the internal PEP/CNP interface.
        assert_eq!(provider.wire.canonical_tags().unwrap(), vec![c(10)]);
        let encoded = topo.encode_egress("s.pnp", &provider).unwrap();
        assert_eq!(
            encoded.canonical_tags().unwrap()[0].tpid,
            EthernetTag::S_TPID
        );
        assert_eq!(&encoded.mac_hex[32..40], &wire().mac_hex[24..32]); // inner C-tag unchanged
        let reverse = topo.handoff("s.cnp", &provider).unwrap();
        assert_eq!((reverse.component.as_str(), reverse.vid), ("c", 10));
        assert_eq!(reverse.wire.canonical_tags().unwrap(), vec![c(10)]);
        assert_eq!(reverse.wire.mac_hex, wire().mac_hex);
        let rewritten =
            crate::runtime::ethernet::retag_wire_frame(&encoded, &[s(200), c(10)]).unwrap();
        assert_eq!(
            &rewritten.mac_hex[32..rewritten.mac_hex.len() - 8],
            &encoded.mac_hex[32..encoded.mac_hex.len() - 8]
        );
        let mut fdb = ComponentFdb::default();
        fdb.learn("s", 100, "02:00:00:00:00:01", "s.pnp");
        fdb.learn("s", 200, "02:00:00:00:00:01", "other.pnp");
        fdb.learn("c", 10, "02:00:00:00:00:01", "c.cep");
        assert_eq!(fdb.lookup("s", 100, "02:00:00:00:00:01"), Some("s.pnp"));
        assert_eq!(fdb.lookup("s", 200, "02:00:00:00:00:01"), Some("other.pnp"));
        assert_eq!(fdb.lookup("c", 10, "02:00:00:00:00:01"), Some("c.cep"));
    }

    #[test]
    fn membership_priority_and_role_control() {
        let topo = topology();
        let priority = serialize_tag_stack_frame(
            "02:00:00:00:00:01",
            "02:00:00:00:00:02",
            0x0800,
            "",
            &[s(0)],
        )
        .unwrap();
        let class = topo.classify_ingress("s.cnp", &priority).unwrap();
        assert_eq!((class.vid, class.priority, class.dei), (100, 5, 1));
        let spoof = serialize_tag_stack_frame(
            "02:00:00:00:00:01",
            "02:00:00:00:00:02",
            0x0800,
            "",
            &[s(200)],
        )
        .unwrap();
        assert!(topo.classify_ingress("s.cnp", &spoof).is_err());
        assert!(topo.classify_ingress("c.cep", &spoof).is_err());
        let port_based = topo.classify_ingress("s.cnp", &wire()).unwrap();
        assert_eq!(port_based.vid, 100);
        let encapsulated = topo.encode_egress("s.pnp", &port_based).unwrap();
        assert_eq!(encapsulated.canonical_tags().unwrap()[1], c(10));
        for role in [PortRole::CustomerNetwork, PortRole::ProviderNetwork] {
            assert_eq!(
                control_disposition(role, "01:80:c2:00:00:00"),
                ControlDisposition::Forward
            );
            assert_eq!(
                control_disposition(role, "01:80:c2:00:00:08"),
                ControlDisposition::Consume
            );
        }
        for role in [PortRole::CustomerEdge, PortRole::ProviderEdge] {
            assert_eq!(
                control_disposition(role, "01:80:c2:00:00:00"),
                ControlDisposition::Consume
            );
            assert_eq!(
                control_disposition(role, "01:80:c2:00:00:08"),
                ControlDisposition::Drop
            );
        }
        let mut provider_bpdu = crate::runtime::ethernet::serialize_frame(
            "02:00:00:00:00:01",
            "02:00:00:00:00:02",
            0x9000,
            "01020304",
        )
        .unwrap();
        let mut bytes = (0..provider_bpdu.mac_hex.len())
            .step_by(2)
            .map(|i| u8::from_str_radix(&provider_bpdu.mac_hex[i..i + 2], 16).unwrap())
            .collect::<Vec<_>>();
        bytes[..6].copy_from_slice(&[1, 128, 194, 0, 0, 8]);
        let fcs = crate::runtime::ethernet::crc32(&bytes[..bytes.len() - 4]).to_le_bytes();
        let end = bytes.len();
        bytes[end - 4..].copy_from_slice(&fcs);
        provider_bpdu = crate::runtime::ethernet::phy::attachment::decode_mac(&bytes, 2048)
            .unwrap()
            .wire;
        assert_eq!(
            topo.classify_ingress("s.pnp", &provider_bpdu).unwrap().vid,
            100
        );
        assert_eq!(
            control_disposition(PortRole::ProviderNetwork, &provider_bpdu.dst_mac),
            ControlDisposition::Consume
        );
        let bad = EthernetWireFrame {
            tags: vec![s(100)],
            ..wire()
        };
        assert!(bad.canonical_tags().is_err());
    }

    #[test]
    fn one_provider_service_can_handoff_to_more_than_two_customer_interfaces() {
        let mut topo = topology();
        for (name, component) in [("c2.pep", "c2"), ("c3.pep", "c3")] {
            topo.ports.insert(
                name.into(),
                port(name, component, PortRole::ProviderEdge, 10, true),
            );
            let cnp = format!("s.cnp_{component}");
            topo.ports.insert(
                cnp.clone(),
                port(&cnp, "s", PortRole::CustomerNetwork, 100, false),
            );
            topo.mappings.push(ServiceMapping {
                c_component: component.into(),
                pep: name.into(),
                c_vid: 10,
                s_component: "s".into(),
                cnp,
                s_vid: 100,
            });
        }
        topo.validate().unwrap();
        let class = topo
            .classify_ingress(
                "s.pnp",
                &serialize_tag_stack_frame(
                    "02:00:00:00:00:01",
                    "02:00:00:00:00:02",
                    0x0800,
                    "01020304",
                    &[s(100), c(10)],
                )
                .unwrap(),
            )
            .unwrap();
        // Each internal ISS has exactly one peer; the S-component bridges
        // the service to its separate CNPs before each independent handoff.
        let delivered: Vec<_> = ["s.cnp", "s.cnp_c2", "s.cnp_c3"]
            .into_iter()
            .flat_map(|cnp| topo.handoff_all(cnp, &class).unwrap())
            .collect();
        assert_eq!(delivered.len(), 3);
        assert_eq!(
            delivered
                .iter()
                .map(|x| x.component.as_str())
                .collect::<BTreeSet<_>>(),
            ["c", "c2", "c3"].into()
        );
    }

    #[test]
    fn fdb_is_finite_and_entries_age_on_physical_time() {
        let mut fdb = ComponentFdb::default();
        fdb.learn_at("c", 10, "02:00:00:00:00:01", "c.cep", 7);
        assert_eq!(
            fdb.lookup_at("c", 10, "02:00:00:00:00:01", 7 + FDB_AGE_PS - 1),
            Some("c.cep")
        );
        assert_eq!(
            fdb.lookup_at("c", 10, "02:00:00:00:00:01", 7 + FDB_AGE_PS),
            None
        );
        for i in 0..=FDB_MAX_ENTRIES {
            fdb.learn_at(
                "c",
                10,
                &format!(
                    "02:00:{:02x}:{:02x}:{:02x}:{:02x}",
                    (i >> 24) & 255,
                    (i >> 16) & 255,
                    (i >> 8) & 255,
                    i & 255
                ),
                "c.cep",
                i as u64,
            );
        }
        assert_eq!(fdb.entries().len(), FDB_MAX_ENTRIES);
        assert_eq!(
            fdb.lookup_at("c", 10, "02:00:00:00:00:00", FDB_MAX_ENTRIES as u64),
            None
        );
    }

    #[test]
    fn topology_change_flushes_only_its_component_port_and_vid() {
        let mut fdb = ComponentFdb::default();
        for (component, vid, mac, port) in [
            ("c", 10, "02:00:00:00:00:01", "pep"),
            ("c", 20, "02:00:00:00:00:01", "pep"),
            ("c", 10, "02:00:00:00:00:02", "cep"),
            ("other-c", 10, "02:00:00:00:00:01", "other-pep"),
        ] {
            fdb.learn_at(component, vid, mac, port, 7);
        }
        assert_eq!(
            fdb.flush_port("c", "pep", &[10].into()),
            vec![("c".into(), 10, "02:00:00:00:00:01".into())]
        );
        assert_eq!(fdb.entries().len(), 3);
        assert_eq!(fdb.lookup("c", 20, "02:00:00:00:00:01"), Some("pep"));
        assert_eq!(fdb.lookup("c", 10, "02:00:00:00:00:02"), Some("cep"));
        assert_eq!(
            fdb.lookup("other-c", 10, "02:00:00:00:00:01"),
            Some("other-pep")
        );
    }
}
