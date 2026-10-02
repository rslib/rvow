use std::path::{Path, PathBuf};

use rvow_core::archive;
use rvow_core::brief;
use rvow_core::change;
use rvow_core::error::Result;
use rvow_core::guide;
use rvow_core::proof::{self, State};
use rvow_core::show;
use rvow_core::store;
use rvow_core::view;

use super::{branch_label, cwd, project};
use crate::output::Reply;
use crate::toon::Node;
use crate::{Artifact, DESCRIPTION};

/// The path with the home directory written as `~`.
pub fn tilde(path: &Path) -> String {
    let home = std::env::var_os("HOME").map(PathBuf::from);
    match home.as_deref().and_then(|h| path.strip_prefix(h).ok()) {
        Some(rest) => Path::new("~").join(rest).display().to_string(),
        None => path.display().to_string(),
    }
}

pub fn home_view() -> Result<Reply> {
    let mut r = Reply::new();
    let bin = std::env::current_exe().map_or_else(|_| "rvow".to_owned(), |p| tilde(&p));
    r.set("bin", bin.as_str());
    r.set("description", DESCRIPTION);
    r.line(format!("rvow: {DESCRIPTION}"));
    r.line(format!("bin: {bin}"));
    let home = store::home_dir()?;
    let cwd = cwd()?;
    let Ok(Some(p)) = store::find(&cwd, &home) else {
        r.set("state", "no rvow project in this repository");
        r.line("no rvow project in this repository; run rvow init (it writes to the rvow home, not the repository)");
        r.help("Run `rvow init` to create this repository's rvow tree in the rvow home; the repository stays untouched");
        r.help("Run `rvow new \"<title>\"` after init to plan a change");
        return Ok(r);
    };
    let Some(v) = view::view(&p, &home, &cwd)? else {
        let b = branch_label();
        r.set("changes", format!("0 active or waiting on branch {b}"));
        r.line(format!("no active change or plan on {b}"));
        r.help("Run `rvow new \"<title>\"` to plan a change");
        r.help("Run `rvow ls` to list every change");
        return Ok(r);
    };
    r.set("branch", text(&v["branch"]));
    r.line(format!("branch {}", text(&v["branch"])));
    home_changes(&mut r, &v);
    home_plans(&mut r, &v);
    home_next(&mut r, &v);
    r.help("Run `rvow status` for drift and pending amendments");
    Ok(r)
}

fn text(v: &serde_json::Value) -> String {
    v.as_str().unwrap_or_default().to_owned()
}

fn list(v: &serde_json::Value) -> Vec<serde_json::Value> {
    v.as_array().cloned().unwrap_or_default()
}

fn home_changes(r: &mut Reply, v: &serde_json::Value) {
    let mut rows = Vec::new();
    for c in list(&v["changes"]) {
        let tasks = list(&c["tasks"]);
        let proved = tasks.iter().filter(|t| t["state"] == "proved").count();
        let count = format!("{proved}/{} proved", tasks.len());
        r.line(format!("  {} {} {count}: {}", text(&c["id"]), text(&c["stage"]), text(&c["title"])));
        rows.push(obj! {
            "id" => text(&c["id"]),
            "title" => text(&c["title"]),
            "stage" => text(&c["stage"]),
            "tasks" => count,
        });
    }
    r.table("changes", rows, "0 active");
}

fn home_plans(r: &mut Reply, v: &serde_json::Value) {
    let plans = list(&v["plans"]);
    for p in &plans {
        r.line(format!("  {} waits for accept: {}", text(&p["id"]), text(&p["title"])));
        r.help(format!("Run `rvow check {}` to see if the plan is ready", text(&p["id"])));
    }
    if !plans.is_empty() {
        r.set("plans", Node::from(serde_json::Value::Array(plans)));
    }
}

