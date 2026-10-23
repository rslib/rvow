//! The hook allocation budgets, built as their own test binary: they set PATH
//! and HOME for the process and count every allocation, which would reach
//! every test of the main binary under `cargo test`.

#[path = "suite/alloc.rs"]
mod alloc;

pub(crate) mod env_lock {
    pub use rvow_core::git::testrepo::env_lock as lock;
}
