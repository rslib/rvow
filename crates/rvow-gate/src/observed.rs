//! The proof of a run the agent made itself: when a Bash call runs the exact
//! verify of a relaxed task, its passing output with at least one test is
//! sealed as a proof of that task. A strict task never gets one.

use std::path::{Path, PathBuf};

use serde_json::{Value, json};

use crate::change::{self, LEDGER, TASKS};
use crate::config;
use crate::error::{Error, Result};
use crate::fingerprint;
use crate::git;
use crate::hardening;
use crate::ledger::{self, Kind};
use crate::proof::{self, Now};
use crate::runner::{self, Outcome, TAIL_LINES};
use crate::store::Project;
use crate::tasks::{Mode, Task};

/// The harness tool whose calls can prove a task: the shell that the Claude
/// mod reports a command, an exit and an output tail for.
const TOOL: &str = "Bash";
/// The folder under `hook-state/` that holds one pending run per call.
const PENDING_DIR: &str = "hook-state/observe";
/// The longest tool call id that names a pending file.
const ID_MAX: usize = 128;

/// Records the pending run of an allowed Bash call: its command is the verify
/// of a relaxed task of an active change on the branch, so a passing run of it
/// is that task's proof. A command that no relaxed task verifies costs one read
/// of each active `tasks.md`, no hash and no file.
pub fn pre(project: &Project, home: Option<&Path>, input: &Value) -> Result<()> {
    let command = input.get("tool_input").and_then(|t| t.get("command")).and_then(Value::as_str).map(str::trim).unwrap_or("");
    let Some(call) = text(input, "tool_use_id") else { return Ok(()) };
    if command.is_empty() || text(input, "tool_name") != Some(TOOL) {
        return Ok(());
    }
    // The cheap gate: a command that no active `tasks.md` holds is not a
    // verify, so it never parses a change, hashes a file or writes a file.
    if !held(project, command)? {
        return Ok(());
    }
    let Some(home) = home else { return Ok(()) };
    let cwd = PathBuf::from(text(input, "cwd").unwrap_or("."));
    let top = git::toplevel(&cwd)?;
    let Some(branch) = git::branch(&top)? else { return Ok(()) };
    let living = change::living(project)?;
    let changes = proof::active(project, &living)?;
    for change in proof::on_branch(&changes, &branch) {
        for task in &change.loaded.tasks {
            if !observable(task, command) || hardening::task_policy_digest(project, home, task)?.is_some() {
                continue;
            }
            let config = config::load(project, home)?;
            let now = Now::new(&top, &config, fingerprint::env(&top, &config)?, &living);
            let inputs = now.inputs(task)?;
            let pending = json!({
                "task": task.id.to_string(),
                "change": change.dir.id.to_string(),
                "inputs": inputs.hash,
                "started": ledger::now_ms(),
            });
            return write(project, call, &pending);
        }
    }
    Ok(())
}

/// Records the observed proof of a call that [`pre`] left a pending run for. The
/// pending file goes either way, and the proof is recorded only when the run
/// exited 0, no file of the task changed during it, and the output reports at
/// least one test.
pub fn post(project: &Project, input: &Value) -> Result<()> {
    let Some(call) = text(input, "tool_use_id") else { return Ok(()) };
    let Some(path) = pending_path(project, call) else { return Ok(()) };
    let Ok(recorded) = std::fs::read_to_string(&path) else { return Ok(()) };
    let _ = std::fs::remove_file(&path);
    let Ok(pending) = serde_json::from_str::<Value>(&recorded) else { return Ok(()) };
    if input.get("exit_ok").and_then(Value::as_bool) != Some(true) {
        return Ok(());
    }
    let Some(tail) = text(input, "output_tail") else { return Ok(()) };
    let Some(tests) = runner::tests_in(tail).filter(|n| *n >= 1) else { return Ok(()) };
    let (Some(task_id), Some(change_id), Some(recorded_inputs)) =
        (text(&pending, "task"), text(&pending, "change"), text(&pending, "inputs"))
    else {
        return Ok(());
    };
    let home = crate::store::home_dir()?;
    let cwd = PathBuf::from(text(input, "cwd").unwrap_or("."));
    let top = git::toplevel(&cwd)?;
    let (living, _) = change::load_living(project)?;
    let changes = proof::active(project, &living)?;
    let Some(change) = changes.iter().find(|c| c.dir.id.to_string() == change_id) else { return Ok(()) };
    let Some(task) = change.loaded.tasks.iter().find(|t| t.id.to_string() == task_id) else { return Ok(()) };
    // A strict task never gets an observed proof, whatever a pending file says.
    if task.mode != Mode::Relaxed {
        return Ok(());
    }
    let config = config::load(project, &home)?;
    let now = Now::new(&top, &config, fingerprint::env(&top, &config)?, &living);
    let inputs = now.inputs(task)?;
    // The run may have read files that changed under it, so its result stands
    // for neither the old content nor the new one.
    if inputs.hash != recorded_inputs {
        return Ok(());
    }
    let started = pending.get("started").and_then(Value::as_u64).unwrap_or_else(ledger::now_ms);
    let outcome = Outcome {
        exit: Some(0),
        timed_out: false,
        tests_run: Some(tests),
        duration_ms: ledger::now_ms().saturating_sub(started),
        tail: tail_lines(tail, &home, &top),
    };
    let run = proof::Run {
        project,
        change,
        task,
        config: &config,
        now: &now,
        inputs: &inputs,
        cmd: task.verify.as_deref().unwrap_or("").trim(),
        // A run rvow can observe names no runner, leaves no artifact and holds no private check.
        runner: None,
        policy: None,
        tree: fingerprint::worktree(&top, project)?,
        plan: None,
        started,
    };
    let mut data = proof::run_data(&run, &outcome, true)?;
    data.insert("observed".into(), true.into());
    {
        let lock = project.lock()?;
        let key = crate::seal::key(&home)?;
        ledger::append_sealed(&change.dir.path.join(LEDGER), &lock, Kind::Prove, data, Some(&key))?;
    }
    crate::lease::end_task(project, task_id)
}