fn home_next(r: &mut Reply, v: &serde_json::Value) {
    let n = &v["needs_you"];
    r.set("needs_you", Node::from(n.clone()));
    r.line(format!(
        "needs you: {} amendments, {} questions, {} blocked, {} plans",
        n["amendments"], n["questions"], n["blocked"], n["plans"]
    ));
    let next = &v["next"];
    if next.is_object() {
        r.set(
            "next",
            Node::Obj(obj! {
                "task" => text(&next["task"]),
                "title" => text(&next["title"]),
                "state" => text(&next["state"]),
            }),
        );
        r.line(format!("next: {} {} ({})", text(&next["task"]), text(&next["title"]), text(&next["state"])));
        r.help(format!("Run `rvow prove {}` after the work to record its proof", text(&next["task"])));
    }
}

pub fn ls() -> Result<Reply> {
    let home = store::home_dir()?;
    let p = project(&home)?;
    let mut r = Reply::new();
    let mut rows = Vec::new();
    for s in change::summaries(&p)? {
        r.line(format!("{} {:<10} {} {}", s.id, s.stage.name(), s.branch, s.title));
        rows.push(obj! {
            "id" => s.id.to_string(),
            "stage" => s.stage.name(),
            "branch" => s.branch,
            "title" => s.title,
        });
    }
    if rows.is_empty() {
        r.line("no changes");
        r.help("Run `rvow new \"<title>\"` to plan a change");
    } else {
        r.set("count", rows.len());
        r.help("Run `rvow show <id>` to see a change in full");
        r.help("Run `rvow status` for the tasks on this branch");
    }
    r.table("changes", rows, "0 in this project");
    Ok(r)
}

pub fn path(id: &str) -> Result<Reply> {
    let home = store::home_dir()?;
    let found = change::locate(&project(&home)?, id)?.to_string();
    let mut r = Reply::new();
    r.line(&found);
    r.set("path", found);
    Ok(r)
}

pub fn show(id: &str, full: bool) -> Result<Reply> {
    let home = store::home_dir()?;
    Ok(Reply::long(show::show(&project(&home)?, id)?, full, &format!("rvow show {id}")))
}

pub fn spec_list() -> Result<Reply> {
    let home = store::home_dir()?;
    let (living, _) = change::load_living(&project(&home)?)?;
    let mut r = Reply::new();
    let mut rows = Vec::new();
    for (cap, lines) in show::overview(&living) {
        r.line(&cap);
        for line in lines {
            r.line(format!("  {line}"));
            let (id, title) = line.split_once(' ').unwrap_or((&line, ""));
            rows.push(obj! { "id" => id, "capability" => cap, "title" => title });
        }
    }
    if rows.is_empty() {
        r.line("no living specs");
        r.help("Run `rvow new --spec \"<title>\"` to plan a change with spec deltas");
    } else {
        r.set("count", format!("{} requirements in {} capabilities", rows.len(), living.len()));
        r.help("Run `rvow show <REQ-id>` to read a requirement");
        r.help("Run `rvow trace <REQ-id>` to list the changes that touched it");
    }
    r.table("requirements", rows, "0 in the living specs");
    Ok(r)
}

pub fn guide(artifact: Artifact) -> Reply {
    Reply::lines(
        match artifact {
            Artifact::Proposal => guide::PROPOSAL,
            Artifact::Spec => guide::SPEC,
            Artifact::Tasks => guide::TASKS,
            Artifact::Design => guide::DESIGN,
        }
        .to_owned(),
    )
}

pub fn brief() -> Result<Reply> {
    let home = store::home_dir()?;
    let p = project(&home)?;
    Ok(Reply::lines(brief::brief(&p, &home, &cwd()?)?))
}

