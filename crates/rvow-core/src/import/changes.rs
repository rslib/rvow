use std::collections::HashSet;
use std::path::{Path, PathBuf};

use super::openspec::{OsReq, OsSpec, find_title, parse_openspec, render_req};
use super::tree::{Cap, Mint, read, sorted_dirs};
use super::{ChangeReport, NO_SCOPE};
use crate::change::{self, Stage, slugify};
use crate::error::Result;
use crate::id::{Id, Kind};

pub(super) fn section(src: &str, names: &[&str]) -> Option<String> {
    let mut found = false;
    let mut out = String::new();
    for line in src.lines() {
        if let Some(h) = line.strip_prefix("## ") {
            found = names.iter().any(|n| h.trim().eq_ignore_ascii_case(n));
        } else if found {
            out.push_str(line);
            out.push('\n');
        }
    }
    let out = out.trim().to_string();
    (!out.is_empty()).then_some(out)
}

pub(super) fn title_of(src: &str, name: &str) -> String {
    let heading = src
        .lines()
        .find_map(|l| l.strip_prefix("# "))
        .map(|t| t.trim().trim_start_matches("Proposal:").trim())
        .filter(|t| !t.is_empty() && !t.eq_ignore_ascii_case("proposal"));
    match heading {
        Some(t) => t.to_string(),
        None => {
            let b = name.as_bytes();
            let dated = b.len() > 11
                && b[..10].iter().enumerate().all(|(i, c)| if i == 4 || i == 7 { *c == b'-' } else { c.is_ascii_digit() })
                && b[10] == b'-';
            let name = if dated { &name[11..] } else { name };
            name.replace(['-', '_'], " ")
        }
    }
}

pub(super) fn proposal(src: &str, name: &str, branch: &str) -> (String, String) {
    let title = title_of(src, name);
    let bullets = |text: String| {
        text.lines()
            .filter(|l| !l.trim().is_empty())
            .map(|l| if l.trim_start().starts_with("- ") { l.to_string() } else { format!("- {}", l.trim()) })
            .collect::<Vec<_>>()
            .join("\n")
    };
    let why = section(src, &["Why"]).unwrap_or_default();
    let what = section(src, &["What Changes"]).unwrap_or_default();
    let scope = section(src, &["Out of scope", "Non-goals"]).unwrap_or_else(|| NO_SCOPE.into());
    let impact = section(src, &["Impact"]).map(bullets).unwrap_or_default();
    let md = format!(
        "branch: {branch}\n\n# {title}\n\n## Why\n\n{why}\n\n## What changes\n\n{what}\n\n## Out of scope\n\n{scope}\n\n## Assumptions\n\n{impact}\n"
    );
    (title, md)
}

/// Splits a trailing `; verify `cmd`` clause off a task title. A clause that
/// is prose, or holds more than one command, stays in the title.
pub(super) fn split_verify(text: &str) -> (&str, Option<&str>) {
    if let Some((title, clause)) = text.rsplit_once("; verify `")
        && let Some((cmd, tail)) = clause.split_once('`')
        && !tail.contains('`')
        && !cmd.is_empty()
    {
        return (title.trim(), Some(cmd));
    }
    (text.trim(), None)
}

pub(super) fn task_line(line: &str) -> Option<(bool, &str)> {
    let mut s = line.trim_start();
    if let Some(r) = s.strip_prefix(['-', '*', '+']) {
        s = r;
    } else if let Some(r) = s.trim_start_matches(|c: char| c.is_ascii_digit()).strip_prefix(['.', ')'])
        && r.len() < s.len() - 1
    {
        s = r;
    }
    let s = s.trim_start().strip_prefix('[')?;
    let (mark, rest) = s.split_once(']')?;
    matches!(mark, " " | "x" | "X").then_some((mark != " ", rest.trim()))
}

pub(super) fn tasks(src: &str, report: &mut ChangeReport) -> String {
    let mut out = String::from("# Tasks\n");
    let mut raw: Vec<String> = Vec::new();
    let mut flush = |raw: &mut Vec<String>, out: &mut String| {
        if let Some(text) = raw.pop() {
            let (number, text) = match text.split_once(' ') {
                Some((n, t)) if n.starts_with(|c: char| c.is_ascii_digit()) && n.chars().all(|c| c.is_ascii_digit() || c == '.') => {
                    (n.trim_end_matches('.').to_string(), t)
                }
                _ => (String::new(), text.as_str()),
            };
            let (title, verify) = split_verify(text);
            report.tasks += 1;
            out.push_str(&format!("- TSK-new{} {title}\n", report.tasks));
            match verify {
                Some(cmd) => out.push_str(&format!("  verify: {cmd}\n")),
                None => report.missing_verify.push(format!("{number} {title}").trim().to_string()),
            }
            out.push_str("  files: **\n");
        }
    };
    for line in src.lines() {
        if let Some((_, text)) = task_line(line) {
            flush(&mut raw, &mut out);
            raw.push(text.to_string());
        } else if let Some(h) = line.strip_prefix('#') {
            flush(&mut raw, &mut out);
            if let Some(name) = h.strip_prefix("# ") {
                let name = name.trim();
                let name = name.split_once(". ").filter(|(n, _)| n.chars().all(|c| c.is_ascii_digit())).map_or(name, |(_, rest)| rest);
                out.push_str(&format!("\n## {name}\n\n"));
            }
        } else if !line.trim().is_empty()
            && let Some(last) = raw.last_mut()
        {
            last.push(' ');
            last.push_str(line.trim());
        }
    }
    flush(&mut raw, &mut out);
    out
}

