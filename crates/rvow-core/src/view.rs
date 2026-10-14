//! One structured view of the branch's rvow state for the harness UIs. The
//! UIs draw it and hold no rules of their own.
//!
//! Shape (`rvow hook claude view`, `rvow status --json`):
//! ```text
//! { "branch": "main" | null,
//!   "changes": [ { "id", "title", "stage", "tasks": [ { "id", "title", "state",
//!                  "reasons": [..] } ],
//!                  "drift": [ { "class", "what" } ],
//!                  "pending": [ { "id", "reason" } ] } ],
//!   "plans": [ { "id", "title", "tasks", "questions", "issues", "ready", "path",
//!                 "task_timeouts_secs": [..], "default_timeout_secs" } ],
//!   "backlog": [ { ...plan fields, "branch", "here" } ],  (every plan, this branch first)
//!   "archive": [ { "id", "title", "outcome", "date" } ],  (newest first, at most 10)
//!   "questions": [ { "change", "question", "recommended" } ],
//!   "next": { "change", "task", "title", "verify", "state" } | null,
//!   "needs_you": { "amendments", "questions", "blocked", "plans" } }
//! ```

use std::path::Path;

use serde_json::{Value, json};

use crate::amend;
use crate::change::{self, Stage};
use crate::config;
use crate::drift;
use crate::error::Result;
use crate::fingerprint;
use crate::git;
use crate::lint::questions;
use crate::proof::{self, Now, State};
use crate::store::Project;

/// How many archived changes the view lists.
pub const ARCHIVE_SHOWN: usize = 10;

fn state_json(s: &State) -> (String, Vec<&'static str>) {
    match s {
        State::Stale(why) => ("stale".into(), why.clone()),
        other => (other.name().into(), Vec::new()),
    }
}

/// The shared view, or `None` when the project has no visible state.
pub fn view(project: &Project, home: &Path, cwd: &Path) -> Result<Option<Value>> {
    let state = dashboard(project, home, cwd)?;
    if ["changes", "backlog", "archive", "specs"].iter().all(|key| state[key].as_array().is_none_or(Vec::is_empty)) {
        Ok(None)
    } else {
        Ok(Some(state))
    }
}

