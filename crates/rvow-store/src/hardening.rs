//! The rvow home that a project's own metadata names, read without the process
//! environment.

use std::path::{Component, PathBuf};

use rvow_base::error::{Error, Result};

use crate::store::{self, Mode, Project};

/// The error for metadata that does not match the store it names.
pub fn context_error() -> Error {
    Error::new("private-context", "project folder does not match its store identity", "pass the owning project and home explicitly")
}

/// Derives home-mode storage from project metadata, without consulting the
/// process environment. Repo mode uses RVOW_HOME; library callers should pass
/// their explicit home to policy and proof APIs instead.
pub fn project_home(project: &Project) -> Result<PathBuf> {
    if project.meta.mode == Mode::Repo {
        return store::home_dir();
    }
    let folder = match &project.meta.remote {
        Some(remote) => PathBuf::from(remote),
        None => PathBuf::from("local").join(project.root.file_name().ok_or_else(context_error)?),
    };
    if folder.as_os_str().is_empty() || !folder.components().all(|c| matches!(c, Component::Normal(_))) || !project.root.ends_with(&folder)
    {
        return Err(context_error());
    }
    let mut home = project.root.as_path();
    for _ in folder.components() {
        home = home.parent().ok_or_else(context_error)?;
    }
    if project.root.strip_prefix(home).ok() != Some(folder.as_path()) {
        return Err(context_error());
    }
    Ok(home.to_path_buf())
}
