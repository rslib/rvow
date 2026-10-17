use std::path::{Path, PathBuf};

use rvow_core::archive;
use rvow_core::brief;
use rvow_core::change;
use rvow_core::config;
use rvow_core::error::Result;
use rvow_core::fingerprint;
use rvow_core::git;
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

pub fn dashboard() -> Result<Reply> {
    let home = store::home_dir()?;
    let p = project(&home)?;
    let v = view::dashboard(&p, &home, &cwd()?)?;
    let mut r = Reply::new();
    let branch = v["branch"].as_str().unwrap_or("detached");
    r.set("branch", branch);
    r.line(format!("branch {branch}"));
    home_changes(&mut r, &v);
    r.line(format!("changes: {} active", list(&v["changes"]).len()));
    for (name, fields) in [
        ("specs", vec!["id", "capability", "title"]),
        ("backlog", vec!["id", "branch", "title"]),
        ("archive", vec!["id", "outcome", "title"]),
    ] {
        let entries = list(&v[name]);
        r.set(name, Node::from(serde_json::Value::Array(entries.clone())));
        r.line(format!("{name}: {}", entries.len()));
        for entry in entries {
            r.line(format!("  {}", fields.iter().map(|field| text(&entry[field])).collect::<Vec<_>>().join(" ")));
        }
    }
    let mut rows = Vec::new();
    for c in list(&v["changes"]) {
        for t in list(&c["tasks"]) {
            r.line(format!("  {} {} {}", text(&t["id"]), text(&t["state"]), text(&t["title"])));
            rows.push(Node::Obj(
                obj! { "id" => text(&t["id"]), "change" => text(&c["id"]), "state" => text(&t["state"]), "title" => text(&t["title"]) },
            ));
        }
    }
    r.line(format!("tasks: {}", rows.len()));
    r.set("tasks", Node::List(rows));
    home_next(&mut r, &v);
    Ok(r)
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
    let cwd = cwd()?;
    Ok(Reply::long(show::show_full(&project(&home)?, &home, &cwd, id)?, full, &format!("rvow show {id}")))
}

/// The full text of one part of a change, or a usage error on exit 2.
pub fn show_part(id: &str, part: &str, full: bool) -> Result<Reply> {
    let home = store::home_dir()?;
    let p = project(&home)?;
    let dir = change::resolve(&p, id)?;
    let text = show::change_part(&p, &dir, part)?;
    Ok(Reply::long(text, full, &format!("rvow show {id} {part}")))
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

pub fn guide(artifact: Artifact) -> Result<Reply> {
    let builtin = match artifact {
        Artifact::Proposal => guide::PROPOSAL,
        Artifact::Spec => guide::SPEC,
        Artifact::Tasks => guide::TASKS,
        Artifact::Design => guide::DESIGN,
        Artifact::Judge => guide::JUDGE,
    };
    let at = cwd()?;
    if rvow_core::git::toplevel(&at).is_err() {
        return Ok(Reply::lines(builtin.to_owned()));
    }
    let home = store::home_dir()?;
    let p = match store::find(&at, &home) {
        Ok(Some(p)) => p,
        Ok(None) => return Ok(Reply::lines(builtin.to_owned())),
        Err(e) if e.code == "no-commit" => return Ok(Reply::lines(builtin.to_owned())),
        Err(e) => return Err(e),
    };
    let config = rvow_core::config::load(&p, &home)?;
    let rule = match artifact {
        Artifact::Proposal => config.rules.proposal.as_deref(),
        Artifact::Spec => config.rules.spec.as_deref(),
        Artifact::Tasks => config.rules.tasks.as_deref(),
        Artifact::Design => config.rules.design.as_deref(),
        Artifact::Judge => None,
    };
    let mut text = builtin.trim_end().to_owned();
    for (heading, extra) in [("Project context", Some(config.context.as_str())), ("Artifact rules", rule)] {
        if let Some(extra) = extra.filter(|text| !text.trim().is_empty()) {
            text.push_str(&format!("\n\n## {heading}\n\n{extra}"));
        }
    }
    Ok(Reply::lines(text))
}

pub fn brief() -> Result<Reply> {
    let home = store::home_dir()?;
    let p = project(&home)?;
    Ok(Reply::lines(brief::brief(&p, &home, &cwd()?)?))
}

/// The next task of this branch with its id, title, state, verify command
/// and files, or a line saying no task is next. Exit 0 either way.
pub fn next() -> Result<Reply> {
    let home = store::home_dir()?;
    let cwd = cwd()?;
    let mut r = Reply::new();
    let Some(p) = store::find(&cwd, &home)? else {
        r.line("no task is next");
        r.set("next", "no task is next");
        r.help("Run `rvow init` to create this repository's rvow tree in the rvow home; the repository stays untouched");
        return Ok(r);
    };
    let top = git::toplevel(&cwd)?;
    let (living, _) = change::load_living(&p)?;
    let changes = proof::active(&p, &living)?;
    let branch = git::branch(&top)?.unwrap_or_default();
    let queue = proof::on_branch(&changes, &branch);
    let config = config::load(&p, &home)?;
    let now = proof::Now { top: &top, config: &config, env: fingerprint::env(&top, &config)?, living: &living };
    match proof::next(&queue, &now)? {
        Some((_, t, s)) => {
            r.line(format!("next: {} {} [{}]", t.id, t.title, s.name()));
            r.line(format!("  verify: {}", t.verify.as_deref().unwrap_or("")));
            r.line(format!("  files: {}", t.files.join(", ")));
            r.set("task", t.id.to_string());
            r.set("title", t.title.clone());
            r.set("state", s.name());
            r.set("verify", t.verify.as_deref().unwrap_or(""));
            r.set("files", t.files.join(", "));
            r.help(format!("Run `rvow prove {}` after the work to record its proof", t.id));
        }
        None => {
            r.line("no task is next");
            r.set("next", "no task is next");
            r.help("Run `rvow status` for the tasks on this branch");
        }
    }
    Ok(r)
}

fn status_change(r: &mut Reply, c: &proof::ChangeStatus) -> (Node, usize) {
    r.line(format!("{} on {}", c.id, c.branch));
    let mut rows = Vec::new();
    for t in &c.tasks {
        let state = match &t.state {
            State::Stale(why) => format!("stale ({})", why.join(", ")),
            s => s.name().to_string(),
        };
        let pending = if t.baseline_pending { "  (baseline pending)" } else { "" };
        r.line(format!("  {:<10} {:<20} {}{pending}", t.task, state, t.title));
        rows.push(obj! { "id" => t.task.to_string(), "state" => state, "title" => t.title });
    }
    let tasks = rows.len();
    let mut item = obj! { "id" => c.id.to_string(), "branch" => c.branch };
    item.push(("tasks".to_owned(), Node::from(rows.into_iter().map(Node::Obj).collect::<Vec<_>>())));
    let pending: Vec<String> = c.tasks.iter().filter(|t| t.baseline_pending).map(|t| t.task.to_string()).collect();
    if !pending.is_empty() {
        item.push(("baseline_pending".to_owned(), Node::from(pending)));
    }
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
