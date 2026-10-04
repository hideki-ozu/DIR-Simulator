//! DIR Simulator: deterministic Classical CAN, Gateway and Ethernet L2/QoS/VLAN simulation.
//!
//! ```no_run
//! let prepared = dir_simulator::prepare(std::path::Path::new("scenario.ini"))?;
//! let report = dir_simulator::run(prepared, std::path::Path::new("results"))?;
//! assert_eq!(report.exit_code, 0);
//! # Ok::<(), dir_simulator::types::Diagnostic>(())
//! ```
pub mod input;
pub mod output;
mod run;
pub mod runtime;
#[path = "lib/snapshot.rs"]
pub mod snapshot;
pub mod tool;
#[path = "lib/types.rs"]
pub mod types;

pub use input::prepare;
pub use run::{RunFailure, RunReport, run, run_with_diagnostics};
pub use runtime::can::protocol as can;
pub use tool::viewer;
pub use types::{Diagnostic, PreparedSimulation};
