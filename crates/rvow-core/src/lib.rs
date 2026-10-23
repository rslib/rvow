// The file locks, process groups and permission checks are written for Unix, and
// CI tests only these two, so a build elsewhere would skip checks without a word.
#[cfg(not(any(target_os = "macos", target_os = "linux")))]
compile_error!("rvow supports only macOS and Linux; Windows and the other platforms are not supported");

pub mod amend;
pub mod archive;
pub mod attest;
pub mod brief;
pub mod bundle;
pub mod drift;
pub mod export;
pub mod gate;
pub mod guard;
pub mod guide;
pub mod hardening;
pub mod import;
pub mod install;
pub mod interview;
pub mod land;
pub mod lease;
pub mod mint;
pub mod notes;
pub mod proof;
pub mod protected;
pub mod put;
pub mod reprove;
pub mod review;
pub mod show;
pub mod stats;
// Always built for the same reason as `rvow_git::git::testrepo`.
#[doc(hidden)]
pub mod testutil;
pub mod view;
pub mod yolo;

pub use rvow_base::{diag, error, id};
pub use rvow_git::{git, worktree};
pub use rvow_plan::{
    change, config, fingerprint, junit, ledger, lint, plan, proposal, record, registry, runner, seal, spec, tasks, template, vow,
};
pub use rvow_store::{provenance, store};
pub use rvow_syntax::{guard_rust, syntax};
