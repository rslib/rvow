//! `rvow land`: applies the proved task branches of a change onto its branch
//! in a temporary worktree, re-proves the tasks the merge moved, gates the
//! result and only then moves the change branch.

use std::collections::BTreeMap;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::Command;

use crate::change;
use crate::config;
use crate::error::{Error, Result};
use crate::fingerprint;
use crate::gate;
use crate::git;
use crate::id::{self, Id};
use crate::lease;
use crate::ledger;
use crate::proof::{self, Now, State};
use crate::store::Project;

/// How to land each task branch: its commits one by one, or one merge commit.
#[derive(Debug, Clone, Copy, Default)]
pub struct Options {
    pub merge: bool,
}

/// A task branch that landed on the change branch.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Landed {
    pub task: String,
    pub branch: String,
}

/// A task branch that did not apply cleanly, with the files git could not merge.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Conflict {
    pub task: String,
    pub branch: String,
    pub files: Vec<String>,
    /// The commands that rebase the task, prove it and land it again.
    pub steps: Vec<String>,
}

/// The result of one land: what moved, what conflicted and what the gate refused.
#[derive(Debug, Clone)]
pub struct Report {
    pub branch: String,
    /// The tip of the change branch before anything moved.
    pub before: String,
    /// The tip of the change branch after the move, `before` when nothing moved.
    pub tip: String,
    pub landed: Vec<Landed>,
    pub conflicts: Vec<Conflict>,
    pub failed: Vec<String>,
    /// Nothing had a task branch to apply.
    pub nothing: bool,
}

impl Report {
    pub fn ok(&self) -> bool {
        self.failed.is_empty() && !(self.landed.is_empty() && !self.conflicts.is_empty())
    }
}

/// The branch of `task` among its leases, keeping the newest lease of each task.
struct Candidate {
    task: String,
    branch: String,
    /// The commits of the branch that are not on the change branch yet, oldest first.
    commits: Vec<String>,
}

/// The commits of `branch` whose change `tip` does not hold yet, by patch id,
/// so a branch that landed before by cherry-pick is not picked again.
fn unpicked_commits(top: &Path, tip: &str, branch: &str) -> Result<Vec<String>> {
    Ok(git::run(top, &["cherry", tip, branch])?.lines().filter_map(|l| l.strip_prefix("+ ")).map(String::from).collect())
}

/// The commits of `branch` that `tip` does not reach, oldest first.
fn unmerged_commits(top: &Path, tip: &str, branch: &str) -> Result<Vec<String>> {
    Ok(git::run(top, &["rev-list", "--reverse", &format!("{tip}..{branch}")])?.lines().map(String::from).collect())
}

/// Git writes go through `rgit` when it is on PATH, else through `git`.
#[derive(Clone)]
struct Writer {
    bin: String,
    /// The name to print in steps for the user: `rgit` or `git`.
    name: &'static str,
}

impl Writer {
    fn new(rgit: Option<String>) -> Writer {
        match rgit {
            Some(bin) => Writer { bin, name: "rgit" },
            None => Writer { bin: git::bin(), name: "git" },
        }
    }

    fn run(&self, dir: &Path, args: &[&str]) -> Result<String> {
        let out = Command::new(&self.bin)
            .current_dir(dir)
            .args(args)
            // A git hook sets these for its own repository, and they would
            // override `current_dir`.
            .env_remove("GIT_DIR")
            .env_remove("GIT_WORK_TREE")
            .env_remove("GIT_INDEX_FILE")
            .output()
            .map_err(|e| Error::new("land", format!("cannot run {}: {e}", self.bin), "install git and run rvow land again"))?;
        if !out.status.success() {
            return Err(Error::new(
                "land",
                format!("{} {} failed: {}", self.bin, args.join(" "), String::from_utf8_lossy(&out.stderr).trim()),
                "run git status in the repository to see what is in the way",
            ));
        }
        Ok(String::from_utf8_lossy(&out.stdout).trim_end().to_string())
    }
}

/// The temporary worktree the result is built in, on a scratch branch that
/// only land uses. Both go away on every exit path, and the change branch is
/// never checked out there, so an error or a kill leaves it where it was.
struct Temp {
    writer: Writer,
    top: PathBuf,
    path: PathBuf,
    scratch: String,
}

