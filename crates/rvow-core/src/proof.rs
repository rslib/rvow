//! `rvow prove` and the state of each task, read from the ledger.

use std::collections::BTreeMap;
use std::io::Write;
use std::path::{Path, PathBuf};

use serde_json::{Map, Value, json};

use crate::change::{self, CapSpec, ChangeDir, LEDGER, Loaded, Stage};
use crate::config::{self, Config};
use crate::diag::Diagnostic;
use crate::error::{Error, Result};
use crate::fingerprint::{self, Env};
use crate::git;
use crate::id::{self, Id, Ref};
use crate::ledger::{self, Event, Kind};
use crate::runner::{self, Outcome};
use crate::spec::Requirement;
use crate::store::Project;
use crate::tasks::Task;
use crate::vow::{requirement_hash, sha256_hex, task_hash};

/// Loops: this many failed runs in a row with the same output suggest a block.
pub const LOOP_RUNS: usize = 3;

/// An accepted change with its parsed files and its ledger.
#[derive(Debug)]
pub struct Active {
    pub dir: ChangeDir,
    pub loaded: Loaded,
    pub events: Vec<Event>,
    pub ledger_diags: Vec<Diagnostic>,
    /// The branches of the live leases of this change: a session in the
    /// worktree of one of them works on this change, whatever its branch is.
    pub lease_branches: Vec<String>,
}

impl Active {
    pub fn vow(&self) -> Option<&Event> {
        self.events.iter().find(|e| e.kind == Kind::Vow)
    }

    pub fn base_commit(&self) -> Option<&str> {
        self.vow()?.data.get("base_commit")?.as_str()
    }

    /// The newest baseline run that a prove recorded for `task` while its
    /// `verify:` text was `verify`. A title edit keeps it, a new check does not.
    pub fn baseline_run(&self, task: &str, verify: &str) -> Option<&Event> {
        self.events.iter().rev().find(|e| {
            e.kind == Kind::Prove
                && e.data.get("baseline").and_then(Value::as_bool) == Some(true)
                && str_of(e, "task") == task
                && str_of(e, "task_verify") == verify
        })
    }

    /// The snapshot that drift is measured against: the snapshot of the
    /// accepted amendment recorded last, else the vow. The order of the confirm
    /// events does not count, since one confirm can accept several at once. An
    /// unlock is skipped: its snapshot is a copy from when it was recorded and
    /// would undo a plan edit accepted in the same round.
    pub fn baseline(&self) -> Option<&Map<String, Value>> {
        let mut base = self.vow().map(|e| &e.data);
        for a in self.events.iter().filter(|e| e.kind == Kind::Amend && !e.data.contains_key("unlock")) {
            let id = a.data.get("amendment");
            let accepted = self.events.iter().any(|e| {
                e.kind == Kind::Confirm && e.data.get("amendment") == id && e.data.get("decision").and_then(Value::as_str) == Some("accept")
            });
            if let (true, Some(snapshot)) = (accepted, a.data.get("snapshot").and_then(Value::as_object)) {
                base = Some(snapshot);
            }
        }
        base
    }

    /// The base hashes of the effective baseline. An amendment from before
    /// bases were snapshotted has none, so the vow's apply.
    pub fn bases(&self) -> Map<String, Value> {
        self.baseline()
            .and_then(|b| b.get("bases"))
            .or_else(|| self.vow()?.data.get("bases"))
            .and_then(Value::as_object)
            .cloned()
            .unwrap_or_default()
    }

    /// Each base whose living requirement changed or went away since it was
    /// recorded, with the current living hash.
    pub fn moved_bases(&self, living: &[CapSpec]) -> Vec<(String, Option<String>)> {
        self.bases()
            .into_iter()
            .filter_map(|(id, base)| {
                let now = living.iter().flat_map(|(_, s)| s.requirements.iter()).find(|r| r.id.to_string() == id).map(requirement_hash);
                (now.as_deref() != base.as_str()).then_some((id, now))
            })
            .collect()
    }

    pub fn branch(&self) -> Option<&str> {
        self.baseline()?.get("branch")?.as_str()
    }

    fn vow_time(&self) -> u64 {
        self.vow().map_or(u64::MAX, |e| e.time)
    }
}

/// Every change in `active/`, oldest vow first.
pub fn active(project: &Project, living: &[CapSpec]) -> Result<Vec<Active>> {
    let (changes, _) = change::list(project)?;
    let now = crate::ledger::now_ms();
    let leases = crate::lease::leases(project)?;
    let mut out = Vec::new();
    for dir in changes.into_iter().filter(|c| c.stage == Stage::Active) {
        let loaded = change::load(&dir.path, living)?;
        let (events, ledger_diags) = ledger::read(&dir.path.join(LEDGER))?;
        let id = dir.id.to_string();
        let lease_branches = leases.iter().filter(|l| l.change == id && l.live(now)).filter_map(|l| l.branch.clone()).collect();
        out.push(Active { dir, loaded, events, ledger_diags, lease_branches });
    }
    out.sort_by_key(|a| a.vow_time());
    Ok(out)
}

/// The active changes whose vow names `branch`, or that a live lease whose
/// worktree is on `branch` works on.
pub fn on_branch<'a>(changes: &'a [Active], branch: &str) -> Vec<&'a Active> {
    changes
        .iter()
        .filter(|a| {
            a.branch() == Some(branch) || a.lease_branches.iter().any(|b| b == branch) || land_branch(&a.dir.id.to_string()) == branch
        })
        .collect()
}

/// The scratch branch that `rvow land` builds the result of change `id` on,
/// which counts as the change's own branch.
pub fn land_branch(id: &str) -> String {
    format!("rvow/land/{id}")
}

/// The current hash of every requirement a task covers: the delta text when
/// the change adds or changes it, else the living text.
pub fn covered(task: &Task, loaded: &Loaded, living: &[CapSpec]) -> BTreeMap<String, String> {
    task.covers
        .iter()
        .map(|r| {
            let hash = find_requirement(r, loaded, living).map_or("missing".into(), requirement_hash);
            (r.to_string(), hash)
        })
        .collect()
}

pub fn find_requirement<'a>(r: &Ref, loaded: &'a Loaded, living: &'a [CapSpec]) -> Option<&'a Requirement> {
    loaded
        .deltas
        .iter()
        .flat_map(|(_, f)| f.deltas.iter())
        .filter(|d| d.op.has_body())
        .map(|d| &d.req)
        .chain(living.iter().flat_map(|(_, s)| s.requirements.iter()))
        .find(|req| &req.id == r)
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum State {
    Missing,
    Failed,
    Blocked,
    Flaky,
    Stale(Vec<&'static str>),
    Proved,
}

impl State {
    pub fn name(&self) -> &'static str {
        match self {
            State::Missing => "missing",
            State::Failed => "failed",
            State::Blocked => "blocked",
            State::Flaky => "flaky",
            State::Stale(_) => "stale",
            State::Proved => "proved",
        }
    }

    /// Whether the task still needs work before the agent may stop.
    pub fn open(&self) -> bool {
        !matches!(self, State::Proved | State::Blocked)
    }
}

/// What the files and tools look like now, computed once per command.
pub struct Now<'a> {
    pub top: &'a Path,
    pub config: &'a Config,
    pub env: Env,
    pub living: &'a [CapSpec],
}

impl Now<'_> {
    pub fn inputs(&self, task: &Task) -> Result<fingerprint::Inputs> {
        let globs: Vec<String> = task.files.iter().chain(&self.config.inputs).cloned().collect();
        fingerprint::inputs(self.top, &globs)
    }
}

