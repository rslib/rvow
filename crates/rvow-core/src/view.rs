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
//!   "plans": [ { "id", "title" } ],
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

fn state_json(s: &State) -> (String, Vec<&'static str>) {
    match s {
        State::Stale(why) => ("stale".into(), why.clone()),
        other => (other.name().into(), Vec::new()),
    }
}

/// The view, or `None` when the branch has neither an active change nor a plan
/// waiting for accept.
pub fn view(project: &Project, home: &Path, cwd: &Path) -> Result<Option<Value>> {
    let top = git::toplevel(cwd)?;
    let branch = git::branch(&top)?;
    let (living, _) = change::load_living(project)?;
    let active = proof::active(project, &living)?;
    let queue = match &branch {
        Some(b) => proof::on_branch(&active, b),
        None => Vec::new(),
    };

    let (all, _) = change::list(project)?;
    let mut plans = Vec::new();
    let mut open_questions = Vec::new();
    for c in all.iter().filter(|c| c.stage == Stage::Backlog) {
        let loaded = change::load(&c.path, &living)?;
        if Some(&loaded.proposal.branch) != branch.as_ref() {
            continue;
        }
        for q in questions(&loaded.texts) {
            open_questions.push(json!({
                "change": c.id.to_string(),
                "question": q.question,
                "recommended": q.recommended,
            }));
        }
        plans.push(json!({"id": c.id.to_string(), "title": loaded.proposal.title}));
    }
    if queue.is_empty() && plans.is_empty() {
        return Ok(None);
    }

    let config = config::load(project, home)?;
    let now = Now {
        top: &top,
        config: &config,
        env: fingerprint::env(&top, &config)?,
        living: &living,
    };
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
        let drift: Vec<Value> = drift::drift(project, &top, &config, c)?
            .into_iter()
            .map(|i| json!({"class": i.class.name(), "what": i.what}))
            .collect();
        let pending: Vec<Value> = amend::pending(std::slice::from_ref(c))
            .into_iter()
            .map(|p| json!({"id": p.id.to_string(), "reason": p.reason}))
            .collect();
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
    Ok(Some(json!({
        "branch": branch,
        "changes": changes,
        "plans": plans,
        "questions": open_questions,
        "next": next,
        "needs_you": {
            "amendments": amendments,
            "questions": open_questions.len(),
            "blocked": blocked,
            "plans": plans.len(),
        },
    })))
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
        std::fs::write(
            &prop,
            text + "[NEEDS CLARIFICATION: Which? | recommended: this]\n",
        )
        .unwrap();
        let v = view(&p, &home, &repo).unwrap().unwrap();
        assert_eq!(v["plans"][0]["id"], plan.id.to_string().as_str());
        assert_eq!(v["questions"][0]["recommended"], "this");
        assert_eq!(v["needs_you"]["questions"], 1);

        crate::git::testrepo::git(&repo, &["checkout", "-q", "-b", "other"]);
        assert_eq!(view(&p, &home, &repo).unwrap(), None);
    }
}
