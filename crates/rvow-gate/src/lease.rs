//! Leases: one task for one subagent or session. `leases.jsonl` in the project
//! folder is append-only; the newest events decide what a lease is now.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::change;
use crate::config;
use crate::error::{Error, Result};
use crate::fingerprint;
use crate::git;
use crate::ledger::{append_line, now_ms};
use crate::proof::{self, Now, State};
use crate::store::Project;

pub use rvow_base::layout::LEASE_FILE;
pub use rvow_git::worktree::Place;

/// A lease ends this long after it was issued: 4 hours.
pub const TTL_MS: u64 = 4 * 3_600_000;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "event", rename_all = "snake_case")]
enum Event {
    Issue {
        task: String,
        change: String,
        files: Vec<String>,
        time: u64,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        worktree: Option<PathBuf>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        branch: Option<String>,
    },
    Bind {
        task: String,
        agent_id: String,
        time: u64,
        /// The worktree of the lease a session bound by its worktree, so the
        /// bind takes that lease and not a newer one of the task elsewhere.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        worktree: Option<PathBuf>,
    },
    End {
        agent_id: String,
        time: u64,
    },
    EndTask {
        task: String,
        time: u64,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Lease {
    pub task: String,
    pub change: String,
    pub files: Vec<String>,
    pub issued: u64,
    pub agent_id: Option<String>,
    /// The place of the bind among all lease events, so the lease an agent
    /// bound last wins over one that was only issued later.
    pub bound: usize,
    pub ended: bool,
    /// The worktree the task is built in, when the lease has one of its own.
    pub worktree: Option<PathBuf>,
    /// The branch checked out in `worktree`.
    pub branch: Option<String>,
}

/// The task id of the first `lease: TSK-xxxxxx` line of a prompt, if any.
pub fn marker(prompt: &str) -> Option<String> {
    prompt.lines().find_map(|line| {
        let id = line.trim().strip_prefix("lease:")?.trim();
        let tail = id.strip_prefix("TSK-")?;
        (tail.len() == 6 && tail.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit())).then(|| id.to_string())
    })
}

/// A lease just issued, with what its brief needs from the task.
#[derive(Debug, Clone)]
pub struct Delegation {
    pub lease: Lease,
    pub title: String,
    pub verify: String,
    /// The filled runner command, when the verify names a runner that fills.
    pub run: Option<String>,
}

impl Lease {
    /// Not ended, and either its worktree still exists or it is not older
    /// than `TTL_MS` at `now`. A worktree lease has no time limit, since an
    /// agent on a long task must not lose it.
    pub fn live(&self, now: u64) -> bool {
        if self.ended {
            return false;
        }
        match &self.worktree {
            Some(w) => w.is_dir(),
            None => now.saturating_sub(self.issued) <= TTL_MS,
        }
    }

