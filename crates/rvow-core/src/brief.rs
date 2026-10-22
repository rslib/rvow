//! `rvow brief`: the short plan summary an agent reads at the start of a session.

use std::path::Path;

use serde_json::Value;

use crate::change::{self, CapSpec};
use crate::config;
use crate::error::Result;
use crate::fingerprint;
use crate::git;
use crate::ledger::Kind;
use crate::proof::{self, Active, Now, State};
use crate::store::Project;
use crate::tasks::Task;

const MAX_LINES: usize = 25;
const MAX_CHARS: usize = 1500;
const DECISIONS: usize = 3;
const RULES: &str = "rules: rvow accept and confirm belong to the user; stay inside files:; use rvow block if a task cannot pass.\n";
const PLANNING_RULES: &str = "rules: only the user runs rvow accept; ask with rvow_ask; write the plan files only\n";

/// A planning brief starts with `plan `; no task exists there, so the task rules would mislead.
fn rules_for(text: &str) -> &'static str {
    if text.starts_with("plan ") { PLANNING_RULES } else { RULES }
}

fn short_line(line: &str) -> String {
    if line.len() <= 240 {
        return line.to_string();
    }
    let mut bytes = 0;
    let head: String = line
        .chars()
        .take_while(|ch| {
            bytes += ch.len_utf8();
            bytes <= 237
        })
        .collect();
    format!("{head}...")
}

/// Adds a warning without losing the brief's next step or exceeding its final budget.
pub fn with_warning(text: &str, warning: &str) -> String {
    let clean: String = warning.chars().map(|ch| if ch.is_control() { ' ' } else { ch }).collect();
    let warning = short_line(&clean);
    let combined = format!("{warning}\n{text}");
    if fits(&combined) {
        return combined;
    }
    let next = text.lines().rfind(|line| line.starts_with("next:")).map(short_line).unwrap_or_default();
    format!("{warning}\n{next}\nmore: rvow status\n{}", rules_for(text))
}

fn bounded(text: String) -> String {
    if text.is_empty() {
        return text;
    }
    let rules = rules_for(&text);
    let all = format!("{text}{rules}");
    if fits(&all) {
        return all;
    }
    let lines: Vec<&str> = text.lines().collect();
    let next = lines.iter().rposition(|line| line.starts_with("next:")).unwrap_or(lines.len());
    let end = lines[next..].iter().position(|line| line.starts_with("plan ")).map_or(lines.len(), |n| next + n);
    let mut tail = lines[next..end].join("\n");
    if !tail.is_empty() {
        tail.push('\n');
    }
    let suffix = format!("brief: truncated\nmore: rvow status\n{rules}");
    if !fits(&format!("{tail}{suffix}")) {
        tail = lines.get(next).map(|line| format!("{}\n", short_line(line))).unwrap_or_default();
        if let Some(task) = lines.get(next).and_then(|line| line.split_whitespace().nth(1)).filter(|id| id.starts_with("TSK-")) {
            tail.push_str(&format!("more: rvow show {task}\n"));
        }
    }
    let mut prefix = String::new();
    for line in &lines[..next] {
        let line = short_line(line);
        if fits(&format!("{prefix}{line}\n{tail}{suffix}")) {
            prefix.push_str(&line);
            prefix.push('\n');
        }
    }
    format!("{prefix}{tail}{suffix}")
}

// The subtype key is `note`, since a data key `kind` would clash with the event's own `kind`.
fn note_text(change: &Active, kind: &str) -> Vec<(u64, String)> {
    change
        .events
        .iter()
        .filter(|e| e.kind == Kind::Note && e.data.get("note").and_then(Value::as_str) == Some(kind))
        .filter_map(|e| Some((e.time, e.data.get("text")?.as_str()?.trim().to_string())))
        .collect()
}

fn compact(task: &Task, change: &Active, living: &[CapSpec]) -> String {
    let mut out = format!(
        "next: {} {}\n  verify: {}\n  files: {}\n",
        task.id,
        task.title,
        task.verify.as_deref().unwrap_or(""),
        task.files.join(", ")
    );
    let mut more = String::new();
    for r in &task.covers {
        match proof::find_requirement(r, &change.loaded, living) {
            Some(req) => {
                out.push_str(&format!("  {} {}\n", req.id, req.title));
                if let Some(sc) = req.scenarios.first() {
                    out.push_str(&format!("    scenario: {}\n", sc.title));
                }
                more.push_str(&format!("more: rvow path {r}\n"));
            }
            None => out.push_str(&format!("  {r}: not found\n")),
        }
    }
    out + &more
}

