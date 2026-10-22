//! The worktree of a delegated task: made beside the repository from the
//! change's branch, or recorded when an orchestrator already made it.

use std::path::{Path, PathBuf};
use std::process::Command;

use crate::error::{Error, Result};
use crate::git;
use crate::lease::Place;

/// The branch of the worktree of a task: `rvow/<change>/<task>`.
pub fn branch_name(change: &str, task: &str) -> String {
    format!("rvow/{change}/{task}")
}

/// The folder of the worktree of a task: `<parent of top>/<top folder>.worktrees/<task>`.
fn path_of(top: &Path, task: &str) -> Result<PathBuf> {
    let parent = top.parent().ok_or_else(|| {
        Error::new(
            "worktree",
            format!("{} has no parent folder to put a worktree in", top.display()),
            "run rvow delegate <task> --worktree <path> with a worktree you made",
        )
    })?;
    let name = top.file_name().map(|n| n.to_string_lossy().into_owned()).ok_or_else(|| {
        Error::new(
            "worktree",
            format!("{} has no folder name to name its worktree after", top.display()),
            "run rvow delegate <task> --worktree <path> with a worktree you made",
        )
    })?;
    Ok(parent.join(format!("{name}.worktrees")).join(task))
}

/// Whether `path` is an executable file.
fn executable(path: &Path) -> bool {
    let Ok(meta) = std::fs::metadata(path) else { return false };
    if !meta.is_file() {
        return false;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        meta.permissions().mode() & 0o111 != 0
    }
    #[cfg(not(unix))]
    {
        true
    }
}

/// The `rgit` on `PATH`, when one is there.
fn rgit() -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path).map(|dir| dir.join("rgit")).find(|p| executable(p))
}

/// The git to run: `rgit` when `PATH` has one, else the git that rvow reads with.
fn git_command() -> String {
    rgit().map_or_else(git::bin, |p| p.display().to_string())
}

/// Makes the worktree of a task: branch `rvow/<change>/<task>` at the tip of
/// `base`, the branch of the change, checked out beside the repository.
pub fn make(top: &Path, base: &str, change: &str, task: &str) -> Result<Place> {
    let branch = branch_name(change, task);
    if git::branch_tip(top, &branch).is_some() {
        return Err(Error::new(
            "worktree",
            format!("branch {branch} already exists"),
            format!("run rvow delegate {task} to lease the task in the current checkout, or git branch -D {branch} to drop it"),
        ));
    }
    if !local_branch(top, base) {
        return Err(Error::new(
            "worktree",
            format!("the change's branch {base} does not exist in {}", top.display()),
            format!("git checkout {base} in the checkout of the change, then run rvow delegate {task} --worktree again"),
        ));
    }
    let worktree = path_of(top, task)?;
    let out = Command::new(git_command())
        .current_dir(top)
        .args(["worktree", "add", "-b", &branch, &worktree.to_string_lossy(), base])
        .output()
        .map_err(|e| {
            Error::new(
                "worktree",
                format!("{} did not run: {e}", git_command()),
                "install git, then run rvow delegate <task> --worktree again",
            )
        })?;
    if !out.status.success() {
        return Err(Error::new(
            "worktree",
            format!("git worktree add -b {branch} {} {base} failed: {}", worktree.display(), String::from_utf8_lossy(&out.stderr).trim()),
            format!(
                "git worktree remove {} and git branch -D {branch} if they exist, then run rvow delegate {task} --worktree again",
                worktree.display()
            ),
        ));
    }
    Ok(Place { worktree, branch })
}

fn local_branch(top: &Path, branch: &str) -> bool {
    git::run(top, &["rev-parse", "--verify", "--quiet", &format!("refs/heads/{branch}^{{commit}}")]).is_ok()
}

/// The linked worktree of the repository at `path`, with its branch, from
/// `git worktree list --porcelain`. An error when `path` is no worktree of
/// it, is the main checkout, has no branch, or is on `base`, the change's own
/// branch: a lease there would put the whole checkout under one task.
pub fn existing(top: &Path, path: &Path, base: &str) -> Result<Place> {
    let want = std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
    let out = git::run(top, &["worktree", "list", "--porcelain"])?;
    let own = |why: String| {
        Error::new(
            "worktree",
            why,
            "run rvow delegate <task> --worktree to give the lease a worktree of its own, or name a linked worktree on its own branch",
        )
    };
    for (n, entry) in out.split("\n\n").enumerate() {
        let mut found = None;
        let mut branch = None;
        for line in entry.lines() {
            if let Some(p) = line.strip_prefix("worktree ") {
                let p = PathBuf::from(p);
                found = Some(std::fs::canonicalize(&p).unwrap_or(p));
            } else if let Some(b) = line.strip_prefix("branch refs/heads/") {
                branch = Some(b.to_string());
            }
        }
        if found.as_ref() != Some(&want) {
            continue;
        }
        if n == 0 {
            return Err(own(format!("{} is the main checkout of the repository", path.display())));
        }
        return match branch {
            Some(b) if b == base => Err(own(format!("{} has {base}, the branch of the change, checked out", path.display()))),
            Some(branch) => Ok(Place { worktree: want, branch }),
            None => Err(own(format!("{} has no branch checked out", path.display()))),
        };
    }
    Err(Error::new(
        "worktree",
        format!("{} is not a worktree of {}", path.display(), top.display()),
        "make it with git worktree add, or run rvow delegate <task> --worktree to make one",
    ))
}