    /// Whether `top`, a worktree root, is the worktree of this lease.
    pub fn in_worktree(&self, top: &Path) -> bool {
        self.worktree.as_deref().is_some_and(|w| same_dir(w, top))
    }
}

fn same_dir(a: &Path, b: &Path) -> bool {
    match (std::fs::canonicalize(a), std::fs::canonicalize(b)) {
        (Ok(a), Ok(b)) => a == b,
        _ => a == b,
    }
}

fn read(project: &Project) -> Result<Vec<Event>> {
    let path = project.root.join(LEASE_FILE);
    let text = match std::fs::read_to_string(&path) {
        Ok(t) => t,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(Error::io(&path, e)),
    };
    Ok(text.lines().filter_map(|l| serde_json::from_str(l).ok()).collect())
}

fn write(project: &Project, event: &Event) -> Result<()> {
    let line = serde_json::to_string(event).map_err(|e| Error::internal(format!("lease event does not serialize: {e}")))?;
    append_line(&project.root.join(LEASE_FILE), &line)
}

/// Every lease, oldest first, with its binding and end applied.
pub fn leases(project: &Project) -> Result<Vec<Lease>> {
    let mut out: Vec<Lease> = Vec::new();
    for (seq, event) in read(project)?.into_iter().enumerate() {
        match event {
            Event::Issue { task, change, files, time, worktree, branch } => {
                out.push(Lease { task, change, files, issued: time, agent_id: None, bound: 0, ended: false, worktree, branch })
            }
            Event::Bind { task, agent_id, worktree, .. } => {
                let fits = |l: &Lease| l.task == task && l.agent_id.is_none() && !l.ended && (worktree.is_none() || l.worktree == worktree);
                if let Some(l) = out.iter_mut().rev().find(|l| fits(l)) {
                    l.agent_id = Some(agent_id);
                    l.bound = seq;
                }
            }
            Event::End { agent_id, .. } => {
                for l in out.iter_mut().filter(|l| l.agent_id.as_deref() == Some(agent_id.as_str())) {
                    l.ended = true;
                }
            }
            Event::EndTask { task, .. } => {
                for l in out.iter_mut().filter(|l| l.task == task) {
                    l.ended = true;
                }
            }
        }
    }
    Ok(out)
}

/// Records a lease for `input`, a task id or a unique prefix. A task that is
/// proved or blocked has nothing left to lease.
pub fn issue(project: &Project, home: &Path, cwd: &Path, input: &str) -> Result<Delegation> {
    issue_at(project, home, cwd, input, None)
}

/// The task and change that `input` names, refused when the task is proved
/// or blocked, so a caller can check before it makes a worktree.
pub fn open_task(project: &Project, home: &Path, cwd: &Path, input: &str) -> Result<(String, String)> {
    let top = git::toplevel(cwd)?;
    let (living, _) = change::load_living(project)?;
    let changes = proof::active(project, &living)?;
    let (c, task) = proof::find_task(&changes, input)?;
    let config = config::load(project, home)?;
    let now = Now::new(&top, &config, fingerprint::env(&top, &config)?, &living);
    let state = proof::state(c, task, &now)?;
    if matches!(state, State::Proved | State::Blocked) {
        return Err(Error::new(
            "lease",
            format!("{} is {}, so it has nothing left to delegate", task.id, state.name()),
            "run rvow status and delegate a task that is missing, failed or stale",
        ));
    }
    Ok((task.id.to_string(), c.dir.id.to_string()))
}

/// Records a lease for `input` like `issue`, built in the worktree of `place` when it has one.
pub fn issue_at(project: &Project, home: &Path, cwd: &Path, input: &str, place: Option<Place>) -> Result<Delegation> {
    open_task(project, home, cwd, input)?;
    let (living, _) = change::load_living(project)?;
    let changes = proof::active(project, &living)?;
    let (c, task) = proof::find_task(&changes, input)?;
    let lease = Lease {
        task: task.id.to_string(),
        change: c.dir.id.to_string(),
        files: task.files.clone(),
        issued: now_ms(),
        agent_id: None,
        bound: 0,
        ended: false,
        worktree: place.as_ref().map(|p| p.worktree.clone()),
        branch: place.map(|p| p.branch),
    };
    let lock = project.lock()?;
    write(
        project,
        &Event::Issue {
            task: lease.task.clone(),
            change: lease.change.clone(),
            files: lease.files.clone(),
            time: lease.issued,
            worktree: lease.worktree.clone(),
            branch: lease.branch.clone(),
        },
    )?;
    drop(lock);
    let runners = crate::config::runners(project).unwrap_or_default();
    let run = crate::template::plan_display(&runners, task).ok().filter(|p| p.runner.is_some()).map(|p| p.command.clone());
    Ok(Delegation { lease, title: task.title.clone(), verify: task.verify.clone().unwrap_or_default(), run })
}

/// What a bind did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Bound {
    Yes,
    /// No unbound lease of the task exists.
    NoLease,
    /// The agent still holds the live lease of this other task.
    Holds(String),
}

/// Binds a subagent id to the newest unbound lease of `task`, unless the agent
/// still holds the live lease of another task that `is_open` says is open.
pub fn bind_checked(project: &Project, task: &str, agent_id: &str, is_open: impl Fn(&str) -> Result<bool>) -> Result<Bound> {
    let _lock = project.lock()?;
    let all = leases(project)?;
    if let Some(held) = bound_last(&all, agent_id)
        && held.task != task
        && held.live(now_ms())
        && is_open(&held.task)?
    {
        return Ok(Bound::Holds(held.task.clone()));
    }
    if !all.iter().any(|l| l.task == task && l.agent_id.is_none() && !l.ended) {
        return Ok(Bound::NoLease);
    }
    write(project, &Event::Bind { task: task.into(), agent_id: agent_id.into(), time: now_ms(), worktree: None })?;
    Ok(Bound::Yes)
}

/// Binds a subagent id to the newest unbound lease of `task`. Returns whether a lease took it.
pub fn bind(project: &Project, task: &str, agent_id: &str) -> Result<bool> {
    Ok(bind_checked(project, task, agent_id, |_| Ok(true))? == Bound::Yes)
}

/// Ends the lease that `agent_id` holds, when the subagent stops.
pub fn end(project: &Project, agent_id: &str) -> Result<()> {
    let _lock = project.lock()?;
    write(project, &Event::End { agent_id: agent_id.into(), time: now_ms() })
}

