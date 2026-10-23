//! The plan behind a change: the specification, the tasks, the config, the
//! ledger, the vow and the runners that a proof runs.

pub mod change;
pub mod config;
pub mod fingerprint;
pub mod junit;
pub mod ledger;
pub mod lint;
pub mod plan;
pub mod proposal;
pub mod record;
pub mod registry;
pub mod runner;
pub mod seal;
pub mod spec;
pub mod tasks;
pub mod template;
pub mod vow;

pub use rvow_base::{diag, error, id};
pub use rvow_git::git;
pub use rvow_store::store;
pub use rvow_syntax::syntax;
