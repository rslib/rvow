//! Change folders: where they live, how they are named and how they load.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use crate::diag::{Diagnostic, Severity};
use crate::error::{Error, Result};
use crate::id::{self, Id, Kind, Ref};
use crate::lint::{self, Finding, Living};
use crate::proposal::{Proposal, parse_proposal};
use crate::record::{self, Record};
use crate::registry;
use crate::spec::{DeltaFile, DeltaOp, Requirement, Spec, parse_deltas, parse_spec};
use crate::store::Project;
use crate::tasks::{Mode, Task, apply_default, parse_tasks};

pub use rvow_base::layout::{DELTAS, DESIGN, LEDGER, PROPOSAL, TASKS};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Stage {
    Backlog,
    Active,
    Done,
    Dropped,
    Superseded,
}

impl Stage {
    pub const ALL: [Stage; 5] = [Stage::Backlog, Stage::Active, Stage::Done, Stage::Dropped, Stage::Superseded];

    pub fn dir(self) -> &'static str {
        match self {
            Stage::Backlog => "backlog",
            Stage::Active => "active",
            Stage::Done => "archive/done",
            Stage::Dropped => "archive/dropped",
            Stage::Superseded => "archive/superseded",
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Stage::Backlog => "backlog",
            Stage::Active => "active",
            Stage::Done => "done",
            Stage::Dropped => "dropped",
            Stage::Superseded => "superseded",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChangeDir {
    pub id: Id,
    pub slug: String,
    pub stage: Stage,
    pub path: PathBuf,
}

pub const MAX_DESIGN_LINES: usize = 100;

/// Splits `CHG-xxxxxx-slug`, with an optional `YYYY-MM-DD-` prefix in the
/// archive, into id and slug.
fn parse_dir_name(name: &str) -> Option<(Id, String)> {
    let start = name.find("CHG-")?;
    let prefix = &name[..start];
    let dated = prefix.len() == 11
        && prefix.ends_with('-')
        && prefix[..10].chars().enumerate().all(|(i, c)| if i == 4 || i == 7 { c == '-' } else { c.is_ascii_digit() });
    if !prefix.is_empty() && !dated {
        return None;
    }
    let rest = &name[start..];
    let id_len = 4 + id::BODY_LEN;
    let id: Id = rest.get(..id_len)?.parse().ok()?;
    let slug = match &rest[id_len..] {
        "" => String::new(),
        s => s.strip_prefix('-')?.to_string(),
    };
    Some((id, slug))
}

/// Every change folder of the project, and every folder in a stage directory
/// that does not have a change name.
pub fn list(project: &Project) -> Result<(Vec<ChangeDir>, Vec<PathBuf>)> {
    let mut changes = Vec::new();
    let mut stray = Vec::new();
    for stage in Stage::ALL {
        let dir = project.root.join(stage.dir());
        let entries = match std::fs::read_dir(&dir) {
            Ok(e) => e,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => continue,
            Err(e) => return Err(Error::io(&dir, e)),
        };
        for e in entries.flatten() {
            let path = e.path();
            if !path.is_dir() {
                continue;
            }
            let name = e.file_name().to_string_lossy().into_owned();
            match parse_dir_name(&name) {
                Some((id, slug)) => changes.push(ChangeDir { id, slug, stage, path }),
                None => stray.push(path),
            }
        }
    }
    changes.sort_by(|a, b| (a.stage, &a.id).cmp(&(b.stage, &b.id)));
    Ok((changes, stray))
}

/// Finds one change by a unique prefix of its id.
pub fn resolve(project: &Project, input: &str) -> Result<ChangeDir> {
    let (changes, _) = list(project)?;
    let ids: Vec<Id> = changes.iter().map(|c| c.id.clone()).collect();
    let input = if let Some((id, _)) = parse_dir_name(input) {
        id.to_string()
    } else if input.contains('-') {
        input.to_string()
    } else {
        format!("CHG-{input}")
    };
    let id = id::resolve(&input, &ids).map_err(|e| Error::new("change", format!("{input}: {e}"), "run rvow ls to see the changes"))?;
    changes.into_iter().find(|c| c.id == id).ok_or_else(|| Error::internal(format!("{id} resolved but is not listed")))
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Summary {
    pub id: Id,
    pub stage: Stage,
    pub branch: String,
    pub title: String,
}

/// One line of data per change in every stage.
pub fn summaries(project: &Project) -> Result<Vec<Summary>> {
    let (changes, _) = list(project)?;
    let mut out = Vec::new();
    for c in changes {
        let (p, _) = parse_proposal(&read(&c.path.join(PROPOSAL))?);
        out.push(Summary { id: c.id, stage: c.stage, branch: p.branch, title: p.title });
    }
    Ok(out)
}

/// Where a change, task or requirement lives: a folder for a change, else a
/// file and a line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Place {
    pub id: Id,
    pub file: PathBuf,
    pub line: Option<usize>,
}

/// Finds a change, task or requirement by a unique prefix of its id. Tasks
/// come from open changes, requirements from the living specs or from an open
/// change that adds them.
pub fn find(project: &Project, input: &str) -> Result<Place> {
    let (changes, _) = list(project)?;
    let (living, _) = load_living(project)?;
    let mut places: Vec<Place> = changes.iter().map(|c| Place { id: c.id.clone(), file: c.path.clone(), line: None }).collect();
    for (cap, spec) in &living {
        let file = project.root.join("specs").join(cap).join("spec.md");
        for r in &spec.requirements {
            if let Ref::Id(id) = &r.id {
                places.push(Place { id: id.clone(), file: file.clone(), line: Some(r.line) });
            }
        }
    }
    for c in changes.iter().filter(|c| matches!(c.stage, Stage::Backlog | Stage::Active)) {
        let loaded = load(&c.path, &living)?;
        for t in &loaded.tasks {
            if let Ref::Id(id) = &t.id {
                places.push(Place { id: id.clone(), file: c.path.join(TASKS), line: Some(t.line) });
            }
        }
        for (cap, f) in &loaded.deltas {
            let file = c.path.join(DELTAS).join(format!("{cap}.md"));
            for d in f.deltas.iter().filter(|d| d.op == DeltaOp::Add) {
                if let Ref::Id(id) = &d.req.id {
                    places.push(Place { id: id.clone(), file: file.clone(), line: Some(d.req.line) });
                }
            }
        }
    }
    let id = id::resolve(input, places.iter().map(|p| &p.id))
        .map_err(|e| Error::new("path", format!("{input}: {e}"), "give more characters of the id, or run rvow ls to see the changes"))?;
    places.into_iter().find(|p| p.id == id).ok_or_else(|| Error::internal(format!("{id} resolved but is not listed")))
}

/// `find` as one printable location: a folder, or `file:line`.
pub fn locate(project: &Project, input: &str) -> Result<String> {
    let place = find(project, input)?;
    Ok(match place.line {
        Some(line) => format!("{}:{line}", place.file.display()),
        None => place.file.display().to_string(),
    })
}

/// A lowercase ASCII slug of at most 40 characters.
pub fn slugify(title: &str) -> String {
    let mut slug = String::new();
    for c in title.chars() {
        if c.is_ascii_alphanumeric() {
            slug.push(c.to_ascii_lowercase());
        } else if !slug.ends_with('-') && !slug.is_empty() {
            slug.push('-');
        }
    }
    let mut slug: String = slug.trim_end_matches('-').chars().take(40).collect();
    while slug.ends_with('-') {
        slug.pop();
    }
    slug
}

fn proposal_template(branch: &str, title: &str, mode: Option<Mode>) -> String {
    let mode = mode.map(|m| format!("mode: {}\n", m.name())).unwrap_or_default();
    format!("branch: {branch}\n{mode}\n# {title}\n\n## Why\n\n## What changes\n\n## Out of scope\n\n## Assumptions\n")
}

/// Creates a change in `backlog/`. A small change has no `specs/` folder.
pub fn new(project: &Project, title: &str, branch: &str, small: bool) -> Result<ChangeDir> {
    new_with_mode(project, title, branch, small, None)
}

/// Like `new`, with the change-level task mode written into the proposal.
pub fn new_with_mode(project: &Project, title: &str, branch: &str, small: bool, mode: Option<Mode>) -> Result<ChangeDir> {
    let slug = slugify(title);
    if slug.is_empty() {
        return Err(Error::new("title", "the title has no letters or digits", "give the change a title in words"));
    }
    let lock = project.lock()?;
    let id = registry::mint(project, &lock, Kind::Chg, None)?;
    let path = project.root.join(Stage::Backlog.dir()).join(format!("{id}-{slug}"));
    let mut dirs = vec![path.clone()];
    if !small {
        dirs.push(path.join(DELTAS));
    }
    for d in &dirs {
        std::fs::create_dir_all(d).map_err(|e| Error::io(d, e))?;
    }
    for (name, body) in [(PROPOSAL, proposal_template(branch, title, mode)), (TASKS, "# Tasks\n".to_string())] {
        let p = path.join(name);
        std::fs::write(&p, body).map_err(|e| Error::io(&p, e))?;
    }
    Ok(ChangeDir { id, slug, stage: Stage::Backlog, path })
}

fn read(path: &Path) -> Result<String> {
    std::fs::read_to_string(path).map_err(|e| Error::io(path, e))
}

pub fn file_name(path: &Path) -> Result<String> {
    path.file_name().map(|n| n.to_string_lossy().into_owned()).ok_or_else(|| Error::internal(format!("{} has no name", path.display())))
}

fn md_files(dir: &Path) -> Result<Vec<PathBuf>> {
    let mut files = Vec::new();
    match std::fs::read_dir(dir) {
        Ok(entries) => {
            for e in entries.flatten() {
                let p = e.path();
                if p.extension().is_some_and(|x| x == "md") && p.is_file() {
                    files.push(p);
                }
            }
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => return Err(Error::io(dir, e)),
    }
    files.sort();
    Ok(files)
}

/// A living spec and its capability name.
pub type CapSpec = (String, Spec);

/// The text of each living spec, keyed by capability, in name order.
pub fn living_texts(project: &Project) -> Result<Vec<(String, String)>> {
    let dir = project.root.join("specs");
    let mut caps: Vec<PathBuf> = match std::fs::read_dir(&dir) {
        Ok(entries) => entries.flatten().map(|e| e.path()).filter(|p| p.is_dir()).collect(),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Vec::new(),
        Err(e) => return Err(Error::io(&dir, e)),
    };
    caps.sort();
    let mut out = Vec::new();
    for cap_dir in caps {
        let cap = file_name(&cap_dir)?;
        let path = cap_dir.join("spec.md");
        if path.is_file() {
            out.push((cap, read(&path)?));
        }
    }
    Ok(out)
}

/// Parses and checks living spec texts as `living_texts` returns them.
pub fn living_from(texts: &[(String, String)]) -> (Vec<CapSpec>, Vec<Finding>) {
    let mut specs = Vec::new();
    let mut findings = Vec::new();
    for (cap, text) in texts {
        let (spec, diags) = parse_spec(text);
        let rel = format!("specs/{cap}/spec.md");
        findings.extend(diags.into_iter().map(|d| (rel.clone(), d)));
        specs.push((cap.clone(), spec));
    }
    findings.extend(Living { specs: &specs }.lint());
    (specs, findings)
}

/// The living specs, keyed by capability, parsed without the lint or the
/// record: for a caller that needs the requirements and not their findings,
/// such as a hook.
pub fn living(project: &Project) -> Result<Vec<CapSpec>> {
    Ok(living_texts(project)?.into_iter().map(|(cap, text)| (cap, parse_spec(&text).0)).collect())
}

/// The living specs, keyed by capability, and their parse findings, with the
/// findings of untouched imported requirements marked legacy.
pub fn load_living(project: &Project) -> Result<(Vec<CapSpec>, Vec<Finding>)> {
    let (specs, mut findings) = living_from(&living_texts(project)?);
    mark_legacy(&mut findings, &specs, &record::load(project)?);
    Ok((specs, findings))
}

/// Marks each finding whose line lies in the block of a legacy requirement.
fn mark_legacy(findings: &mut [Finding], specs: &[CapSpec], record: &Record) {
    for (path, d) in findings {
        let Some((_, spec)) = specs.iter().find(|(cap, _)| *path == format!("specs/{cap}/spec.md")) else {
            continue;
        };
        let owner = spec.requirements.iter().rfind(|r| r.line <= d.line);
        d.legacy = owner.is_some_and(|r| match &r.id {
            Ref::Id(id) => record.get(&id.to_string()).is_some_and(|e| e.legacy),
            Ref::Placeholder { .. } => false,
        });
    }
}

#[derive(Debug, Clone)]
pub struct Loaded {
    pub proposal: Proposal,
    pub tasks: Vec<Task>,
    pub deltas: Vec<(String, DeltaFile)>,
    /// Every Markdown file, keyed by its path in the change.
    pub texts: Vec<(String, String)>,
    pub findings: Vec<Finding>,
}

impl Loaded {
    pub fn errors(&self) -> impl Iterator<Item = &Finding> {
        self.findings.iter().filter(|(_, d)| d.severity() == Severity::Error)
    }
}

/// One change, parsed without its texts or its lint: what a caller that reads
/// the tasks or the proposal only, such as a hook, needs.
#[derive(Debug, Clone)]
pub struct Parsed {
    pub proposal: Proposal,
    pub tasks: Vec<Task>,
    pub deltas: Vec<(String, DeltaFile)>,
}

fn tag(file: &str, diags: Vec<Diagnostic>) -> impl Iterator<Item = Finding> + '_ {
    diags.into_iter().map(move |d| (file.to_string(), d))
}

/// What a parse keeps for its caller: `load` keeps every text and finding, a
/// hook keeps neither, and so copies nothing and reads no unused file.
enum Sink<'a> {
    All { texts: &'a mut Vec<(String, String)>, findings: &'a mut Vec<Finding> },
    None,
}

impl Sink<'_> {
    /// Whether the caller wants what the parse finds, texts and all.
    fn open(&self) -> bool {
        matches!(self, Sink::All { .. })
    }