/// Ends every lease of `task`, bound or not, when the task is proved or blocked.
pub fn end_task(project: &Project, task: &str) -> Result<()> {
    let _lock = project.lock()?;
    if !leases(project)?.iter().any(|l| l.task == task && !l.ended) {
        return Ok(());
    }
    write(project, &Event::EndTask { task: task.into(), time: now_ms() })
}

/// Binds `session` to the live lease of the worktree at `top` that no one
/// holds yet, unless the session already holds a live lease. Returns the
/// lease the session holds after the call, if any. Takes the project lock
/// only to bind.
pub fn bind_worktree(project: &Project, top: &Path, session: &str) -> Result<Option<Lease>> {
    let now = now_ms();
    let held = |all: &[Lease]| bound_last(all, session).filter(|l| l.live(now)).cloned();
    let all = leases(project)?;
    if let Some(l) = held(&all) {
        return Ok(Some(l));
    }
    if !all.iter().any(|l| l.agent_id.is_none() && l.live(now) && l.in_worktree(top)) {
        return Ok(None);
    }
    let _lock = project.lock()?;
    let all = leases(project)?;
    if let Some(l) = held(&all) {
        return Ok(Some(l));
    }
    let Some(free) = all.iter().rev().find(|l| l.agent_id.is_none() && l.live(now) && l.in_worktree(top)) else {
        return Ok(None);
    };
    write(project, &Event::Bind { task: free.task.clone(), agent_id: session.into(), time: now, worktree: free.worktree.clone() })?;
    Ok(leases(project)?.into_iter().filter(|l| l.agent_id.as_deref() == Some(session)).max_by_key(|l| l.bound))
}

/// The live lease whose worktree is `top`, held or not.
pub fn of_worktree(project: &Project, top: &Path) -> Result<Option<Lease>> {
    let now = now_ms();
    Ok(leases(project)?.into_iter().rev().find(|l| l.live(now) && l.in_worktree(top)))
}

/// Whether `task` has a live lease at `now` that `session` does not hold:
/// another worker builds it.
pub fn leased_away(project: &Project, task: &str, session: Option<&str>, now: u64) -> Result<bool> {
    Ok(leased_away_in(&leases(project)?, task, session, now))
}

/// `leased_away` over leases the caller already read, for a loop over tasks.
pub fn leased_away_in(all: &[Lease], task: &str, session: Option<&str>, now: u64) -> bool {
    all.iter().any(|l| l.task == task && l.live(now) && (session.is_none() || l.agent_id.as_deref() != session))
}

/// Whether `lease` holds a relaxed task of `active`, the active changes a
/// caller already loaded: a relaxed task audits an edit outside its files
/// instead of denying it. A change or task that is gone counts as strict.
pub fn relaxed(active: &[proof::Active], lease: &Lease) -> bool {
    let (Ok(change), Ok(task)) = (lease.change.parse::<crate::id::Id>(), lease.task.parse::<crate::id::Ref>()) else {
        return false;
    };
    active
        .iter()
        .filter(|c| c.dir.id == change)
        .flat_map(|c| c.loaded.tasks.iter())
        .any(|t| t.id == task && t.mode == crate::tasks::Mode::Relaxed)
}

/// The lease that `agent_id` bound last, live or not.
fn bound_last<'a>(all: &'a [Lease], agent_id: &str) -> Option<&'a Lease> {
    all.iter().filter(|l| l.agent_id.as_deref() == Some(agent_id)).max_by_key(|l| l.bound)
}

/// The live lease of `agent_id` at `now`, or `None` when it has none, it
/// stopped, it is older than `TTL_MS`, or its task is proved or blocked.
pub fn holder(project: &Project, home: &Path, cwd: &Path, agent_id: &str, now: u64) -> Result<Option<Lease>> {
    let all = leases(project)?;
    let Some(lease) = bound_last(&all, agent_id).cloned() else {
        return Ok(None);
    };
    if !lease.live(now) {
        return Ok(None);
    }
    Ok(task_open(project, home, cwd, &lease.task)?.then_some(lease))
}

/// Whether `task` is in an active change and is neither proved nor blocked.
pub fn task_open(project: &Project, home: &Path, cwd: &Path, task: &str) -> Result<bool> {
    let top = git::toplevel(cwd)?;
    let living = change::living(project)?;
    let changes = proof::active(project, &living)?;
    let Ok((c, task)) = proof::find_task(&changes, task) else {
        return Ok(false);
    };
    let config = config::load(project, home)?;
    let state_now = Now::cached(&top, &config, fingerprint::check_env(&top, &config)?, &living);
    Ok(!matches!(proof::state(c, task, &state_now)?, State::Proved | State::Blocked))
}
