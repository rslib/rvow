//! `rvow import openspec`: converts the living specs and the open changes of an
//! OpenSpec tree into rvow's form. The requirement text is copied as written, so
//! imported requirements start as legacy and keep their lint errors as warnings.

use std::collections::HashSet;
use std::fmt;
use std::path::{Path, PathBuf};

use crate::change::{self, Stage, slugify};
use crate::error::{Error, Result};
use crate::id::{Id, Kind};
use crate::lint;
use crate::record::{self, Entry};
use crate::registry;
use crate::spec::parse_spec;
use crate::store::Project;
use crate::vow::requirement_hash;

const STEPS: [&str; 4] = ["GIVEN", "WHEN", "THEN", "AND"];
const NO_SCOPE: &str = "- Not stated in the OpenSpec proposal.";

#[derive(Debug, Default, PartialEq, Eq)]
pub struct Report {
    pub dry_run: bool,
    pub capabilities: usize,
    pub requirements: usize,
    pub legacy_warnings: usize,
    pub purposes_skipped: usize,
    pub archived: usize,
    pub changes: Vec<ChangeReport>,
}

#[derive(Debug, PartialEq, Eq)]
pub struct ChangeReport {
    pub name: String,
    pub id: Id,
    pub tasks: usize,
    /// Task number and title of each task that got no verify command.
    pub missing_verify: Vec<String>,
    /// Delta titles that no living requirement matches.
    pub unmatched: Vec<String>,
}

impl fmt::Display for Report {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mode = if self.dry_run { " (dry run)" } else { "" };
        writeln!(f, "openspec import{mode}")?;
        writeln!(f, "capabilities: {}", self.capabilities)?;
        writeln!(f, "requirements: {}", self.requirements)?;
        writeln!(f, "legacy lint warnings: {}", self.legacy_warnings)?;
        writeln!(f, "placeholder purposes skipped: {}", self.purposes_skipped)?;
        writeln!(f, "open changes: {}", self.changes.len())?;
        for c in &self.changes {
            writeln!(
                f,
                "  {} {}: {} tasks, {} without verify, {} unmatched titles",
                c.id,
                c.name,
                c.tasks,
                c.missing_verify.len(),
                c.unmatched.len()
            )?;
        }
        writeln!(f, "archived changes left alone: {}", self.archived)?;
        let missing: Vec<_> = self
            .changes
            .iter()
            .flat_map(|c| c.missing_verify.iter().map(move |t| (&c.name, t)))
            .collect();
        if !missing.is_empty() {
            writeln!(f, "tasks without verify, fix before accept:")?;
            for (name, t) in missing {
                writeln!(f, "  {name} {t}")?;
            }
        }
        let unmatched: Vec<_> = self
            .changes
            .iter()
            .flat_map(|c| c.unmatched.iter().map(move |t| (&c.name, t)))
            .collect();
        if !unmatched.is_empty() {
            writeln!(f, "unmatched titles, not converted:")?;
            for (name, t) in unmatched {
                writeln!(f, "  {name} {t}")?;
            }
        }
        Ok(())
    }
}

#[derive(Clone)]
struct OsScenario {
    title: String,
    steps: Vec<String>,
}

#[derive(Clone)]
struct OsReq {
    title: String,
    text: String,
    scenarios: Vec<OsScenario>,
}

#[derive(Default)]
struct OsSpec {
    purpose: String,
    /// Requirements by the name of their `##` group.
    groups: Vec<(String, Vec<OsReq>)>,
    /// RENAMED pairs, FROM then TO.
    renames: Vec<(String, String)>,
}

impl OsSpec {
    fn group(&self, name: &str) -> &[OsReq] {
        self.groups
            .iter()
            .find(|(g, _)| g == name)
            .map_or(&[], |(_, r)| r)
    }
}

fn step(line: &str) -> Option<String> {
    let rest = line.trim_start().strip_prefix("- **")?;
    let (word, text) = rest.split_once("**")?;
    STEPS
        .contains(&word)
        .then(|| format!("{word} {}", text.trim()))
}

fn requirement_title(s: &str) -> Option<&str> {
    let s = s.trim().trim_matches('`');
    Some(s.strip_prefix("### Requirement:")?.trim())
}