pub(super) fn deltas(os: &OsSpec, cap: Option<&Cap>, next: &mut u32, rep: &mut ChangeReport) -> String {
    let empty = Vec::new();
    let reqs = cap.map_or(&empty, |c| &c.reqs);
    let label = |kind: &str, title: &str| format!("{} {kind} {title:?}", cap.map_or("?", |c| &c.name));
    let mut out = String::new();
    let mut seen: HashSet<&Id> = HashSet::new();
    let mut emit = |s: String| {
        out.push_str(&s);
        out.push('\n');
    };
    for r in os.group("ADDED Requirements") {
        *next += 1;
        emit(render_req(&format!("ADD REQ-new{next} {}", r.title), r));
    }
    let renamed_from = |title: &str| os.renames.iter().find(|(_, to)| to.eq_ignore_ascii_case(title)).map(|(from, _)| from.as_str());
    for r in os.group("MODIFIED Requirements") {
        let hit = find_title(reqs, &r.title).or_else(|| renamed_from(&r.title).and_then(|from| find_title(reqs, from)));
        match hit {
            Some((id, _)) if seen.insert(id) => emit(render_req(&format!("CHANGE {id} {}", r.title), r)),
            Some(_) => {}
            None => rep.unmatched.push(label("MODIFIED", &r.title)),
        }
    }
    for r in os.group("REMOVED Requirements") {
        match find_title(reqs, &r.title) {
            Some((id, _)) if seen.insert(id) => emit(format!("## REMOVE {id}\n")),
            Some(_) => {}
            None => rep.unmatched.push(label("REMOVED", &r.title)),
        }
    }
    for (from, to) in &os.renames {
        match find_title(reqs, from) {
            Some((id, living)) if seen.insert(id) => {
                let renamed = OsReq { title: to.clone(), ..living.clone() };
                emit(render_req(&format!("CHANGE {id} {to}"), &renamed));
            }
            Some(_) => {}
            None => rep.unmatched.push(label("RENAMED", from)),
        }
    }
    out.trim_end().to_string() + "\n"
}

pub(super) fn change(
    dir: &Path,
    caps: &[Cap],
    branch: &str,
    root: &Path,
    mint: &mut Mint,
    files: &mut Vec<(PathBuf, String)>,
) -> Result<ChangeReport> {
    let name = change::file_name(dir)?;
    let proposal_src = std::fs::read_to_string(dir.join("proposal.md")).unwrap_or_default();
    let (title, proposal_md) = proposal(&proposal_src, &name, branch);
    let id = mint(Kind::Chg, None)?;
    let slug = Some(slugify(&title)).filter(|s| !s.is_empty()).unwrap_or_else(|| "change".into());
    let out = root.join(Stage::Backlog.dir()).join(format!("{id}-{slug}"));
    let mut rep = ChangeReport { name, id, tasks: 0, missing_verify: Vec::new(), unmatched: Vec::new() };
    files.push((out.join(change::PROPOSAL), proposal_md));
    let tasks_src = std::fs::read_to_string(dir.join("tasks.md")).unwrap_or_default();
    files.push((out.join(change::TASKS), tasks(&tasks_src, &mut rep)));
    let checked: Vec<_> =
        tasks_src.lines().filter_map(task_line).map(|(checked, text)| serde_json::json!({"checked": checked, "task": text})).collect();
    if !checked.is_empty() {
        let note = serde_json::json!({
            "kind": "note", "time": crate::ledger::now_ms(), "parents": [],
            "source": "openspec", "tasks": checked
        });
        files.push((out.join(change::LEDGER), format!("{note}\n")));
    }
    if let Ok(design) = std::fs::read_to_string(dir.join("design.md")) {
        files.push((out.join(change::DESIGN), design));
    }
    let mut next = 0;
    for cap_dir in sorted_dirs(&dir.join("specs"))? {
        let cap_name = change::file_name(&cap_dir)?;
        let path = cap_dir.join("spec.md");
        if !path.is_file() {
            continue;
        }
        let os = parse_openspec(&read(&path)?);
        let md = deltas(&os, caps.iter().find(|c| c.name == cap_name), &mut next, &mut rep);
        files.push((out.join(change::DELTAS).join(format!("{cap_name}.md")), md));
    }
    Ok(rep)
}