fn str_of<'a>(e: &'a Event, k: &str) -> &'a str {
    e.data.get(k).and_then(Value::as_str).unwrap_or("")
}

/// When the run began: runs overlap, so the ledger order of two runs is not their start order.
/// An event with no `started` (a block) sorts by the time it was written.
fn started(e: &Event) -> u64 {
    e.data.get("started").and_then(Value::as_u64).unwrap_or(e.time)
}

fn is_proved(e: &Event) -> bool {
    e.data.get("proved").and_then(Value::as_bool) == Some(true)
}

/// Compatibility state lookup using the project that owns the change path.
/// Home-mode metadata determines the store home without environment lookup.
/// Synthetic changes without metadata have no private policy. Repo-mode library
/// callers should use state_for with their explicit home.
pub fn state(change: &Active, task: &Task, now: &Now) -> Result<State> {
    match crate::hardening::owned_context(&change.dir.path)? {
        Some((project, home)) => state_for(&project, &home, change, task, now),
        None => state_with_policy(change, task, now, None, None),
    }
}

/// State lookup with explicit private storage context. No ambient home lookup.
pub fn state_for(project: &Project, home: &Path, change: &Active, task: &Task, now: &Now) -> Result<State> {
    let policy = crate::hardening::task_policy_digest(project, home, task)?;
    let runners = crate::config::runners(project)?;
    state_with_policy(change, task, now, policy.as_deref(), runner_stamp(&runners, task).as_deref())
}

fn state_with_policy(change: &Active, task: &Task, now: &Now, policy: Option<&str>, runner: Option<&str>) -> Result<State> {
    let id = task.id.to_string();
    let mine: Vec<&Event> =
        change.events.iter().filter(|e| matches!(e.kind, Kind::Prove | Kind::Block) && str_of(e, "task") == id).collect();
    let Some(last) = mine.iter().enumerate().max_by_key(|(i, e)| (started(e), *i)).map(|(_, e)| *e) else {
        return Ok(State::Missing);
    };
    if last.kind == Kind::Block {
        // A confirmed amendment that changed the task ends the block.
        return Ok(if str_of(last, "task_hash") == task_hash(task) { State::Blocked } else { State::Missing });
    }
    // A red baseline is no proof attempt: the task stays missing while the
    // work goes on, so the stop hook keeps holding it.
    if !is_proved(last) && last.data.get("red").and_then(Value::as_bool) == Some(true) {
        return Ok(State::Missing);
    }
    if !is_proved(last) {
        let recorded = str_of(last, "inputs");
        return Ok(if !recorded.is_empty() && recorded != now.inputs(task)?.hash {
            State::Stale(vec!["inputs-changed"])
        } else {
            State::Failed
        });
    }
    let key = str_of(last, "key");
    let last_tree = last.data.get("worktree").and_then(Value::as_str).unwrap_or("");
    let flaky = !last_tree.is_empty()
        && mine.iter().any(|e| {
            e.kind == Kind::Prove
                && str_of(e, "key") == key
                && !is_proved(e)
                && e.data.get("worktree").and_then(Value::as_str) == Some(last_tree)
        });
    let mut reasons = Vec::new();
    if str_of(last, "task_hash") != task_hash(task) {
        reasons.push("task-changed");
    }
    let reqs: BTreeMap<String, String> =
        last.data.get("requirements").and_then(|v| serde_json::from_value(v.clone()).ok()).unwrap_or_default();
    if reqs != covered(task, &change.loaded, now.living) {
        reasons.push("requirement-changed");
    }
    if str_of(last, "inputs") != now.inputs(task)?.hash {
        reasons.push("inputs-changed");
    }
    if last.data.get("private_policy").and_then(Value::as_str) != policy {
        reasons.push("private-policy-changed");
    }
    if str_of(last, "env_key") != now.env.key() {
        reasons.push("env-changed");
    }
    if last.data.get("runner").and_then(Value::as_str) != runner {
        reasons.push("runner-changed");
    }
    if dependency_changed(change, task, last, now) {
        reasons.push("dependency-changed");
    }
    Ok(if !reasons.is_empty() {
        State::Stale(reasons)
    } else if flaky {
        State::Flaky
    } else {
        State::Proved
    })
}

/// The task on the branch to work on next: missing or failed work of the newest change first, then stale work.
pub fn next<'a>(queue: &[&'a Active], now: &Now) -> Result<Option<(&'a Active, &'a Task, State)>> {
    next_with(queue, now, None)
}

/// Queue lookup with explicit private storage context.
pub fn next_for<'a>(project: &Project, home: &Path, queue: &[&'a Active], now: &Now) -> Result<Option<(&'a Active, &'a Task, State)>> {
    next_with(queue, now, Some((project, home)))
}

fn next_with<'a>(queue: &[&'a Active], now: &Now, context: Option<(&Project, &Path)>) -> Result<Option<(&'a Active, &'a Task, State)>> {
    // The newest accepted change comes first, and a missing or failed task beats a stale one, so a
    // finished change that went stale does not hide the work in progress.
    let mut order: Vec<&'a Active> = queue.to_vec();
    order.sort_by_key(|c| std::cmp::Reverse(c.vow().map_or(0, |e| e.time)));
    let mut stale = None;
    for change in order {
        for task in &change.loaded.tasks {
            let s = match context {
                Some((project, home)) => state_for(project, home, change, task, now)?,
                None => state(change, task, now)?,
            };
            if !s.open() {
                continue;
            }
            if matches!(s, State::Stale(_) | State::Flaky) {
                stale.get_or_insert((change, task, s));
                continue;
            }
            return Ok(Some((change, task, s)));
        }
    }
    Ok(stale)
}

/// A task in full: id, title, command, scope and the text of what it covers.
pub fn render_task(task: &Task, loaded: &Loaded, living: &[CapSpec]) -> String {
    render_task_verify(task, loaded, living, task.verify.as_deref().unwrap_or(""))
}

/// Like `render_task`, with a `@runner` verify shown as the filled command
/// that rvow runs.
pub fn render_task_filled(task: &Task, loaded: &Loaded, living: &[CapSpec], runners: &BTreeMap<String, crate::template::Runner>) -> String {
    let verify = match task.verify.as_deref() {
        Some(v) => match crate::template::plan(runners, task) {
            Ok(plan) => plan.command.clone(),
            Err(_) => v.to_string(),
        },
        None => String::new(),
    };
    render_task_verify(task, loaded, living, &verify)
}

fn render_task_verify(task: &Task, loaded: &Loaded, living: &[CapSpec], verify: &str) -> String {
    let mut out = format!("{} {}\n  verify: {}\n  files: {}\n", task.id, task.title, verify, task.files.join(", "));
    for r in &task.covers {
        match find_requirement(r, loaded, living) {
            Some(req) => {
                out.push_str(&format!("  {} {}: {}\n", req.id, req.title, req.text));
                for sc in &req.scenarios {
                    out.push_str(&format!("    scenario: {}\n", sc.title));
                    for st in &sc.steps {
                        out.push_str(&format!("      {} {}\n", st.kind.word(), st.text));
                    }
                }
            }
            None => out.push_str(&format!("  {r}: not found\n")),
        }
    }
    out
}

