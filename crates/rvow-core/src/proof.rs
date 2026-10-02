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
}

impl Active {
    pub fn vow(&self) -> Option<&Event> {
        self.events.iter().find(|e| e.kind == Kind::Vow)
    }

    /// The snapshot that drift is measured against: the snapshot of the latest
    /// accepted amendment, else the vow.
    pub fn baseline(&self) -> Option<&Map<String, Value>> {
        let mut base = self.vow().map(|e| &e.data);
        for e in self.events.iter().filter(|e| e.kind == Kind::Confirm) {
            if e.data.get("decision").and_then(Value::as_str) != Some("accept") {
                continue;
            }
            let id = e.data.get("amendment");
            let snapshot = self
                .events
                .iter()
                .find(|a| a.kind == Kind::Amend && a.data.get("amendment") == id)
                .and_then(|a| a.data.get("snapshot"))
                .and_then(Value::as_object);
            if snapshot.is_some() {
                base = snapshot;
            }
        }
        base
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
    let mut out = Vec::new();
    for dir in changes.into_iter().filter(|c| c.stage == Stage::Active) {
        let loaded = change::load(&dir.path, living)?;
        let (events, ledger_diags) = ledger::read(&dir.path.join(LEDGER))?;
        out.push(Active {
            dir,
            loaded,
            events,
            ledger_diags,
        });
    }
    out.sort_by_key(|a| a.vow_time());
    Ok(out)
}

/// The active changes whose vow names `branch`.
pub fn on_branch<'a>(changes: &'a [Active], branch: &str) -> Vec<&'a Active> {
    changes
        .iter()
        .filter(|a| a.branch() == Some(branch))
        .collect()
}

/// The current hash of every requirement a task covers: the delta text when
/// the change adds or changes it, else the living text.
pub fn covered(task: &Task, loaded: &Loaded, living: &[CapSpec]) -> BTreeMap<String, String> {
    task.covers
        .iter()
        .map(|r| {
            let hash =
                find_requirement(r, loaded, living).map_or("missing".into(), requirement_hash);
            (r.to_string(), hash)
        })
        .collect()
}

pub fn find_requirement<'a>(
    r: &Ref,
    loaded: &'a Loaded,
    living: &'a [CapSpec],
) -> Option<&'a Requirement> {
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
        let globs: Vec<String> = task
            .files
            .iter()
            .chain(&self.config.inputs)
            .cloned()
            .collect();
        fingerprint::inputs(self.top, &globs)
    }
}

fn str_of<'a>(e: &'a Event, k: &str) -> &'a str {
    e.data.get(k).and_then(Value::as_str).unwrap_or("")
}

fn is_proved(e: &Event) -> bool {
    e.data.get("proved").and_then(Value::as_bool) == Some(true)
}

/// The state of `task` in `change`, from its latest proof or block.
pub fn state(change: &Active, task: &Task, now: &Now) -> Result<State> {
    let id = task.id.to_string();
    let mine: Vec<&Event> = change
        .events
        .iter()
        .filter(|e| matches!(e.kind, Kind::Prove | Kind::Block) && str_of(e, "task") == id)
        .collect();
    let Some(last) = mine.last() else {
        return Ok(State::Missing);
    };
    if last.kind == Kind::Block {
        // A confirmed amendment that changed the task ends the block.
        return Ok(if str_of(last, "task_hash") == task_hash(task) {
            State::Blocked
        } else {
            State::Missing
        });
    }
    if !is_proved(last) {
        return Ok(State::Failed);
    }
    let key = str_of(last, "key");
    let flaky = mine
        .iter()
        .any(|e| e.kind == Kind::Prove && str_of(e, "key") == key && !is_proved(e));
    let mut reasons = Vec::new();
    if str_of(last, "task_hash") != task_hash(task) {
        reasons.push("task-changed");
    }
    let reqs: BTreeMap<String, String> = last
        .data
        .get("requirements")
        .and_then(|v| serde_json::from_value(v.clone()).ok())
        .unwrap_or_default();
    if reqs != covered(task, &change.loaded, now.living) {
        reasons.push("requirement-changed");
    }
    if str_of(last, "inputs") != now.inputs(task)?.hash {
        reasons.push("inputs-changed");
    }
    if str_of(last, "env_key") != now.env.key() {
        reasons.push("env-changed");
    }
    Ok(if !reasons.is_empty() {
        State::Stale(reasons)
    } else if flaky {
        State::Flaky
    } else {
        State::Proved
    })
}

