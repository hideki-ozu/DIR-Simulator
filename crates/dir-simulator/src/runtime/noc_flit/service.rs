//! Bounded embedded use of the exact standalone flit engine, without another scheduler.
use super::*;
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct FlitService {
    config: FlitConfig,
    workload: FlitWorkload,
    state: State,
    reported: usize,
}
#[derive(Clone, Debug)]
pub(crate) struct Delivery {
    pub id: String,
    pub payload: Vec<u8>,
    pub status: String,
}
impl FlitService {
    pub fn new(config: FlitConfig) -> Self {
        let network = new_network(
            config,
            FlitWorkload {
                schema: "dir.noc-flit.workload".into(),
                version: 1,
                packets: vec![],
                faults: vec![],
            },
        );
        Self {
            config: network.config,
            workload: network.workload,
            state: network.state,
            reported: 0,
        }
    }
    pub fn submit(
        &mut self,
        now: u64,
        source: usize,
        destination: usize,
        id: String,
        payload: Vec<u8>,
    ) -> ModelResult<bool> {
        if source >= self.state.sources.len()
            || destination >= self.state.sources.len()
            || now % self.config.cycle_ps != 0
            || self.workload.packets.iter().any(|p| p.id == id)
        {
            return Err("invalid embedded NoC packet identity/time/endpoint".into());
        }
        let pending = self.state.sources[source].len()
            + self
                .workload
                .packets
                .iter()
                .zip(&self.state.packets)
                .filter(|(p, s)| p.source == source && !s.admitted)
                .count();
        if pending > self.config.pending_packets {
            return Ok(false);
        }
        self.workload.packets.push(Packet {
            id,
            at_ps: now,
            source,
            destination,
            payload,
        });
        self.state.packets.push(PacketState::default());
        Ok(true)
    }
    pub fn advance(
        &mut self,
        c: &mut Context<'_>,
        global_sequence: &mut u64,
    ) -> ModelResult<Vec<Delivery>> {
        let network = Network {
            config: self.config.clone(),
            workload: self.workload.clone(),
            topology: Topology::new(&self.config.topology),
            state: self.state.clone(),
            timer: None,
        };
        let mut next = self.state.clone();
        next.sequence = *global_sequence;
        network.tick(&mut next, c)?;
        next.last_tick = Some(c.now());
        next.next_tick = network.next_time(&next, c.now())?;
        network.validate_state(&next)?;
        *global_sequence = next.sequence;
        let mut result = Vec::new();
        for (i, p) in next.packets.iter().enumerate() {
            if self.state.packets[i].terminal.is_none() {
                if let Some(status) = &p.terminal {
                    result.push(Delivery {
                        id: self.workload.packets[i].id.clone(),
                        payload: p.payload.clone(),
                        status: status.clone(),
                    });
                }
            }
        }
        self.state = next;
        self.reported += result.len();
        Ok(result)
    }
    pub fn packets(&self) -> Vec<(&Packet, Option<&str>, &[u8])> {
        self.workload
            .packets
            .iter()
            .zip(&self.state.packets)
            .map(|(p, s)| (p, s.terminal.as_deref(), s.payload.as_slice()))
            .collect()
    }
    pub fn idle(&self) -> bool {
        self.state.packets.iter().all(|p| p.terminal.is_some())
            && self.state.returns.is_empty()
            && self.state.pipeline.is_empty()
    }
    pub fn validate(&self, config: &FlitConfig) -> ModelResult {
        if &self.config != config
            || self.workload.schema != "dir.noc-flit.workload"
            || self.workload.version != 1
            || !self.workload.faults.is_empty()
            || self.reported
                != self
                    .state
                    .packets
                    .iter()
                    .filter(|p| p.terminal.is_some())
                    .count()
        {
            return Err("invalid embedded NoC immutable/report identity".into());
        }
        let network = Network {
            config: self.config.clone(),
            workload: self.workload.clone(),
            topology: Topology::new(&self.config.topology),
            state: self.state.clone(),
            timer: None,
        };
        network.validate_state(&self.state)
    }
}