fn hostname() -> String {
    #[cfg(unix)]
    {
        let mut buf = [0u8; 256];
        // SAFETY: gethostname writes at most buf.len() bytes into buf.
        let rc = unsafe { libc::gethostname(buf.as_mut_ptr().cast(), buf.len()) };
        if rc == 0 {
            let end = buf.iter().position(|&b| b == 0).unwrap_or(buf.len());
            return String::from_utf8_lossy(&buf[..end]).into_owned();
        }
    }
    String::new()
}

fn invocation_id() -> Result<String> {
    let b = Error::random::<8>()?;
    Ok(b.iter().map(|x| format!("{x:02x}")).collect())
}

/// The machine as recorded: a short hash of the hostname and the user's name
/// for it, never the hostname itself.
pub fn machine(config: &Config) -> Value {
    json!({
        "host": &sha256_hex(hostname().as_bytes())[..12],
        "name": config.machine,
    })
}

/// The proof agent, with an invocation override before the configured name.
pub fn agent(config: &Config) -> String {
    std::env::var("RVOW_AGENT")
        .ok()
        .filter(|name| !name.trim().is_empty())
        .or_else(|| config.agent.clone())
        .unwrap_or_else(|| "unknown".into())
}

pub struct Proved {
    pub change: Id,
    pub task: Ref,
    pub outcome: Outcome,
    pub mutants: Option<Outcome>,
    pub private_check: Option<crate::hardening::PrivateResult>,
    /// The JUnit check of the run, when the runner's run names {junit}.
    pub junit: Option<crate::junit::Check>,
    /// A note for the result, such as a CMake reconfigure the run printed.
    pub note: Option<String>,
    pub proved: bool,
    pub event: String,
    /// The same failure repeated, so a block is worth considering.
    pub looping: bool,
    pub lfs_pointers: Vec<String>,
    pub next: Option<String>,
}

/// The failed test names the baseline recorded for `task`, which a JUnit
/// report must show passed for a proof: the names of its baseline run at
/// prove, else those of the accepted baseline. An amendment snapshot from
/// before the names were carried forward holds none, so it falls back to the
/// vow event that recorded them.
pub fn junit_recorded(change: &Active, task: &str) -> Vec<String> {
    let run = change
        .loaded
        .tasks
        .iter()
        .find(|t| t.id.to_string() == task)
        .and_then(|t| change.baseline_run(task, t.verify.as_deref().unwrap_or("")));
    if let Some(run) = run {
        return run
            .data
            .get("junit_baseline")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(|v| v.as_str().map(str::to_string))
            .collect();
    }
    let names = |data: &Map<String, Value>| {
        data.get("junit")
            .and_then(Value::as_object)
            .and_then(|m| m.get(task))
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(|v| v.as_str().map(str::to_string))
            .collect::<Vec<_>>()
    };
    if let Some(base) = change.baseline() {
        let found = names(base);
        if !found.is_empty() {
            return found;
        }
        // A baseline with a junit map that names other tasks still means
        // this task recorded nothing; only a baseline with no junit map at
        // all is from before the carry-forward.
        if base.get("junit").and_then(Value::as_object).is_some() {
            return found;
        }
    }
    change.vow().map(|e| names(&e.data)).unwrap_or_default()
}

/// Finds a task id among the active changes by a unique prefix.
pub fn find_task<'a>(changes: &'a [Active], input: &str) -> Result<(&'a Active, &'a Task)> {
    let ids: Vec<Id> = changes
        .iter()
        .flat_map(|c| c.loaded.tasks.iter())
        .filter_map(|t| match &t.id {
            Ref::Id(id) => Some(id.clone()),
            Ref::Placeholder { .. } => None,
        })
        .collect();
    let input = if input.contains('-') { input.to_string() } else { format!("TSK-{input}") };
    let id = id::resolve(&input, &ids)
        .map_err(|e| Error::new("task", format!("{input}: {e}"), "run rvow status to see the tasks, or rvow ids to name new ones"))?;
    let r = Ref::Id(id);
    changes
        .iter()
        .find_map(|c| c.loaded.tasks.iter().find(|t| t.id == r).map(|t| (c, t)))
        .ok_or_else(|| Error::new("task", "task vanished", "run rvow status"))
}

/// The name and hash of the runner a task's verify names, when it names one
/// the project config holds. A runner change keeps the verify text but runs
/// a new command, so proofs and the gate cache key on this.
pub fn runner_stamp(runners: &BTreeMap<String, crate::template::Runner>, task: &Task) -> Option<String> {
    let (name, _) = crate::template::reference(task.verify.as_deref().unwrap_or(""))?;
    runners.get(name).map(|r| format!("{name}:{}", r.hash()))
}

/// Shared identity for local and portable runs. Without private policy and
/// without a runner this retains the original four-element proof key
/// byte-for-byte.
pub(crate) fn run_key(
    task_hash: &str,
    requirements: &BTreeMap<String, String>,
    inputs: &str,
    env_key: &str,
    private_policy: Option<&str>,
    runner: Option<&str>,
) -> String {
    let value = match (private_policy, runner) {
        (None, None) => json!([task_hash, requirements, inputs, env_key]),
        (Some(digest), None) => json!([task_hash, requirements, inputs, env_key, digest]),
        (policy, run) => json!([task_hash, requirements, inputs, env_key, policy, run]),
    };
    sha256_hex(value.to_string().as_bytes())
}

/// The programs a verify command starts directly, each with its resolved path
/// and SHA-256. A shell builtin or a name that resolves to nothing is absent,
/// so it must still be absent at the gate.
pub fn verify_programs(cmd: &str) -> BTreeMap<String, Value> {
    let mut out = BTreeMap::new();
    for word in crate::hook::programs(cmd) {
        if crate::hook::is_builtin(&word) {
            continue;
        }
        if let Some((path, hash)) = crate::gate::cache::tool_id(&word) {
            out.insert(word, json!({ "path": path, "sha256": hash }));
        }
    }
    out
}

/// The input hash of each `depends_on:` task at prove time, so a later change
/// in a dependency's files stales the dependent. A task that cannot be
/// resolved records `missing`, which only matches another missing one.
pub fn dependency_inputs(tasks: &[Task], task: &Task, now: &Now) -> BTreeMap<String, String> {
    task.depends_on
        .iter()
        .map(|d| {
            let hash = tasks.iter().find(|t| t.id == *d).and_then(|t| now.inputs(t).ok()).map_or_else(|| "missing".to_string(), |i| i.hash);
            (d.to_string(), hash)
        })
        .collect()
}

/// Whether a task named in `depends_on:` differs from the input hash its
/// dependent's proof recorded. A task with no record counts as changed.
fn dependency_changed(change: &Active, task: &Task, last: &Event, now: &Now) -> bool {
    if task.depends_on.is_empty() {
        return false;
    }
    let recorded: BTreeMap<String, String> =
        last.data.get("depends").and_then(|v| serde_json::from_value(v.clone()).ok()).unwrap_or_default();
    let current = dependency_inputs(&change.loaded.tasks, task, now);
    task.depends_on.iter().any(|d| recorded.get(&d.to_string()) != current.get(&d.to_string()))
}

