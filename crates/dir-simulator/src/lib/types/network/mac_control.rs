//! MAC Control configuration is independent of bridge protocol adapters and traffic classes.
#[derive(Debug, Clone)]
pub struct PreparedMacControl {
    pub ports: Vec<Option<PausePort>>,
    /// Stable configuration order breaks ties at the same physical time.
    pub controls: Vec<Control>,
}
#[derive(Debug, Clone)]
pub struct PausePort {
    pub station_mac: [u8; 6],
    pub rx_enabled: bool,
    pub tx_enabled: bool,
    pub experimental_merge: bool,
    pub experimental_project_generation: bool,
}
#[derive(Debug, Clone)]
pub struct Control {
    pub at_ps: u64,
    pub direction: usize,
    pub action: Action,
}
#[derive(Debug, Clone, Copy)]
pub enum Action {
    PauseRequest(u16),
    RxEnable(bool),
    TxEnable(bool),
    TransmitEnable(bool),
}
