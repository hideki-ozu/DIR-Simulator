//! Membership audit construction from immutable before/after snapshots.
use super::*;
use std::net::IpAddr;
fn key(k: &membership::Key, router: bool) -> Value {
    json!({"switch":k.bridge,"vid":k.vid,"family":if k.group.is_ipv6(){"ipv6"}else{"ipv4"},"group":if router{None}else{Some(k.group.to_string())},"port":k.port})
}
fn changes(
    before: &membership::Snooper,
    after: &membership::Snooper,
    bridge: &str,
    port: &str,
    vid: u16,
    v6: bool,
) -> Vec<Value> {
    let mut out = vec![];
    let keys: BTreeSet<_> = before
        .leases
        .keys()
        .chain(after.leases.keys())
        .filter(|(k, _)| {
            k.bridge == bridge && k.port == port && k.vid == vid && k.group.is_ipv6() == v6
        })
        .cloned()
        .collect();
    for k in keys {
        let b = before.leases.get(&k);
        let a = after.leases.get(&k);
        if b == a {
            continue;
        }
        out.push(json!({"key":key(&k.0,false),"lease_mode":if k.1.exclude{"exclude"}else{"include"},"sources":k.1.sources.iter().map(ToString::to_string).collect::<Vec<_>>(),"expires_before_ps":b.map(|l|l.deadline.to_string()),"expires_after_ps":a.map(|l|l.deadline.to_string()),"generation":after.generations.get(&k).map(ToString::to_string),"action":if a.is_none(){"expire"}else if b.is_none(){"insert"}else{"refresh"}}));
    }
    let keys: BTreeSet<_> = before
        .routers
        .keys()
        .chain(after.routers.keys())
        .filter(|k| k.bridge == bridge && k.port == port && k.vid == vid && k.group.is_ipv6() == v6)
        .cloned()
        .collect();
    for k in keys {
        let b = before.routers.get(&k);
        let a = after.routers.get(&k);
        if a == b {
            continue;
        }
        out.insert(0,json!({"key":key(&k,true),"lease_mode":"router","sources":[],"expires_before_ps":b.map(|l|l.deadline.to_string()),"expires_after_ps":a.map(|l|l.deadline.to_string()),"generation":after.router_generations.get(&k).map(ToString::to_string),"action":if a.is_none(){"expire"}else if b.is_none(){"insert"}else{"refresh"}}));
    }
    let f = (bridge.into(), vid, v6);
    if after.flood.contains(&f) && !before.flood.contains(&f) {
        out.push(json!({"key":{"switch":bridge,"vid":vid,"family":if v6{"ipv6"}else{"ipv4"},"group":null,"port":port},"lease_mode":"compatibility","sources":[],"expires_before_ps":null,"expires_after_ps":null,"generation":null,"action":"enable_flood"}));
    }
    out
}
#[allow(clippy::too_many_arguments)] // Frozen audit-construction interface.
fn row(
    a: &PreparedWireAdapter,
    port: &str,
    vid: u16,
    now: u64,
    epoch_before: u64,
    epoch_after: u64,
    outcome: &str,
    reason: Option<&str>,
    packet: Option<&str>,
    bytes: usize,
    m: Option<&membership::Message>,
    before: &membership::Snooper,
    after: &membership::Snooper,
    forwards: Vec<String>,
) -> Value {
    let v6 = a.protocol == WireProtocol::Mld;
    let query=m.and_then(|m|m.query.as_ref()).map(|q|json!({"group":q.group.to_string(),"suppress":q.suppress,"qrv":q.qrv,"qqic":q.qqic,"response_code":q.response_code,"response_ps":q.response_ps.to_string(),"interval_ps":q.interval_ps.to_string()}));
    json!({"effect_seq":"0","packet_id":packet,"protocol_revision":a.protocol.revision(),"role":"passive_conservative_snooper_v1","switch":a.bridge,"ingress_port":port,"vid":vid,"family":if v6{"ipv6"}else{"ipv4"},"time_ps":now.to_string(),"epoch_before":epoch_before.to_string(),"epoch_after":epoch_after.to_string(),"outcome":outcome,"reason":reason,"message_kind":if packet.is_none(){"timer"}else{m.map_or("unknown",|m|if m.legacy{"legacy"}else if m.is_query{"query"}else{"report"})},"byte_length":bytes,"record_count":m.map_or(0,|m|m.records.len()),"query":query,"compatibility_flood":after.flood.contains(&(a.bridge.clone(),vid,v6)),"changes":if reason.is_some(){vec![]}else{changes(before,after,&a.bridge,port,vid,v6)},"forwarded_ports":forwards})
}
pub(super) fn append(
    before: &WireState,
    d: &mut ProtocolDelta,
    event: &WireEvent,
    now: u64,
    epoch_before: u64,
    epoch_after: u64,
) -> Result<()> {
    match event {
        WireEvent::Receive {
            protocol,
            ingress_port,
            packet_id,
            bytes,
            vid,
            ..
        } if !protocol.active() => {
            let a = before.prepared.owner(*protocol, ingress_port).unwrap();
            let port = a
                .ports
                .iter()
                .find(|p| {
                    **p == *ingress_port || before.prepared.ports[*p].ingress == *ingress_port
                })
                .unwrap();
            let WireOptions::Membership(c) = &a.options else {
                unreachable!()
            };
            let reason = d
                .records
                .iter()
                .rev()
                .find(|r| r.kind == "ethernet.control.wire.event")
                .and_then(|r| r.data["reason"].as_str());
            let reason = reason.map(|r| {
                if r == "link_down" {
                    "ingress_ineligible"
                } else {
                    r
                }
            });
            let m = membership::decode(
                bytes,
                *protocol == WireProtocol::Mld,
                c.max_sources_per_record,
            )
            .ok();
            let forwarding = d
                .forwards
                .iter()
                .find(|f| f.packet_id == *packet_id)
                .map_or_else(Vec::new, |f| f.ports.clone());
            let data = row(
                a,
                port,
                vid.ok_or_else(|| fail("membership audit VID"))?,
                now,
                epoch_before,
                epoch_after,
                if reason.is_some() {
                    "control_drop"
                } else {
                    "accepted"
                },
                reason,
                Some(packet_id),
                bytes.len(),
                m.as_ref(),
                &before.snooper,
                &d.next.snooper,
                forwarding,
            );
            d.records.push(WireRecord {
                kind: "ethernet.wire.membership/1".into(),
                data,
            });
        }
        WireEvent::Timer {
            adapter_id,
            key,
            generation,
            deadline_ps,
        } if key.slot == "membership" => {
            let a = before.prepared.adapter(adapter_id).unwrap();
            let stale = d.records.iter().any(|r| {
                r.kind == "ethernet.control.wire.event" && r.data["reason"] == "timer_token"
            });
            let mut partitions = BTreeSet::new();
            let mut expired = 0;
            for (k, f) in before.snooper.leases.keys() {
                if k.bridge == a.bridge
                    && !d.next.snooper.leases.contains_key(&(k.clone(), f.clone()))
                {
                    partitions.insert((k.group.is_ipv6(), k.vid, k.port.clone()));
                    expired += 1;
                }
            }
            for k in before.snooper.routers.keys() {
                if k.bridge == a.bridge && !d.next.snooper.routers.contains_key(k) {
                    partitions.insert((k.group.is_ipv6(), k.vid, k.port.clone()));
                    expired += 1;
                }
            }
            let bp = before.snooper.projection();
            let ap = d.next.snooper.projection();
            let projected = bp
                .keys()
                .chain(ap.keys())
                .collect::<BTreeSet<_>>()
                .iter()
                .filter(|k| k.bridge == a.bridge && bp.get(**k) != ap.get(**k))
                .count()
                + before
                    .snooper
                    .routers
                    .keys()
                    .filter(|k| k.bridge == a.bridge && !d.next.snooper.routers.contains_key(*k))
                    .count();
            d.records.push(WireRecord{kind:"ethernet.wire.membership_timer/1".into(),data:json!({"effect_seq":"0","switch":a.bridge,"time_ps":now.to_string(),"scheduled_ps":deadline_ps.to_string(),"wake_generation":generation.to_string(),"outcome":if stale{"stale_timer"}else{"drained"},"epoch_before":epoch_before.to_string(),"epoch_after":epoch_after.to_string(),"expired_leases":expired,"affected_partitions":partitions.len(),"projected_controls":projected})});
            for (v6, vid, port) in partitions {
                let protocol = if v6 {
                    WireProtocol::Mld
                } else {
                    WireProtocol::Igmp
                };
                let owner = before
                    .prepared
                    .owner(protocol, &port)
                    .ok_or_else(|| fail("expired membership owner missing"))?;
                let data = row(
                    owner,
                    &port,
                    vid,
                    now,
                    epoch_before,
                    epoch_after,
                    "expired",
                    None,
                    None,
                    0,
                    None,
                    &before.snooper,
                    &d.next.snooper,
                    vec![],
                );
                d.records.push(WireRecord {
                    kind: "ethernet.wire.membership/1".into(),
                    data,
                });
            }
        }
        _ => {}
    }
    Ok(())
}
/// Source predicates apply to metadata only; link/VID/tree gates remain host-owned.
#[allow(clippy::too_many_arguments)] // Frozen multicast-selector interface.
pub(crate) fn candidates(
    s: &membership::Snooper,
    bridge: &str,
    vid: u16,
    source: IpAddr,
    group: IpAddr,
    eligible: &BTreeSet<String>,
    ingress: &str,
    static_view: &WireStaticMulticast,
) -> BTreeSet<String> {
    let v6 = group.is_ipv6();
    let always = match group {
        IpAddr::V4(a) => a.octets()[..3] == [224, 0, 0],
        IpAddr::V6(a) => a.segments()[0] == 0xff02,
    };
    let projection = s.projection();
    let known = static_view.known
        || projection
            .keys()
            .any(|k| k.bridge == bridge && k.vid == vid && k.group == group);
    eligible
        .iter()
        .filter(|p| {
            p.as_str() != ingress
                && (always
                    || !known
                    || s.flood.contains(&(bridge.into(), vid, v6))
                    || static_view.members.contains(*p)
                    || static_view.routers.contains(*p)
                    || s.routers.keys().any(|k| {
                        k.bridge == bridge
                            && k.vid == vid
                            && k.port == **p
                            && k.group.is_ipv6() == v6
                    })
                    || projection.iter().any(|(k, f)| {
                        k.bridge == bridge
                            && k.vid == vid
                            && k.port == **p
                            && k.group == group
                            && (f.sources.contains(&source) != f.exclude)
                    }))
        })
        .cloned()
        .collect()
}