fn fits(text: &str) -> bool {
    text.lines().count() <= MAX_LINES && text.len() <= MAX_CHARS
}

/// The brief while the branch has only plans waiting for accept: what is
/// open and the next steps, so an agent keeps interviewing after compaction.
fn planning(project: &Project, cwd: &Path) -> Result<String> {
    let mut out = String::new();
    for r in crate::plan::check(project, cwd, None)?.into_iter().filter(|r| r.change.stage == crate::change::Stage::Backlog) {
        let id = &r.change.id;
        let mut counters = Vec::new();
        match r.questions.len() {
            0 => {}
            1 => counters.push("1 open question".to_string()),
            n => counters.push(format!("{n} open questions")),
        }
        match r.missing.len() {
            0 => {}
            1 => counters.push("1 check that cannot run".to_string()),
            n => counters.push(format!("{n} checks that cannot run")),
        }
        let counters = if counters.is_empty() { String::new() } else { format!(": {}", counters.join(", ")) };
        out.push_str(&format!("plan {id} waits for accept{counters}\n"));
        let next = if !r.questions.is_empty() {
            "next: ask the open questions with the rvow_ask tool, or ask in chat and record each answer with rvow answer <ref> \"<choice>\""
        } else if r.blocking() {
            "next: run rvow check and fix what it reports"
        } else {
            "next: ask the user to run /rvow-accept <id> (or ! rvow accept <id>); code edits wait for it"
        };
        out.push_str(&next.replace("<id>", &id.to_string()));
        out.push('\n');
    }
    Ok(out)
}

/// The task of `queue` that `agent` holds a live lease of, if any.
fn leased_task<'a>(
    project: &Project,
    home: &Path,
    cwd: &Path,
    agent: &str,
    queue: &[&'a Active],
) -> Result<Option<(&'a Active, &'a Task)>> {
    let Some(lease) = crate::lease::holder(project, home, cwd, agent, crate::ledger::now_ms())? else {
        return Ok(None);
    };
    Ok(queue
        .iter()
        .find(|c| c.dir.id.to_string() == lease.change)
        .and_then(|c| c.loaded.tasks.iter().find(|t| t.id.to_string() == lease.task).map(|t| (*c, t))))
}

/// The brief for the current branch, or an empty string when the branch has no
/// active change and no plan waiting for accept.
pub fn brief(project: &Project, home: &Path, cwd: &Path) -> Result<String> {
    brief_for(project, home, cwd, None)
}

