//! Validated run input; model payloads are explicit and independent of runtime state.
use super::{PreparedCan, PreparedCommon, PreparedEthernet, PreparedGateway};

#[derive(Debug, Clone)]
pub struct PreparedSimulation {
    pub common: PreparedCommon,
    pub can: PreparedCan,
    pub gateway: PreparedGateway,
    pub ethernet: Option<PreparedEthernet>,
    pub canfd: Option<super::PreparedCanFd>,
    pub axi: Option<super::axi::PreparedAxi>,
    pub soc: Option<super::soc::PreparedSoc>,
    pub memory_ipc: Option<super::memory_ipc::PreparedMemoryIpc>,
}

impl PreparedSimulation {
    pub fn node_count(&self) -> usize {
        if let Some(model) = &self.axi {
            model.managers.len() + 2
        } else if let Some(model) = &self.soc {
            model.nodes.len()
        } else if let Some(model) = &self.memory_ipc {
            model.placements.len()
        } else if let Some(model) = &self.ethernet {
            model.devices.len()
        } else if let Some(model) = &self.canfd {
            model.controllers.len() + 1
        } else {
            self.can.controllers.len() + self.can.buses.len()
        }
    }
}