fn parse_openspec(src: &str) -> OsSpec {
    let mut out = OsSpec::default();
    let mut group = String::new();
    let mut fenced = false;
    for line in src.lines() {
        if line.trim_start().starts_with("```") {
            fenced = !fenced;
        }
        if !fenced && let Some(name) = line.strip_prefix("## ") {
            group = name.trim().to_string();
            if group.ends_with("Requirements") {
                out.groups.push((group.clone(), Vec::new()));
            }
            continue;
        }
        if group == "Purpose" {
            out.purpose.push_str(line);
            out.purpose.push('\n');
        } else if group == "RENAMED Requirements" {
            let item = line.trim().trim_start_matches("- ");
            if let Some(from) = item.strip_prefix("FROM:") {
                out.renames.push((
                    requirement_title(from).unwrap_or("").to_string(),
                    String::new(),
                ));
            } else if let Some(to) = item.strip_prefix("TO:")
                && let Some(last) = out.renames.last_mut()
            {
                last.1 = requirement_title(to).unwrap_or("").to_string();
            }
        } else if let Some((_, reqs)) = out.groups.last_mut().filter(|_| !group.is_empty()) {
            if !fenced && let Some(title) = line.strip_prefix("### Requirement:") {
                reqs.push(OsReq {
                    title: title.trim().to_string(),
                    text: String::new(),
                    scenarios: Vec::new(),
                });
            } else if let Some(req) = reqs.last_mut() {
                if !fenced && let Some(title) = line.strip_prefix("#### Scenario:") {
                    req.scenarios.push(OsScenario {
                        title: title.trim().to_string(),
                        steps: Vec::new(),
                    });
                } else if let Some(sc) = req.scenarios.last_mut() {
                    match (step(line), sc.steps.last_mut()) {
                        (Some(s), _) => sc.steps.push(s),
                        (None, Some(prev)) if !line.trim().is_empty() => {
                            prev.push(' ');
                            prev.push_str(line.trim());
                        }
                        _ => {}
                    }
                } else if !line.trim().is_empty() {
                    if !req.text.is_empty() {
                        req.text.push(' ');
                    }
                    req.text.push_str(line.trim());
                }
            }
        }
    }
    out.purpose = out.purpose.trim().to_string();
    out
}

fn render_req(heading: &str, req: &OsReq) -> String {
    let mut s = format!("## {heading}\n\n{}\n", req.text);
    for sc in &req.scenarios {
        s.push_str(&format!("\n### Scenario: {}\n", sc.title));
        for st in &sc.steps {
            s.push_str(&format!("- {st}\n"));
        }
    }
    s
}

fn find_title<'a>(reqs: &'a [(Id, OsReq)], title: &str) -> Option<&'a (Id, OsReq)> {
    reqs.iter().find(|(_, r)| r.title == title).or_else(|| {
        reqs.iter()
            .find(|(_, r)| r.title.eq_ignore_ascii_case(title))
    })
}

struct Cap {
    name: String,
    reqs: Vec<(Id, OsReq)>,
}

type Mint<'a> = dyn FnMut(Kind, Option<&Id>) -> Result<Id> + 'a;

fn read(path: &Path) -> Result<String> {
    std::fs::read_to_string(path).map_err(|e| Error::io(path, e))
}

fn sorted_dirs(dir: &Path) -> Result<Vec<PathBuf>> {
    let mut dirs: Vec<PathBuf> = match std::fs::read_dir(dir) {
        Ok(entries) => entries
            .flatten()
            .map(|e| e.path())
            .filter(|p| p.is_dir())
            .collect(),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Vec::new(),
        Err(e) => return Err(Error::io(dir, e)),
    };
    dirs.sort();
    Ok(dirs)
}

fn file_name(path: &Path) -> String {
    path.file_name().unwrap().to_string_lossy().into_owned()
}

fn is_placeholder_purpose(purpose: &str) -> bool {
    purpose.starts_with("TBD")
}

