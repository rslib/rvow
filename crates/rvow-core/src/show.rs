//! The read-only views behind `rvow spec list` and `rvow show`.

use std::collections::BTreeMap;
use std::fmt::Write;
use std::time::Duration;

use crate::change::{self, CapSpec, ChangeDir, Loaded, Stage};
use crate::error::{Error, Result};
use crate::id::{Id, Kind, Ref};
use crate::ledger::{self, Event};
use crate::proof::find_requirement;
use crate::spec::Requirement;
use crate::store::Project;
use crate::tasks::Task;

/// Each capability with one `REQ-id title` line per requirement. Never the text.
pub fn overview(living: &[CapSpec]) -> Vec<(String, Vec<String>)> {
    living
        .iter()
        .map(|(cap, spec)| {
            let lines = spec
                .requirements
                .iter()
                .map(|r| {
                    let note = if r.deprecated { " (deprecated)" } else { "" };
                    format!("{} {}{note}", r.id, r.title)
                })
                .collect();
            (cap.clone(), lines)
        })
        .collect()
}

/// One requirement, task or change in full, found by a unique id prefix.
pub fn show(project: &Project, input: &str) -> Result<String> {
    render(project, input, false)
}

/// Like `show`, but a change lists the check fields of each task.
pub fn review(project: &Project, input: &str) -> Result<String> {
    render(project, input, true)
}

fn render(project: &Project, input: &str, checks: bool) -> Result<String> {
    let id = change::find(project, input)?.id;
    let (living, _) = change::load_living(project)?;
    let (dirs, _) = change::list(project)?;
    if id.kind() == Kind::Chg {
        let dir = dirs.iter().find(|c| c.id == id).ok_or_else(|| not_listed(&id))?;
        return show_change(dir, &living, checks);
    }
    let mut open = Vec::new();
    for dir in dirs.into_iter().filter(|c| matches!(c.stage, Stage::Backlog | Stage::Active)) {
        let loaded = change::load(&dir.path, &living)?;
        open.push((dir, loaded));
    }
    if id.kind() == Kind::Req { show_requirement(project, &id, &living, &open) } else { show_task(&id, &living, &open) }
}

fn not_listed(id: &Id) -> Error {
    Error::internal(format!("{id} resolved but is not in the project"))
}

fn render_requirement(req: &Requirement) -> String {
    let mut out = format!("## {} {}\n", req.id, req.title);
    if req.deprecated {
        out.push_str("status: deprecated\n");
    }
    let _ = writeln!(out, "\n{}", req.text);
    for sc in &req.scenarios {
        let _ = writeln!(out, "\n### Scenario: {}", sc.title);
        for st in &sc.steps {
            let _ = writeln!(out, "- {} {}", st.kind.word(), st.text);
        }
    }
    out
}

fn show_requirement(project: &Project, id: &Id, living: &[CapSpec], open: &[(ChangeDir, Loaded)]) -> Result<String> {
    let want = Ref::Id(id.clone());
    let rel = |p: &std::path::Path| p.strip_prefix(&project.root).unwrap_or(p).display().to_string();
    let mut found = living.iter().find_map(|(cap, spec)| {
        let req = spec.requirements.iter().find(|r| r.id == want)?;
        Some((req, format!("specs/{cap}/spec.md")))
    });
    let mut touched = Vec::new();
    for (dir, loaded) in open {
        for (cap, file) in &loaded.deltas {
            for d in file.deltas.iter().filter(|d| d.req.id == want) {
                touched.push(format!("  {} {}", dir.id, d.op.word()));
                if found.is_none() {
                    let path = dir.path.join(change::DELTAS).join(format!("{cap}.md"));
                    found = Some((&d.req, rel(&path)));
                }
            }
        }
    }
    let (req, file) = found.ok_or_else(|| not_listed(id))?;
    let mut out = render_requirement(req);
    let _ = writeln!(out, "\nin: {file}");
    if !touched.is_empty() {
        let _ = writeln!(out, "touched by:\n{}", touched.join("\n"));
    }
    Ok(out)
}

fn duration(d: Duration) -> String {
    match d.as_secs() {
        s if s % 3600 == 0 => format!("{}h", s / 3600),
        s if s % 60 == 0 => format!("{}m", s / 60),
        s => format!("{s}s"),
    }
}

