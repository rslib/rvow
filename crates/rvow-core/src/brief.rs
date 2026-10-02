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

// The subtype key is `note`, since a data key `kind` would clash with the event's own `kind`.
fn note_text(change: &Active, kind: &str) -> Vec<(u64, String)> {
    change
        .events
        .iter()
        .filter(|e| {
            e.kind == Kind::Note && e.data.get("note").and_then(Value::as_str) == Some(kind)
        })
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

/// The brief for the current branch, or an empty string when the branch has no
/// active change.
pub fn brief(project: &Project, home: &Path, cwd: &Path) -> Result<String> {
    let top = git::toplevel(cwd)?;
    let (living, _) = change::load_living(project)?;
    let changes = proof::active(project, &living)?;
    let Some(branch) = git::branch(&top)? else {
        if changes.is_empty() {
            return Ok(String::new());
        }
        let ids: Vec<String> = changes.iter().map(|c| c.dir.id.to_string()).collect();
        return Ok(format!(
            "HEAD is detached; active changes: {}; check out a branch\n",
            ids.join(", ")
        ));
    };
    let queue = proof::on_branch(&changes, &branch);
    if queue.is_empty() {
        return Ok(String::new());
    }
    let config = config::load(project, home)?;
    let now = Now {
        top: &top,
        config: &config,
        env: fingerprint::env(&top, &config)?,
        living: &living,
    };

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
    let parts: Vec<String> = names
        .iter()
        .zip(counts)
        .map(|(n, c)| format!("{c} {n}"))
        .collect();
    out.push_str(&format!("tasks: {}\n", parts.join(", ")));

    let mut decisions: Vec<(u64, String)> = queue
        .iter()
        .flat_map(|c| note_text(c, "decision"))
        .collect();
    decisions.sort();
    for (_, text) in decisions.iter().rev().take(DECISIONS) {
        out.push_str(&format!("decision: {text}\n"));
    }
    let handoff = queue
        .iter()
        .flat_map(|c| note_text(c, "handoff"))
        .max_by_key(|(time, _)| *time);
    if let Some((_, text)) = handoff {
        out.push_str(&format!("handoff: {text}\n"));
    }

    match proof::next(&queue, &now)? {
        Some((c, t, _)) => {
            let full = format!("next: {}", proof::render_task(t, &c.loaded, &living));
            let all = format!("{out}{full}");
            out = if fits(&all) {
                all
            } else {
                out + &compact(t, c, &living)
            };
        }
        None => out.push_str("next: every task on this branch is proved or blocked\n"),
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::git::testrepo::*;
    use crate::ledger;
    use crate::proof::tests::accepted;
    use serde_json::{Map, json};

    fn note(p: &Project, kind: &str, text: &str) {
        let dir = std::fs::read_dir(p.root.join("active"))
            .unwrap()
            .next()
            .unwrap()
            .unwrap()
            .path();
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
        assert_eq!(
            lines[1],
            "tasks: 0 proved, 0 stale, 0 flaky, 0 failed, 0 blocked, 1 missing"
        );
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
        assert!(b.ends_with("next: every task on this branch is proved or blocked\n"));
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
    fn silent_without_a_change_and_hints_on_a_detached_head() {
        let (tmp, p, repo, _) = accepted();
        let home = tmp.0.join("home");
        git(&repo, &["checkout", "-q", "-b", "other"]);
        assert_eq!(brief(&p, &home, &repo).unwrap(), "");
        git(&repo, &["checkout", "-q", "--detach"]);
        let b = brief(&p, &home, &repo).unwrap();
        assert!(
            b.contains("CHG-") && b.contains("check out a branch"),
            "{b}"
        );
        let empty = TempDir::new();
        let repo = repo_with_commit(&empty.0, "r");
        let p = crate::store::init(&repo, &empty.0.join("home"), crate::store::Mode::Home).unwrap();
        git(&repo, &["checkout", "-q", "--detach"]);
        assert_eq!(brief(&p, &empty.0.join("home"), &repo).unwrap(), "");
    }
}
