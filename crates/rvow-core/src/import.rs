//! `rvow import openspec`: converts the living specs and the open changes of an
//! OpenSpec tree into rvow's form. The requirement text is copied as written, so
//! imported requirements start as legacy and keep their lint errors as warnings.

use std::fmt;
use std::path::{Path, PathBuf};

use crate::change;
use crate::error::{Error, Result};
use crate::id::{Id, Kind};
use crate::lint;
use crate::record::{self, Entry};
use crate::registry;
use crate::spec::parse_spec;
use crate::store::Project;
use crate::vow::requirement_hash;

mod changes;
mod openspec;
mod tree;

#[cfg(test)]
use crate::change::Stage;
use changes::change;
#[cfg(test)]
use changes::{proposal, split_verify, tasks};
use openspec::{parse_openspec, render_req};
use tree::{Cap, Mint, copy_history, existing_caps, preflight, read, sorted_dirs, validate_schema};

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
        let missing: Vec<_> = self.changes.iter().flat_map(|c| c.missing_verify.iter().map(move |t| (&c.name, t))).collect();
        if !missing.is_empty() {
            writeln!(f, "tasks without verify, fix before accept:")?;
            for (name, t) in missing {
                writeln!(f, "  {name} {t}")?;
            }
        }
        let unmatched: Vec<_> = self.changes.iter().flat_map(|c| c.unmatched.iter().map(move |t| (&c.name, t))).collect();
        if !unmatched.is_empty() {
            writeln!(f, "unmatched titles, not converted:")?;
            for (name, t) in unmatched {
                writeln!(f, "  {name} {t}")?;
            }
        }
        Ok(())
    }
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
    let mut caps = existing_caps(root)?;
    let mut rec = record::Record::new();
    for dir in sorted_dirs(&from.join("specs"))? {
        let path = dir.join("spec.md");
        if !path.is_file() {
            continue;
        }
        let name = change::file_name(&dir)?;
        let os = parse_openspec(&read(&path)?);
        let spec_dir = root.join("specs").join(&name);
        let spec_path = spec_dir.join("spec.md");
        let mut md = if spec_path.is_file() { read(&spec_path)? } else { format!("# {name}\n") };
        if !md.ends_with('\n') {
            md.push('\n');
        }
        let mut reqs = Vec::new();
        for r in os.group("Requirements") {
            let id = mint(Kind::Req, None)?;
            md.push('\n');
            md.push_str(&render_req(&format!("{id} {}", r.title), r));
            reqs.push((id, r.clone()));
        }
        let (spec, diags) = parse_spec(&md);
        report.legacy_warnings +=
            diags.iter().chain(&lint::lint_spec(&spec)).filter(|d| d.severity() == crate::diag::Severity::Error).count();
        for r in &spec.requirements {
            if let crate::id::Ref::Id(id) = &r.id
                && reqs.iter().any(|(new_id, _)| new_id == id)
            {
                rec.insert(id.to_string(), Entry { hash: requirement_hash(r), legacy: true });
            }
        }
        report.requirements += reqs.len();
        report.capabilities += 1;
        files.push((spec_path, md));
        if is_placeholder_purpose(&os.purpose) {
            report.purposes_skipped += 1;
        } else if !os.purpose.is_empty() && !spec_dir.join("design.md").exists() {
            files.push((spec_dir.join("design.md"), format!("# Purpose\n\n{}\n", os.purpose)));
        }
        if let Some(cap) = caps.iter_mut().find(|cap| cap.name == name) {
            cap.reqs.extend(reqs);
        } else {
            caps.push(Cap { name, reqs });
        }
    }
    Ok((caps, rec))
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
    validate_schema(from)?;
    let lock = (!dry_run).then(|| project.lock()).transpose()?;
    preflight(from, &project.root)?;
    let mut taken = registry::load(project)?;
    let mut mint = |kind: Kind, change: Option<&Id>| match &lock {
        Some(lock) => registry::mint(project, lock, kind, change),
        None => {
            let id = Id::mint(kind, &taken)?;
            taken.insert(id.clone());
            Ok(id)
        }
    };
    let mut report = Report { dry_run, ..Report::default() };
    let mut files = Vec::new();
    let (caps, mut rec) = living(from, &project.root, &mut mint, &mut report, &mut files)?;
    let changes = from.join("changes");
    for dir in sorted_dirs(&changes)? {
        if change::file_name(&dir)? == "archive" {
            for historical in sorted_dirs(&dir)? {
                let name = change::file_name(&historical)?;
                copy_history(&historical, &project.root.join("archive/openspec").join(name), &mut files)?;
                report.archived += 1;
            }
        } else {
            let c = change(&dir, &caps, branch, &project.root, &mut mint, &mut files)?;
            report.changes.push(c);
        }
    }
    if let Some(lock) = &lock {
        for (path, text) in &files {
            if let Some(parent) = path.parent() {
                std::fs::create_dir_all(parent).map_err(|e| Error::io(parent, e))?;
            }
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
        w("changes/2026-10-01-proof-context/design.md", "# Design\n\nD1.\n");
        w("changes/2026-10-01-proof-context/specs/ledger/spec.md", DELTA);
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
        assert_eq!(d.renames[0], ("Staleness".to_string(), "Freshness".to_string()));
    }

    #[test]
    fn verify_clause_rules() {
        assert_eq!(split_verify("Do it; verify `cargo test x` passes"), ("Do it", Some("cargo test x")));
        assert_eq!(split_verify("Do it; verify with tests"), ("Do it; verify with tests", None));
        assert_eq!(split_verify("Do; verify `a` then `b` exits"), ("Do; verify `a` then `b` exits", None));
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
            let crate::id::Ref::Id(id) = &q.id else { panic!() };
            let e = &rec[&id.to_string()];
            assert!(e.legacy);
            assert_eq!(e.hash, requirement_hash(q));
        }

        let c = &r.changes[0];
        assert_eq!(c.tasks, 4);
        assert_eq!(c.missing_verify.len(), 2);
        assert_eq!(c.unmatched, ["ledger REMOVED \"Never heard of it\"", "ledger RENAMED \"Missing one\""]);
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
            loaded.findings.iter().all(|(f, d)| f != "specs/ledger.md" || d.rule != crate::diag::Rule::UnknownId),
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
        assert_eq!(openspec(&p, &tmp.0.join("none"), "main", false).unwrap_err().code, "import");
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
        assert!(findings.iter().any(|(_, d)| d.severity() == crate::diag::Severity::Error));
    }
}