/// The shared dashboard, including explicit empty lists and living specs.
pub fn dashboard(project: &Project, home: &Path, cwd: &Path) -> Result<Value> {
    let top = git::toplevel(cwd)?;
    let branch = git::branch(&top)?;
    let config = config::load(project, home)?;
    let (living, _) = change::load_living(project)?;
    let active = proof::active(project, &living)?;
    let queue = match &branch {
        Some(b) => proof::on_branch(&active, b),
        None => Vec::new(),
    };

    let (all, _) = change::list(project)?;
    let mut plans = Vec::new();
    let mut backlog = Vec::new();
    let mut open_questions = Vec::new();
    for c in all.iter().filter(|c| c.stage == Stage::Backlog) {
        let loaded = change::load(&c.path, &living)?;
        let here = Some(&loaded.proposal.branch) == branch.as_ref();
        let asked = questions(&loaded.texts);
        if here {
            for q in &asked {
                open_questions.push(json!({
                    "change": c.id.to_string(),
                    "question": q.question,
                    "recommended": q.recommended,
                }));
            }
        }
        let runners = config::runners(project).unwrap_or_default();
        let filled = crate::plan::filled_commands(&runners, &loaded.tasks);
        let missing = crate::plan::blamed_programs(&top, &loaded.tasks, &runners, &filled).len();
        let runner_errors = crate::lint::runner_checks(&runners, &loaded.tasks)
            .iter()
            .filter(|(_, d)| d.severity() == crate::diag::Severity::Error)
            .count();
        let errors = loaded.errors().count() + runner_errors + crate::plan::proposal_blanks(&loaded).len();
        let entry = json!({
            "id": c.id.to_string(),
            "title": loaded.proposal.title,
            "branch": loaded.proposal.branch,
            "here": here,
            "tasks": loaded.tasks.len(),
            "questions": asked.len(),
            "issues": errors + missing,
            "ready": asked.is_empty() && errors + missing == 0 && !loaded.tasks.is_empty(),
            "path": c.path.display().to_string(),
            // The accept budget of the plan: each task's timeout, 0 when it
            // uses the configured default, so a harness can size its accept run.
            "task_timeouts_secs": loaded
                .tasks
                .iter()
                .map(|t| t.timeout.map(|d| d.as_secs()).unwrap_or(0))
                .collect::<Vec<u64>>(),
            "default_timeout_secs": config.timeout_secs,
        });
        if here {
            plans.push(entry.clone());
        }
        backlog.push((!here, entry));
    }
    backlog.sort_by_key(|(other, _)| *other);
    let backlog: Vec<Value> = backlog.into_iter().map(|(_, v)| v).collect();
    let mut archive: Vec<(String, Value)> = Vec::new();
    for c in all.iter().filter(|c| matches!(c.stage, Stage::Done | Stage::Dropped | Stage::Superseded)) {
        let name = c.path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        let date = name.get(..10).filter(|d| d.as_bytes().get(4) == Some(&b'-')).unwrap_or("").to_string();
        let title =
            std::fs::read_to_string(c.path.join(change::PROPOSAL)).map(|t| crate::proposal::parse_proposal(&t).0.title).unwrap_or_default();
        archive
            .push((format!("{date} {}", c.id), json!({"id": c.id.to_string(), "title": title, "outcome": c.stage.name(), "date": date})));
    }
    archive.sort_by(|a, b| b.0.cmp(&a.0));
    let archive: Vec<Value> = archive.into_iter().take(ARCHIVE_SHOWN).map(|(_, v)| v).collect();
    let specs: Vec<Value> = living
        .iter()
        .flat_map(|(cap, spec)| {
            spec.requirements
                .iter()
                .map(move |req| json!({"id": req.id.to_string(), "capability": cap, "title": req.title, "deprecated": req.deprecated}))
        })
        .collect();

    let now = Now { top: &top, config: &config, env: fingerprint::env(&top, &config)?, living: &living };
    let mut changes = Vec::new();
    let mut blocked = 0;
    let mut amendments = 0;
    for c in &queue {
        let mut tasks = Vec::new();
        for t in &c.loaded.tasks {
            let s = proof::state(c, t, &now)?;
            if s == State::Blocked {
                blocked += 1;
            }
            let (name, reasons) = state_json(&s);
            tasks.push(json!({
                "id": t.id.to_string(),
                "title": t.title,
                "state": name,
                "reasons": reasons,
            }));
        }
        let drift: Vec<Value> = drift::drift_with_home(project, Some(home), &top, &config, c)?
            .into_iter()
            .map(|i| json!({"class": i.class.name(), "what": i.what}))
            .collect();
        let pending: Vec<Value> =
            amend::pending(std::slice::from_ref(c)).into_iter().map(|p| json!({"id": p.id.to_string(), "reason": p.reason})).collect();
        amendments += pending.len();
        changes.push(json!({
            "id": c.dir.id.to_string(),
            "title": c.loaded.proposal.title,
            "stage": c.dir.stage.name(),
            "tasks": tasks,
            "drift": drift,
            "pending": pending,
        }));
    }
    let next = proof::next(&queue, &now)?.map(|(c, t, s)| {
        json!({
            "change": c.dir.id.to_string(),
            "task": t.id.to_string(),
            "title": t.title,
            "verify": t.verify,
            "state": state_json(&s).0,
        })
    });
    Ok(json!({
        "branch": branch,
        "specs": specs,
        "changes": changes,
        "plans": plans,
        "backlog": backlog,
        "archive": archive,
        "questions": open_questions,
        "next": next,
        "needs_you": {
            "amendments": amendments,
            "questions": open_questions.len(),
            "blocked": blocked,
            "plans": plans.len(),
        },
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::proof::tests::accepted;

    #[test]
    fn view_of_an_active_change_and_a_waiting_plan() {
        let (tmp, p, repo, task) = accepted();
        let home = tmp.0.join("home");
        let v = view(&p, &home, &repo).unwrap().unwrap();
        assert_eq!(v["changes"][0]["tasks"][0]["id"], task.as_str());
        assert_eq!(v["changes"][0]["tasks"][0]["state"], "missing");
        assert_eq!(v["next"]["task"], task.as_str());
        assert_eq!(v["needs_you"]["plans"], 0);

        let plan = change::new(&p, "Later", "main", true).unwrap();
        let prop = plan.path.join("proposal.md");
        let text = std::fs::read_to_string(&prop).unwrap();
        std::fs::write(&prop, text + "[NEEDS CLARIFICATION: Which? | recommended: this]\n").unwrap();
        let v = view(&p, &home, &repo).unwrap().unwrap();
        assert_eq!(v["plans"][0]["id"], plan.id.to_string().as_str());
        assert_eq!(v["plans"][0]["questions"], 1);
        assert_eq!(v["plans"][0]["tasks"], 0);
        assert_eq!(v["plans"][0]["ready"], false);
        assert!(v["plans"][0]["path"].as_str().unwrap().ends_with("-later"));
        assert_eq!(v["archive"].as_array().unwrap().len(), 0);
        assert_eq!(v["questions"][0]["recommended"], "this");
        assert_eq!(v["needs_you"]["questions"], 1);

        crate::git::testrepo::git(&repo, &["checkout", "-q", "-b", "other"]);
        let v = view(&p, &home, &repo).unwrap().unwrap();
        assert_eq!(v["plans"].as_array().unwrap().len(), 0);
        assert_eq!(v["backlog"][0]["branch"], "main");
        assert_eq!(v["backlog"][0]["here"], false);

        crate::archive::drop_change(&p, &plan.id.to_string(), "not needed").unwrap();
        let v = view(&p, &home, &repo).unwrap().unwrap();
        assert_eq!(v["archive"][0]["outcome"], "dropped");
        assert_eq!(v["archive"][0]["title"], "Later");
    }

    #[test]
    fn view_carries_the_accept_budget_of_each_plan() {
        let (tmp, p, repo, _task) = accepted();
        let home = tmp.0.join("home");
        let plan = change::new(&p, "Budget", "main", true).unwrap();
        std::fs::write(
            plan.path.join("tasks.md"),
            "- TSK-new1 One\n  verify: true\n  files: a\n  timeout: 20m\n- TSK-new2 Two\n  verify: true\n  files: a\n- TSK-new3 Three\n  verify: true\n  files: a\n  timeout: 90s\n",
        )
        .unwrap();
        let v = view(&p, &home, &repo).unwrap().unwrap();
        assert_eq!(v["plans"][0]["task_timeouts_secs"], json!([1200, 0, 90]));
        assert_eq!(v["plans"][0]["default_timeout_secs"], json!(600));
    }

    #[test]
    fn dashboard_counts_an_unknown_runner_once() {
        let tmp = crate::git::testrepo::TempDir::new();
        let home = tmp.0.join("home");
        let repo = crate::git::testrepo::repo_with_commit(&tmp.0, "r");
        let p = crate::store::init(&repo, &home, crate::store::Mode::Home).unwrap();
        let plan = change::new(&p, "Runner", "main", true).unwrap();
        std::fs::write(
            plan.path.join("proposal.md"),
            "branch: main\n\n# Runner\n\n## Why\nw\n\n## What changes\nw\n\n## Out of scope\nNone.\n",
        )
        .unwrap();
        std::fs::write(plan.path.join("tasks.md"), "- TSK-new1 One\n  verify: @nope\n  files: a\n").unwrap();
        let v = view(&p, &home, &repo).unwrap().unwrap();
        assert_eq!(v["plans"][0]["issues"], 1);
        assert_eq!(v["plans"][0]["ready"], false);
    }

    #[test]
    fn dashboard_counts_a_blank_proposal_section() {
        let tmp = crate::git::testrepo::TempDir::new();
        let home = tmp.0.join("home");
        let repo = crate::git::testrepo::repo_with_commit(&tmp.0, "r");
        let p = crate::store::init(&repo, &home, crate::store::Mode::Home).unwrap();
        let plan = change::new(&p, "Blank", "main", true).unwrap();
        std::fs::write(
            plan.path.join("proposal.md"),
            "branch: main\n\n# Blank\n\n## Why\n\n## What changes\nw\n\n## Out of scope\nNone.\n",
        )
        .unwrap();
        std::fs::write(plan.path.join("tasks.md"), "- TSK-new1 One\n  verify: cargo test one\n  files: a\n").unwrap();
        let v = view(&p, &home, &repo).unwrap().unwrap();
        assert_eq!(v["plans"][0]["issues"], 1);
        assert_eq!(v["plans"][0]["ready"], false);
    }
}
