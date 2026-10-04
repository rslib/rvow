use rvow_core::store::{Meta, Mode, Project};
use rvow_core::{change, import, ledger, record, registry};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

struct Fixture {
    dir: PathBuf,
    project: Project,
    source: PathBuf,
}
impl Fixture {
    fn new(name: &str) -> Self {
        let dir = std::env::temp_dir().join(format!("rvow-audit-import-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let root = dir.join("home/project");
        std::fs::create_dir_all(&root).unwrap();
        let source = dir.join("openspec");
        std::fs::create_dir_all(source.join("specs")).unwrap();
        Self { dir, source, project: Project { root, meta: Meta { roots: vec![], remote: None, mode: Mode::Home } } }
    }
    fn put(&self, path: &str, text: &str) {
        put(&self.source.join(path), text);
    }
    fn import(&self, dry: bool) -> rvow_core::error::Result<import::Report> {
        import::openspec(&self.project, &self.source, "main", dry)
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}
fn put(path: &Path, text: &str) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, text).unwrap();
}
fn spec(titles: &[&str]) -> String {
    let mut text = String::from("## Requirements\n");
    for title in titles {
        text.push_str(&format!("### Requirement: {title}\nrvow SHALL retain data.\n#### Scenario: Retained\n- **GIVEN** data\n- **WHEN** imported\n- **THEN** data remains\n"));
    }
    text
}
fn snapshot(dir: &Path) -> BTreeMap<PathBuf, Vec<u8>> {
    fn walk(dir: &Path, out: &mut BTreeMap<PathBuf, Vec<u8>>) {
        for entry in std::fs::read_dir(dir).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                walk(&path, out);
            } else if path.file_name().unwrap() != ".lock" {
                out.insert(path.clone(), std::fs::read(path).unwrap());
            }
        }
    }
    let mut out = BTreeMap::new();
    walk(dir, &mut out);
    out
}

#[test]
fn audit_import_merges_and_resolves_existing_requirements() {
    let f = Fixture::new("merge");
    f.put("specs/cap/spec.md", &spec(&["Existing"]));
    f.put("specs/unrelated/spec.md", &spec(&["Unrelated"]));
    f.import(false).unwrap();
    let old = record::load(&f.project).unwrap();
    let unrelated = std::fs::read(f.project.root.join("specs/unrelated/spec.md")).unwrap();
    let before = std::fs::read_to_string(f.project.root.join("specs/cap/spec.md")).unwrap();
    f.put("specs/cap/spec.md", &spec(&["Incoming"]));
    std::fs::remove_dir_all(f.source.join("specs/unrelated")).unwrap();
    f.put("changes/update/specs/cap/spec.md", "## MODIFIED Requirements\n### Requirement: Existing\nrvow SHALL keep it.\n");
    let result = f.import(false).unwrap();
    assert!(result.changes[0].unmatched.is_empty());
    let merged = std::fs::read_to_string(f.project.root.join("specs/cap/spec.md")).unwrap();
    assert!(merged.starts_with(&before));
    assert!(merged.contains(" Incoming\n"));
    assert_eq!(std::fs::read(f.project.root.join("specs/unrelated/spec.md")).unwrap(), unrelated);
    let entries = record::load(&f.project).unwrap();
    for (id, entry) in old {
        assert_eq!(entries[&id], entry);
    }
    assert_eq!(entries.len(), 3);
    let dir = change::resolve(&f.project, &result.changes[0].id.to_string()).unwrap();
    let delta = std::fs::read_to_string(dir.path.join("specs/cap.md")).unwrap();
    let existing_id = merged.lines().find(|l| l.ends_with(" Existing")).unwrap().split_whitespace().nth(1).unwrap();
    assert!(delta.contains(&format!("CHANGE {existing_id} Existing")));
}

#[test]
fn audit_import_lists_all_conflicts_without_writes_or_mints() {
    let f = Fixture::new("conflict");
    f.put("specs/cap/spec.md", &spec(&["First", "Second"]));
    f.import(false).unwrap();
    let before = snapshot(&f.project.root);
    let ids = registry::load(&f.project).unwrap();
    f.put("specs/aaa/spec.md", &spec(&["Safe"]));
    f.put("specs/cap/spec.md", &spec(&["first", "Second"]));
    let error = f.import(false).unwrap_err();
    assert_eq!(error.code, "import");
    assert!(error.message.contains("cap: first"), "{}", error.message);
    assert!(error.message.contains("cap: Second"));
    assert_eq!(snapshot(&f.project.root), before);
    assert_eq!(registry::load(&f.project).unwrap(), ids);
}

