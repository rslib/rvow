//! `rvow put`: the only way an agent writes a plan, one part or one block at a time.

use std::io::Read;
use std::path::Path;

use crate::change::{self, ChangeDir, Stage};
use crate::error::{Error, Result};
use crate::id::Ref;
use crate::plan;
use crate::show::parts_of;
use crate::spec::parse_deltas;
use crate::store::Project;
use crate::tasks::parse_tasks;

/// What a put wrote and what the check says about it: `ok`, or the findings.
pub struct Outcome {
    pub change: ChangeDir,
    /// The printed text: `ok` when the change is clean, one line per finding.
    pub lines: Vec<String>,
}

/// Reads all of stdin as the replacement text.
pub fn stdin() -> Result<String> {
    let mut text = String::new();
    std::io::stdin().read_to_string(&mut text).map_err(|e| Error::io(Path::new("<stdin>"), e))?;
    Ok(text)
}

/// Replaces one part of a change, or one task or requirement block, with `text`.
pub fn put(project: &Project, cwd: &Path, target: &str, part: Option<&str>, text: &str) -> Result<Outcome> {
    let dir = match part {
        Some(part) => put_part(project, target, part, text)?,
        None => put_block(project, target, text)?,
    };
    let lines = findings(project, cwd, &dir)?;
    let lines = if lines.is_empty() { vec!["ok".to_string()] } else { lines };
    Ok(Outcome { change: dir, lines })
}

/// One part of one change: `proposal`, `tasks`, `design` or `spec:<name>`.
/// `design` and `spec:<name>` are created when they do not exist yet.
fn put_part(project: &Project, target: &str, part: &str, text: &str) -> Result<ChangeDir> {
    let dir = change::resolve(project, target)?;
    if !matches!(dir.stage, Stage::Backlog | Stage::Active) {
        return Err(refused(&dir));
    }
    let (living, _) = change::load_living(project)?;
    let loaded = change::load(&dir.path, &living)?;
    let wrong = || {
        Error::new("part", format!("{part} is not a part of {}", dir.id), format!("use one of the parts: {}", parts_of(&loaded).join(", ")))
    };
    let want = match part {
        "proposal" => Some(change::PROPOSAL.to_string()),
        "tasks" => Some(change::TASKS.to_string()),
        "design" => Some(change::DESIGN.to_string()),
        other => other.strip_prefix("spec:").and_then(|cap| {
            (cap.chars().next().is_some_and(|c| c.is_ascii_lowercase() || c.is_ascii_digit())
                && cap.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-'))
            .then(|| format!("{}/{cap}.md", change::DELTAS))
        }),
    };
    let Some(want) = want else {
        return Err(Error::new(
            "part",
            format!("{part} is not a part of {}", dir.id),
            format!("use a spec name of [a-z0-9][a-z0-9-]*, or one of the parts: {}", parts_of(&loaded).join(", ")),
        ));
    };
    let is_spec = part.starts_with("spec:");
    if !is_spec && part != "design" && loaded.texts.iter().all(|(k, _)| k != &want) {
        return Err(wrong());
    }
    let path = dir.path.join(&want);
    if is_spec && let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| Error::io(parent, e))?;
    }
    write_atomically(&path, text)?;
    Ok(dir)
}

enum Block {
    Task,
    Requirement,
}

/// One task or requirement block of an open change, found by id or placeholder.
fn put_block(project: &Project, target: &str, text: &str) -> Result<ChangeDir> {
    let want = parse_ref(project, target)?;
    let (living, _) = change::load_living(project)?;
    let open = change::list(project)?.0.into_iter().filter(|c| matches!(c.stage, Stage::Backlog | Stage::Active));
    let mut holders = Vec::new();
    for dir in open {
        let loaded = change::load(&dir.path, &living)?;
        if loaded.tasks.iter().any(|t| t.id == want) {
            let path = dir.path.join(change::TASKS);
            holders.push((dir, path, Block::Task));
        } else if let Some(cap) = loaded.deltas.iter().find_map(|(cap, f)| f.deltas.iter().find(|d| d.req.id == want).map(|_| cap.clone()))
        {
            let path = dir.path.join(change::DELTAS).join(format!("{cap}.md"));
            holders.push((dir, path, Block::Requirement));
        }
    }
    if holders.len() > 1 {
        let ids: Vec<String> = holders.iter().map(|(dir, _, _)| dir.id.to_string()).collect();
        let first = &holders[0].0.id;
        return Err(Error::new(
            "ambiguous",
            format!("{target} is held by {} open changes: {}", ids.len(), ids.join(", ")),
            format!("put the whole part: rvow put {first} tasks, or rvow put {first} spec:<name>"),
        ));
    }
    let written = match holders.pop() {
        Some((dir, path, Block::Task)) => {
            replace_task_block(&path, want, text)?;
            Some(dir)
        }
        Some((dir, path, Block::Requirement)) => {
            replace_req_block(&path, want, text)?;
            Some(dir)
        }
        None => None,
    };
    written.ok_or_else(|| {
        Error::new(
            "put",
            format!("{target} is not a task or requirement of an open change"),
            "run rvow status or rvow ls to see the open changes and their ids",
        )
    })
}

/// A `TSK-` or `REQ-` id or placeholder, or a unique prefix of a real id.
fn parse_ref(project: &Project, target: &str) -> Result<Ref> {
    if let Ok(r) = target.parse::<Ref>() {
        return Ok(r);
    }
    Ok(Ref::Id(change::find(project, target)?.id))
}

