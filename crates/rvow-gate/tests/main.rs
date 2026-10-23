//! The integration tests of the rvow-gate crate, built as one binary. Each module is one former test file; the files live in suite/.

#[path = "suite/amend_baseline.rs"]
mod amend_baseline;
#[path = "suite/audit_integrity.rs"]
mod audit_integrity;
#[path = "suite/fix_gate.rs"]
mod fix_gate;
#[path = "suite/flaky_tree.rs"]
mod flaky_tree;
#[path = "suite/protected_growth.rs"]
mod protected_growth;
#[path = "suite/syntax_queries.rs"]
mod syntax_queries;
#[path = "suite/syntax_units.rs"]
mod syntax_units;

/// Tests in this binary share the process env, so every one that sets an
/// env var holds this lock: syntax_units sets RVOW_GRAMMAR_OFFLINE.
pub(crate) mod env_lock {
    pub use rvow_gate::git::testrepo::env_lock as lock;
}

/// A temp folder that is removed when the guard drops, so a test never
/// leaves one behind.
pub(crate) mod temp {
    pub use rvow_gate::git::testrepo::NamedTempDir as TempDir;
}
