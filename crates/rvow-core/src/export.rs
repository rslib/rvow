//! `rvow export openspec`: a one-way export of the living specs, the open
//! changes and the done changes into an OpenSpec tree, so a user can leave
//! rvow. Every requirement keeps its rvow id in an HTML comment.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::{Path, PathBuf};

use crate::change::{self, ChangeDir, Stage};
use crate::error::{Error, Result};
use crate::spec::{DeltaOp, Requirement, parse_deltas, parse_spec};
use crate::store::Project;
use crate::tasks::{Task, parse_tasks};

const NO_PURPOSE: &str = "TBD - created by rvow export, add a purpose.";

#[derive(Debug, Default)]
pub struct Report {
    pub dry_run: bool,
    pub target: PathBuf,
    pub specs: usize,
    pub requirements: usize,
    pub open: usize,
    pub archived: usize,
    pub dropped: usize,
    pub superseded: usize,
    /// Every file written, or that a dry run would write, relative to the target.
    pub files: Vec<String>,
    pub deprecated_as_removed: usize,
    pub moved_as_added: usize,
    pub unknown_titles: usize,
    pub verify_in_subline: usize,
    pub dropped_task_fields: usize,
}

impl Report {
    /// What the export loses or approximates, one sentence each.
    pub fn lost(&self) -> Vec<String> {
        let mut out = vec![
            "ledgers and proofs of every change are not exported, so OpenSpec tasks start unproven".to_owned(),
            "vow hashes, the spec record and the legacy flags are not exported".to_owned(),
            "branch and after lines of proposals are not exported".to_owned(),
            "verify commands live only on the task line, as ``verify `cmd` ``".to_owned(),
        ];
        let mut add = |n: usize, text: &str| {
            if n > 0 {
                out.push(format!("{n} {text}"));
            }
        };
        add(self.deprecated_as_removed, "DEPRECATE deltas became REMOVED, because OpenSpec has no deprecate");
        add(self.moved_as_added, "MOVE deltas became ADDED in the target capability, remove the old copy by hand");
        add(self.unknown_titles, "removed requirements of archived changes have no known title, so the id stands in");
        add(self.verify_in_subline, "verify commands hold a backtick and sit in a sub-line, not on the task line");
        add(self.dropped_task_fields, "tasks lose their mutants, baseline or timeout fields");
        add(self.dropped, "dropped changes were not exported");
        add(self.superseded, "superseded changes were not exported");
        out
    }
}

type Files = Vec<(String, String)>;

fn read(path: &Path) -> Result<String> {
    std::fs::read_to_string(path).map_err(|e| Error::io(path, e))
}

fn block(r: &Requirement, note: Option<&str>) -> String {
    let flag = if r.deprecated { " deprecated" } else { "" };
    let mut s = format!("### Requirement: {}\n<!-- rvow: {}{flag} -->\n", r.title, r.id);
    if let Some(n) = note {
        s.push_str(&format!("<!-- {n} -->\n"));
    }
    if r.deprecated {
        s.push_str("(deprecated)\n");
    }
    if !r.text.is_empty() {
        s.push_str(&format!("{}\n", r.text));
    }
    for sc in &r.scenarios {
        s.push_str(&format!("\n#### Scenario: {}\n", sc.title));
        for st in &sc.steps {
            s.push_str(&format!("- **{}** {}\n", st.kind.word(), st.text));
        }
    }
    s
}

fn removed_block(id: &str, title: &str, note: &str, change: &str) -> String {
    format!(
        "### Requirement: {title}\n<!-- rvow: {id} -->\n<!-- {note} -->\n**Reason**: removed by rvow change {change}.\n**Migration**: none recorded in rvow.\n"
    )
}

fn purpose(project: &Project, cap: &str) -> String {
    let text = std::fs::read_to_string(project.root.join("specs").join(cap).join("design.md")).unwrap_or_default();
    let body = text.strip_prefix("# Purpose").map(str::trim).unwrap_or("");
    if body.is_empty() { NO_PURPOSE.to_owned() } else { body.to_owned() }
}

fn sections(src: &str) -> Vec<(String, String)> {
    let mut out: Vec<(String, String)> = Vec::new();
    for line in src.lines() {
        if let Some(name) = line.strip_prefix("## ") {
            out.push((name.trim().to_owned(), String::new()));
        } else if let Some((_, body)) = out.last_mut() {
            body.push_str(line);
            body.push('\n');
        }
    }
    out
}