/// Converts the living specs. Returns each capability with its minted ids, the
/// files to write and the record entries.
fn living(
    from: &Path,
    root: &Path,
    mint: &mut Mint,
    report: &mut Report,
    files: &mut Vec<(PathBuf, String)>,
) -> Result<(Vec<Cap>, record::Record)> {
    let mut caps = Vec::new();
    let mut rec = record::Record::new();
    for dir in sorted_dirs(&from.join("specs"))? {
        let path = dir.join("spec.md");
        if !path.is_file() {
            continue;
        }
        let name = file_name(&dir);
        let os = parse_openspec(&read(&path)?);
        let mut md = format!("# {name}\n");
        let mut reqs = Vec::new();
        for r in os.group("Requirements") {
            let id = mint(Kind::Req, None)?;
            md.push('\n');
            md.push_str(&render_req(&format!("{id} {}", r.title), r));
            reqs.push((id, r.clone()));
        }
        let (spec, diags) = parse_spec(&md);
        report.legacy_warnings += diags
            .iter()
            .chain(&lint::lint_spec(&spec))
            .filter(|d| d.severity() == crate::diag::Severity::Error)
            .count();
        for r in &spec.requirements {
            if let crate::id::Ref::Id(id) = &r.id {
                rec.insert(
                    id.to_string(),
                    Entry {
                        hash: requirement_hash(r),
                        legacy: true,
                    },
                );
            }
        }
        report.requirements += reqs.len();
        report.capabilities += 1;
        let spec_dir = root.join("specs").join(&name);
        files.push((spec_dir.join("spec.md"), md));
        if is_placeholder_purpose(&os.purpose) {
            report.purposes_skipped += 1;
        } else if !os.purpose.is_empty() {
            files.push((
                spec_dir.join("design.md"),
                format!("# Purpose\n\n{}\n", os.purpose),
            ));
        }
        caps.push(Cap { name, reqs });
    }
    Ok((caps, rec))
}

fn section(src: &str, names: &[&str]) -> Option<String> {
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

fn title_of(src: &str, name: &str) -> String {
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
                && b[..10].iter().enumerate().all(|(i, c)| {
                    if i == 4 || i == 7 {
                        *c == b'-'
                    } else {
                        c.is_ascii_digit()
                    }
                })
                && b[10] == b'-';
            let name = if dated { &name[11..] } else { name };
            name.replace(['-', '_'], " ")
        }
    }
}