/// Runs a task's stored verify command and records the proof.
pub fn prove(project: &Project, home: &Path, cwd: &Path, input: &str, echo: &mut dyn Write) -> Result<Proved> {
    let top = git::toplevel(cwd)?;
    let (living, _) = change::load_living(project)?;
    let changes = active(project, &living)?;
    let (change, task) = find_task(&changes, input)?;
    if let Some((path, d)) = change.loaded.errors().next() {
        return Err(Error::new(
            "invalid",
            format!("{}: {path}:{}: {}", change.dir.id, d.line, d.message),
            "run rvow validate and fix the change first",
        ));
    }
    let config = config::load(project, home)?;
    crate::drift::fetch_missing_grammars(project, &top, home, &config, change);
    let now = Now { top: &top, config: &config, env: fingerprint::env(&top, &config)?, living: &living };
    let inputs = now.inputs(task)?;
    let tree = fingerprint::worktree(&top, project)?;
    let runners = config::runners(project)?;
    let plan = crate::template::plan(&runners, task)?;
    if let Some(path) = plan.missing_need(&top) {
        return Err(Error::new(
            "needs",
            format!("{} needs {}, which does not exist", task.id, path),
            format!("configure {} first (for example with `cmake --preset ...`), then run rvow prove again", path),
        ));
    }
    let cmd = plan.command.clone();
    let id = task.id.to_string();
    let thash = task_hash(task);
    let started = ledger::now_ms();
    let mut watch = Watch { inner: echo, reconfigured: false };
    let outcome = runner::run(&cmd, &top, &plan.env, task.timeout.or(plan.timeout).unwrap_or(config.timeout()), &mut watch)?;
    // The run may have read files that changed under it, so its result
    // stands for neither the old content nor the new one.
    if now.inputs(task)?.hash != inputs.hash {
        return Err(Error::new(
            "inputs-changed",
            format!("the files of {id} changed while its check ran, so no proof was recorded"),
            format!("run rvow prove {id} again when the edits are done"),
        ));
    }
    let note = watch.note(&plan);
    let recorded = junit_recorded(change, &id);
    let junit = plan.junit.as_ref().map(|report| crate::junit::check(report, &recorded));
    let mutants = task
        .mutants
        .as_deref()
        .map(|command| {
            // The mutants command sees only the task's own changed lines:
            // RVOW_DIFF names a temp file with the diff of the task's files
            // against the vow's base commit, and RVOW_CHANGED_FILES lists the
            // changed paths, one per line. Both reach only the child. A vow
            // with no base commit predates the diff, so the diff is empty.
            let scoped = match change.base_commit() {
                Some(base) => git::task_diff(&top, base, &task.files)?,
                None => git::TaskDiff { diff: String::new(), files: Vec::new() },
            };
            let path = diff_file(&scoped.diff)?;
            let vars =
                vec![("RVOW_DIFF".to_string(), path.display().to_string()), ("RVOW_CHANGED_FILES".to_string(), scoped.files.join("\n"))];
            let outcome = runner::run(command, &top, &vars, task.timeout.unwrap_or(config.timeout()), echo);
            let _ = std::fs::remove_file(&path);
            outcome
        })
        .transpose()?;
    let private_check = crate::hardening::run_private(project, home, &top, task, task.timeout.unwrap_or(config.timeout()))?;
    let private_policy = private_check.as_ref().map(|c| c.digest.clone());
    let proved = outcome.passed()
        && outcome.tests_run != Some(0)
        && mutants.as_ref().is_none_or(Outcome::passed)
        && private_check.as_ref().is_none_or(|c| c.passed)
        && junit.as_ref().is_none_or(|c| *c == crate::junit::Check::Passed);

    let reqs = covered(task, &change.loaded, &living);
    let env_key = now.env.key();
    let stamp = runner_stamp(&runners, task);
    let key = run_key(&thash, &reqs, &inputs.hash, &env_key, private_policy.as_deref(), stamp.as_deref());
    let mut data = Map::new();
    data.insert("task".into(), task.id.to_string().into());
    data.insert("task_hash".into(), thash.into());
    data.insert("requirements".into(), json!(reqs));
    data.insert("verify".into(), cmd.clone().into());
    data.insert("runner".into(), json!(stamp));
    data.insert("proved".into(), proved.into());
    if let Some(check) = &mutants {
        let mut value = json!(check);
        value["passed"] = check.passed().into();
        // The survivor count lives inside the mutants object.
        if let Some(n) = runner::survived(&check.tail) {
            value["survived"] = n.into();
        }
        data.insert("mutants".into(), value);
    }
    if let Some(check) = &private_check {
        data.insert("private_check".into(), json!(check));
        data.insert("private_policy".into(), json!(private_policy));
    }
    data.insert("exit".into(), json!(outcome.exit));
    data.insert("timed_out".into(), outcome.timed_out.into());
    data.insert("tests_run".into(), json!(outcome.tests_run));
    if let Some(check) = &junit {
        let (passed, reason) = match check {
            crate::junit::Check::Passed => (true, None),
            crate::junit::Check::Failed(why) => (false, Some(why.clone())),
        };
        data.insert("junit".into(), json!({ "recorded": recorded, "passed": passed, "reason": reason }));
    }
    data.insert("started".into(), started.into());
    data.insert("duration_ms".into(), outcome.duration_ms.into());
    data.insert("tail".into(), json!(outcome.tail));
    data.insert("inputs".into(), inputs.hash.clone().into());
    data.insert("input_files".into(), inputs.files.into());
    data.insert("worktree".into(), tree.into());
    data.insert("depends".into(), json!(dependency_inputs(&change.loaded.tasks, task, &now)));
    data.insert("env".into(), json!(now.env));
    data.insert("env_key".into(), env_key.into());
    data.insert("key".into(), key.into());
    data.insert("programs".into(), json!(verify_programs(&cmd)));
    if proved {
        data.insert("tests".into(), Value::Object(crate::guard::capture(project, &top, &config, task)?));
        if plan.build.is_some()
            && let Some(hashes) = artifact_hashes(&top, &plan.artifacts)
        {
            data.insert("artifacts".into(), json!(hashes));
        }
    }
    data.insert(
        "run".into(),
        json!({
            "invocation": invocation_id()?,
            "rvow": env!("CARGO_PKG_VERSION"),
            "machine": machine(&config),
            "agent": agent(&config),
            "head": git::run(&top, &["rev-parse", "HEAD"]).unwrap_or_default(),
            "branch": git::branch(&top).ok().flatten(),
            "ci": fingerprint::ci(),
        }),
    );
    let ledger_path = change.dir.path.join(LEDGER);
    let event = {
        let lock = project.lock()?;
        let key = crate::seal::key(home)?;
        ledger::append_sealed(&ledger_path, &lock, Kind::Prove, data, Some(&key))?
    };
    if proved {
        crate::lease::end_task(project, &task.id.to_string())?;
    }

    // The changes are re-read for the new event anyway, so the loop check reads
    // its runs from that one read rather than the ledger a second time.
    let changes = active(project, &living)?;
    let runs: Vec<&Event> = changes
        .iter()
        .find(|c| c.dir.id == change.dir.id)
        .map(|c| c.events.iter().filter(|e| e.kind == Kind::Prove && str_of(e, "task") == id).collect())
        .unwrap_or_default();
    let looping = runs.len() >= LOOP_RUNS
        && runs[runs.len() - LOOP_RUNS..].iter().all(|e| !is_proved(e) && e.data.get("tail") == runs[runs.len() - 1].data.get("tail"));

    let branch = change_branch(&changes, &change.dir.id);
    let queue = on_branch(&changes, &branch);
    let next = next_for(project, home, &queue, &now)?
        .map(|(c, t, s)| format!("[{}] {}", s.name(), render_task_filled(t, &c.loaded, &living, &runners)));
    Ok(Proved {
        change: change.dir.id.clone(),
        task: task.id.clone(),
        outcome,
        mutants,
        private_check,
        junit,
        note,
        proved,
        event: event.hash,
        looping,
        lfs_pointers: inputs.lfs_pointers,
        next,
    })
}