/// The last lines of an observed run, masked like the tail of a run rvow made.
fn tail_lines(tail: &str, home: &Path, top: &Path) -> Vec<String> {
    let lines: Vec<&str> = tail.lines().collect();
    let keep = &lines[lines.len().saturating_sub(TAIL_LINES)..];
    let (Some(home), Some(top)) = (home.to_str(), top.to_str()) else { return keep.iter().map(|line| (*line).to_string()).collect() };
    runner::mask(&keep.join("\n"), Some(home), top).lines().map(String::from).collect()
}

/// Whether `command` is the verify of `task`, a relaxed task whose run rvow can
/// read for itself: no mutants, and no runner, which would fill the command and
/// bring its own JUnit report, build and artifacts. A private check is read
/// from the store, so [`pre`] refuses a task that has one.
fn observable(task: &Task, command: &str) -> bool {
    task.mode == Mode::Relaxed
        && task.mutants.is_none()
        && crate::template::reference(task.verify.as_deref().unwrap_or("")).is_none()
        && task.verify.as_deref().is_some_and(|verify| verify.trim() == command)
}

/// Whether an active change's `tasks.md` holds `command`, so a command that no
/// task verifies costs one read per active change and no parse.
fn held(project: &Project, command: &str) -> Result<bool> {
    // A short command such as `ls` appears in most plan text; only a whole verify line counts.
    let needle = format!("verify: {command}");
    let needle = needle.as_bytes();
    let (changes, _) = change::list(project)?;
    for dir in changes.iter().filter(|c| c.stage == change::Stage::Active) {
        let Ok(text) = std::fs::read(dir.path.join(TASKS)) else { continue };
        if text.windows(needle.len()).any(|window| window == needle) {
            return Ok(true);
        }
    }
    Ok(false)
}

/// The pending file of one call, or `None` for an id that is not a plain name.
fn pending_path(project: &Project, call: &str) -> Option<PathBuf> {
    let plain = !call.is_empty() && call.len() <= ID_MAX && call.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_');
    if !plain {
        return None;
    }
    let mut name = String::with_capacity(call.len() + 5);
    name.push_str(call);
    name.push_str(".json");
    Some(project.root.join(PENDING_DIR).join(name))
}

/// Writes the pending run of `call` over any it had before.
fn write(project: &Project, call: &str, pending: &Value) -> Result<()> {
    let Some(path) = pending_path(project, call) else { return Ok(()) };
    if let Some(dir) = path.parent()
        && let Err(e) = std::fs::create_dir_all(dir)
    {
        return Err(Error::io(dir, e));
    }
    std::fs::write(&path, pending.to_string()).map_err(|e| Error::io(&path, e))
}

/// A string field of `input`.
fn text<'a>(input: &'a Value, key: &str) -> Option<&'a str> {
    input.get(key).and_then(Value::as_str).filter(|s| !s.is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::{Meta, Mode as StoreMode};

    #[test]
    fn observed_proof_reads_only_a_plain_relaxed_verify() {
        let (tasks, diags) = crate::tasks::parse_tasks(
            "- TSK-new1 Plain\n  verify: test -f a.txt\n  files: a.txt\n  mode: relaxed\n\
             - TSK-new2 Runner\n  verify: @cargo pkg=a\n  files: a.txt\n  mode: relaxed\n\
             - TSK-new3 Mutants\n  verify: test -f a.txt\n  files: a.txt\n  mode: relaxed\n  mutants: cargo mutants\n\
             - TSK-new4 Strict\n  verify: test -f a.txt\n  files: a.txt\n",
        );
        assert!(diags.is_empty(), "{diags:?}");
        let want = [true, false, false, false];
        for (task, want) in tasks.iter().zip(want) {
            assert_eq!(observable(task, "test -f a.txt"), want, "{}", task.id);
        }
        assert!(!observable(&tasks[0], "test -f b.txt"), "another command is not the verify");
    }

    #[test]
    fn observed_proof_pending_path_takes_plain_ids_only() {
        let dir = std::path::PathBuf::from("/tmp/rvow");
        let project = Project { root: dir.clone(), meta: Meta { roots: vec![], remote: None, mode: StoreMode::Home } };
        assert_eq!(pending_path(&project, "tool-1"), Some(dir.join(PENDING_DIR).join("tool-1.json")));
        for bad in ["", "../x", "a/b", "a b", "\u{e9}"] {
            assert_eq!(pending_path(&project, bad), None, "{bad:?}");
        }
    }
}