fn render_task(task: &Task) -> String {
    format!("- {} {}\n{}", task.id, task.title, task_fields(task))
}

/// The indented fields under a task: covers, verify, files and the optional ones.
fn task_fields(task: &Task) -> String {
    let mut out = String::new();
    if !task.covers.is_empty() {
        let covers: Vec<String> = task.covers.iter().map(Ref::to_string).collect();
        let _ = writeln!(out, "  covers: {}", covers.join(", "));
    }
    let _ = writeln!(out, "  verify: {}", task.verify.as_deref().unwrap_or(""));
    let _ = writeln!(out, "  files: {}", task.files.join(", "));
    if task.baseline_pass {
        out.push_str("  baseline: pass\n");
    }
    if let Some(mutants) = &task.mutants {
        let _ = writeln!(out, "  mutants: {mutants}");
    }
    if let Some(t) = task.timeout {
        let _ = writeln!(out, "  timeout: {}", duration(t));
    }
    out
}

fn show_task(id: &Id, living: &[CapSpec], open: &[(ChangeDir, Loaded)]) -> Result<String> {
    let want = Ref::Id(id.clone());
    let (dir, loaded, task) =
        open.iter().find_map(|(d, l)| Some((d, l, l.tasks.iter().find(|t| t.id == want)?))).ok_or_else(|| not_listed(id))?;
    let mut out = render_task(task);
    let _ = writeln!(out, "change: {}", dir.id);
    for r in &task.covers {
        match find_requirement(r, loaded, living) {
            Some(req) => {
                let _ = writeln!(out, "{r} {}", req.title);
            }
            None => {
                let _ = writeln!(out, "{r} not found");
            }
        }
    }
    out.push_str(&proof_history(&dir.path, id));
    Ok(out)
}

const HISTORY_RUNS: usize = 10;

/// The newest proof runs of a task from the change's ledger, with the total.
fn proof_history(change: &std::path::Path, id: &Id) -> String {
    let (events, _) = ledger::read(&change.join(change::LEDGER)).unwrap_or_default();
    let id = id.to_string();
    let runs: Vec<_> =
        events.iter().filter(|e| e.kind == ledger::Kind::Prove && e.data.get("task").and_then(|v| v.as_str()) == Some(&id)).collect();
    if runs.is_empty() {
        return "proof runs: none yet\n".to_string();
    }
    let mut out = format!("proof runs: {} in total, newest {} first\n", runs.len(), runs.len().min(HISTORY_RUNS));
    for e in runs.iter().rev().take(HISTORY_RUNS) {
        let verdict = if e.data.get("proved").and_then(|v| v.as_bool()) == Some(true) { "proved" } else { "failed" };
        let tests = e.data.get("tests_run").and_then(|v| v.as_u64()).map_or("unknown tests".to_string(), |n| format!("{n} tests"));
        let ms = e.data.get("duration_ms").and_then(|v| v.as_u64()).unwrap_or(0);
        let machine = e.data.get("run").and_then(|r| r.get("machine")).and_then(|v| v.as_str()).unwrap_or("unknown");
        let mutants = e
            .data
            .get("mutants")
            .and_then(|m| m.get("survived"))
            .and_then(|v| v.as_u64())
            .map(|n| format!(", mutants: {n} survived"))
            .unwrap_or_default();
        let _ = writeln!(out, "  {verdict}, {tests}, {ms} ms, {machine}{mutants}");
    }
    env_diffs(&mut out, &runs);
    out
}

fn env_diffs(out: &mut String, runs: &[&Event]) {
    for (i, newest) in runs.iter().enumerate().rev() {
        let passed = newest.data.get("proved").and_then(|v| v.as_bool()) == Some(true);
        if let Some(older) = runs[..i]
            .iter()
            .rev()
            .find(|older| (older.data.get("proved").and_then(|v| v.as_bool()) == Some(true)) != passed && same_target(newest, older))
        {
            let (pass, fail) = if passed { (*newest, *older) } else { (*older, *newest) };
            let (penv, fenv) = (env_fields(pass), env_fields(fail));
            let keys: std::collections::BTreeSet<_> = penv.keys().chain(fenv.keys()).collect();
            let parts: Vec<_> = keys
                .into_iter()
                .filter(|k| penv.get(*k) != fenv.get(*k))
                .map(|k| {
                    let value = |env: &BTreeMap<String, String>| {
                        let raw = env.get(k).map(String::as_str).unwrap_or("<absent>");
                        if k.strip_prefix("var ").is_some_and(crate::runner::names_secret) { "<masked>".into() } else { sanitize_env(raw) }
                    };
                    format!("{}: passed {}, failed {}", sanitize_env(k), value(&penv), value(&fenv))
                })
                .collect();
            if !parts.is_empty() {
                let _ = writeln!(out, "  env differs on: {}", parts.join(", "));
            }
            return;
        }
    }
}