fn proposal(src: &str, name: &str, branch: &str) -> (String, String) {
    let title = title_of(src, name);
    let bullets = |text: String| {
        text.lines()
            .filter(|l| !l.trim().is_empty())
            .map(|l| {
                if l.trim_start().starts_with("- ") {
                    l.to_string()
                } else {
                    format!("- {}", l.trim())
                }
            })
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
fn split_verify(text: &str) -> (&str, Option<&str>) {
    if let Some((title, clause)) = text.rsplit_once("; verify `")
        && let Some((cmd, tail)) = clause.split_once('`')
        && !tail.contains('`')
        && !cmd.is_empty()
    {
        return (title.trim(), Some(cmd));
    }
    (text.trim(), None)
}

fn task_line(line: &str) -> Option<(bool, &str)> {
    let mut s = line.trim_start();
    if let Some(r) = s.strip_prefix(['-', '*', '+']) {
        s = r;
    } else if let Some(r) = s
        .trim_start_matches(|c: char| c.is_ascii_digit())
        .strip_prefix(['.', ')'])
        && r.len() < s.len() - 1
    {
        s = r;
    }
    let s = s.trim_start().strip_prefix('[')?;
    let (mark, rest) = s.split_once(']')?;
    matches!(mark, " " | "x" | "X").then_some((mark != " ", rest.trim()))
}

fn tasks(src: &str, report: &mut ChangeReport) -> String {
    let mut out = String::from("# Tasks\n");
    let mut raw: Vec<String> = Vec::new();
    let mut flush = |raw: &mut Vec<String>, out: &mut String| {
        if let Some(text) = raw.pop() {
            let (number, text) = match text.split_once(' ') {
                Some((n, t))
                    if n.starts_with(|c: char| c.is_ascii_digit())
                        && n.chars().all(|c| c.is_ascii_digit() || c == '.') =>
                {
                    (n.trim_end_matches('.').to_string(), t)
                }
                _ => (String::new(), text.as_str()),
            };
            let (title, verify) = split_verify(text);
            report.tasks += 1;
            out.push_str(&format!("- TSK-new{} {title}\n", report.tasks));
            match verify {
                Some(cmd) => out.push_str(&format!("  verify: {cmd}\n")),
                None => report
                    .missing_verify
                    .push(format!("{number} {title}").trim().to_string()),
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
                let name = name
                    .split_once(". ")
                    .filter(|(n, _)| n.chars().all(|c| c.is_ascii_digit()))
                    .map_or(name, |(_, rest)| rest);
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

fn deltas(os: &OsSpec, cap: Option<&Cap>, next: &mut u32, rep: &mut ChangeReport) -> String {
    let empty = Vec::new();
    let reqs = cap.map_or(&empty, |c| &c.reqs);
    let label =
        |kind: &str, title: &str| format!("{} {kind} {title:?}", cap.map_or("?", |c| &c.name));
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
    let renamed_from = |title: &str| {
        os.renames
            .iter()
            .find(|(_, to)| to.eq_ignore_ascii_case(title))
            .map(|(from, _)| from.as_str())
    };
    for r in os.group("MODIFIED Requirements") {
        let hit = find_title(reqs, &r.title)
            .or_else(|| renamed_from(&r.title).and_then(|from| find_title(reqs, from)));
        match hit {
            Some((id, _)) if seen.insert(id) => {
                emit(render_req(&format!("CHANGE {id} {}", r.title), r))
            }
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
                let renamed = OsReq {
                    title: to.clone(),
                    ..living.clone()
                };
                emit(render_req(&format!("CHANGE {id} {to}"), &renamed));
            }
            Some(_) => {}
            None => rep.unmatched.push(label("RENAMED", from)),
        }
    }
    out.trim_end().to_string() + "\n"
}

fn change(
    dir: &Path,
    caps: &[Cap],
    branch: &str,
    root: &Path,
    mint: &mut Mint,
    files: &mut Vec<(PathBuf, String)>,
) -> Result<ChangeReport> {
    let name = file_name(dir);
    let proposal_src = std::fs::read_to_string(dir.join("proposal.md")).unwrap_or_default();
    let (title, proposal_md) = proposal(&proposal_src, &name, branch);
    let id = mint(Kind::Chg, None)?;
    let slug = Some(slugify(&title))
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "change".into());
    let out = root.join(Stage::Backlog.dir()).join(format!("{id}-{slug}"));
    let mut rep = ChangeReport {
        name,
        id,
        tasks: 0,
        missing_verify: Vec::new(),
        unmatched: Vec::new(),
    };
    files.push((out.join(change::PROPOSAL), proposal_md));
    let tasks_src = std::fs::read_to_string(dir.join("tasks.md")).unwrap_or_default();
    files.push((out.join(change::TASKS), tasks(&tasks_src, &mut rep)));
    if let Ok(design) = std::fs::read_to_string(dir.join("design.md")) {
        files.push((out.join(change::DESIGN), design));
    }
    let mut next = 0;
    for cap_dir in sorted_dirs(&dir.join("specs"))? {
        let cap_name = file_name(&cap_dir);
        let path = cap_dir.join("spec.md");
        if !path.is_file() {
            continue;
        }
        let os = parse_openspec(&read(&path)?);
        let md = deltas(
            &os,
            caps.iter().find(|c| c.name == cap_name),
            &mut next,
            &mut rep,
        );
        files.push((out.join(change::DELTAS).join(format!("{cap_name}.md")), md));
    }
    Ok(rep)
}

/// Converts the OpenSpec tree at `from` into the project: living specs, the
/// record, and one backlog change per open OpenSpec change. With `dry_run`
/// nothing is written and no id is registered.
pub fn openspec(project: &Project, from: &Path, branch: &str, dry_run: bool) -> Result<Report> {
    if !from.join("specs").is_dir() && !from.join("changes").is_dir() {
        return Err(Error::new(
            "import",
            format!("{} has no specs or changes folder", from.display()),
            "pass the OpenSpec folder with --from <dir>",
        ));
    }
    let lock = (!dry_run).then(|| project.lock()).transpose()?;
    if !change::load_living(project)?.0.is_empty() {
        return Err(Error::new(
            "import",
            "this project already has living specs, and import does not merge",
            "import into a project with no specs",
        ));
    }
    let mut taken = registry::load(project)?;
    let mut mint = |kind: Kind, change: Option<&Id>| match &lock {
        Some(lock) => registry::mint(project, lock, kind, change),
        None => {
            let id = Id::mint(kind, &taken);
            taken.insert(id.clone());
            Ok(id)
        }
    };
    let mut report = Report {
        dry_run,
        ..Report::default()
    };
    let mut files = Vec::new();
    let (caps, mut rec) = living(from, &project.root, &mut mint, &mut report, &mut files)?;
    let changes = from.join("changes");
    for dir in sorted_dirs(&changes)? {
        if file_name(&dir) == "archive" {
            report.archived = sorted_dirs(&dir)?.len();
        } else {
            let c = change(&dir, &caps, branch, &project.root, &mut mint, &mut files)?;
            report.changes.push(c);
        }
    }
    if let Some(lock) = &lock {
        for (path, text) in &files {
            let parent = path.parent().unwrap();
            std::fs::create_dir_all(parent).map_err(|e| Error::io(parent, e))?;
            std::fs::write(path, text).map_err(|e| Error::io(path, e))?;
        }
        let mut all = record::load(project)?;
        all.append(&mut rec);
        record::save(project, lock, &all)?;
    }
    Ok(report)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::git::testrepo::*;
    use crate::store::{Mode, init};

    const LIVING: &str = "\
# ledger Specification

## Purpose
Keep one proof ledger per change.

## Requirements

### Requirement: Record a proof run

ralibi SHALL run a command for a named task
and SHALL append one record. ralibi SHALL NOT rewrite records.

#### Scenario: First run appends a record
- **WHEN** `ralibi run` is invoked for task `2.1`
- **THEN** the ledger holds exactly one record
  for `2.1`
- **AND** the run exits with the command's status

### Requirement: Staleness

A proof record SHALL be stale when HEAD moved.

#### Scenario: Moved
- **GIVEN** a proof at `A`
- **WHEN** HEAD moves
- **THEN** the proof is stale
";

    const DELTA: &str = "\
# Ledger

## ADDED Requirements

### Requirement: Leave git to the project

ralibi SHALL only read the repository.

#### Scenario: Run leaves the index alone
- **WHEN** `ralibi run` records a proof
- **THEN** the git index is unchanged

## MODIFIED Requirements

### Requirement: record a PROOF run

ralibi SHALL run a command and record it.

#### Scenario: First run appends a record
- **WHEN** a run ends
- **THEN** a record is appended

## REMOVED Requirements

### Requirement: Staleness

**Reason**: folded into status.

### Requirement: Never heard of it

## RENAMED Requirements

- FROM: `### Requirement: Staleness`
- TO: `### Requirement: Freshness`

- FROM: `### Requirement: Missing one`
- TO: `### Requirement: Other`
";

    const TASKS: &str = "\
# Tasks

## 1. Proof context

- [x] 1.1 Record a fingerprint; verify `cargo test -p ralibi fingerprint` passes
- [ ] 1.2 Update the skill; verify with CLI tests for \"X\"
- [ ] 1.3 Run it; verify `a b` then `c d` exits zero

## 2. Skill
* [ ] 2.1 Star bullet
  continued here; verify `true`
";

    const PROPOSAL: &str = "\
# Proposal

## Why

Gaps exist.

## What Changes

- `run` records more.

## Impact

Touches the ledger.
- And the skill.
";

    fn setup(tmp: &TempDir) -> (Project, PathBuf) {
        let repo = repo_with_commit(&tmp.0, "r");
        let p = init(&repo, &tmp.0.join("home"), Mode::Home).unwrap();
        let os = tmp.0.join("openspec");
        let w = |rel: &str, text: &str| {
            let path = os.join(rel);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, text).unwrap();
        };
        w("specs/ledger/spec.md", LIVING);
        w("changes/2026-10-01-proof-context/proposal.md", PROPOSAL);
        w("changes/2026-10-01-proof-context/tasks.md", TASKS);
        w(
            "changes/2026-10-01-proof-context/design.md",
            "# Design\n\nD1.\n",
        );
        w(
            "changes/2026-10-01-proof-context/specs/ledger/spec.md",
            DELTA,
        );
        w("changes/archive/2026-09-29-old/proposal.md", "# Proposal\n");
        (p, os)
    }

    #[test]
    fn parses_headings_and_steps() {
        let os = parse_openspec(LIVING);
        assert_eq!(os.purpose, "Keep one proof ledger per change.");
        let reqs = os.group("Requirements");
        assert_eq!(reqs.len(), 2);
        assert_eq!(
            reqs[0].text,
            "ralibi SHALL run a command for a named task and SHALL append one record. ralibi SHALL NOT rewrite records."
        );
        assert_eq!(
            reqs[0].scenarios[0].steps,
            [
                "WHEN `ralibi run` is invoked for task `2.1`",
                "THEN the ledger holds exactly one record for `2.1`",
                "AND the run exits with the command's status"
            ]
        );
        let d = parse_openspec(DELTA);
        assert_eq!(d.group("ADDED Requirements").len(), 1);
        assert_eq!(d.group("REMOVED Requirements").len(), 2);
        assert_eq!(
            d.renames[0],
            ("Staleness".to_string(), "Freshness".to_string())
        );
    }

    #[test]
    fn verify_clause_rules() {
        assert_eq!(
            split_verify("Do it; verify `cargo test x` passes"),
            ("Do it", Some("cargo test x"))
        );
        assert_eq!(
            split_verify("Do it; verify with tests"),
            ("Do it; verify with tests", None)
        );
        assert_eq!(
            split_verify("Do; verify `a` then `b` exits"),
            ("Do; verify `a` then `b` exits", None)
        );
    }

    #[test]
    fn tasks_are_converted() {
        let mut rep = ChangeReport {
            name: "c".into(),
            id: "CHG-7k2mq8".parse().unwrap(),
            tasks: 0,
            missing_verify: Vec::new(),
            unmatched: Vec::new(),
        };
        let md = tasks(TASKS, &mut rep);
        assert_eq!(
            md,
            "\
# Tasks

## Proof context

- TSK-new1 Record a fingerprint
  verify: cargo test -p ralibi fingerprint
  files: **
- TSK-new2 Update the skill; verify with CLI tests for \"X\"
  files: **
- TSK-new3 Run it; verify `a b` then `c d` exits zero
  files: **

## Skill

- TSK-new4 Star bullet continued here
  verify: true
  files: **
"
        );
        assert_eq!(rep.tasks, 4);
        assert_eq!(rep.missing_verify.len(), 2);
        assert!(rep.missing_verify[0].starts_with("1.2 "));
        let (_, diags) = crate::tasks::parse_tasks(&md);
        assert_eq!(diags.len(), 2, "{diags:?}");
    }

    #[test]
    fn proposal_form() {
        let (title, md) = proposal(PROPOSAL, "2026-10-01-proof-context", "feat/x");
        assert_eq!(title, "proof context");
        let (p, diags) = crate::proposal::parse_proposal(&md);
        assert!(diags.is_empty(), "{diags:?}");
        assert_eq!(p.branch, "feat/x");
        assert!(md.contains("## Out of scope\n\n- Not stated in the OpenSpec proposal."));
        assert!(md.contains("## Assumptions\n\n- Touches the ledger.\n- And the skill.\n"));
        let (title, _) = proposal("# Add tails\n", "x", "b");
        assert_eq!(title, "Add tails");
        let (_, md) = proposal("## Why\nw\n## Non-goals\nNo signing.\n", "x", "b");
        assert!(md.contains("## Out of scope\n\nNo signing.\n"));
    }

    #[test]
    fn dry_run_writes_nothing() {
        let tmp = TempDir::new();
        let (p, os) = setup(&tmp);
        let r = openspec(&p, &os, "main", true).unwrap();
        assert!(r.dry_run);
        assert_eq!(r.requirements, 2);
        assert_eq!(r.changes.len(), 1);
        assert!(!p.root.join("specs/ledger").exists());
        assert!(!p.root.join("spec-record.json").exists());
        assert!(registry::load(&p).unwrap().is_empty());
    }

    #[test]
    fn imports_specs_changes_and_record() {
        let tmp = TempDir::new();
        let (p, os) = setup(&tmp);
        let r = openspec(&p, &os, "main", false).unwrap();
        assert_eq!((r.capabilities, r.requirements, r.archived), (1, 2, 1));
        assert_eq!(r.legacy_warnings, 2);

        let (living, findings) = change::load_living(&p).unwrap();
        let reqs = &living[0].1.requirements;
        assert_eq!(reqs.len(), 2);
        assert!(findings.iter().all(|(_, d)| d.legacy));
        assert!(!findings.is_empty());
        let design = std::fs::read_to_string(p.root.join("specs/ledger/design.md")).unwrap();
        assert_eq!(design, "# Purpose\n\nKeep one proof ledger per change.\n");
        let rec = record::load(&p).unwrap();
        assert_eq!(rec.len(), 2);
        for q in reqs {
            let crate::id::Ref::Id(id) = &q.id else {
                panic!()
            };
            let e = &rec[&id.to_string()];
            assert!(e.legacy);
            assert_eq!(e.hash, requirement_hash(q));
        }

        let c = &r.changes[0];
        assert_eq!(c.tasks, 4);
        assert_eq!(c.missing_verify.len(), 2);
        assert_eq!(
            c.unmatched,
            [
                "ledger REMOVED \"Never heard of it\"",
                "ledger RENAMED \"Missing one\""
            ]
        );
        let dir = change::resolve(&p, &c.id.to_string()).unwrap();
        assert_eq!(dir.stage, Stage::Backlog);
        assert_eq!(dir.slug, "proof-context");
        assert!(dir.path.join("design.md").is_file());
        let (a, b) = (&reqs[0].id, &reqs[1].id);
        let delta = std::fs::read_to_string(dir.path.join("specs/ledger.md")).unwrap();
        assert!(delta.contains("## ADD REQ-new1 Leave git to the project"));
        assert!(delta.contains(&format!("## CHANGE {a} record a PROOF run")));
        assert!(delta.contains(&format!("## REMOVE {b}\n")));
        assert!(!delta.contains("Freshness"));
        let loaded = change::load(&dir.path, &living).unwrap();
        assert!(
            loaded
                .findings
                .iter()
                .all(|(f, d)| f != "specs/ledger.md" || d.rule != crate::diag::Rule::UnknownId),
            "{:?}",
            loaded.findings
        );
        assert_eq!(loaded.deltas[0].1.deltas.len(), 3);
    }

    #[test]
    fn rename_without_modified_carries_the_living_text() {
        let tmp = TempDir::new();
        let (p, os) = setup(&tmp);
        std::fs::write(
            os.join("changes/2026-10-01-proof-context/specs/ledger/spec.md"),
            "## RENAMED Requirements\n\n- FROM: `### Requirement: Staleness`\n- TO: `### Requirement: Freshness`\n",
        )
        .unwrap();
        let r = openspec(&p, &os, "main", false).unwrap();
        let dir = change::resolve(&p, &r.changes[0].id.to_string()).unwrap();
        let delta = std::fs::read_to_string(dir.path.join("specs/ledger.md")).unwrap();
        assert!(delta.contains(" Freshness\n\nA proof record SHALL be stale when HEAD moved.\n"));
        assert!(delta.contains("- GIVEN a proof at `A`"));
        assert!(r.changes[0].unmatched.is_empty());
    }

    #[test]
    fn refuses_a_project_with_specs() {
        let tmp = TempDir::new();
        let (p, os) = setup(&tmp);
        openspec(&p, &os, "main", false).unwrap();
        assert_eq!(openspec(&p, &os, "main", false).unwrap_err().code, "import");
        assert_eq!(
            openspec(&p, &tmp.0.join("none"), "main", false)
                .unwrap_err()
                .code,
            "import"
        );
    }

    #[test]
    fn a_non_legacy_entry_keeps_its_errors() {
        let tmp = TempDir::new();
        let (p, os) = setup(&tmp);
        openspec(&p, &os, "main", false).unwrap();
        let mut rec = record::load(&p).unwrap();
        for e in rec.values_mut() {
            e.legacy = false;
        }
        let lock = p.lock().unwrap();
        record::save(&p, &lock, &rec).unwrap();
        let (_, findings) = change::load_living(&p).unwrap();
        assert!(
            findings
                .iter()
                .any(|(_, d)| d.severity() == crate::diag::Severity::Error)
        );
    }
}