/// Every file the artifact `patterns` name under `top`, sorted. A pattern with `*`, `?` or `[` is a glob
/// and must match at least one file. `None` for a pattern that is absolute, leaves the work tree or matches nothing.
fn expand_artifacts(top: &Path, patterns: &[String]) -> Option<Vec<String>> {
    fn walk(dir: &Path, top: &Path, set: &globset::GlobSet, out: &mut std::collections::BTreeSet<String>) {
        let Ok(entries) = std::fs::read_dir(dir) else { return };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                if entry.file_name() != ".git" {
                    walk(&path, top, set, out);
                }
            } else if let Some(rel) = path.strip_prefix(top).ok().and_then(Path::to_str)
                && set.is_match(rel)
            {
                out.insert(rel.to_string());
            }
        }
    }
    let mut out = std::collections::BTreeSet::new();
    for pattern in patterns {
        let path = Path::new(pattern);
        if path.is_absolute() || path.components().any(|c| matches!(c, std::path::Component::ParentDir)) {
            return None;
        }
        if !pattern.contains(['*', '?', '[']) {
            out.insert(pattern.clone());
            continue;
        }
        let literal: PathBuf = path.components().take_while(|c| !c.as_os_str().to_string_lossy().contains(['*', '?', '['])).collect();
        let set = fingerprint::globset(std::slice::from_ref(pattern)).ok()?;
        let before = out.len();
        walk(&top.join(literal), top, &set, &mut out);
        if out.len() == before {
            return None;
        }
    }
    Some(out.into_iter().collect())
}

/// The SHA-256 of each artifact the `patterns` name under `top`, or `None` when one is missing, is not a
/// file or leaves the work tree.
fn artifact_hashes(top: &Path, patterns: &[String]) -> Option<BTreeMap<String, String>> {
    expand_artifacts(top, patterns)?
        .into_iter()
        .map(|path| {
            let file = top.join(&path);
            let bytes = if file.is_file() { std::fs::read(&file).ok()? } else { return None };
            Some((path, sha256_hex(&bytes)))
        })
        .collect()
}

/// Keeps the stale proof of a task whose runner builds its artifacts, when the files changed but
/// the build leaves every artifact as the last passing proof recorded it. Records a new proof that
/// carries the old results and returns its event hash. Returns `None`, and records nothing, when
/// the task is not stale for its inputs alone or the artifacts differ, so the caller runs `prove`.
pub fn revalidate(project: &Project, home: &Path, cwd: &Path, input: &str, echo: &mut dyn Write) -> Result<Option<String>> {
    let top = git::toplevel(cwd)?;
    let (living, _) = change::load_living(project)?;
    let changes = active(project, &living)?;
    let (change, task) = find_task(&changes, input)?;
    let config = config::load(project, home)?;
    let now = Now { top: &top, config: &config, env: fingerprint::env(&top, &config)?, living: &living };
    let State::Stale(reasons) = state_for(project, home, change, task, &now)? else { return Ok(None) };
    if reasons.iter().any(|r| !matches!(*r, "inputs-changed" | "dependency-changed")) {
        return Ok(None);
    }
    let runners = config::runners(project)?;
    let plan = crate::template::plan(&runners, task)?;
    let (Some(build), false) = (plan.build.as_deref(), plan.artifacts.is_empty()) else { return Ok(None) };
    if plan.missing_need(&top).is_some() {
        return Ok(None);
    }
    let id = task.id.to_string();
    let attempts = change.events.iter().filter(|e| matches!(e.kind, Kind::Prove | Kind::Block) && str_of(e, "task") == id);
    let Some(last) = attempts.enumerate().max_by_key(|(i, e)| (started(e), *i)).map(|(_, e)| e) else { return Ok(None) };
    // A mutants or private check judged the old files, so only a full run can stand for the new ones.
    if last.kind != Kind::Prove || !is_proved(last) || last.data.contains_key("mutants") || last.data.contains_key("private_check") {
        return Ok(None);
    }
    let recorded: BTreeMap<String, String> = match last.data.get("artifacts").and_then(|v| serde_json::from_value(v.clone()).ok()) {
        Some(map) => map,
        None => return Ok(None),
    };
    let timeout = task.timeout.or(plan.timeout).unwrap_or(config.timeout());
    for check in &plan.always {
        if !runner::run(check, &top, &plan.env, timeout, echo)?.passed() {
            return Ok(None);
        }
    }
    let started = ledger::now_ms();
    let built = runner::run(build, &top, &plan.env, timeout, echo)?;
    if !built.passed() || artifact_hashes(&top, &plan.artifacts).as_ref() != Some(&recorded) {
        return Ok(None);
    }
    let inputs = now.inputs(task)?;
    let thash = task_hash(task);
    let reqs = covered(task, &change.loaded, &living);
    let env_key = now.env.key();
    let stamp = runner_stamp(&runners, task);
    let policy = last.data.get("private_policy").and_then(Value::as_str);
    let mut data = last.data.clone();
    data.remove(crate::seal::SEAL_FIELD);
    for field in ["baseline", "red", "baseline_from", "baseline_run", "junit_baseline", "task_verify"] {
        data.remove(field);
    }
    data.insert("revalidated".into(), true.into());
    data.insert("revalidated_from".into(), last.hash.clone().into());
    data.insert("started".into(), started.into());
    data.insert("duration_ms".into(), built.duration_ms.into());
    data.insert("inputs".into(), inputs.hash.clone().into());
    data.insert("input_files".into(), inputs.files.into());
    data.insert("worktree".into(), fingerprint::worktree(&top, project)?.into());
    data.insert("depends".into(), json!(dependency_inputs(&change.loaded.tasks, task, &now)));
    data.insert("env".into(), json!(now.env));
    data.insert("env_key".into(), env_key.clone().into());
    data.insert("key".into(), run_key(&thash, &reqs, &inputs.hash, &env_key, policy, stamp.as_deref()).into());
    data.insert("tests".into(), Value::Object(crate::guard::capture(project, &top, &config, task)?));
    let event = {
        let lock = project.lock()?;
        let key = crate::seal::key(home)?;
        ledger::append_sealed(&change.dir.path.join(LEDGER), &lock, Kind::Prove, data, Some(&key))?
    };
    crate::lease::end_task(project, &id)?;
    Ok(Some(event.hash))
}

/// Streams a verify run to `inner` and watches the output for a CMake
/// reconfigure, so the result can tell the user why the build was cold.
struct Watch<'a> {
    inner: &'a mut dyn Write,
    reconfigured: bool,
}

impl Write for Watch<'_> {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        if buf.windows(b"Re-running CMake".len()).any(|w| w == b"Re-running CMake") {
            self.reconfigured = true;
        }
        self.inner.write(buf)
    }

    fn flush(&mut self) -> std::io::Result<()> {
        self.inner.flush()
    }
}

impl Watch<'_> {
    /// The note the prove result shows: the build reconfigured, and with a
    /// needs path, where to ask ninja why.
    fn note(&self, plan: &crate::template::Plan) -> Option<String> {
        if !self.reconfigured {
            return None;
        }
        Some(match plan.needs.first() {
            Some(dir) => format!("the build reconfigured; run `ninja -C {dir} -d explain` to see why"),
            None => "the build reconfigured".to_string(),
        })
    }
}

