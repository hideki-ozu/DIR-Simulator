//! Validated run input; model payloads are explicit and independent of runtime state.
use super::{PreparedCan, PreparedCommon, PreparedEthernet, PreparedGateway};

#[derive(Debug, Clone)]
pub struct PreparedSimulation {
    pub common: PreparedCommon,
    pub can: PreparedCan,
    pub gateway: PreparedGateway,
    pub ethernet: Option<PreparedEthernet>,
}
