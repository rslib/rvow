//! Change folders: where they live, how they are named and how they load.

use std::path::{Path, PathBuf};

use crate::diag::{Diagnostic, Severity};
use crate::error::{Error, Result};
use crate::id::{self, Id, Kind, Ref};
use crate::lint::{self, Finding, Living};
use crate::proposal::{Proposal, parse_proposal};
use crate::registry;
use crate::spec::{DeltaFile, DeltaOp, Spec, parse_deltas, parse_spec};
use crate::store::Project;
use crate::tasks::{Task, parse_tasks};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Stage {
    Backlog,
    Active,
    Done,
    Dropped,
    Superseded,
}

impl Stage {
    pub const ALL: [Stage; 5] = [
        Stage::Backlog,
        Stage::Active,
        Stage::Done,
        Stage::Dropped,
        Stage::Superseded,
    ];

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

pub const PROPOSAL: &str = "proposal.md";
pub const DESIGN: &str = "design.md";
pub const TASKS: &str = "tasks.md";
pub const DELTAS: &str = "specs";
pub const LEDGER: &str = "ledger.jsonl";
pub const MAX_DESIGN_LINES: usize = 300;

/// Splits `CHG-xxxxxx-slug`, with an optional `YYYY-MM-DD-` prefix in the
/// archive, into id and slug.
fn parse_dir_name(name: &str) -> Option<(Id, String)> {
    let start = name.find("CHG-")?;
    let prefix = &name[..start];
    let dated = prefix.len() == 11
        && prefix.ends_with('-')
        && prefix[..10].chars().enumerate().all(|(i, c)| {
            if i == 4 || i == 7 {
                c == '-'
            } else {
                c.is_ascii_digit()
            }
        });
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
                Some((id, slug)) => changes.push(ChangeDir {
                    id,
                    slug,
                    stage,
                    path,
                }),
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
    let id = id::resolve(&input, &ids).map_err(|e| {
        Error::new(
            "change",
            format!("{input}: {e}"),
            "run rvow ls to see the changes",
        )
    })?;
    Ok(changes
        .into_iter()
        .find(|c| c.id == id)
        .expect("resolved id is listed"))
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
        out.push(Summary {
            id: c.id,
            stage: c.stage,
            branch: p.branch,
            title: p.title,
        });
    }
    Ok(out)
}