fn same_target(a: &Event, b: &Event) -> bool {
    let fields = ["task_hash", "requirements", "verify", "inputs"];
    if fields.iter().all(|k| a.data.contains_key(*k) && b.data.contains_key(*k)) {
        return fields.iter().all(|k| a.data.get(*k) == b.data.get(*k));
    }
    // Legacy synthetic runs have no target fields, so only their key can bind them.
    fields.iter().all(|k| !a.data.contains_key(*k) && !b.data.contains_key(*k))
        && a.data.get("key").is_some_and(|key| !key.is_null() && Some(key) == b.data.get("key"))
}

fn sanitize_env(value: &str) -> String {
    crate::runner::mask(value, std::env::var("HOME").ok().as_deref(), "")
        .chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .take(256)
        .collect()
}

fn env_fields(e: &Event) -> BTreeMap<String, String> {
    let mut fields = BTreeMap::new();
    let Some(env) = e.data.get("env") else { return fields };
    for group in ["commands", "vars"] {
        if let Some(values) = env.get(group).and_then(|v| v.as_object()) {
            for (key, value) in values {
                let name = if group == "vars" { format!("var {key}") } else { key.clone() };
                let value = value.as_str().unwrap_or("");
                fields.insert(name, value.into());
            }
        }
    }
    for key in ["os", "arch"] {
        if let Some(value) = env.get(key).and_then(|v| v.as_str()) {
            fields.insert(key.into(), value.into());
        }
    }
    fields
}

