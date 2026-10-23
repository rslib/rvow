//! The integration tests of the rvow-hook crate, built as one binary. Each module is one former test file; the files live in suite/.

#[path = "suite/confirm_friction.rs"]
mod confirm_friction;
#[path = "suite/gate_shells.rs"]
mod gate_shells;
#[path = "suite/script_finder.rs"]
mod script_finder;
#[path = "suite/syntax_drift.rs"]
mod syntax_drift;

/// Tests in this binary share the process env, so every one that sets an
/// env var holds this lock: confirm_friction sets RVOW_HOME and
/// syntax_drift sets RVOW_GRAMMAR_URL.
pub(crate) mod env_lock {
    pub use rvow_hook::git::testrepo::env_lock as lock;
}

/// A temp folder that is removed when the guard drops, so a test never
/// leaves one behind.
pub(crate) mod temp {
    pub use rvow_hook::git::testrepo::NamedTempDir as TempDir;
}