/// Where a change, task or requirement lives: a folder for a change, else
/// `file:line`. Tasks come from open changes, requirements from the living
/// specs or from an open change that adds them.
pub fn locate(project: &Project, input: &str) -> Result<String> {
    let (changes, _) = list(project)?;
    let (living, _) = load_living(project)?;
    let mut places: Vec<(Id, String)> = changes
        .iter()
        .map(|c| (c.id.clone(), c.path.display().to_string()))
        .collect();
    for (cap, spec) in &living {
        let file = project.root.join("specs").join(cap).join("spec.md");
        for r in &spec.requirements {
            if let Ref::Id(id) = &r.id {
                places.push((id.clone(), format!("{}:{}", file.display(), r.line)));
            }
        }
    }
    for c in changes
        .iter()
        .filter(|c| matches!(c.stage, Stage::Backlog | Stage::Active))
    {
        let loaded = load(&c.path, &living)?;
        for t in &loaded.tasks {
            if let Ref::Id(id) = &t.id {
                let file = c.path.join(TASKS);
                places.push((id.clone(), format!("{}:{}", file.display(), t.line)));
            }
        }
        for (cap, f) in &loaded.deltas {
            let file = c.path.join(DELTAS).join(format!("{cap}.md"));
            for d in f.deltas.iter().filter(|d| d.op == DeltaOp::Add) {
                if let Ref::Id(id) = &d.req.id {
                    places.push((id.clone(), format!("{}:{}", file.display(), d.req.line)));
                }
            }
        }
    }
    let id = id::resolve(input, places.iter().map(|(id, _)| id)).map_err(|e| {
        Error::new(
            "path",
            format!("{input}: {e}"),
            "give more characters of the id, or run rvow ls to see the changes",
        )
    })?;
    Ok(places
        .into_iter()
        .find(|(i, _)| *i == id)
        .expect("resolved id is listed")
        .1)
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

fn proposal_template(branch: &str, title: &str) -> String {
    format!(
        "branch: {branch}\n\n# {title}\n\n## Why\n\n## What changes\n\n## Out of scope\n\n## Assumptions\n"
    )
}

/// Creates a change in `backlog/`. A small change has no `specs/` folder.
pub fn new(project: &Project, title: &str, branch: &str, small: bool) -> Result<ChangeDir> {
    let slug = slugify(title);
    if slug.is_empty() {
        return Err(Error::new(
            "title",
            "the title has no letters or digits",
            "give the change a title in words",
        ));
    }
    let lock = project.lock()?;
    let id = registry::mint(project, &lock, Kind::Chg, None)?;
    let path = project
        .root
        .join(Stage::Backlog.dir())
        .join(format!("{id}-{slug}"));
    let mut dirs = vec![path.clone()];
    if !small {
        dirs.push(path.join(DELTAS));
    }
    for d in &dirs {
        std::fs::create_dir_all(d).map_err(|e| Error::io(d, e))?;
    }
    for (name, body) in [
        (PROPOSAL, proposal_template(branch, title)),
        (TASKS, "# Tasks\n".to_string()),
    ] {
        let p = path.join(name);
        std::fs::write(&p, body).map_err(|e| Error::io(&p, e))?;
    }
    Ok(ChangeDir {
        id,
        slug,
        stage: Stage::Backlog,
        path,
    })
}

fn read(path: &Path) -> Result<String> {
    std::fs::read_to_string(path).map_err(|e| Error::io(path, e))
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

/// The living specs, keyed by capability, and their parse findings.
pub fn load_living(project: &Project) -> Result<(Vec<CapSpec>, Vec<Finding>)> {
    let dir = project.root.join("specs");
    let mut specs = Vec::new();
    let mut findings = Vec::new();
    let mut caps: Vec<PathBuf> = match std::fs::read_dir(&dir) {
        Ok(entries) => entries
            .flatten()
            .map(|e| e.path())
            .filter(|p| p.is_dir())
            .collect(),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Vec::new(),
        Err(e) => return Err(Error::io(&dir, e)),
    };
    caps.sort();
    for cap_dir in caps {
        let cap = cap_dir.file_name().unwrap().to_string_lossy().into_owned();
        let path = cap_dir.join("spec.md");
        if !path.is_file() {
            continue;
        }
        let (spec, diags) = parse_spec(&read(&path)?);
        let rel = format!("specs/{cap}/spec.md");
        findings.extend(diags.into_iter().map(|d| (rel.clone(), d)));
        specs.push((cap, spec));
    }
    findings.extend(Living { specs: &specs }.lint());
    Ok((specs, findings))
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
        self.findings
            .iter()
            .filter(|(_, d)| d.severity() == Severity::Error)
    }
}

fn tag(file: &str, diags: Vec<Diagnostic>) -> impl Iterator<Item = Finding> + '_ {
    diags.into_iter().map(move |d| (file.to_string(), d))
}

