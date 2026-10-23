//! A temp folder that is removed when the guard drops, so a test never leaves one behind.

pub use rvow_core::git::testrepo::NamedTempDir as TempDir;