/// Writes `text` to `path` as one step: a temp file in the same folder, then a rename.
fn write_atomically(path: &Path, text: &str) -> Result<()> {
    let tmp = path.with_extension("put-tmp");
    std::fs::write(&tmp, text).map_err(|e| Error::io(&tmp, e))?;
    std::fs::rename(&tmp, path).map_err(|e| Error::io(path, e))?;
    Ok(())
}

fn refused(dir: &ChangeDir) -> Error {
    Error::new(
        "archived",
        format!("{} is {} and its plan does not change", dir.id, dir.stage.name()),
        format!("run rvow show {} to read it, or rvow new \"<title>\" to start a change", dir.id),
    )
}

/// Replaces the task block for `want` in tasks.md with `text`, which must hold
/// exactly one block for `want`.
fn replace_task_block(path: &Path, want: Ref, text: &str) -> Result<()> {
    let parsed = parse_tasks(&format!("# Tasks\n{text}"));
    if !parsed.1.is_empty() {
        return Err(block_error(want, "it does not parse as one task block"));
    }
    if parsed.0.len() != 1 {
        return Err(block_error(want, "the text must hold one block only"));
    }
    let task = &parsed.0[0];
    if task.id != want {
        return Err(block_error(want, &format!("the text holds one block for {}", task.id)));
    }
    let src = std::fs::read_to_string(path).map_err(|e| Error::io(path, e))?;
    let parsed = parse_tasks(&src);
    let Some(task) = parsed.0.iter().find(|t| t.id == want) else { return Err(block_error(want, "it is not in tasks.md")) };
    let starts: Vec<usize> = parsed.0.iter().map(|t| t.line).collect();
    replace_span(path, &src, task.line, starts, text)
}

/// Replaces the requirement block for `want` in a spec file with `text`.
fn replace_req_block(path: &Path, want: Ref, text: &str) -> Result<()> {
    let parsed = parse_deltas(&format!("{text}\n"));
    if !parsed.1.is_empty() {
        return Err(block_error(want, "it does not parse as one requirement block"));
    }
    if parsed.0.deltas.len() != 1 {
        return Err(block_error(want, "the text must hold one block only"));
    }
    let delta = &parsed.0.deltas[0];
    if delta.req.id != want {
        return Err(block_error(want, &format!("the text holds one block for {}", delta.req.id)));
    }
    let src = std::fs::read_to_string(path).map_err(|e| Error::io(path, e))?;
    let file = parse_deltas(&src).0;
    let Some(delta) = file.deltas.iter().find(|d| d.req.id == want) else { return Err(block_error(want, "it is not in the spec file")) };
    let starts: Vec<usize> = file.deltas.iter().map(|d| d.req.line).collect();
    replace_span(path, &src, delta.req.line, starts, text)
}

fn block_error(want: Ref, why: &str) -> Error {
    Error::new(
        "block",
        format!("the text is not a block for {want}: {why}"),
        format!("put exactly one block for {want} on stdin: rvow put {want}"),
    )
}

/// Replaces the block that starts at 1-based `start` with `text`, ending before
/// the next block start of `starts` or at the end of the file, keeping one blank
/// line around the block.
fn replace_span(path: &Path, src: &str, start: usize, starts: Vec<usize>, text: &str) -> Result<()> {
    let lines: Vec<&str> = src.lines().collect();
    let after = starts.iter().filter(|l| **l > start).min().copied().unwrap_or(lines.len() + 1);
    // The block's last content line, without the blank lines that separate blocks.
    let mut end = after;
    while end > start && lines.get(end - 1 - 1).is_some_and(|l| l.trim().is_empty()) {
        end -= 1;
    }
    let block: Vec<&str> = text.lines().collect();
    let mut out: Vec<String> = lines[..start - 1].iter().map(|s| s.to_string()).collect();
    if !out.is_empty() && out.last().is_some_and(|l| !l.trim().is_empty()) && !block.is_empty() {
        out.push(String::new());
    }
    out.extend(block.iter().map(|s| s.to_string()));
    let rest = &lines[end.saturating_sub(1)..];
    if !rest.is_empty() && !rest[0].trim().is_empty() && !out.is_empty() {
        out.push(String::new());
    }
    out.extend(rest.iter().map(|s| s.to_string()));
    let mut out = out.join("\n");
    out.push('\n');
    write_atomically(path, &out)
}

/// The check findings of the change, as one line each.
fn findings(project: &Project, cwd: &Path, dir: &ChangeDir) -> Result<Vec<String>> {
    let reports = plan::check(project, cwd, Some(&dir.id.to_string()))?;
    let Some(report) = reports.first() else { return Ok(Vec::new()) };
    let mut out = Vec::new();
    for (file, d) in &report.findings {
        let level = if d.severity() == crate::diag::Severity::Error { "error" } else { "warning" };
        out.push(format!("{file}:{}: {level}[{}]: {}", d.line, d.rule.code(), d.message));
    }
    for s in &report.slow {
        out.push(format!("{}: warning[full-suite]: {}", s.task, s.message));
    }
    for (task, prog) in &report.missing {
        out.push(format!("{task}: error[missing-program]: {prog} is neither on PATH nor in the repository"));
    }
    for q in &report.questions {
        out.push(format!("{}:{}: open question: {}", q.path, q.line, q.question));
    }
    if report.no_tasks {
        out.push("tasks.md: error[no-tasks]: tasks.md has no task".to_string());
    }
    Ok(out)
}