impl Drop for Temp {
    fn drop(&mut self) {
        let path = self.path.to_string_lossy().into_owned();
        let _ = self.writer.run(&self.top, &["worktree", "remove", "--force", &path]);
        let _ = self.writer.run(&self.top, &["worktree", "prune"]);
        let _ = git::run(&self.top, &["branch", "-D", &self.scratch]);
    }
}

impl Temp {
    fn add(writer: Writer, top: &Path, tip: &str, scratch: &str) -> Result<Temp> {
        let path = std::env::temp_dir().join(format!("rvow-land-{}-{}", std::process::id(), ledger::now_ms()));
        let place = path.to_string_lossy().into_owned();
        // `-B` resets a scratch branch that a killed land left behind.
        writer.run(top, &["worktree", "add", "-B", scratch, &place, tip])?;
        Ok(Temp { writer, top: top.to_path_buf(), path, scratch: scratch.to_string() })
    }
}

/// Which change to land, and the one task of it when the input names a task.
fn resolve(project: &Project, input: &str) -> Result<(Id, Option<String>)> {
    let (living, _) = change::load_living(project)?;
    let changes = proof::active(project, &living)?;
    let ids: Vec<Id> = changes.iter().map(|c| c.dir.id.clone()).collect();
    let as_change = if input.contains('-') { input.to_string() } else { format!("CHG-{input}") };
    if let Ok(id) = id::resolve(&as_change, &ids) {
        return Ok((id, None));
    }
    let (c, task) = proof::find_task(&changes, input)?;
    Ok((c.dir.id.clone(), Some(task.id.to_string())))
}

/// The newest lease of each task of the change that carries a branch.
fn leases_by_task(project: &Project, change: &Id) -> Result<BTreeMap<String, lease::Lease>> {
    let mut newest: BTreeMap<String, lease::Lease> = BTreeMap::new();
    for l in lease::leases(project)?.into_iter().filter(|l| l.change == change.to_string() && l.branch.is_some()) {
        let newer = newest.get(&l.task).is_none_or(|old| (l.issued, l.bound) >= (old.issued, old.bound));
        if newer {
            newest.insert(l.task.clone(), l);
        }
    }
    Ok(newest)
}

/// Which task branches to take: one task or all, and against which tip.
struct Pick<'a> {
    only: Option<&'a str>,
    tip: &'a str,
    merge: bool,
}

/// The branch of every task in task order whose lease has a branch and has
/// ended, that is not blocked, and that holds work `tip` does not have yet. A
/// task whose branch is gone goes to `missing`.
fn candidates(
    project: &Project,
    home: &Path,
    top: &Path,
    change: &proof::Active,
    pick: Pick,
    missing: &mut Vec<String>,
) -> Result<Vec<Candidate>> {
    let Pick { only, tip, merge } = pick;
    let leases = leases_by_task(project, &change.dir.id)?;
    let all = lease::leases(project)?;
    let now = ledger::now_ms();
    let config = config::load(project, home)?;
    let (living, _) = change::load_living(project)?;
    let now_files = Now::new(top, &config, fingerprint::env(top, &config)?, &living);
    let mut out = Vec::new();
    for task in &change.loaded.tasks {
        let id = task.id.to_string();
        if only.is_some_and(|only| only != id) {
            continue;
        }
        if all.iter().any(|l| l.task == id && l.live(now)) {
            continue;
        }
        let Some(lease) = leases.get(&id) else { continue };
        let Some(branch) = &lease.branch else { continue };
        if matches!(proof::state_for(project, home, change, task, &now_files)?, State::Blocked) {
            continue;
        }
        if git::run(top, &["rev-parse", "--verify", "--quiet", &format!("refs/heads/{branch}^{{commit}}")]).is_err() {
            missing.push(format!("{id}: its branch {branch} is gone"));
            continue;
        }
        let commits = if merge { unmerged_commits(top, tip, branch)? } else { unpicked_commits(top, tip, branch)? };
        if commits.is_empty() {
            continue;
        }
        out.push(Candidate { task: id, branch: branch.clone(), commits });
    }
    Ok(out)
}