fn proposal(src: &str, c: &ChangeDir) -> String {
    let title = src
        .lines()
        .find_map(|l| l.strip_prefix("# "))
        .map(str::trim)
        .filter(|t| !t.is_empty())
        .map_or_else(|| c.slug.replace('-', " "), str::to_owned);
    let mut out = format!("# {title}\n<!-- rvow: {} -->\n", c.id);
    for (name, body) in sections(src) {
        let name = match name.as_str() {
            "What changes" => "What Changes",
            "Out of scope" => "Non-goals",
            other => other,
        };
        out.push_str(&format!("\n## {name}\n\n{}\n", body.trim()));
    }
    out
}

fn tasks_md(tasks: &[Task], done: bool, rep: &mut Report) -> String {
    let mark = if done { "x" } else { " " };
    let mut groups: Vec<(&str, Vec<&Task>)> = Vec::new();
    for t in tasks {
        let name = t.group.as_deref().unwrap_or("Tasks");
        match groups.iter_mut().find(|(g, _)| *g == name) {
            Some((_, ts)) => ts.push(t),
            None => groups.push((name, vec![t])),
        }
    }
    let mut out = String::from("# Tasks\n");
    for (gi, (name, ts)) in groups.iter().enumerate() {
        out.push_str(&format!("\n## {}. {name}\n\n", gi + 1));
        for (ti, t) in ts.iter().enumerate() {
            let mut subs = Vec::new();
            let mut verify = String::new();
            match &t.verify {
                Some(v) if !v.contains('`') => verify = format!("; verify `{v}`"),
                Some(v) => {
                    rep.verify_in_subline += 1;
                    subs.push(format!("verify: {v}"));
                }
                None => {}
            }
            if t.mutants.is_some() || t.baseline_pass || t.timeout.is_some() {
                rep.dropped_task_fields += 1;
            }
            if !t.covers.is_empty() {
                subs.push(format!("covers: {}", t.covers.iter().map(|r| r.to_string()).collect::<Vec<_>>().join(", ")));
            }
            if !t.files.is_empty() {
                subs.push(format!("files: {}", t.files.join(", ")));
            }
            out.push_str(&format!("- [{mark}] {}.{} {}{verify}\n", gi + 1, ti + 1, t.title));
            for s in subs {
                out.push_str(&format!("  - {s}\n"));
            }
        }
    }
    out
}

#[derive(Default)]
struct CapDelta {
    added: Vec<String>,
    modified: Vec<String>,
    removed: Vec<String>,
    renamed: Vec<(String, String)>,
}

impl CapDelta {
    fn render(&self) -> Option<String> {
        let mut out = String::new();
        for (name, blocks) in [("ADDED", &self.added), ("MODIFIED", &self.modified), ("REMOVED", &self.removed)] {
            if !blocks.is_empty() {
                out.push_str(&format!("## {name} Requirements\n\n{}\n", blocks.join("\n")));
            }
        }
        if !self.renamed.is_empty() {
            out.push_str("## RENAMED Requirements\n\n");
            for (from, to) in &self.renamed {
                out.push_str(&format!("- FROM: `### Requirement: {from}`\n- TO: `### Requirement: {to}`\n"));
            }
        }
        (!out.is_empty()).then_some(out)
    }
}

type Living = HashMap<String, (String, Requirement)>;

fn change_files(dir: &str, c: &ChangeDir, living: &Living, rep: &mut Report, files: &mut Files) -> Result<()> {
    let done = c.stage == Stage::Done;
    let mut push = |name: &str, text: String| files.push((format!("changes/{dir}/{name}"), text));
    push("proposal.md", proposal(&read(&c.path.join(change::PROPOSAL))?, c));
    if let Ok(design) = std::fs::read_to_string(c.path.join(change::DESIGN)) {
        push("design.md", design);
    }
    let (tasks, _) = parse_tasks(&read(&c.path.join(change::TASKS))?);
    push("tasks.md", tasks_md(&tasks, done, rep));

    let specs = c.path.join(change::DELTAS);
    let mut paths: Vec<PathBuf> = match std::fs::read_dir(&specs) {
        Ok(entries) => entries.flatten().map(|e| e.path()).filter(|p| p.extension().is_some_and(|x| x == "md")).collect(),
        Err(_) => Vec::new(),
    };
    paths.sort();
    let id = c.id.to_string();
    for path in paths {
        let cap = path.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
        let (file, _) = parse_deltas(&read(&path)?);
        let mut d = CapDelta::default();
        for delta in file.deltas {
            let r = &delta.req;
            let rid = r.id.to_string();
            let known = living.get(&rid);
            match delta.op {
                DeltaOp::Add => d.added.push(block(r, None)),
                DeltaOp::Change => {
                    if let Some((_, old)) = known.filter(|(_, old)| !done && old.title != r.title) {
                        d.renamed.push((old.title.clone(), r.title.clone()));
                    }
                    d.modified.push(block(r, None));
                }
                DeltaOp::Remove | DeltaOp::Deprecate => {
                    let title = known.map_or_else(
                        || {
                            rep.unknown_titles += 1;
                            rid.clone()
                        },
                        |(_, old)| old.title.clone(),
                    );
                    let note = if delta.op == DeltaOp::Deprecate {
                        rep.deprecated_as_removed += 1;
                        "rvow DEPRECATE became REMOVED"
                    } else {
                        "rvow REMOVE"
                    };
                    d.removed.push(removed_block(&rid, &title, note, &id));
                }
                DeltaOp::Move => match known {
                    Some((from, old)) => {
                        rep.moved_as_added += 1;
                        let note = format!("rvow MOVE from capability {from}; OpenSpec has no move, remove the old copy there");
                        d.added.push(block(old, Some(&note)));
                    }
                    None => {
                        rep.unknown_titles += 1;
                        d.added.push(removed_block(&rid, &rid, "rvow MOVE of a requirement with no living text", &id));
                    }
                },
            }
        }
        if let Some(text) = d.render() {
            push(&format!("specs/{cap}/spec.md"), text);
        }
    }
    Ok(())
}