fn show_change(dir: &ChangeDir, living: &[CapSpec], checks: bool) -> Result<String> {
    let loaded = change::load(&dir.path, living)?;
    let (events, _) = ledger::read(&dir.path.join(change::LEDGER))?;
    let mut out =
        format!("{}\nstage: {}\nbranch: {}\ntitle: {}\n", dir.id, dir.stage.name(), loaded.proposal.branch, loaded.proposal.title);
    if !loaded.tasks.is_empty() {
        out.push_str("tasks:\n");
        for t in &loaded.tasks {
            let _ = writeln!(out, "  {} {}", t.id, t.title);
            if checks {
                for line in task_fields(t).lines() {
                    let _ = writeln!(out, "  {line}");
                }
            }
        }
    }
    let deltas: Vec<_> = loaded.deltas.iter().flat_map(|(_, f)| &f.deltas).collect();
    if !deltas.is_empty() {
        out.push_str("deltas:\n");
        for d in deltas {
            let title = find_requirement(&d.req.id, &loaded, living).map_or("", |r| &r.title);
            let _ = writeln!(out, "  {} {} {title}", d.op.word(), d.req.id);
        }
    }
    let mut kinds: BTreeMap<String, usize> = BTreeMap::new();
    for e in &events {
        *kinds.entry(format!("{:?}", e.kind).to_lowercase()).or_default() += 1;
    }
    if !kinds.is_empty() {
        out.push_str("ledger:\n");
        for (kind, n) in kinds {
            let _ = writeln!(out, "  {kind} {n}");
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::git::testrepo::*;
    use crate::ledger::{self, Kind as Ev};
    use crate::store::{Mode, init};

    const SPEC: &str = "# Ledger\n\n## REQ-q8w2c7 Old thing\nstatus: deprecated\n\nrvow SHALL keep it.\n\n### Scenario: kept\n- WHEN a\n- THEN b\n\n## REQ-k3f9m2 Record a proof run\n\nWHEN a task runs, rvow SHALL record one event.\n\n### Scenario: one run\n- GIVEN a task\n- WHEN it runs\n- THEN one event exists\n- AND it has exit 0\n";

    fn fixture() -> (TempDir, Project, ChangeDir) {
        let tmp = TempDir::new();
        let repo = repo_with_commit(&tmp.0, "r");
        let p = init(&repo, &tmp.0.join("home"), Mode::Home).unwrap();
        let cap = p.root.join("specs/ledger");
        std::fs::create_dir_all(&cap).unwrap();
        std::fs::write(cap.join("spec.md"), SPEC).unwrap();
        let c = change::new(&p, "Output tail", "main", false).unwrap();
        std::fs::write(
            c.path.join("tasks.md"),
            "# Tasks\n\n- TSK-x7k2q4 Write tail\n  covers: REQ-new1, REQ-k3f9m2\n  verify: cargo test tail\n  files: crates/**, Cargo.toml\n  baseline: pass\n  timeout: 20m\n",
        )
        .unwrap();
        std::fs::write(
            c.path.join("specs/ledger.md"),
            "## ADD REQ-new1 Tail\n\nWHEN a run ends, rvow SHALL keep the tail.\n\n### Scenario: tail\n- WHEN a run ends\n- THEN the tail is kept\n\n## DEPRECATE REQ-q8w2c7\n",
        )
        .unwrap();
        (tmp, p, c)
    }

    #[test]
    fn overview_lists_ids_and_titles_only() {
        let (_t, p, _) = fixture();
        let (living, _) = change::load_living(&p).unwrap();
        assert_eq!(
            overview(&living),
            [("ledger".to_string(), vec!["REQ-q8w2c7 Old thing (deprecated)".to_string(), "REQ-k3f9m2 Record a proof run".to_string()])]
        );
        assert!(overview(&[]).is_empty());
    }

    #[test]
    fn shows_a_requirement_and_who_touches_it() {
        let (_t, p, c) = fixture();
        std::fs::write(
            c.path.join("specs/ledger.md"),
            "## DEPRECATE REQ-q8w2c7\n## CHANGE REQ-k3f9m2 Record a proof run\n\nWHEN a task runs, rvow SHALL record two events.\n\n### Scenario: s\n- WHEN a\n- THEN b\n",
        )
        .unwrap();
        let out = show(&p, "REQ-q8w").unwrap();
        assert!(out.starts_with("## REQ-q8w2c7 Old thing\nstatus: deprecated\n\nrvow SHALL keep it.\n"));
        assert!(out.contains("\n### Scenario: kept\n- WHEN a\n- THEN b\n"));
        assert!(out.contains("\nin: specs/ledger/spec.md\n"));
        assert!(out.ends_with(&format!("touched by:\n  {} DEPRECATE\n", c.id)));
        let out = show(&p, "k3f9").unwrap();
        assert!(out.contains("- AND it has exit 0\n"));
        assert!(!out.contains("status:"));
        assert!(out.contains(&format!("touched by:\n  {} CHANGE", c.id)));
    }

    #[test]
    fn shows_an_added_requirement_from_its_delta() {
        let (_t, p, c) = fixture();
        std::fs::write(
            c.path.join("specs/ledger.md"),
            "## ADD REQ-m4n5p6 Tail\n\nWHEN a run ends, rvow SHALL keep the tail.\n\n### Scenario: tail\n- WHEN a run ends\n- THEN the tail is kept\n",
        )
        .unwrap();
        let out = show(&p, "m4n5").unwrap();
        let rel = format!("backlog/{}-output-tail/specs/ledger.md", c.id);
        assert!(out.contains(&format!("\nin: {rel}\n")), "{out}");
        assert!(out.contains(&format!("touched by:\n  {} ADD", c.id)));
    }

    #[test]
    fn shows_a_task_with_the_titles_it_covers() {
        let (_t, p, c) = fixture();
        std::fs::write(
            c.path.join("tasks.md"),
            "- TSK-x7k2q4 Write tail\n  covers: REQ-k3f9m2, REQ-zzzzzz\n  verify: cargo test tail\n  files: crates/**, Cargo.toml\n  baseline: pass\n  timeout: 90s\n",
        )
        .unwrap();
        let out = show(&p, "x7k").unwrap();
        assert_eq!(
            out,
            format!(
                "- TSK-x7k2q4 Write tail\n  covers: REQ-k3f9m2, REQ-zzzzzz\n  verify: cargo test tail\n  files: crates/**, Cargo.toml\n  baseline: pass\n  timeout: 90s\nchange: {}\nREQ-k3f9m2 Record a proof run\nREQ-zzzzzz not found\nproof runs: none yet\n",
                c.id
            )
        );
        std::fs::write(c.path.join("tasks.md"), "- TSK-x7k2q4 Plain\n  verify: true\n  files: a\n").unwrap();
        let out = show(&p, "x7k").unwrap();
        assert_eq!(out, format!("- TSK-x7k2q4 Plain\n  verify: true\n  files: a\nchange: {}\nproof runs: none yet\n", c.id));
    }

    #[test]
    fn task_proof_history() {
        let (_t, p, c) = fixture();
        let none = show(&p, "TSK-x7k2q4").unwrap();
        assert!(none.ends_with("proof runs: none yet\n"), "{none}");

        let lock = p.lock().unwrap();
        for i in 0..12u64 {
            let mut data = serde_json::Map::new();
            data.insert("task".into(), "TSK-x7k2q4".into());
            data.insert("proved".into(), (i != 0).into());
            data.insert("tests_run".into(), 3.into());
            data.insert("duration_ms".into(), (100 + i).into());
            data.insert("run".into(), serde_json::json!({"machine": "laptop"}));
            ledger::append(&c.path.join(change::LEDGER), &lock, Ev::Prove, data).unwrap();
        }
        drop(lock);
        let out = show(&p, "TSK-x7k2q4").unwrap();
        assert!(out.contains("proof runs: 12 in total, newest 10 first\n"), "{out}");
        let lines: Vec<_> = out.lines().filter(|l| l.starts_with("  proved")).collect();
        assert_eq!(lines.len(), 10, "{out}");
        assert_eq!(lines[0], "  proved, 3 tests, 111 ms, laptop", "{out}");
        assert!(!out.contains("failed"), "{out}");
    }

    #[test]
    fn show_notes_env_fields_that_differ_between_pass_and_fail() {
        let (_t, p, c) = fixture();
        let lock = p.lock().unwrap();
        let mut failed = serde_json::Map::new();
        failed.insert("task".into(), "TSK-x7k2q4".into());
        failed.insert("proved".into(), false.into());
        failed.insert("key".into(), "k".into());
        failed.insert("env".into(), serde_json::json!({ "commands": { "rustc --version": "rustc 1.80" } }));
        ledger::append(&c.path.join(change::LEDGER), &lock, Ev::Prove, failed).unwrap();
        let mut passed = serde_json::Map::new();
        passed.insert("task".into(), "TSK-x7k2q4".into());
        passed.insert("proved".into(), true.into());
        passed.insert("key".into(), "k".into());
        passed.insert("env".into(), serde_json::json!({ "commands": { "rustc --version": "rustc 1.82" } }));
        ledger::append(&c.path.join(change::LEDGER), &lock, Ev::Prove, passed).unwrap();
        drop(lock);
        let out = show(&p, "TSK-x7k2q4").unwrap();
        assert!(out.contains("env differs on: rustc --version: passed rustc 1.82, failed rustc 1.80"), "{out}");
    }

    #[test]
    fn durations_use_the_largest_whole_unit() {
        assert_eq!(duration(Duration::from_secs(7200)), "2h");
        assert_eq!(duration(Duration::from_secs(1200)), "20m");
        assert_eq!(duration(Duration::from_secs(90)), "90s");
    }

    #[test]
    fn shows_a_change_with_its_ledger_counts() {
        let (_t, p, c) = fixture();
        let lock = p.lock().unwrap();
        for kind in [Ev::Note, Ev::Prove, Ev::Note] {
            ledger::append(&c.path.join(change::LEDGER), &lock, kind, Default::default()).unwrap();
        }
        let out = show(&p, &c.id.to_string()).unwrap();
        assert_eq!(
            out,
            format!(
                "{}\nstage: backlog\nbranch: main\ntitle: Output tail\ntasks:\n  TSK-x7k2q4 Write tail\ndeltas:\n  ADD REQ-new1 Tail\n  DEPRECATE REQ-q8w2c7 Old thing\nledger:\n  note 2\n  prove 1\n",
                c.id
            )
        );
        assert_eq!(show(&p, "zzzzzz").unwrap_err().code, "path");
    }
}