/// The files of a cherry-pick or merge that did not apply.
fn unmerged(dir: &Path) -> Vec<String> {
    git::run(dir, &["diff", "--name-only", "--diff-filter=U"]).unwrap_or_default().lines().map(String::from).collect()
}

/// The worktree that has the change branch checked out, if any, refused
/// when it has uncommitted work, since land moves that branch under it.
fn safe_checkout(top: &Path, branch: &str) -> Result<Option<PathBuf>> {
    let Some(checkout) = git::worktree_for_branch(top, branch)? else { return Ok(None) };
    let status = git::run(&checkout, &["status", "--porcelain"])?;
    if !status.trim().is_empty() {
        return Err(Error::new(
            "land",
            format!("{} has {} checked out with uncommitted work", checkout.display(), branch),
            format!("run git -C {} status and commit or stash the changes, then run rvow land again", checkout.display()),
        ));
    }
    Ok(Some(checkout))
}

/// Applies the proved task branches of `input` onto the change branch, gates
/// the result in a temporary worktree and moves the branch only when it passes.
pub fn land(
    project: &Project,
    home: &Path,
    cwd: &Path,
    input: &str,
    options: Options,
    rgit: Option<String>,
    echo: &mut dyn Write,
) -> Result<Report> {
    let top = git::toplevel(cwd)?;
    let (living, _) = change::load_living(project)?;
    let changes = proof::active(project, &living)?;
    let (change_id, only) = resolve(project, input)?;
    let change = changes
        .iter()
        .find(|c| c.dir.id == change_id)
        .ok_or_else(|| Error::new("land", format!("{change_id} is not an active change"), "run rvow status to see the active changes"))?;
    let branch = change
        .branch()
        .ok_or_else(|| Error::new("land", format!("{change_id} has no branch in its vow"), "run rvow status to see the changes"))?
        .to_string();
    let before = git::run(&top, &["rev-parse", "--verify", &format!("refs/heads/{branch}")])
        .map_err(|_| Error::new("land", format!("branch {branch} does not exist here"), format!("check out {branch} first")))?;
    safe_checkout(&top, &branch)?;

    let mut failed = Vec::new();
    let candidates =
        candidates(project, home, &top, change, Pick { only: only.as_deref(), tip: &before, merge: options.merge }, &mut failed)?;
    if candidates.is_empty() {
        return Ok(Report {
            branch,
            before: before.clone(),
            tip: before,
            landed: Vec::new(),
            conflicts: Vec::new(),
            nothing: failed.is_empty(),
            failed,
        });
    }

    let writer = Writer::new(rgit);
    let leases = leases_by_task(project, &change_id)?;
    let temp = Temp::add(writer, &top, &before, &proof::land_branch(&change_id.to_string()))?;
    let mut landed = Vec::new();
    let mut conflicts = Vec::new();
    for candidate in candidates {
        let applied = if options.merge {
            let title =
                change.loaded.tasks.iter().find(|t| t.id.to_string() == candidate.task).map(|t| t.title.clone()).unwrap_or_default();
            let message = format!("land {}: {title}", candidate.task);
            temp.writer.run(&temp.path, &["merge", "--no-ff", "--no-edit", "-m", &message, &candidate.branch])
        } else {
            let mut args = vec!["cherry-pick"];
            args.extend(candidate.commits.iter().map(String::as_str));
            temp.writer.run(&temp.path, &args)
        };
        match applied {
            Ok(_) => landed.push(Landed { task: candidate.task, branch: candidate.branch }),
            Err(_) => {
                let files = unmerged(&temp.path);
                let abort = if options.merge { "merge" } else { "cherry-pick" };
                if let Err(e) = temp.writer.run(&temp.path, &[abort, "--abort"]) {
                    // A result that cannot be cleaned up must not land.
                    failed.push(format!("{}: {abort} --abort failed: {}", candidate.task, e.message));
                    break;
                }
                let steps = conflict_steps(&temp.writer, &leases, &change_id, &branch, &candidate, &files);
                conflicts.push(Conflict { task: candidate.task, branch: candidate.branch, files, steps });
            }
        }
    }

    let names: Vec<String> = landed.iter().map(|l| l.task.clone()).collect();
    if !landed.is_empty() && failed.is_empty() {
        reprove(project, home, &temp.path, &change_id, &names, echo, &mut failed)?;
        let (living, _) = change::load_living(project)?;
        let changes = proof::active(project, &living)?;
        if let Some(change) = changes.iter().find(|c| c.dir.id == change_id) {
            let report = gate::land_gate(project, home, &temp.path, change, &names, echo)?;
            failed.extend(report.failures);
        }
    }

    let new_tip = git::run(&temp.path, &["rev-parse", "HEAD"])?;
    let tip = if failed.is_empty() && new_tip != before {
        move_branch(&temp.writer, &top, &branch, &before, &new_tip)?;
        new_tip
    } else {
        before.clone()
    };
    drop(temp);
    Ok(Report { branch, before, tip, landed, conflicts, failed, nothing: false })
}