/// Writes `diff` to a temp file for `RVOW_DIFF`. The caller removes it after
/// the mutants run.
fn diff_file(diff: &str) -> Result<PathBuf> {
    let bytes = Error::random::<8>()?;
    let hex: String = bytes.iter().map(|b| format!("{b:02x}")).collect();
    let path = std::env::temp_dir().join(format!("rvow-diff-{hex}"));
    std::fs::write(&path, diff).map_err(|e| Error::io(&path, e))?;
    Ok(path)
}

fn change_branch(changes: &[Active], id: &Id) -> String {
    changes.iter().find(|c| &c.dir.id == id).and_then(|c| c.branch()).unwrap_or("").to_string()
}

pub struct TaskLine {
    pub task: Ref,
    pub title: String,
    pub state: State,
}

pub struct ChangeStatus {
    pub id: Id,
    pub path: PathBuf,
    pub branch: String,
    pub tasks: Vec<TaskLine>,
    pub problems: Vec<String>,
    pub drift: Vec<crate::drift::Item>,
    pub pending: Vec<crate::amend::Pending>,
}

/// The state of every task in the active changes of the current branch, or of
/// every active change with `all`.
pub fn status(project: &Project, home: &Path, cwd: &Path, all: bool) -> Result<Vec<ChangeStatus>> {
    let top = git::toplevel(cwd)?;
    let (living, _) = change::load_living(project)?;
    let changes = active(project, &living)?;
    let config = config::load(project, home)?;
    let now = Now { top: &top, config: &config, env: fingerprint::env(&top, &config)?, living: &living };
    let branch = git::branch(&top)?;
    let mut out = Vec::new();
    for c in &changes {
        if !all && c.branch() != branch.as_deref() {
            continue;
        }
        let mut problems: Vec<String> = c.loaded.errors().map(|(p, d)| format!("{p}:{}: {}", d.line, d.message)).collect();
        problems.extend(c.ledger_diags.iter().map(|d| format!("{LEDGER}:{}: {}", d.line, d.message)));
        if c.vow().is_none() {
            problems.push("no vow event, the change was moved by hand".into());
        }
        let mut tasks = Vec::new();
        for t in &c.loaded.tasks {
            tasks.push(TaskLine { task: t.id.clone(), title: t.title.clone(), state: state_for(project, home, c, t, &now)? });
        }
        out.push(ChangeStatus {
            id: c.dir.id.clone(),
            path: c.dir.path.clone(),
            branch: c.branch().unwrap_or("").to_string(),
            tasks,
            problems,
            drift: crate::drift::drift(project, &top, &config, c)?,
            pending: crate::amend::pending(std::slice::from_ref(c)),
        });
    }
    Ok(out)
}

