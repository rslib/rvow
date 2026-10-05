//! Optional local checks. Same-user shell access is not a security boundary.
//!
//! The hidden checks live under `<home>/hidden-checks`. `rvow hook pre-tool-use`
//! denies that folder for Edit, Write, Read, Grep, Glob, MCP tools and for any
//! Bash command that names it, but only for the calls the harness sends to the
//! hook: the settings-mode matcher includes Read, Grep and Glob, while a harness
//! that does not route them (or a program run by a script) is not covered.

use crate::error::{Error, Result};
use crate::id::Ref;
use crate::runner;
use crate::store::{self, Mode, PROJECT_FILE, Project};
use crate::tasks::Task;
use crate::vow::sha256_hex;
use serde::Serialize;
use std::collections::BTreeMap;
use std::path::{Component, Path, PathBuf};
use std::time::Duration;

/// Private-check directory that the hook denies to the implementing agent. Repo-mode projects
/// use a metadata identity digest rather than a path inside the checkout.
pub fn private_dir(project: &Project, home: &Path) -> Result<PathBuf> {
    let folder = if project.meta.mode == Mode::Home {
        project.root.strip_prefix(home).map(Path::to_path_buf).map_err(|_| context_error())?
    } else {
        let identity = serde_json::to_vec(&project.meta).map_err(|e| Error::internal(e.to_string()))?;
        PathBuf::from("repo").join(sha256_hex(&identity))
    };
    Ok(home.join("hidden-checks").join(folder))
}

fn context_error() -> Error {
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

/// Finds the owned project in change ancestors. Synthetic changes without
/// metadata have no implicit private policy. Repo-mode callers can use explicit
/// proof::state_for to avoid environment-based home resolution.
pub fn owned_context(change_path: &Path) -> Result<Option<(Project, PathBuf)>> {
    for root in change_path.ancestors() {
        let file = root.join(PROJECT_FILE);
        if !file.is_file() {
            continue;
        }
        let text = std::fs::read_to_string(&file).map_err(|e| Error::io(&file, e))?;
        let meta = toml::from_str(&text).map_err(|_| context_error())?;
        let project = Project { root: root.to_path_buf(), meta };
        let home = project_home(&project)?;
        return Ok(Some((project, home)));
    }
    Ok(None)
}

/// Path of a user-managed TSK or REQ check. No check is created by rvow.
pub fn private_path(project: &Project, home: &Path, id: &Ref) -> Result<PathBuf> {
    Ok(private_dir(project, home)?.join(format!("{id}.sh")))
}

fn read(project: &Project, home: &Path, id: &Ref) -> Result<Option<Vec<u8>>> {
    match std::fs::read(private_path(project, home, id)?) {
        Ok(bytes) => Ok(Some(bytes)),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(_) => Err(private_error()),
    }
}

fn private_error() -> Error {
    Error::new("private-check", "private check could not be read or run", "ask the user to repair their private check")
}

/// Opaque identity of one TSK or REQ script, absent when disabled.
pub fn policy_digest(project: &Project, home: &Path, id: &Ref) -> Result<Option<String>> {
    Ok(read(project, home, id)?.map(|bytes| sha256_hex(&bytes)))
}

fn scripts(project: &Project, home: &Path, task: &Task) -> Result<BTreeMap<String, Vec<u8>>> {
    let mut scripts = BTreeMap::new();
    for id in std::iter::once(&task.id).chain(task.covers.iter()) {
        if let Some(bytes) = read(project, home, id)? {
            scripts.insert(id.to_string(), bytes);
        }
    }
    Ok(scripts)
}

fn digest(scripts: &BTreeMap<String, Vec<u8>>, task: &Task) -> Option<String> {
    if scripts.is_empty() {
        return None;
    }
    if scripts.len() == 1 && scripts.contains_key(&task.id.to_string()) {
        return scripts.values().next().map(|bytes| sha256_hex(bytes));
    }
    let identities: BTreeMap<_, _> = scripts.iter().map(|(id, bytes)| (id, sha256_hex(bytes))).collect();
    Some(sha256_hex(serde_json::json!(identities).to_string().as_bytes()))
}

/// Aggregate identity of the task script and every covered requirement script.
/// Enabling, changing or removing any script changes this identity.
pub fn task_policy_digest(project: &Project, home: &Path, task: &Task) -> Result<Option<String>> {
    Ok(digest(&scripts(project, home, task)?, task))
}

#[derive(Debug, Clone, Serialize)]
pub struct CheckResult {
    pub digest: String,
    pub passed: bool,
    pub started: u64,
    pub duration_ms: u64,
    pub exit: Option<i32>,
    pub timed_out: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct PrivateResult {
    pub digest: String,
    pub passed: bool,
    pub started: u64,
    pub duration_ms: u64,
    pub exit: Option<i32>,
    pub timed_out: bool,
    pub checks: Vec<CheckResult>,
}

/// Runs all enabled task and requirement scripts with no output echo. Only
/// opaque identities and status/time leave this function. All bytes are read
/// before execution; the policy digest identifies exactly those bytes.
pub fn run_private(project: &Project, home: &Path, top: &Path, task: &Task, timeout: Duration) -> Result<Option<PrivateResult>> {
    let scripts = scripts(project, home, task)?;
    let Some(digest) = digest(&scripts, task) else { return Ok(None) };
    let started = crate::ledger::now_ms();
    let mut checks = Vec::new();
    for bytes in scripts.values() {
        let command = std::str::from_utf8(bytes).map_err(|_| private_error())?;
        let check_started = crate::ledger::now_ms();
        let result = runner::run(command, top, &[], timeout, &mut std::io::sink()).map_err(|_| private_error())?;
        checks.push(CheckResult {
            digest: sha256_hex(bytes),
            passed: result.passed(),
            started: check_started,
            duration_ms: result.duration_ms,
            exit: result.exit,
            timed_out: result.timed_out,
        });
    }
    let passed = checks.iter().all(|c| c.passed);
    let exit = checks.iter().find(|c| !c.passed).map_or(Some(0), |c| c.exit);
    let timed_out = checks.iter().any(|c| c.timed_out);
    let duration_ms = checks.iter().map(|c| c.duration_ms).sum();
    Ok(Some(PrivateResult { digest, passed, started, duration_ms, exit, timed_out, checks }))
}
