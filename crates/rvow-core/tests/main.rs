//! The integration tests of the rvow-core crate, built as one binary. Each module is one former test file; the files live in suite/.

#[path = "suite/amend_fixes.rs"]
mod amend_fixes;
#[path = "suite/audit_import.rs"]
mod audit_import;
#[path = "suite/drift_fixes.rs"]
mod drift_fixes;
#[path = "suite/verify_runners.rs"]
mod verify_runners;

/// A temp folder that is removed when the guard drops, so a test never
/// leaves one behind.
pub(crate) mod temp {
    pub use rvow_core::git::testrepo::NamedTempDir as TempDir;
}