/// The first task on the branch that still needs work, in queue order.
pub fn next<'a>(queue: &[&'a Active], now: &Now) -> Result<Option<(&'a Active, &'a Task, State)>> {
    for change in queue {
        for task in &change.loaded.tasks {
            let s = state(change, task, now)?;
            if s.open() {
                return Ok(Some((change, task, s)));
            }
        }
    }
    Ok(None)
}

/// A task in full: id, title, command, scope and the text of what it covers.
pub fn render_task(task: &Task, loaded: &Loaded, living: &[CapSpec]) -> String {
    let mut out = format!(
        "{} {}\n  verify: {}\n  files: {}\n",
        task.id,
        task.title,
        task.verify.as_deref().unwrap_or(""),
        task.files.join(", ")
    );
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

fn invocation_id() -> String {
    let mut b = [0u8; 8];
    getrandom::fill(&mut b).expect("operating system random source");
    b.iter().map(|x| format!("{x:02x}")).collect()
}

/// The machine as recorded: a short hash of the hostname and the user's name
/// for it, never the hostname itself.
pub fn machine(config: &Config) -> Value {
    json!({
        "host": &sha256_hex(hostname().as_bytes())[..12],
        "name": config.machine,
    })
}

pub struct Proved {
    pub change: Id,
    pub task: Ref,
    pub outcome: Outcome,
    pub proved: bool,
    pub event: String,
    /// The same failure repeated, so a block is worth considering.
    pub looping: bool,
    pub lfs_pointers: Vec<String>,
    pub next: Option<String>,
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
    let input = if input.contains('-') {
        input.to_string()
    } else {
        format!("TSK-{input}")
    };
    let id = id::resolve(&input, &ids).map_err(|e| {
        Error::new(
            "task",
            format!("{input}: {e}"),
            "run rvow status to see the tasks, or rvow ids to name new ones",
        )
    })?;
    let r = Ref::Id(id);
    changes
        .iter()
        .find_map(|c| c.loaded.tasks.iter().find(|t| t.id == r).map(|t| (c, t)))
        .ok_or_else(|| Error::new("task", "task vanished", "run rvow status"))
}

/// Runs a task's stored verify command and records the proof.
pub fn prove(
    project: &Project,
    home: &Path,
    cwd: &Path,
    input: &str,
    echo: &mut dyn Write,
) -> Result<Proved> {
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
    let now = Now {
        top: &top,
        config: &config,
        env: fingerprint::env(&top, &config)?,
        living: &living,
    };
    let inputs = now.inputs(task)?;
    let cmd = task.verify.as_deref().expect("validated tasks have verify");
    let started = ledger::now_ms();
    let outcome = runner::run(
        cmd,
        &top,
        &[],
        task.timeout.unwrap_or(config.timeout()),
        echo,
    )?;
    let proved = outcome.passed() && outcome.tests_run != Some(0);

    let thash = task_hash(task);
    let reqs = covered(task, &change.loaded, &living);
    let env_key = now.env.key();
    let key = sha256_hex(
        json!([thash, reqs, inputs.hash, env_key])
            .to_string()
            .as_bytes(),
    );
    let mut data = Map::new();
    data.insert("task".into(), task.id.to_string().into());
    data.insert("task_hash".into(), thash.into());
    data.insert("requirements".into(), json!(reqs));
    data.insert("verify".into(), cmd.into());
    data.insert("proved".into(), proved.into());
    data.insert("exit".into(), json!(outcome.exit));
    data.insert("timed_out".into(), outcome.timed_out.into());
    data.insert("tests_run".into(), json!(outcome.tests_run));
    data.insert("started".into(), started.into());
    data.insert("duration_ms".into(), outcome.duration_ms.into());
    data.insert("tail".into(), json!(outcome.tail));
    data.insert("inputs".into(), inputs.hash.clone().into());
    data.insert("input_files".into(), inputs.files.into());
    data.insert("env".into(), json!(now.env));
    data.insert("env_key".into(), env_key.into());
    data.insert("key".into(), key.into());
    data.insert(
        "run".into(),
        json!({
            "invocation": invocation_id(),
            "rvow": env!("CARGO_PKG_VERSION"),
            "machine": machine(&config),
            "agent": std::env::var("RVOW_AGENT").unwrap_or_else(|_| "unknown".into()),
            "head": git::run(&top, &["rev-parse", "HEAD"]).unwrap_or_default(),
            "branch": git::branch(&top).ok().flatten(),
        }),
    );
    let ledger_path = change.dir.path.join(LEDGER);
    let event = {
        let lock = project.lock()?;
        ledger::append(&ledger_path, &lock, Kind::Prove, data)?
    };

    let (events, _) = ledger::read(&ledger_path)?;
    let id = task.id.to_string();
    let runs: Vec<&Event> = events
        .iter()
        .filter(|e| e.kind == Kind::Prove && str_of(e, "task") == id)
        .collect();
    let looping = runs.len() >= LOOP_RUNS
        && runs[runs.len() - LOOP_RUNS..]
            .iter()
            .all(|e| !is_proved(e) && e.data.get("tail") == runs[runs.len() - 1].data.get("tail"));

    let changes = active(project, &living)?;
    let branch = change_branch(&changes, &change.dir.id);
    let queue = on_branch(&changes, &branch);
    let next = next(&queue, &now)?
        .map(|(c, t, s)| format!("[{}] {}", s.name(), render_task(t, &c.loaded, &living)));
    Ok(Proved {
        change: change.dir.id.clone(),
        task: task.id.clone(),
        outcome,
        proved,
        event: event.hash,
        looping,
        lfs_pointers: inputs.lfs_pointers,
        next,
    })
}

fn change_branch(changes: &[Active], id: &Id) -> String {
    changes
        .iter()
        .find(|c| &c.dir.id == id)
        .and_then(|c| c.branch())
        .unwrap_or("")
        .to_string()
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
    let now = Now {
        top: &top,
        config: &config,
        env: fingerprint::env(&top, &config)?,
        living: &living,
    };
    let branch = git::branch(&top)?;
    let mut out = Vec::new();
    for c in &changes {
        if !all && c.branch() != branch.as_deref() {
            continue;
        }
        let mut problems: Vec<String> = c
            .loaded
            .errors()
            .map(|(p, d)| format!("{p}:{}: {}", d.line, d.message))
            .collect();
        problems.extend(
            c.ledger_diags
                .iter()
                .map(|d| format!("{LEDGER}:{}: {}", d.line, d.message)),
        );
        if c.vow().is_none() {
            problems.push("no vow event, the change was moved by hand".into());
        }
        let mut tasks = Vec::new();
        for t in &c.loaded.tasks {
            tasks.push(TaskLine {
                task: t.id.clone(),
                title: t.title.clone(),
                state: state(c, t, &now)?,
            });
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
    let now = Now {
        top: &top,
        config: &config,
        env: fingerprint::env(&top, &config)?,
        living: &living,
    };
    let branch = git::branch(&top)?.unwrap_or_default();
    let mut out = Vec::new();
    for c in on_branch(&changes, &branch) {
        for t in &c.loaded.tasks {
            if matches!(state(c, t, &now)?, State::Stale(_)) {
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
        let task = vowed
            .minted
            .iter()
            .find(|(from, _)| from == "TSK-new1")
            .unwrap()
            .1
            .to_string();
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

    #[test]
    fn zero_tests_is_not_a_proof_and_flakes_show() {
        let (tmp, p, repo, task) = accepted();
        let home = tmp.0.join("home");
        let active_dir = std::fs::read_dir(p.root.join("active"))
            .unwrap()
            .next()
            .unwrap()
            .unwrap()
            .path();
        let tasks = active_dir.join("tasks.md");
        let text = std::fs::read_to_string(&tasks).unwrap();
        std::fs::write(&tasks, text.replace("1 passed", "0 passed")).unwrap();
        std::fs::write(repo.join("out.txt"), "ok").unwrap();
        let r = prove(&p, &home, &repo, &task, &mut std::io::sink()).unwrap();
        assert!(r.outcome.passed());
        assert!(!r.proved, "0 tests run is not a proof");
        std::fs::write(&tasks, text).unwrap();

        let flaky = "f=$(cat n 2>/dev/null || echo 0); echo $((f+1)) > n; [ $f -ne 0 ] && echo 'test result: ok. 1 passed; 0 failed;'";
        let text = std::fs::read_to_string(&tasks).unwrap();
        let verify_line = text
            .lines()
            .find(|l| l.contains("verify:"))
            .unwrap()
            .to_string();
        std::fs::write(
            &tasks,
            text.replace(&verify_line, &format!("  verify: {flaky}")),
        )
        .unwrap();
        assert!(
            !prove(&p, &home, &repo, &task, &mut std::io::sink())
                .unwrap()
                .proved
        );
        assert!(
            prove(&p, &home, &repo, &task, &mut std::io::sink())
                .unwrap()
                .proved
        );
        assert_eq!(state_of(&p, &home, &repo), "flaky");
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