/// Parses and checks one change against the living specs.
pub fn load(dir: &Path, living: &[(String, Spec)]) -> Result<Loaded> {
    let mut texts = Vec::new();
    let mut findings = Vec::new();
    let proposal_text = read(&dir.join(PROPOSAL))?;
    let (proposal, diags) = parse_proposal(&proposal_text);
    findings.extend(tag(PROPOSAL, diags));
    texts.push((PROPOSAL.to_string(), proposal_text));
    let tasks_text = read(&dir.join(TASKS))?;
    let (tasks, diags) = parse_tasks(&tasks_text);
    findings.extend(tag(TASKS, diags));
    texts.push((TASKS.to_string(), tasks_text));
    if let Ok(design) = std::fs::read_to_string(dir.join(DESIGN)) {
        let lines = design.lines().count();
        if lines > MAX_DESIGN_LINES {
            findings.push((
                DESIGN.to_string(),
                Diagnostic::new(
                    crate::diag::Rule::Size,
                    1,
                    format!("design.md has {lines} lines, more than {MAX_DESIGN_LINES}; keep only decisions, constraints and reasons"),
                ),
            ));
        }
        texts.push((DESIGN.to_string(), design));
    }
    let mut deltas = Vec::new();
    for path in md_files(&dir.join(DELTAS))? {
        let cap = path.file_stem().unwrap().to_string_lossy().into_owned();
        let text = read(&path)?;
        let (file, diags) = parse_deltas(&text);
        let rel = format!("{DELTAS}/{cap}.md");
        findings.extend(tag(&rel, diags));
        texts.push((rel, text));
        deltas.push((cap, file));
    }
    findings.extend(
        lint::Change {
            tasks: &tasks,
            deltas: &deltas,
            texts: &texts,
        }
        .lint(&Living { specs: living }),
    );
    Ok(Loaded {
        proposal,
        tasks,
        deltas,
        texts,
        findings,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::git::testrepo::*;
    use crate::store::{Mode, init};

    #[test]
    fn dir_names() {
        let id: Id = "CHG-7k2mq8".parse().unwrap();
        assert_eq!(
            parse_dir_name("CHG-7k2mq8-output-tail"),
            Some((id.clone(), "output-tail".into()))
        );
        assert_eq!(
            parse_dir_name("2026-10-01-CHG-7k2mq8-x"),
            Some((id.clone(), "x".into()))
        );
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
        assert!(
            sums.iter()
                .any(|s| s.id == c.id && s.stage == Stage::Backlog && s.title == "Output tail")
        );
        assert_eq!(loaded.errors().count(), 0, "{:?}", loaded.findings);
    }

    #[test]
    fn locate_finds_changes_tasks_and_requirements() {
        let tmp = TempDir::new();
        let repo = repo_with_commit(&tmp.0, "r");
        let p = init(&repo, &tmp.0.join("home"), Mode::Home).unwrap();
        let c = new(&p, "Out", "main", false).unwrap();
        std::fs::write(
            c.path.join(TASKS),
            "# Tasks\n\n- TSK-x7k2q4 Write\n  covers: REQ-k3f9m2\n  verify: true\n  files: a\n",
        )
        .unwrap();
        std::fs::write(
            c.path.join("specs/out.md"),
            "## ADD REQ-k3f9m2 Out\nrvow SHALL.\n### Scenario: s\n- WHEN a\n- THEN b\n",
        )
        .unwrap();
        let spec = p.root.join("specs/cap");
        std::fs::create_dir_all(&spec).unwrap();
        std::fs::write(
            spec.join("spec.md"),
            "# Cap\n\n## REQ-q8w2c7 Old\nrvow SHALL.\n### Scenario: s\n- WHEN a\n- THEN b\n",
        )
        .unwrap();

        assert_eq!(
            locate(&p, &c.id.to_string()).unwrap(),
            c.path.display().to_string()
        );
        assert_eq!(
            locate(&p, "x7k").unwrap(),
            format!("{}:3", c.path.join(TASKS).display())
        );
        assert_eq!(
            locate(&p, "REQ-k3f").unwrap(),
            format!("{}:1", c.path.join("specs/out.md").display())
        );
        assert_eq!(
            locate(&p, "q8w").unwrap(),
            format!("{}:3", spec.join("spec.md").display())
        );
        assert_eq!(locate(&p, "zzz").unwrap_err().code, "path");
        assert!(
            locate(&p, "REQ-")
                .unwrap_err()
                .message
                .contains("no id matches")
        );
    }
}
