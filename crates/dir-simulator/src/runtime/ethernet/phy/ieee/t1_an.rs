//! Clause 98 high-speed Auto-Negotiation over sampled DME pages.
//!
//! The node owns only its local half-duplex transmit/receive state.  A caller
//! supplies the received MDI sample after resolving both transmitters through
//! the channel; no shared controller assigns a page turn.
use super::super::Result;
use super::an_hsm::{self, BasePage, DME_POSITIONS, Resolution};
use super::t1_gigabit::Role;
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    pub advertised_page: BasePage,
    pub random_seed: u64,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Phase {
    Backoff,
    Transmit,
    Blind,
    WaitReceive,
    Receive,
    Silent,
    Complete,
    Fault,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Event {
    PageReceived(BasePage),
    PageRejected,
    Completed(Role),
    Fault(String),
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Node {
    config: Config,
    samples_per_position: u32,
    phase: Phase,
    remaining: u64,
    tx_levels: Vec<i8>,
    tx_sample: usize,
    rx_levels: Vec<i8>,
    rx_sample: usize,
    saw_partner: bool,
    local_ack_sent: bool,
    partner_ack_received: bool,
    remote_page: Option<BasePage>,
    selected_role: Option<Role>,
    random_state: u64,
}
impl Node {
    /// HSM positions are 30 ns at 750 MBd.  A 16/32 sample-per-PAM-symbol
    /// reference grid represents every required timer value exactly.
    pub fn new(config: Config, samples_per_ui: u32) -> Result<Self> {
        if ![16, 32].contains(&samples_per_ui) {
            return Err("Clause 98 HSM requires 16 or 32 samples per 750 MBd UI".into());
        }
        config.advertised_page.to_bits()?;
        let mut node = Self {
            config,
            samples_per_position: 45 * samples_per_ui / 2,
            phase: Phase::Backoff,
            remaining: 0,
            tx_levels: vec![],
            tx_sample: 0,
            rx_levels: vec![],
            rx_sample: 0,
            saw_partner: false,
            local_ack_sent: false,
            partner_ack_received: false,
            remote_page: None,
            selected_role: None,
            random_state: config.random_seed,
        };
        node.remaining = node.backoff_samples();
        Ok(node)
    }
    fn ns_samples(&self, ns: u64) -> u64 {
        // 750e6 * samples_per_ui * ns/1e9; these selected ns are exact.
        ns * self.samples_per_position as u64 / 30
    }
    fn backoff_samples(&mut self) -> u64 {
        self.random_state = self.random_state.wrapping_add(0x9e37_79b9_7f4a_7c15);
        let mut x = self.random_state;
        x = (x ^ (x >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
        x = (x ^ (x >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
        let random = (x ^ (x >> 31)) & 15;
        let t4_master = self.config.advertised_page.transmitted_nonce & 0x10 != 0;
        self.ns_samples(if t4_master { 6865 } else { 7955 } + random * 2180)
    }
    pub fn phase(&self) -> Phase {
        self.phase
    }
    pub fn selected_role(&self) -> Option<Role> {
        self.selected_role
    }
    pub fn drive_sample(&mut self) -> Result<i8> {
        if matches!(
            self.phase,
            Phase::Backoff | Phase::WaitReceive | Phase::Silent
        ) && self.remaining == 0
        {
            self.start_page()?;
        }
        if self.phase == Phase::Transmit {
            let position = self.tx_sample / self.samples_per_position as usize;
            let level = *self
                .tx_levels
                .get(position)
                .ok_or("Clause 98 transmit position outside DME page")?;
            self.tx_sample += 1;
            // Position 157 is the transition back to quiet at T5 = 156*T1.
            // Its zero level belongs to the following blind/silent interval.
            if self.tx_sample == (DME_POSITIONS - 1) * self.samples_per_position as usize {
                if self.config.advertised_page.acknowledge {
                    self.local_ack_sent = true;
                }
                if self.partner_ack_received && self.local_ack_sent {
                    self.phase = Phase::Complete;
                } else {
                    self.phase = Phase::Blind;
                    self.remaining = self.ns_samples(2060);
                }
            }
            return Ok(level);
        }
        if matches!(
            self.phase,
            Phase::Backoff | Phase::WaitReceive | Phase::Blind | Phase::Silent
        ) && self.remaining > 0
        {
            self.remaining -= 1;
            if self.remaining == 0 && self.phase == Phase::Blind {
                self.phase = Phase::WaitReceive;
                // Figure98-10 RECEIVE WAIT starts a fresh random backoff.
                // Silence/overlapping or interrupted pages cannot strand AN.
                self.remaining = self.backoff_samples();
            }
        }
        Ok(0)
    }
    fn start_page(&mut self) -> Result<()> {
        let bits = self.config.advertised_page.to_bits()?;
        // A reproducible polarity draw is independent of page contents.
        self.random_state = self.random_state.wrapping_add(0x9e37_79b9_7f4a_7c15);
        let positive = self.random_state & 1 != 0;
        self.tx_levels = an_hsm::encode_page(&bits, positive)?.to_vec();
        self.tx_sample = 0;
        self.phase = Phase::Transmit;
        Ok(())
    }
    /// `received_level` is the echo-cancelled partner MDI sample, in µV.
    /// `threshold_uv` is an explicitly selected reference slicer threshold.
    pub fn observe_sample(&mut self, received_level: i64, threshold_uv: i64) -> Result<Vec<Event>> {
        if threshold_uv <= 0 {
            return Err("Clause 98 reference slicer threshold must be positive".into());
        }
        if !matches!(
            self.phase,
            Phase::Backoff | Phase::WaitReceive | Phase::Receive
        ) {
            return Ok(vec![]);
        }
        if self.phase != Phase::Receive {
            if received_level.unsigned_abs() < threshold_uv as u64 {
                return Ok(vec![]);
            }
            self.phase = Phase::Receive;
            self.rx_levels.clear();
            self.rx_sample = 0;
        }
        let position = self.rx_sample / self.samples_per_position as usize;
        let within = self.rx_sample % self.samples_per_position as usize;
        if position == DME_POSITIONS - 1 && within == 0 {
            self.rx_levels
                .push(if received_level.unsigned_abs() < threshold_uv as u64 {
                    0
                } else {
                    2
                });
        } else if within == self.samples_per_position as usize / 2 {
            self.rx_levels.push(if received_level >= threshold_uv {
                1
            } else if received_level <= -threshold_uv {
                -1
            } else {
                0
            });
        }
        self.rx_sample += 1;
        if position == DME_POSITIONS - 1 && within == 0 {
            self.phase = Phase::Silent;
            self.remaining = self.ns_samples(2180);
            let levels: [i8; DME_POSITIONS] = self
                .rx_levels
                .as_slice()
                .try_into()
                .map_err(|_| "Clause 98 receive page length mismatch")?;
            let page =
                match an_hsm::decode_page(&levels).and_then(|bits| BasePage::from_bits(&bits)) {
                    Ok(page) => page,
                    Err(_) => return Ok(vec![Event::PageRejected]),
                };
            self.saw_partner = true;
            let mut events = vec![Event::PageReceived(page)];
            let local = self.config.advertised_page;
            match an_hsm::resolve_1000base_t1(local, page) {
                Resolution::LocalRole(role) => self.selected_role = Some(role),
                Resolution::RegenerateNonce => {
                    self.config.advertised_page.transmitted_nonce =
                        (local.transmitted_nonce + 1 + (self.random_state & 15) as u8) & 31;
                    self.config.advertised_page.acknowledge = false;
                    self.saw_partner = false;
                    self.local_ack_sent = false;
                    self.partner_ack_received = false;
                    self.selected_role = None;
                    return Ok(events);
                }
                other => {
                    let reason = format!("Clause 98 arbitration failed: {other:?}");
                    self.phase = Phase::Fault;
                    events.push(Event::Fault(reason));
                    return Ok(events);
                }
            }
            if page.acknowledge && page.echoed_nonce == local.transmitted_nonce {
                self.partner_ack_received = true;
            }
            self.remote_page = Some(page);
            self.config.advertised_page.echoed_nonce = page.transmitted_nonce;
            self.config.advertised_page.acknowledge = true;
            if self.partner_ack_received && self.local_ack_sent {
                self.phase = Phase::Complete;
                events.push(Event::Completed(
                    self.selected_role.ok_or("resolved AN role absent")?,
                ));
            }
            return Ok(events);
        }
        Ok(vec![])
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn hsm_page_returns_to_quiet_at_4680_ns_not_one_position_later() {
        let mut node = Node::new(
            Config {
                advertised_page: BasePage::new_1000base_t1(0x13, true),
                random_seed: 11,
            },
            16,
        )
        .unwrap();
        let mut active = 0;
        let mut started = false;
        for _ in 0..1_000_000 {
            let level = node.drive_sample().unwrap();
            if !started && level != 0 {
                started = true;
            }
            if started {
                if level == 0 {
                    break;
                }
                active += 1;
            }
        }
        assert!(started);
        assert_eq!(active, (DME_POSITIONS - 1) * 360);
    }
    #[test]
    fn corrupted_sampled_data_position_rejects_page_without_role_resolution() {
        let mut tx = Node::new(
            Config {
                advertised_page: BasePage::new_1000base_t1(0x13, true),
                random_seed: 11,
            },
            16,
        )
        .unwrap();
        let mut rx = Node::new(
            Config {
                advertised_page: BasePage::new_1000base_t1(0x02, true),
                random_seed: 22,
            },
            16,
        )
        .unwrap();
        rx.phase = Phase::WaitReceive;
        rx.remaining = 0;
        let mut active = None;
        let mut rejected = 0;
        for sample in 0..1_000_000u64 {
            let mut level = tx.drive_sample().unwrap();
            if level != 0 && active.is_none() {
                active = Some(sample);
            }
            if active.is_some_and(|start| sample - start == 27 * 360 + 180) {
                level = -level;
            }
            for event in rx
                .observe_sample(level as i64 * 1_000_000, 100_000)
                .unwrap()
            {
                match event {
                    Event::PageRejected => rejected += 1,
                    Event::Completed(_) => panic!("corrupt page established link"),
                    _ => {}
                }
            }
            if rejected > 0 {
                break;
            }
        }
        assert_eq!(rejected, 1);
        assert_eq!(rx.selected_role(), None);
    }
    #[test]
    fn hsm_pages_alternate_with_backoff_silent_and_blind_timers() {
        let mut a = Node::new(
            Config {
                advertised_page: BasePage::new_1000base_t1(0x13, true),
                random_seed: 11,
            },
            16,
        )
        .unwrap();
        let mut b = Node::new(
            Config {
                advertised_page: BasePage::new_1000base_t1(0x02, true),
                random_seed: 22,
            },
            16,
        )
        .unwrap();
        let mut events = Vec::new();
        let mut overlap = 0;
        for _ in 0..2_000_000 {
            let av = a.drive_sample().unwrap();
            let bv = b.drive_sample().unwrap();
            overlap += usize::from(av != 0 && bv != 0);
            events.extend(a.observe_sample(bv as i64 * 1_000_000, 100_000).unwrap());
            events.extend(b.observe_sample(av as i64 * 1_000_000, 100_000).unwrap());
            if a.phase() == Phase::Complete && b.phase() == Phase::Complete {
                break;
            }
        }
        assert_eq!(overlap, 0);
        assert_eq!(a.selected_role(), Some(Role::Master));
        assert_eq!(b.selected_role(), Some(Role::Slave));
        assert!(
            events
                .iter()
                .filter(|e| matches!(e, Event::PageReceived(_)))
                .count()
                >= 3
        );
        assert_eq!(a.phase(), Phase::Complete);
        assert_eq!(b.phase(), Phase::Complete);
    }
}
