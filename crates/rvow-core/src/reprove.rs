//! The background re-prove: when an agent's turn ends and a change on the
//! branch has only proved and stale tasks, a detached rvow proves its stale
//! tasks again, so `rvow archive` finds them current.

use std::io::Write;
use std::path::{Path, PathBuf};

use crate::change;
use crate::config;
use crate::error::{Error, Result};
use crate::fingerprint;
use crate::git;
use crate::proof::{self, Now, State};
use crate::store::Project;

/// The file that the running background re-prove locks, holding its pid.
/// It lives in the ignored `hook-state/`, like the other machine-local files.
const PID_FILE: &str = "hook-state/reprove.pid";
/// The output of the last background re-prove.
const LOG_FILE: &str = "hook-state/reprove.log";

/// The changes on the branch at `cwd` whose tasks are all proved or stale,
/// with at least one stale: the work is done and only the proofs are old.
pub fn ready(project: &Project, home: &Path, cwd: &Path) -> Result<Vec<String>> {
    let top = git::toplevel(cwd)?;
    let Some(branch) = git::branch(&top)? else {
        return Ok(Vec::new());
    };
    let (living, _) = change::load_living(project)?;
    let changes = proof::active(project, &living)?;
    let config = config::load(project, home)?;
    let now = Now { top: &top, config: &config, env: fingerprint::env(&top, &config)?, living: &living };
    let mut out = Vec::new();
    for c in proof::on_branch(&changes, &branch) {
        let mut stale = false;
        let mut done = !c.loaded.tasks.is_empty();
        for t in &c.loaded.tasks {
            match proof::state_for(project, home, c, t, &now)? {
                State::Proved => {}
                State::Stale(_) => stale = true,
                _ => done = false,
            }
        }
        if done && stale {
            out.push(c.dir.id.to_string());
        }
    }
    Ok(out)
}

/// Whether a background re-prove runs now: a process holds the lock on
/// `PID_FILE`. The lock ends with the process, also after a crash or a kill.
pub fn running(project: &Project) -> bool {
    let Ok(file) = std::fs::File::open(pid_path(project)) else {
        return false;
    };
    matches!(file.try_lock_shared(), Err(std::fs::TryLockError::WouldBlock))
}

/// Waits until no background re-prove runs, and says so on `echo` once.
pub fn wait(project: &Project, echo: &mut dyn Write) {
    if !running(project) {
        return;
    }
    let _ = writeln!(echo, "waiting for the background re-prove to finish");
    if let Ok(file) = std::fs::File::open(pid_path(project)) {
        let _ = file.lock_shared();
    }
}

/// The right to run the background re-prove, held until it is dropped or
/// the process ends.
pub struct Slot {
    _file: std::fs::File,
}

/// Takes the background slot for this process, or `None` while another
/// process holds it.
pub fn claim(project: &Project) -> Result<Option<Slot>> {
    let path = pid_path(project);
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|e| Error::io(dir, e))?;
    }
    let mut file = std::fs::File::options().create(true).truncate(false).write(true).open(&path).map_err(|e| Error::io(&path, e))?;
    match file.try_lock() {
        Ok(()) => {}
        Err(std::fs::TryLockError::WouldBlock) => return Ok(None),
        Err(std::fs::TryLockError::Error(e)) => return Err(Error::io(&path, e)),
    }
    file.set_len(0).map_err(|e| Error::io(&path, e))?;
    write!(file, "{}", std::process::id()).map_err(|e| Error::io(&path, e))?;
    Ok(Some(Slot { _file: file }))
}

fn pid_path(project: &Project) -> PathBuf {
    project.root.join(PID_FILE)
}

/// Where the background re-prove writes its output.
pub fn log_path(project: &Project) -> PathBuf {
    project.root.join(LOG_FILE)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::proof::tests::accepted;

    #[test]
    fn one_slot_at_a_time_and_free_once_dropped() {
        let (_tmp, p, _repo, _task) = accepted();
        assert!(!running(&p));
        let slot = claim(&p).unwrap().expect("a free slot");
        assert!(running(&p));
        assert!(claim(&p).unwrap().is_none(), "a second claim waits");
        drop(slot);
        assert!(!running(&p));
        let mut quiet = Vec::new();
        wait(&p, &mut quiet);
        assert!(quiet.is_empty(), "no run, no wait");
        assert!(claim(&p).unwrap().is_some());
    }
}
