//! The logic behind the harness hooks: what a tool call may do, the denial it
//! gets, and the state a session keeps between calls.

pub mod hook;

pub use rvow_base::{error, id};
pub use rvow_gate::{amend, brief, drift, gate, guard, install, lease, proof, stats, yolo};
pub use rvow_git::git;
pub use rvow_plan::{change, config, fingerprint, ledger, plan, record, vow};
pub use rvow_repo::{provenance, store};