/// The brief of `agent`'s session: the task it holds a live lease of comes
/// first in `next:`, since that is the one work it may edit.
pub fn brief_for(project: &Project, home: &Path, cwd: &Path, agent: Option<&str>) -> Result<String> {
    let top = git::toplevel(cwd)?;
    let living = change::living(project)?;
    let changes = proof::active(project, &living)?;
    let Some(branch) = git::branch(&top)? else {
        if changes.is_empty() {
            return Ok(String::new());
        }
        let ids: Vec<String> = changes.iter().map(|c| c.dir.id.to_string()).collect();
        return Ok(bounded(format!(
            "HEAD is detached; active changes: {}; check out a branch\nnext: check out a branch\n",
            ids.join(", ")
        )));
    };
    // An empty queue already means this branch has no active change, whatever
    // other branches hold. A branch with no plan gets one line, with no task
    // rules after it; a waiting plan keeps its planning brief instead.
    let queue = proof::on_branch(&changes, &branch);
    if queue.is_empty() {
        let text = planning(project, cwd)?;
        if text.is_empty() {
            return Ok(
                "rvow is here and this branch has no plan: run `rvow new \"<title>\"` to start one, `rvow guide proposal` shows how\n"
                    .to_string(),
            );
        }
        return Ok(bounded(text));
    }
    let config = config::load(project, home)?;
    let now = Now::cached(&top, &config, fingerprint::check_env(&top, &config)?, &living);

    let mut out = String::new();
    for c in &queue {
        out.push_str(&format!("{} {}\n", c.dir.id, c.loaded.proposal.title));
    }
    let mut counts = [0usize; 6];
    for c in &queue {
        for t in &c.loaded.tasks {
            let i = match proof::state(c, t, &now)? {
                State::Proved => 0,
                State::Stale(_) => 1,
                State::Flaky => 2,
                State::Failed => 3,
                State::Blocked => 4,
                State::Missing => 5,
            };
            counts[i] += 1;
        }
    }
    let names = ["proved", "stale", "flaky", "failed", "blocked", "missing"];
    let parts: Vec<String> = names.iter().zip(counts).filter(|(_, c)| *c > 0).map(|(n, c)| format!("{c} {n}")).collect();
    if parts.is_empty() {
        out.push_str("tasks: every task is proved or blocked\n");
    } else {
        out.push_str(&format!("tasks: {}\n", parts.join(", ")));
    }

    let mut decisions: Vec<(u64, String)> = queue.iter().flat_map(|c| note_text(c, "decision")).collect();
    decisions.sort();
    for (_, text) in decisions.iter().rev().take(DECISIONS) {
        out.push_str(&format!("decision: {text}\n"));
    }
    let handoff = queue.iter().flat_map(|c| note_text(c, "handoff")).max_by_key(|(time, _)| *time);
    if let Some((_, text)) = handoff {
        out.push_str(&format!("handoff: {text}\n"));
    }

    let leased = match agent {
        Some(agent) => leased_task(project, home, cwd, agent, &queue)?,
        None => None,
    };
    let next = match leased {
        Some(pair) => Some(pair),
        None => proof::next(&queue, &now)?.map(|(c, t, _)| (c, t)),
    };
    match next {
        Some((c, t)) => {
            let full = format!("next: {}", proof::render_task(t, &c.loaded, &living));
            let all = format!("{out}{full}");
            out = if fits(&all) { all } else { out + &compact(t, c, &living) };
            out.push_str(&format!(
                "loop: run `rvow prove {}` before the work to record that the check fails, and after the work to prove it; to change the plan run `rvow amend --reason` and keep working; to read the plan run `rvow show {}`\n",
                t.id, c.dir.id
            ));
        }
        None => out.push_str("next: every task on this branch is proved or blocked\n"),
    }
    Ok(bounded(out))
}