fn status_change(r: &mut Reply, c: &proof::ChangeStatus) -> (Node, usize) {
    r.line(format!("{} on {}", c.id, c.branch));
    let mut rows = Vec::new();
    for t in &c.tasks {
        let state = match &t.state {
            State::Stale(why) => format!("stale ({})", why.join(", ")),
            s => s.name().to_string(),
        };
        r.line(format!("  {:<10} {:<20} {}", t.task, state, t.title));
        rows.push(obj! { "id" => t.task.to_string(), "state" => state, "title" => t.title });
    }
    let tasks = rows.len();
    let mut item = obj! { "id" => c.id.to_string(), "branch" => c.branch };
    item.push(("tasks".to_owned(), Node::from(rows.into_iter().map(Node::Obj).collect::<Vec<_>>())));
    for problem in &c.problems {
        r.line(format!("  error: {problem}"));
    }
    if !c.problems.is_empty() {
        item.push(("errors".to_owned(), Node::from(c.problems.clone())));
    }
    let mut rows = Vec::new();
    for d in &c.drift {
        r.line(format!("  drift {}: {}", d.class.name(), d.what));
        rows.push(Node::Obj(obj! { "class" => d.class.name(), "what" => d.what }));
    }
    if !rows.is_empty() {
        item.push(("drift".to_owned(), Node::List(rows)));
    }
    let mut rows = Vec::new();
    for a in &c.pending {
        r.line(format!("  pending {}: {}", a.id, a.reason));
        rows.push(Node::Obj(obj! { "id" => a.id.to_string(), "reason" => a.reason }));
    }
    if !rows.is_empty() {
        item.push(("pending".to_owned(), Node::List(rows)));
    }
    (Node::Obj(item), tasks)
}

pub fn status(all: bool, json: bool) -> Result<Reply> {
    let home = store::home_dir()?;
    let p = project(&home)?;
    let mut r = Reply::new();
    if json {
        let v = view::view(&p, &home, &cwd()?)?.unwrap_or(serde_json::Value::Null);
        println!("{v}");
        return Ok(r);
    }
    let changes = proof::status(&p, &home, &cwd()?, all)?;
    if changes.is_empty() {
        r.line("no active change on this branch");
        let scope = if all { "in this project".to_owned() } else { format!("on branch {}", branch_label()) };
        r.set("changes", format!("0 active {scope}"));
        r.help("Run `rvow ls` to list every change");
        return Ok(r);
    }
    let mut items = Vec::new();
    let mut tasks = 0;
    let (mut drift, mut pending) = (false, false);
    for c in &changes {
        let (item, n) = status_change(&mut r, c);
        tasks += n;
        drift |= !c.drift.is_empty();
        pending |= !c.pending.is_empty();
        items.push(item);
    }
    r.set("count", format!("{tasks} tasks in {} changes", changes.len()));
    r.set("changes", items);
    r.help("Run `rvow prove <task-id>` to run a task's check and record the proof");
    if drift {
        r.help("Run `rvow amend --reason \"<why>\"` to record the drift for the user to confirm");
    }
    if pending {
        r.help("Ask the user to run `rvow confirm` to decide the pending amendments");
    }
    Ok(r)
}

pub fn trace(id: &str) -> Result<Reply> {
    let home = store::home_dir()?;
    let (id, touches) = archive::trace(&project(&home)?, id)?;
    let mut r = Reply::new();
    if touches.is_empty() {
        r.line(format!("no change touched {id}"));
    }
    let mut rows = Vec::new();
    for t in &touches {
        let date = t.date.as_deref().unwrap_or("-");
        r.line(format!("{} {:<10} {date:<10} {:<10} {}", t.change, t.stage.name(), t.ops.join(","), t.title));
        rows.push(obj! {
            "change" => t.change.to_string(),
            "stage" => t.stage.name(),
            "date" => date,
            "ops" => t.ops.join(" "),
            "title" => t.title,
        });
    }
    r.set("id", id.to_string());
    if !rows.is_empty() {
        r.set("count", rows.len());
    }
    r.table("changes", rows, format!("0 touched {id}"));
    Ok(r)
}
