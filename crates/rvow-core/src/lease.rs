//! Leases: one task for one subagent. `leases.jsonl` in the project folder is
//! append-only; the newest events decide what a lease is now.

use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::change;
use crate::config;
use crate::error::{Error, Result};
use crate::fingerprint;
use crate::git;
use crate::ledger::{append_line, now_ms};
use crate::proof::{self, Now, State};
use crate::store::Project;

pub const LEASE_FILE: &str = "leases.jsonl";

/// A lease ends this long after it was issued: 4 hours.
pub const TTL_MS: u64 = 4 * 3_600_000;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "event", rename_all = "snake_case")]
enum Event {
    Issue { task: String, change: String, files: Vec<String>, time: u64 },
    Bind { task: String, agent_id: String, time: u64 },
    End { agent_id: String, time: u64 },
    EndTask { task: String, time: u64 },
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
    /// Not stopped by hand and not older than `TTL_MS` at `now`.
    pub fn live(&self, now: u64) -> bool {
        !self.ended && now.saturating_sub(self.issued) <= TTL_MS
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
            Event::Issue { task, change, files, time } => {
                out.push(Lease { task, change, files, issued: time, agent_id: None, bound: 0, ended: false })
            }
            Event::Bind { task, agent_id, .. } => {
                if let Some(l) = out.iter_mut().rev().find(|l| l.task == task && l.agent_id.is_none() && !l.ended) {
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
    let top = git::toplevel(cwd)?;
    let (living, _) = change::load_living(project)?;
    let changes = proof::active(project, &living)?;
    let (c, task) = proof::find_task(&changes, input)?;
    let config = config::load(project, home)?;
    let now = Now { top: &top, config: &config, env: fingerprint::env(&top, &config)?, living: &living };
    let state = proof::state(c, task, &now)?;
    if matches!(state, State::Proved | State::Blocked) {
        return Err(Error::new(
            "lease",
            format!("{} is {}, so it has nothing left to delegate", task.id, state.name()),
            "run rvow status and delegate a task that is missing, failed or stale",
        ));
    }
    let lease = Lease {
        task: task.id.to_string(),
        change: c.dir.id.to_string(),
        files: task.files.clone(),
        issued: now_ms(),
        agent_id: None,
        bound: 0,
        ended: false,
    };
    let lock = project.lock()?;
    write(
        project,
        &Event::Issue { task: lease.task.clone(), change: lease.change.clone(), files: lease.files.clone(), time: lease.issued },
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
    write(project, &Event::Bind { task: task.into(), agent_id: agent_id.into(), time: now_ms() })?;
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

/// Whether `task` has a live lease at `now` that `session` does not hold:
/// another worker builds it.
pub fn leased_away(project: &Project, task: &str, session: Option<&str>, now: u64) -> Result<bool> {
    Ok(leases(project)?.iter().any(|l| l.task == task && l.live(now) && (session.is_none() || l.agent_id.as_deref() != session)))
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
    let (living, _) = change::load_living(project)?;
    let changes = proof::active(project, &living)?;
    let Ok((c, task)) = proof::find_task(&changes, task) else {
        return Ok(false);
    };
    let config = config::load(project, home)?;
    let state_now = Now { top: &top, config: &config, env: fingerprint::env(&top, &config)?, living: &living };
    Ok(!matches!(proof::state(c, task, &state_now)?, State::Proved | State::Blocked))
}