    fn text(&mut self, name: &str, text: String) {
        if let Sink::All { texts, .. } = self {
            texts.push((name.to_string(), text));
        }
    }

    fn diags(&mut self, file: &str, diags: Vec<Diagnostic>) {
        if let Sink::All { findings, .. } = self {
            findings.extend(tag(file, diags));
        }
    }
}

/// Parses and checks one change against the living specs.
pub fn load(dir: &Path, living: &[(String, Spec)]) -> Result<Loaded> {
    let mut texts = Vec::new();
    let mut findings = Vec::new();
    let parsed = parse(dir, &mut Sink::All { texts: &mut texts, findings: &mut findings })?;
    findings.extend(lint::Change { tasks: &parsed.tasks, deltas: &parsed.deltas, texts: &texts }.lint(&Living { specs: living }));
    Ok(Loaded { proposal: parsed.proposal, tasks: parsed.tasks, deltas: parsed.deltas, texts, findings })
}

/// Parses one change with no lint and no text copies: the hook path reads the
/// tasks and the proposal, never the findings.
pub fn parsed(dir: &Path) -> Result<Parsed> {
    parse(dir, &mut Sink::None)
}

fn parse(dir: &Path, sink: &mut Sink) -> Result<Parsed> {
    let proposal_text = read(&dir.join(PROPOSAL))?;
    let (proposal, diags) = parse_proposal(&proposal_text);
    sink.diags(PROPOSAL, diags);
    sink.text(PROPOSAL, proposal_text);
    let tasks_text = read(&dir.join(TASKS))?;
    let (mut tasks, diags) = parse_tasks(&tasks_text);
    apply_default(&mut tasks, proposal.mode);
    sink.diags(TASKS, diags);
    sink.text(TASKS, tasks_text);
    if sink.open()
        && let Ok(design) = std::fs::read_to_string(dir.join(DESIGN))
    {
        let lines = design.lines().count();
        if lines > MAX_DESIGN_LINES {
            sink.diags(
                DESIGN,
                vec![Diagnostic::new(
                    crate::diag::Rule::Size,
                    1,
                    format!("design.md has {lines} lines, more than {MAX_DESIGN_LINES}; keep only decisions, constraints and reasons"),
                )],
            );
        }
        sink.text(DESIGN, design);
    }
    let mut deltas = Vec::new();
    for path in md_files(&dir.join(DELTAS))? {
        let cap =
            path.file_stem().ok_or_else(|| Error::internal(format!("{} has no file name", path.display())))?.to_string_lossy().into_owned();
        let text = read(&path)?;
        let (file, diags) = parse_deltas(&text);
        let rel = format!("{DELTAS}/{cap}.md");
        sink.diags(&rel, diags);
        sink.text(&rel, text);
        deltas.push((cap, file));
    }
    Ok(Parsed { proposal, tasks, deltas })
}

