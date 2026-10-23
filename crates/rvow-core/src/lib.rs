// The file locks, process groups and permission checks are written for Unix, and
// CI tests only these two, so a build elsewhere would skip checks without a word.
#[cfg(not(any(target_os = "macos", target_os = "linux")))]
compile_error!("rvow supports only macOS and Linux; Windows and the other platforms are not supported");

pub mod attest;
pub mod bundle;
pub mod export;
pub mod guide;
pub mod import;
pub mod interview;
pub mod land;
pub mod mint;
pub mod notes;
pub mod put;
pub mod show;
pub mod view;

pub use rvow_base::{diag, error, id};
pub use rvow_gate::{amend, archive, brief, drift, gate, guard, hardening, install};
pub use rvow_gate::{lease, proof, protected, reprove, review, stats, testutil, yolo};
pub use rvow_git::{git, worktree};
pub use rvow_plan::{
    change, config, fingerprint, junit, ledger, lint, plan, proposal, record, registry, runner, seal, spec, tasks, template, vow,
};
pub use rvow_repo::{provenance, store};
pub use rvow_syntax::{guard_rust, syntax};