/// The brief for a subagent that holds a lease. The first line is the marker
/// that the Claude mod reads to bind the subagent.
pub fn delegate_brief(d: &crate::lease::Delegation) -> String {
    let l = &d.lease;
    let mut out = format!(
        "lease: {task}\n\
         You hold one task of {change}: {title}\n\
         Edit only: {files}\n\
         Prove it with: {verify}\n",
        task = l.task,
        change = l.change,
        title = d.title,
        files = l.files.join(", "),
        verify = d.verify,
    );
    if let Some(run) = &d.run {
        out.push_str(&format!("run: {run}\n"));
    }
    out.push_str(&format!(
        "Rules:\n\
             - The hook denies edits outside those files and names this task.\n\
             - When done run `rvow prove {task}`. If it cannot pass as written run `rvow block {task} --reason \"...\"`.\n\
             - rvow accept and confirm belong to the user.\n\
             - A terminal tool gets a private socket (`tmux -L <name>`). Never run a bare tmux or kill-server.\n",
        task = l.task,
    ));
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::git::testrepo::*;
    use crate::ledger;
    use crate::proof::tests::accepted;
    use serde_json::{Map, json};

    fn note(p: &Project, kind: &str, text: &str) {
        let dir = std::fs::read_dir(p.root.join("active")).unwrap().next().unwrap().unwrap().path();
        let mut data = Map::new();
        data.insert("note".into(), json!(kind));
        data.insert("text".into(), json!(text));
        let lock = p.lock().unwrap();
        ledger::append(&dir.join(change::LEDGER), &lock, Kind::Note, data).unwrap();
    }

    #[test]
    fn brief_lists_changes_counts_notes_and_the_next_task() {
        let (tmp, p, repo, task) = accepted();
        let home = tmp.0.join("home");
        for i in 1..=4 {
            note(&p, "decision", &format!("choice {i}"));
        }
        note(&p, "handoff", "pick up here");
        let b = brief(&p, &home, &repo).unwrap();
        let lines: Vec<&str> = b.lines().collect();
        assert!(lines[0].ends_with(" Out"), "{b}");
        assert_eq!(lines[1], "tasks: 1 missing");
        assert!(!b.contains("choice 1"));
        assert!(b.contains("decision: choice 4") && b.contains("decision: choice 2"));
        assert!(b.contains("handoff: pick up here"));
        assert!(b.contains(&format!("next: {task} Write ok")), "{b}");
        assert!(b.contains("WHEN it runs"));
        assert!(fits(&b));

        std::fs::write(repo.join("out.txt"), "ok").unwrap();
        proof::prove(&p, &home, &repo, &task, &mut std::io::sink()).unwrap();
        let b = brief(&p, &home, &repo).unwrap();
        assert!(b.contains("1 proved"), "{b}");
        assert!(b.contains("next: every task on this branch is proved or blocked\n"));
        assert!(
            b.ends_with("rules: rvow accept and confirm belong to the user; stay inside files:; use rvow block if a task cannot pass.\n")
        );
    }

    #[test]
    fn loop_line_names_prove_before_the_work() {
        let (tmp, p, repo, task) = accepted();
        let b = brief(&p, &tmp.0.join("home"), &repo).unwrap();
        let line = b.lines().find(|l| l.starts_with("loop:")).unwrap();
        assert!(line.contains(&format!("`rvow prove {task}`")) && line.contains("before the work"), "{line}");
        assert!(line.contains("rvow amend") && !line.contains("rvow_confirm"), "{line}");
        assert!(fits(&b), "{b}");
    }

    #[test]
    fn confirm_once_at_archive_loop_line_names_amend_not_confirm() {
        let (tmp, p, repo, _task) = accepted();
        let b = brief(&p, &tmp.0.join("home"), &repo).unwrap();
        let line = b.lines().find(|l| l.starts_with("loop:")).unwrap();
        assert!(line.contains("rvow amend") && !line.contains("rvow_confirm"), "{line}");
        assert!(fits(&b), "{b}");
    }

    #[test]
    fn a_long_task_gets_the_compact_form() {
        let (tmp, p, repo, task) = accepted();
        let home = tmp.0.join("home");
        note(&p, "handoff", &"line\n".repeat(20));
        let b = brief(&p, &home, &repo).unwrap();
        assert!(!b.contains("WHEN it runs"), "{b}");
        assert!(b.contains("scenario: s\n"));
        assert!(b.contains("more: rvow path REQ-"));
        assert!(b.contains(&format!("next: {task} Write ok\n  verify:")));
    }

    #[test]
    fn a_waiting_plan_gets_a_planning_brief() {
        let tmp = TempDir::new();
        let repo = repo_with_commit(&tmp.0, "r");
        let home = tmp.0.join("home");
        let p = crate::store::init(&repo, &home, crate::store::Mode::Home).unwrap();
        let c = crate::change::new(&p, "T", "main", true).unwrap();
        let prop = c.path.join("proposal.md");
        let text = std::fs::read_to_string(&prop).unwrap();
        std::fs::write(&prop, text + "[NEEDS CLARIFICATION: Which? | recommended: this]\n").unwrap();
        let b = brief(&p, &home, &repo).unwrap();
        assert!(b.contains(&format!("plan {} waits for accept: 1 open question", c.id)), "{b}");
        assert!(b.contains("rvow_ask"), "{b}");
    }

    #[test]
    fn a_branch_without_a_change_points_to_new_and_hints_on_a_detached_head() {
        let (tmp, p, repo, _) = accepted();
        let home = tmp.0.join("home");
        git(&repo, &["checkout", "-q", "-b", "other"]);
        let b = brief(&p, &home, &repo).unwrap();
        assert_eq!(b.lines().count(), 1, "{b}");
        assert!(b.contains("rvow new") && b.contains("rvow guide proposal"), "{b}");
        git(&repo, &["checkout", "-q", "--detach"]);
        let b = brief(&p, &home, &repo).unwrap();
        assert!(b.contains("CHG-") && b.contains("check out a branch"), "{b}");
        let empty = TempDir::new();
        let repo = repo_with_commit(&empty.0, "r");
        let p = crate::store::init(&repo, &empty.0.join("home"), crate::store::Mode::Home).unwrap();
        git(&repo, &["checkout", "-q", "--detach"]);
        assert_eq!(brief(&p, &empty.0.join("home"), &repo).unwrap(), "");
    }
}