/// The current hash of every requirement a task covers: the delta text when
/// the change adds or changes it, else the living text.
pub fn covered(task: &Task, loaded: &Loaded, living: &[CapSpec]) -> BTreeMap<String, String> {
    task.covers
        .iter()
        .map(|r| {
            let hash = find_requirement(r, loaded, living).map_or("missing".into(), crate::vow::requirement_hash);
            (r.to_string(), hash)
        })
        .collect()
}

pub fn find_requirement<'a>(r: &Ref, loaded: &'a Loaded, living: &'a [CapSpec]) -> Option<&'a Requirement> {
    loaded
        .deltas
        .iter()
        .flat_map(|(_, f)| f.deltas.iter())
        .filter(|d| d.op.has_body())
        .map(|d| &d.req)
        .chain(living.iter().flat_map(|(_, s)| s.requirements.iter()))
        .find(|req| &req.id == r)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::git::testrepo::*;
    use rvow_repo::store::{Mode, init};

    #[test]
    fn dir_names() {
        let id: Id = "CHG-7k2mq8".parse().unwrap();
        assert_eq!(parse_dir_name("CHG-7k2mq8-output-tail"), Some((id.clone(), "output-tail".into())));
        assert_eq!(parse_dir_name("2026-10-01-CHG-7k2mq8-x"), Some((id.clone(), "x".into())));
        assert_eq!(parse_dir_name("CHG-7k2mq8"), Some((id, String::new())));
        assert_eq!(parse_dir_name("notes"), None);
        assert_eq!(parse_dir_name("old-CHG-7k2mq8-x"), None);
        assert_eq!(parse_dir_name("CHG-7k2mq8x"), None);
    }

    #[test]
    fn slugs() {
        assert_eq!(slugify("Record the Output tail!"), "record-the-output-tail");
        assert_eq!(slugify("  --a__b--  "), "a-b");
        assert_eq!(slugify("x".repeat(50).as_str()).len(), 40);
        assert_eq!(slugify("..."), "");
    }

    #[test]
    fn new_list_resolve_and_load() {
        let tmp = TempDir::new();
        let repo = repo_with_commit(&tmp.0, "r");
        let p = init(&repo, &tmp.0.join("home"), Mode::Home).unwrap();
        let c = new(&p, "Output tail", "main", false).unwrap();
        assert!(c.path.join(DELTAS).is_dir());
        let small = new(&p, "Small fix", "main", true).unwrap();
        assert!(!small.path.join(DELTAS).exists());
        std::fs::create_dir_all(p.root.join("backlog/notes")).unwrap();

        let (changes, stray) = list(&p).unwrap();
        assert_eq!(changes.len(), 2);
        assert_eq!(stray, [p.root.join("backlog/notes")]);
        assert_eq!(resolve(&p, &c.id.body()[..5]).unwrap(), c);
        assert_eq!(resolve(&p, &c.id.to_string()).unwrap(), c);
        let folder = c.path.file_name().unwrap().to_str().unwrap();
        assert_eq!(resolve(&p, folder).unwrap(), c);

        let loaded = load(&c.path, &[]).unwrap();
        assert_eq!(loaded.proposal.branch, "main");
        assert_eq!(loaded.proposal.title, "Output tail");
        let sums = summaries(&p).unwrap();
        assert_eq!(sums.len(), 2);
        assert!(sums.iter().any(|s| s.id == c.id && s.stage == Stage::Backlog && s.title == "Output tail"));
        assert_eq!(loaded.errors().count(), 0, "{:?}", loaded.findings);
    }

    #[test]
    fn locate_finds_changes_tasks_and_requirements() {
        let tmp = TempDir::new();
        let repo = repo_with_commit(&tmp.0, "r");
        let p = init(&repo, &tmp.0.join("home"), Mode::Home).unwrap();
        let c = new(&p, "Out", "main", false).unwrap();
        std::fs::write(c.path.join(TASKS), "# Tasks\n\n- TSK-x7k2q4 Write\n  covers: REQ-k3f9m2\n  verify: true\n  files: a\n").unwrap();
        std::fs::write(c.path.join("specs/out.md"), "## ADD REQ-k3f9m2 Out\nrvow SHALL.\n### Scenario: s\n- WHEN a\n- THEN b\n").unwrap();
        let spec = p.root.join("specs/cap");
        std::fs::create_dir_all(&spec).unwrap();
        std::fs::write(spec.join("spec.md"), "# Cap\n\n## REQ-q8w2c7 Old\nrvow SHALL.\n### Scenario: s\n- WHEN a\n- THEN b\n").unwrap();

        assert_eq!(locate(&p, &c.id.to_string()).unwrap(), c.path.display().to_string());
        assert_eq!(locate(&p, "x7k").unwrap(), format!("{}:3", c.path.join(TASKS).display()));
        assert_eq!(locate(&p, "REQ-k3f").unwrap(), format!("{}:1", c.path.join("specs/out.md").display()));
        assert_eq!(locate(&p, "q8w").unwrap(), format!("{}:3", spec.join("spec.md").display()));
        assert_eq!(locate(&p, "zzz").unwrap_err().code, "path");
        assert!(locate(&p, "REQ-").unwrap_err().message.contains("no id matches"));
    }

    /// The hook path parses the change `load` parses, and no more: a task that
    /// covers a requirement no living spec holds is an error of the full load,
    /// and `parsed` cannot find it, since it takes no specs to lint against.
    #[test]
    fn scale_change_parsed_matches_load_and_runs_no_lint() {
        let tmp = TempDir::new();
        let repo = repo_with_commit(&tmp.0, "r");
        let p = init(&repo, &tmp.0.join("home"), Mode::Home).unwrap();
        let c = new(&p, "Out", "main", false).unwrap();
        std::fs::write(c.path.join(TASKS), "# Tasks\n\n- TSK-x7k2q4 Write\n  covers: REQ-k3f9m2\n  verify: true\n  files: a\n").unwrap();
        std::fs::write(c.path.join("specs/out.md"), "## ADD REQ-q8w2c7 Out\nrvow SHALL.\n### Scenario: s\n- WHEN a\n- THEN b\n").unwrap();

        let loaded = load(&c.path, &[]).unwrap();
        let errors = loaded.errors().count();
        assert!(errors > 0, "the fixture must fail the lint: {:?}", loaded.findings);
        let parsed = parsed(&c.path).unwrap();
        assert_eq!(parsed.tasks, loaded.tasks);
        assert_eq!(parsed.proposal, loaded.proposal);
        assert_eq!(parsed.deltas.len(), 1);
        assert_eq!(loaded.texts.len(), 3, "the full load keeps every text");
    }

    /// `living` answers from the living specs alone, so a corrupt record costs
    /// the hook path no parse of it at all.
    #[test]
    fn scale_change_living_reads_no_record() {
        let tmp = TempDir::new();
        let repo = repo_with_commit(&tmp.0, "r");
        let p = init(&repo, &tmp.0.join("home"), Mode::Home).unwrap();
        let spec = p.root.join("specs/cap");
        std::fs::create_dir_all(&spec).unwrap();
        std::fs::write(spec.join("spec.md"), "# Cap\n\n## REQ-q8w2c7 Old\nrvow SHALL.\n### Scenario: s\n- WHEN a\n- THEN b\n").unwrap();
        let record = p.root.join(record::RECORD_FILE);
        std::fs::write(&record, "{ not json").unwrap();
        assert!(record::load(&p).is_err(), "the fixture record must be the corrupt one");

        let living = living(&p).unwrap();
        assert_eq!(living.len(), 1);
        assert_eq!(living[0].0, "cap");
        assert_eq!(living[0].1.requirements.len(), 1);
    }
}
