use std::path::{Path, PathBuf};

use rvow_core::error::{Error, Result};
use rvow_core::git;
use rvow_core::store::{self, Project};

pub mod export;
pub mod ingest;
pub mod lifecycle;
pub mod plan;
pub mod prove;
pub mod read;
pub mod setup;

/// The current branch for empty-state text, or `detached HEAD`.
pub fn branch_label() -> String {
    cwd().and_then(|c| git::branch(&c)).ok().flatten().unwrap_or_else(|| "detached HEAD".to_owned())
}

pub fn cwd() -> Result<PathBuf> {
    std::env::current_dir().map_err(|e| Error::io(Path::new("."), e))
}

pub fn project(home: &Path) -> Result<Project> {
    store::find(&cwd()?, home)?.ok_or_else(|| {
        Error::new("no-project", "this repository has no rvow tree", "run rvow init; it writes to the rvow home, not the repository")
    })
}

pub fn current_branch() -> Result<String> {
    git::branch(&cwd()?)?.ok_or_else(|| Error::new("detached", "HEAD is detached, so the change has no branch", "check out a branch first"))
}

pub fn which(name: &str) -> Option<PathBuf> {
    std::env::var_os("PATH").and_then(|paths| std::env::split_paths(&paths).map(|d| d.join(name)).find(|p| p.is_file()))
}