/// The ids of the tasks on the current branch whose proof is stale, in queue
/// order.
pub fn stale_tasks(project: &Project, home: &Path, cwd: &Path) -> Result<Vec<String>> {
    let top = git::toplevel(cwd)?;
    let (living, _) = change::load_living(project)?;
    let changes = active(project, &living)?;
    let config = config::load(project, home)?;
    let now = Now { top: &top, config: &config, env: fingerprint::env(&top, &config)?, living: &living };
    let branch = git::branch(&top)?.unwrap_or_default();
    let mut out = Vec::new();
    for c in on_branch(&changes, &branch) {
        for t in &c.loaded.tasks {
            if matches!(state_for(project, home, c, t, &now)?, State::Stale(_)) {
                out.push(t.id.to_string());
            }
        }
    }
    Ok(out)
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::git::testrepo::*;
    use crate::store::{Mode, init};
    use crate::vow;

    /// A repository with one accepted change whose single task checks that
    /// `out.txt` holds "ok", with libtest-style output.
    pub(crate) fn accepted() -> (TempDir, Project, PathBuf, String) {
        let tmp = TempDir::new();
        let home = tmp.0.join("home");
        let repo = repo_with_commit(&tmp.0, "r");
        let p = init(&repo, &home, Mode::Home).unwrap();
        let c = change::new(&p, "Out", "main", false).unwrap();
        std::fs::write(
            c.path.join("tasks.md"),
            "- TSK-new1 Write ok\n  covers: REQ-new1\n  verify: grep -q ok out.txt && echo 'test result: ok. 1 passed; 0 failed;'\n  files: out.txt\n",
        )
        .unwrap();
        std::fs::write(
            c.path.join("specs/out.md"),
            "## ADD REQ-new1 Out\nrvow SHALL write ok.\n### Scenario: s\n- WHEN it runs\n- THEN out.txt holds ok\n",
        )
        .unwrap();
        let prepared = vow::prepare(&p, &home, c, &repo, &mut std::io::sink()).unwrap();
        let vowed = vow::commit(&p, prepared).unwrap();
        let task = vowed.minted.iter().find(|(from, _)| from == "TSK-new1").unwrap().1.to_string();
        (tmp, p, repo, task)
    }

    fn state_of(p: &Project, home: &Path, repo: &Path) -> String {
        let s = status(p, home, repo, false).unwrap();
        s[0].tasks[0].state.name().to_string()
    }

    #[test]
    fn prove_moves_a_task_through_its_states() {
        let (tmp, p, repo, task) = accepted();
        let home = tmp.0.join("home");
        assert_eq!(state_of(&p, &home, &repo), "missing");

        let r = prove(&p, &home, &repo, &task, &mut std::io::sink()).unwrap();
        assert!(!r.proved);
        assert_eq!(state_of(&p, &home, &repo), "failed");
        assert!(r.next.unwrap().contains(&task));

        std::fs::write(repo.join("out.txt"), "ok").unwrap();
        let r = prove(&p, &home, &repo, &task[4..8], &mut std::io::sink()).unwrap();
        assert!(r.proved, "{:?}", r.outcome);
        assert_eq!(r.outcome.tests_run, Some(1));
        assert_eq!(r.next, None);
        assert_eq!(state_of(&p, &home, &repo), "proved");

        std::fs::write(repo.join("out.txt"), "ok\nmore").unwrap();
        let s = status(&p, &home, &repo, false).unwrap();
        assert_eq!(s[0].tasks[0].state, State::Stale(vec!["inputs-changed"]));

        assert_eq!(stale_tasks(&p, &home, &repo).unwrap(), [task]);

        git(&repo, &["checkout", "-q", "-b", "other"]);
        assert!(stale_tasks(&p, &home, &repo).unwrap().is_empty());
        assert!(status(&p, &home, &repo, false).unwrap().is_empty());
        assert_eq!(status(&p, &home, &repo, true).unwrap().len(), 1);
    }

    const ARTIFACT_CONFIG: &str = "[runners.t]\nrun = \"cp src.bin built.bin && grep -q ok out.txt && echo ran >> ran.log && echo 'test result: ok. 1 passed; 0 failed;'\"\nbuild = \"cp src.bin built.bin\"\nartifacts = [\"built.bin\"]\n";

    /// Like `accepted`, with a runner `t` in the project config and a task that names it.
    fn accepted_runner(config: &str, verify: &str) -> (TempDir, Project, PathBuf, String) {
        let tmp = TempDir::new();
        let home = tmp.0.join("home");
        let repo = repo_with_commit(&tmp.0, "r");
        let p = init(&repo, &home, Mode::Home).unwrap();
        std::fs::write(p.root.join(crate::store::CONFIG_FILE), config).unwrap();
        let c = change::new(&p, "Out", "main", false).unwrap();
        std::fs::write(
            c.path.join("tasks.md"),
            format!("- TSK-new1 Write ok\n  covers: REQ-new1\n  verify: {verify}\n  files: out.txt, src.bin, notes.txt\n"),
        )
        .unwrap();
        std::fs::write(
            c.path.join("specs/out.md"),
            "## ADD REQ-new1 Out\nrvow SHALL write ok.\n### Scenario: s\n- WHEN it runs\n- THEN out.txt holds ok\n",
        )
        .unwrap();
        let prepared = vow::prepare(&p, &home, c, &repo, &mut std::io::sink()).unwrap();
        let vowed = vow::commit(&p, prepared).unwrap();
        let task = vowed.minted.iter().find(|(from, _)| from == "TSK-new1").unwrap().1.to_string();
        (tmp, p, repo, task)
    }

    fn proved_with_artifacts() -> (TempDir, Project, PathBuf, String) {
        let (tmp, p, repo, task) = accepted_runner(ARTIFACT_CONFIG, "@t");
        let home = tmp.0.join("home");
        assert!(!prove(&p, &home, &repo, &task, &mut std::io::sink()).unwrap().proved);
        std::fs::write(repo.join("out.txt"), "ok").unwrap();
        std::fs::write(repo.join("src.bin"), "A").unwrap();
        assert!(prove(&p, &home, &repo, &task, &mut std::io::sink()).unwrap().proved);
        (tmp, p, repo, task)
    }

    fn runs(repo: &Path) -> usize {
        std::fs::read_to_string(repo.join("ran.log")).map_or(0, |t| t.lines().count())
    }

    #[test]
    fn revalidate_proof_keeps_a_stale_proof_when_the_artifacts_are_unchanged() {
        let (tmp, p, repo, task) = proved_with_artifacts();
        let home = tmp.0.join("home");
        assert_eq!(runs(&repo), 1);
        std::fs::write(repo.join("notes.txt"), "a comment").unwrap();
        assert_eq!(state_of(&p, &home, &repo), "stale");
        let event = revalidate(&p, &home, &repo, &task, &mut std::io::sink()).unwrap();
        assert!(event.is_some(), "the artifacts did not change");
        assert_eq!(state_of(&p, &home, &repo), "proved");
        assert_eq!(runs(&repo), 1, "the verify did not run again");
    }

    #[test]
    fn revalidate_proof_refuses_when_an_artifact_changed() {
        let (tmp, p, repo, task) = proved_with_artifacts();
        let home = tmp.0.join("home");
        std::fs::write(repo.join("src.bin"), "B").unwrap();
        assert_eq!(revalidate(&p, &home, &repo, &task, &mut std::io::sink()).unwrap(), None);
        assert_eq!(state_of(&p, &home, &repo), "stale");
        assert!(prove(&p, &home, &repo, &task, &mut std::io::sink()).unwrap().proved);
        assert_eq!(runs(&repo), 2, "a changed artifact runs the verify");
    }

    #[test]
    fn revalidate_proof_refuses_a_task_that_is_not_stale_or_has_no_artifacts() {
        let (tmp, p, repo, task) = proved_with_artifacts();
        let home = tmp.0.join("home");
        assert_eq!(revalidate(&p, &home, &repo, &task, &mut std::io::sink()).unwrap(), None, "a proved task needs nothing");
        let (tmp, p, repo, task) =
            accepted_runner("[runners.t]\nrun = \"true\"\n", "grep -q ok out.txt && echo 'test result: ok. 1 passed; 0 failed;'");
        let home = tmp.0.join("home");
        prove(&p, &home, &repo, &task, &mut std::io::sink()).unwrap();
        std::fs::write(repo.join("out.txt"), "ok").unwrap();
        assert!(prove(&p, &home, &repo, &task, &mut std::io::sink()).unwrap().proved);
        std::fs::write(repo.join("notes.txt"), "x").unwrap();
        assert_eq!(state_of(&p, &home, &repo), "stale");
        assert_eq!(revalidate(&p, &home, &repo, &task, &mut std::io::sink()).unwrap(), None, "a plain verify has no artifacts");
    }

    #[test]
    fn revalidate_proof_refuses_a_stale_proof_of_a_changed_task_or_runner() {
        let (tmp, p, repo, task) = proved_with_artifacts();
        let home = tmp.0.join("home");
        std::fs::write(repo.join("notes.txt"), "x").unwrap();
        let other = ARTIFACT_CONFIG.replace("echo ran", "echo ran twice");
        std::fs::write(p.root.join(crate::store::CONFIG_FILE), other).unwrap();
        assert_eq!(revalidate(&p, &home, &repo, &task, &mut std::io::sink()).unwrap(), None, "a changed run command is runner-changed");
    }

    const GLOB_CONFIG: &str = "[runners.t]\nrun = \"cp src.bin libs/one.so && grep -q ok out.txt && echo ran >> ran.log && echo 'test result: ok. 1 passed; 0 failed;'\"\nbuild = \"cp src.bin libs/one.so\"\nartifacts = [\"libs/*.so\"]\n";

    #[test]
    fn runner_share_glob_artifacts_hash_every_match() {
        let (tmp, p, repo, task) = accepted_runner(GLOB_CONFIG, "@t");
        let home = tmp.0.join("home");
        std::fs::create_dir_all(repo.join("libs")).unwrap();
        assert!(!prove(&p, &home, &repo, &task, &mut std::io::sink()).unwrap().proved);
        std::fs::write(repo.join("out.txt"), "ok").unwrap();
        std::fs::write(repo.join("src.bin"), "A").unwrap();
        assert!(prove(&p, &home, &repo, &task, &mut std::io::sink()).unwrap().proved);
        std::fs::write(repo.join("notes.txt"), "a comment").unwrap();
        assert!(revalidate(&p, &home, &repo, &task, &mut std::io::sink()).unwrap().is_some(), "the same files match and hash the same");
        assert_eq!(runs(&repo), 1);
        std::fs::write(repo.join("notes.txt"), "another").unwrap();
        std::fs::write(repo.join("libs/two.so"), "new library").unwrap();
        assert_eq!(revalidate(&p, &home, &repo, &task, &mut std::io::sink()).unwrap(), None, "a new match changes the set");
    }

    const ALWAYS_CONFIG: &str = "[runners.t]\nrun = \"cp src.bin built.bin && grep -q ok out.txt && echo ran >> ran.log && echo 'test result: ok. 1 passed; 0 failed;'\"\nbuild = \"cp src.bin built.bin\"\nartifacts = [\"built.bin\"]\nalways = [\"test ! -e forbidden.txt\"]\n";

    #[test]
    fn runner_share_always_failure_stops_a_revalidation() {
        let (tmp, p, repo, task) = accepted_runner(ALWAYS_CONFIG, "@t");
        let home = tmp.0.join("home");
        assert!(!prove(&p, &home, &repo, &task, &mut std::io::sink()).unwrap().proved);
        std::fs::write(repo.join("out.txt"), "ok").unwrap();
        std::fs::write(repo.join("src.bin"), "A").unwrap();
        assert!(prove(&p, &home, &repo, &task, &mut std::io::sink()).unwrap().proved);
        std::fs::write(repo.join("notes.txt"), "a comment").unwrap();
        assert!(revalidate(&p, &home, &repo, &task, &mut std::io::sink()).unwrap().is_some(), "the always check passes");
        std::fs::write(repo.join("notes.txt"), "again").unwrap();
        std::fs::write(repo.join("forbidden.txt"), "x").unwrap();
        assert_eq!(revalidate(&p, &home, &repo, &task, &mut std::io::sink()).unwrap(), None, "a failed always check runs the verify");
        assert_eq!(state_of(&p, &home, &repo), "stale");
    }

    #[test]
    fn zero_tests_is_not_a_proof_and_flakes_show() {
        let (tmp, p, repo, task) = accepted();
        let home = tmp.0.join("home");
        let active_dir = std::fs::read_dir(p.root.join("active")).unwrap().next().unwrap().unwrap().path();
        let tasks = active_dir.join("tasks.md");
        let text = std::fs::read_to_string(&tasks).unwrap();
        std::fs::write(&tasks, text.replace("1 passed", "0 passed")).unwrap();
        std::fs::write(repo.join("out.txt"), "ok").unwrap();
        let r = prove(&p, &home, &repo, &task, &mut std::io::sink()).unwrap();
        assert!(r.outcome.passed());
        assert!(!r.proved, "0 tests run is not a proof");
        std::fs::write(&tasks, text).unwrap();

        let flag = tmp.0.join("flaky-flag");
        let flaky = format!("test -f {} && echo 'test result: ok. 1 passed; 0 failed;'", flag.display());
        let text = std::fs::read_to_string(&tasks).unwrap();
        let verify_line = text.lines().find(|l| l.contains("verify:")).unwrap().to_string();
        std::fs::write(&tasks, text.replace(&verify_line, &format!("  verify: {flaky}"))).unwrap();
        assert!(!prove(&p, &home, &repo, &task, &mut std::io::sink()).unwrap().proved);
        std::fs::write(&flag, "go\n").unwrap();
        assert!(prove(&p, &home, &repo, &task, &mut std::io::sink()).unwrap().proved);
        assert_eq!(state_of(&p, &home, &repo), "flaky");
    }

    /// The state of the only task when its change holds `events` instead of the ledger.
    fn state_with_events(p: &Project, home: &Path, repo: &Path, events: impl FnOnce(&[Event]) -> Vec<Event>) -> State {
        let top = git::toplevel(repo).unwrap();
        let (living, _) = change::load_living(p).unwrap();
        let mut changes = active(p, &living).unwrap();
        changes[0].events = events(&changes[0].events);
        let config = config::load(p, home).unwrap();
        let now = Now { top: &top, config: &config, env: fingerprint::env(&top, &config).unwrap(), living: &living };
        state_for(p, home, &changes[0], &changes[0].loaded.tasks[0], &now).unwrap()
    }

    #[test]
    fn run_order_newest_started_run_wins() {
        let (tmp, p, repo, task) = accepted();
        let home = tmp.0.join("home");
        std::fs::write(repo.join("out.txt"), "ok").unwrap();
        assert!(prove(&p, &home, &repo, &task, &mut std::io::sink()).unwrap().proved);
        let state = state_with_events(&p, &home, &repo, |events| {
            let pass = events.iter().rfind(|e| e.kind == Kind::Prove).unwrap();
            let mut fail = pass.clone();
            fail.data.insert("proved".into(), false.into());
            fail.data.insert("worktree".into(), "an-older-tree".into());
            fail.data.insert("started".into(), (pass.data["started"].as_u64().unwrap() - 60_000).into());
            let mut out = events.to_vec();
            out.push(fail);
            out
        });
        assert_eq!(state, State::Proved);
        let state = state_with_events(&p, &home, &repo, |events| {
            let pass = events.iter().rfind(|e| e.kind == Kind::Prove).unwrap();
            let mut fail = pass.clone();
            fail.data.insert("proved".into(), false.into());
            fail.data.insert("worktree".into(), "a-newer-tree".into());
            fail.data.insert("started".into(), (pass.data["started"].as_u64().unwrap() + 60_000).into());
            let mut out = events.to_vec();
            out.insert(out.len() - 1, fail);
            out
        });
        assert_eq!(state, State::Failed, "a fail that started last wins even when it was appended first");
    }

    #[test]
    fn run_order_fail_with_changed_inputs_is_stale() {
        let (tmp, p, repo, task) = accepted();
        let home = tmp.0.join("home");
        assert!(!prove(&p, &home, &repo, &task, &mut std::io::sink()).unwrap().proved);
        assert!(!prove(&p, &home, &repo, &task, &mut std::io::sink()).unwrap().proved);
        std::fs::write(repo.join("out.txt"), "ok").unwrap();
        assert_eq!(state_of(&p, &home, &repo), "stale");
        assert_eq!(stale_tasks(&p, &home, &repo).unwrap(), [task]);
        let s = status(&p, &home, &repo, false).unwrap();
        assert_eq!(s[0].tasks[0].state, State::Stale(vec!["inputs-changed"]));
    }

    #[test]
    fn run_order_fail_with_same_inputs_stays_failed() {
        let (tmp, p, repo, task) = accepted();
        let home = tmp.0.join("home");
        assert!(!prove(&p, &home, &repo, &task, &mut std::io::sink()).unwrap().proved);
        assert!(!prove(&p, &home, &repo, &task, &mut std::io::sink()).unwrap().proved);
        assert_eq!(state_of(&p, &home, &repo), "failed");
        assert!(stale_tasks(&p, &home, &repo).unwrap().is_empty());
    }

    fn prove_with_summary(summary: &str) -> Proved {
        prove_with_verify_line(&format!("  verify: echo '{summary}'"))
    }

    /// The same, for a `baseline: pass` guard, whose first prove may pass.
    fn prove_guard_with_summary(summary: &str) -> Proved {
        prove_with_verify_line(&format!("  verify: echo '{summary}'\n  baseline: pass"))
    }

    fn prove_with_verify_line(line: &str) -> Proved {
        let (tmp, p, repo, task) = accepted();
        let home = tmp.0.join("home");
        let active_dir = std::fs::read_dir(p.root.join("active")).unwrap().next().unwrap().unwrap().path();
        let tasks = active_dir.join("tasks.md");
        let text = std::fs::read_to_string(&tasks).unwrap();
        let verify_line = text.lines().find(|l| l.contains("verify:")).unwrap().to_string();
        std::fs::write(&tasks, text.replace(&verify_line, line)).unwrap();
        prove(&p, &home, &repo, &task, &mut std::io::sink()).unwrap()
    }

    #[test]
    fn node_zero_spec_summary_is_not_a_proof() {
        let r = prove_with_summary("\u{2139} tests 0");
        assert!(r.outcome.passed());
        assert_eq!(r.outcome.tests_run, Some(0), "the result says 0 tests ran");
        assert!(!r.proved, "a node --test run of 0 tests is not a proof");
    }

    #[test]
    fn node_zero_tap_summary_is_not_a_proof() {
        let r = prove_with_summary("# tests 0");
        assert!(r.outcome.passed());
        assert_eq!(r.outcome.tests_run, Some(0), "the result says 0 tests ran");
        assert!(!r.proved, "a node --test run of 0 tests is not a proof");
    }

    #[test]
    fn node_zero_three_tests_still_prove() {
        let r = prove_guard_with_summary("# tests 3");
        assert_eq!(r.outcome.tests_run, Some(3));
        assert!(r.proved, "{:?}", r.outcome);
    }

    #[test]
    fn repeated_failures_suggest_a_block() {
        let (tmp, p, repo, task) = accepted();
        let home = tmp.0.join("home");
        let mut last = None;
        for _ in 0..LOOP_RUNS {
            last = Some(prove(&p, &home, &repo, &task, &mut std::io::sink()).unwrap());
        }
        assert!(last.unwrap().looping);
    }
}
