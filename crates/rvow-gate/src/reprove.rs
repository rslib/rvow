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
use crate::proof::{self, Now, Pick, State};
use crate::store::Project;
use crate::tasks::Mode;

/// The file that the running background re-prove locks, holding its pid.
/// It lives in the ignored `hook-state/`, like the other machine-local files.
const PID_FILE: &str = "hook-state/reprove.pid";
/// The output of the last background re-prove.
const LOG_FILE: &str = "hook-state/reprove.log";

/// The changes on the branch at `cwd` whose tasks are all proved or stale,
/// with at least one stale that this re-prove may run: a strict task, or a
/// relaxed task the user chose to prove `now`. The work is done and only the
/// proofs are old.
pub fn ready(project: &Project, home: &Path, cwd: &Path) -> Result<Vec<String>> {
    let living = change::living(project)?;
    ready_over(project, home, cwd, &living)
}

/// Like `ready`, with the living specs a caller already loaded.
pub fn ready_over(project: &Project, home: &Path, cwd: &Path, living: &[change::CapSpec]) -> Result<Vec<String>> {
    let top = git::toplevel(cwd)?;
    let Some(branch) = git::branch(&top)? else {
        return Ok(Vec::new());
    };
    let changes = proof::active(project, living)?;
    let config = config::load(project, home)?;
    let now = Now::new(&top, &config, fingerprint::env(&top, &config)?, living);
    let mut out = Vec::new();
    for c in proof::on_branch(&changes, &branch) {
        let mut stale = false;
        let mut done = !c.loaded.tasks.is_empty();
        for t in &c.loaded.tasks {
            match proof::state_for(project, home, c, t, &now)? {
                State::Proved => {}
                State::Stale(_) => {
                    // A stale relaxed task waits for the user's decision, so it
                    // counts as done here; only a `now` decision sends it on.
                    if t.mode == Mode::Relaxed && proof::decision(c, t, &now)? != Some(Pick::Now) {
                        continue;
                    }
                    stale = true;
                }
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
    use crate::git::testrepo::*;
    use crate::proof::tests::accepted;
    use crate::store::{Mode as StoreMode, init};

    /// A change with a relaxed task on `out.txt` and a strict task on
    /// `more.txt`, neither proved yet: (tmp, project, repo, relaxed, strict).
    fn pair() -> (TempDir, Project, PathBuf, String, String) {
        let tmp = TempDir::new();
        let home = tmp.0.join("home");
        let repo = repo_with_commit(&tmp.0, "r");
        let p = init(&repo, &home, StoreMode::Home).unwrap();
        let c = crate::change::new(&p, "Out", "main", false).unwrap();
        std::fs::write(
            c.path.join("tasks.md"),
            "- TSK-new1 Write ok\n  verify: grep -q ok out.txt && echo 'test result: ok. 1 passed; 0 failed;'\n  files: out.txt\n  mode: relaxed\n- TSK-new2 Write more\n  verify: grep -q ok more.txt && echo 'test result: ok. 1 passed; 0 failed;'\n  files: more.txt\n",
        )
        .unwrap();
        let prepared = crate::vow::prepare(&p, &home, c, &repo, &mut std::io::sink()).unwrap();
        let vowed = crate::vow::commit(&p, prepared).unwrap();
        let minted = |from: &str| vowed.minted.iter().find(|(f, _)| f == from).unwrap().1.to_string();
        (tmp, p, repo, minted("TSK-new1"), minted("TSK-new2"))
    }

    #[test]
    fn relaxed_stale_background_skips_relaxed_tasks() {
        let (tmp, p, repo, relaxed, strict) = pair();
        let home = tmp.0.join("home");
        std::fs::write(repo.join("out.txt"), "ok").unwrap();
        std::fs::write(repo.join("more.txt"), "ok").unwrap();
        for t in [&relaxed, &strict] {
            assert!(crate::proof::prove(&p, &home, &repo, t, &mut std::io::sink()).unwrap().proved);
        }
        assert!(ready(&p, &home, &repo).unwrap().is_empty(), "nothing is stale");

        std::fs::write(repo.join("out.txt"), "ok\nmore").unwrap();
        assert!(ready(&p, &home, &repo).unwrap().is_empty(), "a stale relaxed task waits for the user");

        // A strict task that went stale is the background's work.
        std::fs::write(repo.join("more.txt"), "ok\nmore").unwrap();
        let id = {
            let (dirs, _) = crate::change::list(&p).unwrap();
            dirs.iter().find(|d| d.stage == crate::change::Stage::Active).unwrap().id.to_string()
        };
        assert_eq!(ready(&p, &home, &repo).unwrap(), [id]);
        assert!(crate::proof::prove(&p, &home, &repo, &strict, &mut std::io::sink()).unwrap().proved);
        assert!(ready(&p, &home, &repo).unwrap().is_empty());

        // The user's `now` sends the relaxed task to the background too.
        crate::proof::decide(&p, &home, &repo, &relaxed, Pick::Now).unwrap();
        assert_eq!(ready(&p, &home, &repo).unwrap().len(), 1);
    }

    /// Whether `ok` holds within 2 s. A child that another test thread forks
    /// before its exec holds a copy of each open lock fd, so a lock frees only
    /// after that child execs.
    fn soon(mut ok: impl FnMut() -> bool) -> bool {
        let end = std::time::Instant::now() + std::time::Duration::from_secs(2);
        while !ok() {
            if std::time::Instant::now() > end {
                return false;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        true
    }

    #[test]
    fn one_slot_at_a_time_and_free_once_dropped() {
        let (_tmp, p, _repo, _task) = accepted();
        assert!(!running(&p));
        let slot = claim(&p).unwrap().expect("a free slot");
        assert!(running(&p));
        assert!(claim(&p).unwrap().is_none(), "a second claim waits");
        drop(slot);
        assert!(soon(|| !running(&p)));
        let mut quiet = Vec::new();
        wait(&p, &mut quiet);
        assert!(quiet.is_empty(), "no run, no wait");
        assert!(soon(|| claim(&p).unwrap().is_some()));
    }
}