/// Moves `branch` from `before` to `new`, only forward and only when it still
/// points at `before`: in its checkout with a fast-forward merge, else with a
/// compare-and-swap of the ref.
fn move_branch(writer: &Writer, top: &Path, branch: &str, before: &str, new: &str) -> Result<()> {
    match safe_checkout(top, branch)? {
        Some(checkout) => {
            let now = git::run(&checkout, &["rev-parse", "HEAD"])?;
            if now != before {
                return Err(Error::new(
                    "land",
                    format!("{branch} moved to {now} while land ran"),
                    "run rvow land again to land onto the new tip",
                ));
            }
            writer.run(&checkout, &["merge", "--ff-only", new])?;
        }
        None => {
            git::run(top, &["update-ref", &format!("refs/heads/{branch}"), new, before])?;
        }
    }
    Ok(())
}

/// The commands that rebase a conflicting task on the change branch, resolve
/// it, prove it and land it again.
fn conflict_steps(
    writer: &Writer,
    leases: &BTreeMap<String, lease::Lease>,
    change: &Id,
    branch: &str,
    candidate: &Candidate,
    files: &[String],
) -> Vec<String> {
    let mut steps = Vec::new();
    let worktree = leases.get(&candidate.task).and_then(|l| l.worktree.clone()).filter(|w| w.is_dir()).unwrap_or_else(|| {
        let path = std::env::temp_dir().join(format!("rvow-land-{}", candidate.task));
        steps.push(format!("{} worktree add {} {}", writer.name, path.display(), candidate.branch));
        path
    });
    steps.push(format!("cd {}", worktree.display()));
    steps.push(format!("{} rebase {branch}", writer.name));
    if !files.is_empty() {
        steps.push(format!("resolve {}", files.join(", ")));
    }
    steps.push(format!("{} rebase --continue", writer.name));
    steps.push(format!("rvow prove {}", candidate.task));
    steps.push(format!("rvow land {change}"));
    steps
}

/// Re-proves the landed tasks whose state in the temporary worktree is not
/// proved, so the gate sees the proofs of the merged tree.
fn reprove(
    project: &Project,
    home: &Path,
    temp: &Path,
    change_id: &Id,
    landed: &[String],
    echo: &mut dyn Write,
    failed: &mut Vec<String>,
) -> Result<()> {
    let (living, _) = change::load_living(project)?;
    let changes = proof::active(project, &living)?;
    let Some(change) = changes.iter().find(|c| c.dir.id == *change_id) else { return Ok(()) };
    let config = config::load(project, home)?;
    let now = Now::new(temp, &config, fingerprint::env(temp, &config)?, &living);
    for task in &change.loaded.tasks {
        let id = task.id.to_string();
        if !landed.contains(&id) || matches!(proof::state_for(project, home, change, task, &now)?, State::Proved) {
            continue;
        }
        match proof::prove(project, home, temp, &id, echo) {
            Ok(proved) if proved.proved => {}
            Ok(_) => failed.push(format!("{id}: the verify failed after the merge")),
            Err(e) => failed.push(format!("{id}: {}", e.message)),
        }
    }
    Ok(())
}
