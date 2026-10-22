//! Passive conservative snooping. Data packets are never parsed here.
use super::codec::*;
use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeMap, BTreeSet},
    net::{IpAddr, Ipv4Addr, Ipv6Addr},
};
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Record {
    pub kind: u8,
    pub group: IpAddr,
    pub sources: BTreeSet<IpAddr>,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Query {
    pub group: IpAddr,
    pub suppress: bool,
    pub qrv: u8,
    pub qqic: u8,
    pub response_code: u16,
    pub response_ps: u64,
    pub interval_ps: u64,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Message {
    pub destination: IpAddr,
    pub query: Option<Query>,
    pub records: Vec<Record>,
    pub legacy: bool,
    pub is_query: bool,
}
fn ip(b: &[u8], v6: bool) -> IpAddr {
    if v6 {
        IpAddr::V6(Ipv6Addr::from(<[u8; 16]>::try_from(b).unwrap()))
    } else {
        IpAddr::V4(Ipv4Addr::from(<[u8; 4]>::try_from(b).unwrap()))
    }
}
fn unspecified(v6: bool) -> IpAddr {
    if v6 {
        IpAddr::V6(Ipv6Addr::UNSPECIFIED)
    } else {
        IpAddr::V4(Ipv4Addr::UNSPECIFIED)
    }
}
fn source_ok(a: IpAddr) -> bool {
    !a.is_unspecified()
        && !a.is_multicast()
        && match a {
            IpAddr::V4(v) => v != Ipv4Addr::BROADCAST,
            IpAddr::V6(_) => true,
        }
}
fn decode8(c: u8) -> u64 {
    if c < 128 {
        c as u64
    } else {
        (16 + (c & 15) as u64) << (3 + ((c >> 4) & 7))
    }
}
fn decode16(c: u16) -> u64 {
    if c < 32768 {
        c as u64
    } else {
        (4096 + (c & 4095) as u64) << (3 + ((c >> 12) & 7))
    }
}
pub fn destination_mac(a: IpAddr) -> [u8; 6] {
    match a {
        IpAddr::V4(v) => {
            let o = v.octets();
            [1, 0, 94, o[1] & 127, o[2], o[3]]
        }
        IpAddr::V6(v) => {
            let o = v.octets();
            [51, 51, o[12], o[13], o[14], o[15]]
        }
    }
}
pub fn decode(b: &[u8], v6: bool, max_sources: usize) -> DecodeResult<Message> {
    let (p, src, dst, ra) = if !v6 {
        if b.len() < 20 {
            return Err(bad("ip_length"));
        }
        let h = (b[0] & 15) as usize * 4;
        if !(20..=60).contains(&h) || h > b.len() || u16be(b, 2)? as usize != b.len() {
            return Err(bad("ip_length"));
        }
        if b[0] >> 4 != 4 || b[8] != 1 || b[9] != 2 || b[6] & 128 != 0 {
            return Err(bad("envelope"));
        }
        if u16be(b, 6)? & 0x3fff != 0 {
            return Err(bad("unsupported_fragment"));
        }
        if checksum(&b[..h]) != 0 {
            return Err(bad("ip_checksum"));
        }
        let mut q = 20;
        let mut ra = 0;
        while q < h {
            match b[q] {
                0 => break,
                1 => q += 1,
                t => {
                    let n = *b.get(q + 1).ok_or_else(|| bad("envelope"))? as usize;
                    if n < 2 || q + n > h {
                        return Err(bad("envelope"));
                    }
                    if t == 148 {
                        if n != 4 || b[q + 2..q + 4] != [0, 0] {
                            return Err(bad("envelope"));
                        }
                        ra += 1;
                    }
                    q += n;
                }
            }
        }
        (h, ip(&b[12..16], false), ip(&b[16..20], false), ra)
    } else {
        if b.len() < 48 || u16be(b, 4)? == 0 || u16be(b, 4)? as usize != b.len() - 40 {
            return Err(bad("ip_length"));
        }
        if b[0] >> 4 != 6 || b[7] != 1 {
            return Err(bad("envelope"));
        }
        if b[6] != 0 || b[40] != 58 {
            return Err(bad("unsupported_extension"));
        }
        let end = 40 + 8 * (b[41] as usize + 1);
        if end > b.len() {
            return Err(bad("ip_length"));
        }
        let mut q = 42;
        let mut ra = 0;
        while q < end {
            let typ = b[q];
            if typ == 0 {
                q += 1;
                continue;
            }
            let n = *b.get(q + 1).ok_or_else(|| bad("envelope"))? as usize;
            if q + 2 + n > end {
                return Err(bad("envelope"));
            }
            if typ == 5 {
                if n != 2 || b[q + 2..q + 4] != [0, 0] {
                    return Err(bad("envelope"));
                }
                ra += 1;
            } else if typ != 1 && typ >> 6 != 0 {
                return Err(bad("envelope"));
            }
            q += 2 + n;
        }
        (end, ip(&b[8..24], true), ip(&b[24..40], true), ra)
    };
    let m = &b[p..];
    if m.len() < 8 {
        return Err(bad("message_length"));
    }
    let query = m[0] == if v6 { 130 } else { 17 };
    let current_report = m[0] == if v6 { 143 } else { 34 };
    let legacy = query && m.len() == if v6 { 24 } else { 8 }
        || if v6 {
            matches!(m[0], 131 | 132)
        } else {
            matches!(m[0], 18 | 22 | 23)
        };
    if ra != 1 && !(ra == 0 && !v6 && (m[0] == 18 || query && legacy)) {
        return Err(bad("envelope"));
    }
    if v6 {
        let linklocal = match src {
            IpAddr::V6(a) => a.segments()[0] & 0xffc0 == 0xfe80,
            _ => false,
        };
        if !(linklocal || src.is_unspecified() && !query) {
            return Err(bad("address"));
        }
    } else if (src.is_loopback() || !source_ok(src)) && (!src.is_unspecified() || query) {
        return Err(bad("address"));
    }
    if v6 {
        let mut pseudo = Vec::new();
        if let (IpAddr::V6(s), IpAddr::V6(d)) = (src, dst) {
            pseudo.extend(s.octets());
            pseudo.extend(d.octets());
        }
        pseudo.extend((m.len() as u32).to_be_bytes());
        pseudo.extend([0, 0, 0, 58]);
        pseudo.extend(m);
        if checksum(&pseudo) != 0 {
            return Err(bad("checksum"));
        }
    } else if checksum(m) != 0 {
        return Err(bad("checksum"));
    }
    let size = if v6 { 16 } else { 4 };
    let group_offset = if v6 { 8 } else { 4 };
    let all: IpAddr = if v6 { "ff02::1" } else { "224.0.0.1" }.parse().unwrap();
    if legacy {
        if m.len() != if v6 { 24 } else { 8 } {
            return Err(bad("message_length"));
        }
        if m[1] != 0 && (!query || v6) {
            return Err(bad("envelope"));
        }
        let g = ip(&m[group_offset..group_offset + size], v6);
        if !(g.is_multicast() || query && g.is_unspecified()) {
            return Err(bad("address"));
        }
        let expected = if query {
            if g.is_unspecified() { all } else { g }
        } else if m[0] == if v6 { 132 } else { 23 } {
            if v6 { "ff02::2" } else { "224.0.0.2" }.parse().unwrap()
        } else {
            g
        };
        if dst != expected {
            return Err(bad("address"));
        }
        return Ok(Message {
            destination: dst,
            query: None,
            records: vec![],
            legacy: true,
            is_query: query,
        });
    }
    if query {
        let base = if v6 { 28 } else { 12 };
        if m.len() < base {
            return Err(bad("message_length"));
        }
        if v6 && m[1] != 0 {
            return Err(bad("envelope"));
        }
        let n = u16be(m, base - 2)? as usize;
        if m.len() < base + n * size {
            return Err(bad("message_length"));
        }
        if n > max_sources {
            return Err(bad("source_limit"));
        }
        let g = ip(&m[group_offset..group_offset + size], v6);
        if !(g.is_multicast() || g.is_unspecified() && n == 0) {
            return Err(bad("address"));
        }
        if dst != if g.is_unspecified() { all } else { g } {
            return Err(bad("address"));
        }
        for s in m[base..base + n * size].chunks_exact(size) {
            if !source_ok(ip(s, v6)) {
                return Err(bad("address"));
            }
        }
        let flags = m[base - 4];
        let qqic = m[base - 3];
        let response = if v6 { u16be(m, 4)? } else { m[1] as u16 };
        return Ok(Message {
            destination: dst,
            query: Some(Query {
                group: g,
                suppress: flags & 8 != 0,
                qrv: flags & 7,
                qqic,
                response_code: response,
                response_ps: if v6 {
                    decode16(response) * 1_000_000_000
                } else {
                    decode8(response as u8) * 100_000_000_000
                },
                interval_ps: if qqic == 0 {
                    125_000_000_000_000
                } else {
                    decode8(qqic) * 1_000_000_000_000
                },
            }),
            records: vec![],
            legacy: false,
            is_query: true,
        });
    }
    if !current_report {
        return Err(bad("unknown_message"));
    }
    let expected: IpAddr = if v6 { "ff02::16" } else { "224.0.0.22" }.parse().unwrap();
    if dst != expected {
        return Err(bad("address"));
    }
    let count = u16be(m, 6)?;
    let mut records = vec![];
    let mut q = 8;
    for _ in 0..count {
        let fixed = 4 + size;
        if q + fixed > m.len() {
            return Err(bad("message_length"));
        }
        let kind = m[q];
        let aux = m[q + 1] as usize;
        let n = u16be(m, q + 2)? as usize;
        let end = q + fixed + n * size + aux * 4;
        if end > m.len() {
            return Err(bad("message_length"));
        }
        if !(1..=6).contains(&kind) {
            return Err(bad("record_type"));
        }
        if n > max_sources {
            return Err(bad("source_limit"));
        }
        let group = ip(&m[q + 4..q + fixed], v6);
        if !group.is_multicast() {
            return Err(bad("address"));
        }
        let mut sources = BTreeSet::new();
        for bytes in m[q + fixed..q + fixed + n * size].chunks_exact(size) {
            let a = ip(bytes, v6);
            if !source_ok(a) {
                return Err(bad("address"));
            }
            sources.insert(a);
        }
        records.push(Record {
            kind,
            group,
            sources,
        });
        q = end;
    }
    Ok(Message {
        destination: dst,
        query: None,
        records,
        legacy: false,
        is_query: false,
    })
}
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct Key {
    pub bridge: String,
    pub vid: u16,
    pub group: IpAddr,
    pub port: String,
}
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct Filter {
    pub exclude: bool,
    pub sources: BTreeSet<IpAddr>,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Lease {
    pub deadline: u64,
    pub generation: u64,
}
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Snooper {
    #[serde(with = "super::codec::ordered_map")]
    pub leases: BTreeMap<(Key, Filter), Lease>,
    #[serde(with = "super::codec::ordered_map")]
    pub generations: BTreeMap<(Key, Filter), u64>,
    #[serde(with = "super::codec::ordered_map")]
    pub routers: BTreeMap<Key, Lease>,
    #[serde(with = "super::codec::ordered_map")]
    pub router_generations: BTreeMap<Key, u64>,
    pub flood: BTreeSet<(String, u16, bool)>,
}
impl Snooper {
    #[allow(clippy::too_many_arguments)] // Preserve the existing pure reducer interface.
    pub fn receive(
        &self,
        bridge: &str,
        port: &str,
        vid: u16,
        v6: bool,
        now: u64,
        m: &Message,
        membership_interval: u64,
        router_interval: u64,
    ) -> Result<Self, String> {
        let mut next = self.clone();
        if m.legacy {
            next.flood.insert((bridge.into(), vid, v6));
        }
        if m.is_query {
            let key = Key {
                bridge: bridge.into(),
                vid,
                group: unspecified(v6),
                port: port.into(),
            };
            let deadline = now
                .checked_add(router_interval)
                .ok_or("router deadline overflow")?;
            if next.routers.get(&key).map(|l| l.deadline) != Some(deadline) {
                let generation = next
                    .router_generations
                    .get(&key)
                    .unwrap_or(&0)
                    .checked_add(1)
                    .ok_or("router generation overflow")?;
                next.router_generations.insert(key.clone(), generation);
                next.routers.insert(
                    key,
                    Lease {
                        deadline,
                        generation,
                    },
                );
            }
            if let Some(q) = &m.query {
                let budget = (if q.qrv == 0 { 2 } else { q.qrv } as u64)
                    .checked_mul(q.interval_ps)
                    .and_then(|v| v.checked_add(q.response_ps))
                    .ok_or("query budget overflow")?;
                if q.group.is_unspecified()
                    && (budget > membership_interval || budget > router_interval)
                {
                    next.flood.insert((bridge.into(), vid, v6));
                }
            }
        }
        for r in &m.records {
            if r.kind == 6 || matches!(r.kind, 1 | 3 | 5) && r.sources.is_empty() {
                continue;
            }
            let key = (
                Key {
                    bridge: bridge.into(),
                    vid,
                    group: r.group,
                    port: port.into(),
                },
                Filter {
                    exclude: matches!(r.kind, 2 | 4),
                    sources: r.sources.clone(),
                },
            );
            let deadline = now
                .checked_add(membership_interval)
                .ok_or("membership deadline overflow")?;
            if next.leases.get(&key).map(|l| l.deadline) == Some(deadline) {
                continue;
            }
            let generation = next
                .generations
                .get(&key)
                .unwrap_or(&0)
                .checked_add(1)
                .ok_or("membership generation overflow")?;
            next.generations.insert(key.clone(), generation);
            next.leases.insert(
                key,
                Lease {
                    deadline,
                    generation,
                },
            );
        }
        Ok(next)
    }
    pub fn expire(&self, bridge: &str, now: u64) -> Result<Self, String> {
        let mut next = self.clone();
        for (k, l) in &self.leases {
            if k.0.bridge == bridge && l.deadline <= now {
                if l.deadline < now {
                    return Err("missed membership deadline".into());
                }
                next.leases.remove(k);
                let g = l.generation.checked_add(1).ok_or("generation overflow")?;
                next.generations.insert(k.clone(), g);
            }
        }
        for (k, l) in &self.routers {
            if k.bridge == bridge && l.deadline <= now {
                if l.deadline < now {
                    return Err("missed router deadline".into());
                }
                next.routers.remove(k);
                next.router_generations.insert(
                    k.clone(),
                    l.generation.checked_add(1).ok_or("generation overflow")?,
                );
            }
        }
        Ok(next)
    }
    pub fn next_deadline(&self, bridge: &str) -> Option<u64> {
        self.leases
            .iter()
            .filter(|(k, _)| k.0.bridge == bridge)
            .map(|(_, l)| l.deadline)
            .chain(
                self.routers
                    .iter()
                    .filter(|(k, _)| k.bridge == bridge)
                    .map(|(_, l)| l.deadline),
            )
            .min()
    }
    pub fn projection(&self) -> BTreeMap<Key, Filter> {
        let mut all: BTreeMap<Key, Vec<&Filter>> = BTreeMap::new();
        for k in self.leases.keys() {
            all.entry(k.0.clone()).or_default().push(&k.1);
        }
        all.into_iter()
            .map(|(key, filters)| {
                let mut include = BTreeSet::new();
                let mut exclude: Option<BTreeSet<IpAddr>> = None;
                for f in filters {
                    if f.exclude {
                        exclude = Some(match exclude {
                            None => f.sources.clone(),
                            Some(e) => e.intersection(&f.sources).copied().collect(),
                        });
                    } else {
                        include.extend(&f.sources);
                    }
                }
                (
                    key,
                    match exclude {
                        None => Filter {
                            exclude: false,
                            sources: include,
                        },
                        Some(e) => Filter {
                            exclude: true,
                            sources: e.difference(&include).copied().collect(),
                        },
                    },
                )
            })
            .collect()
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn predicate_union() {
        let group = "239.1.2.3".parse().unwrap();
        let a = "192.0.2.1".parse().unwrap();
        let b = "192.0.2.2".parse().unwrap();
        let c = "192.0.2.3".parse().unwrap();
        let msg = Message {
            destination: group,
            query: None,
            legacy: false,
            is_query: false,
            records: vec![
                Record {
                    kind: 2,
                    group,
                    sources: BTreeSet::from([a, b]),
                },
                Record {
                    kind: 2,
                    group,
                    sources: BTreeSet::from([b, c]),
                },
                Record {
                    kind: 1,
                    group,
                    sources: BTreeSet::from([b]),
                },
            ],
        };
        let s = Snooper::default()
            .receive("b", "p", 1, false, 10, &msg, 100, 100)
            .unwrap();
        assert_eq!(
            s.projection().values().next().unwrap(),
            &Filter {
                exclude: true,
                sources: BTreeSet::new()
            }
        );
        assert!(s.expire("b", 110).unwrap().projection().is_empty());
        assert_eq!(s.leases.len(), 3);
    }
}