fn dated(name: &str) -> Option<&str> {
    let b = name.as_bytes();
    let ok = b.len() > 11
        && b[10] == b'-'
        && b[..10].iter().enumerate().all(|(i, c)| if i == 4 || i == 7 { *c == b'-' } else { c.is_ascii_digit() });
    ok.then(|| &name[..10])
}

fn unique(taken: &mut HashSet<String>, name: String, c: &ChangeDir) -> String {
    let name = if taken.contains(&name) { format!("{name}-{}", c.id) } else { name };
    taken.insert(name.clone());
    name
}

/// Exports the project into `<to>/openspec`. With `dry_run` nothing is
/// written. An existing `<to>/openspec` is never touched.
pub fn openspec(project: &Project, to: &Path, dry_run: bool) -> Result<Report> {
    let root = to.join("openspec");
    if root.exists() {
        return Err(Error::new(
            "export",
            format!("{} already exists", root.display()),
            "pick another directory with --to <dir>; export never overwrites or merges",
        ));
    }
    let mut rep = Report { dry_run, target: to.to_path_buf(), ..Report::default() };
    let mut files: Files = vec![("config.yaml".to_owned(), "schema: spec-driven\n".to_owned())];

    let mut living = Living::new();
    for (cap, text) in change::living_texts(project)? {
        let (spec, _) = parse_spec(&text);
        let mut md = format!("# {cap} Specification\n\n## Purpose\n\n{}\n\n## Requirements\n", purpose(project, &cap));
        for r in &spec.requirements {
            md.push('\n');
            md.push_str(&block(r, None));
            living.insert(r.id.to_string(), (cap.clone(), r.clone()));
        }
        rep.specs += 1;
        rep.requirements += spec.requirements.len();
        files.push((format!("specs/{cap}/spec.md"), md));
    }

    let (changes, _) = change::list(project)?;
    let mut taken: HashSet<String> = HashSet::from(["archive".to_owned()]);
    let mut archive_taken = HashSet::new();
    for c in &changes {
        let slug = if c.slug.is_empty() { c.id.to_string() } else { c.slug.clone() };
        match c.stage {
            Stage::Backlog | Stage::Active => {
                let dir = unique(&mut taken, slug, c);
                change_files(&dir, c, &living, &mut rep, &mut files)?;
                rep.open += 1;
            }
            Stage::Done => {
                let name = change::file_name(&c.path)?;
                let date = dated(&name).map_or(String::new(), |d| format!("{d}-"));
                let dir = unique(&mut archive_taken, format!("{date}{slug}"), c);
                change_files(&format!("archive/{dir}"), c, &living, &mut rep, &mut files)?;
                rep.archived += 1;
            }
            Stage::Dropped => rep.dropped += 1,
            Stage::Superseded => rep.superseded += 1,
        }
    }

    let by_path: BTreeMap<_, _> = files.into_iter().collect();
    rep.files = by_path.keys().map(|p| format!("openspec/{p}")).collect();
    if !dry_run {
        let write = || -> Result<()> {
            for (rel, text) in &by_path {
                let path = root.join(rel);
                if let Some(parent) = path.parent() {
                    std::fs::create_dir_all(parent).map_err(|e| Error::io(parent, e))?;
                }
                std::fs::write(&path, text).map_err(|e| Error::io(&path, e))?;
            }
            Ok(())
        };
        if let Err(e) = write() {
            let _ = std::fs::remove_dir_all(&root);
            return Err(e);
        }
    }
    Ok(rep)
}
