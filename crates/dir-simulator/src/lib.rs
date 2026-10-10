//! DIR Simulator: deterministic Classical CAN, CAN FD, Gateway and Ethernet L2/QoS/VLAN/media simulation.
//!
//! ```no_run
//! let prepared = dir_simulator::prepare(std::path::Path::new("scenario.ini"))?;
//! let report = dir_simulator::run(prepared, std::path::Path::new("results"))?;
//! assert_eq!(report.exit_code, 0);
//! # Ok::<(), dir_simulator::types::Diagnostic>(())
//! ```
pub(crate) mod allocation;
pub mod input;
pub mod output;
pub mod registry;
mod run;
pub mod runtime;
#[path = "lib/snapshot.rs"]
pub mod snapshot;
pub mod tool;
#[path = "lib/types.rs"]
pub mod types;

pub use input::prepare;
pub use registry::prepare_with_registry;
pub use run::{
    RunFailure, RunReport, run, run_config, run_config_with_registry, run_with_diagnostics,
};
pub use runtime::can::protocol as can;
pub use tool::viewer;
pub use types::{Diagnostic, PreparedSimulation};

#[cfg(test)]
#[allow(dead_code)] // The included binary main is deliberately not called.
mod cli_runtime_boundary_tests;
