//! Proofs, drift, the gate, the guard, leases and install: the rvow code that
//! the hooks need.

pub mod amend;
pub mod archive;
pub mod brief;
pub mod drift;
pub mod gate;
pub mod guard;
pub mod hardening;
pub mod install;
pub mod lease;
pub mod observed;
pub mod proof;
pub mod protected;
pub mod reprove;
pub mod review;
pub mod stats;
// Always built for the same reason as `rvow_git::git::testrepo`.
#[doc(hidden)]
pub mod testutil;
pub mod yolo;

pub use rvow_base::{diag, error, id};
pub use rvow_git::{git, worktree};
pub use rvow_plan::{
    change, config, fingerprint, junit, ledger, lint, plan, proposal, record, registry, runner, seal, spec, tasks, template, vow,
};
pub use rvow_repo::{provenance, store};
pub use rvow_syntax::{guard_rust, syntax};