#[test]
fn audit_import_checked_tasks_are_notes_not_authority() {
    let f = Fixture::new("checked");
    f.put("changes/work/tasks.md", "- [x] 1.1 Done; verify `true`\n- [ ] 1.2 Pending; verify `true`\n");
    let r = f.import(false).unwrap();
    let dir = change::resolve(&f.project, &r.changes[0].id.to_string()).unwrap();
    assert_eq!(dir.stage, change::Stage::Backlog);
    let (events, diags) = ledger::read(&dir.path.join(change::LEDGER)).unwrap();
    assert!(diags.is_empty());
    assert_eq!(events.len(), 1);
    assert!(events.iter().all(|e| e.kind == ledger::Kind::Note));
    assert_eq!(events[0].data["tasks"][0]["checked"], true);
    assert_eq!(events[0].data["tasks"][1]["checked"], false);
    assert!(events[0].data["tasks"][0]["task"].as_str().unwrap().contains("1.1 Done"));
}

#[test]
fn audit_import_archive_is_original_history_only() {
    let f = Fixture::new("archive");
    let proposal = "# Historical proposal\nOriginal text.\n";
    let tasks = "- [x] Historical done\n";
    let delta = "## ADDED Requirements\n### Requirement: Historical\nold text\n";
    f.put("changes/archive/2020-01-01-old/proposal.md", proposal);
    f.put("changes/archive/2020-01-01-old/tasks.md", tasks);
    f.put("changes/archive/2020-01-01-old/specs/cap/spec.md", delta);
    let r = f.import(false).unwrap();
    assert_eq!(r.archived, 1);
    assert!(r.changes.is_empty());
    assert!(registry::load(&f.project).unwrap().is_empty());
    let archive = f.project.root.join("archive/openspec/2020-01-01-old");
    assert_eq!(std::fs::read_to_string(archive.join("proposal.md")).unwrap(), proposal);
    assert_eq!(std::fs::read_to_string(archive.join("tasks.md")).unwrap(), tasks);
    assert_eq!(std::fs::read_to_string(archive.join("specs/cap/spec.md")).unwrap(), delta);
    assert!(!archive.join(change::LEDGER).exists());
    assert!(!f.project.root.join("active").exists());
    assert!(!f.project.root.join("archive/done").exists());
}

#[test]
fn audit_import_refuses_custom_root_schema_before_writes() {
    let f = Fixture::new("schema");
    f.put("specs/cap/spec.md", &spec(&["Safe"]));
    for config in [
        "schema: custom\n",
        "schema: 'custom'\n",
        "schema:\n  name: custom\n",
        "{schema: custom}\n",
        "  schema: custom\n",
        "<<: *custom\n",
        "\"sch\\u0065ma\": custom\n",
    ] {
        f.put("config.yaml", config);
        let before = snapshot(&f.project.root);
        assert_eq!(f.import(false).unwrap_err().code, "import");
        assert_eq!(snapshot(&f.project.root), before);
    }
    f.put("config.yaml", "schema: spec-driven\ncontext: |\n  schema: custom in context\n");
    f.import(false).unwrap();
}

#[test]
fn audit_import_dry_run_leaves_existing_store_and_history_untouched() {
    let f = Fixture::new("dry");
    f.put("specs/cap/spec.md", &spec(&["Existing"]));
    f.import(false).unwrap();
    f.put("specs/cap/spec.md", &spec(&["Incoming"]));
    f.put("changes/work/tasks.md", "- [x] Done\n");
    f.put("changes/archive/old/proposal.md", "# Old\n");
    let before = snapshot(&f.project.root);
    let r = f.import(true).unwrap();
    assert!(r.dry_run);
    assert_eq!(r.requirements, 1);
    assert_eq!(r.archived, 1);
    assert_eq!(r.changes.len(), 1);
    assert_eq!(snapshot(&f.project.root), before);
}
