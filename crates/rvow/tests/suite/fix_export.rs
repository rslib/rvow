use rvow_core::{change, store, tasks};
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

struct Fixture {
    root: PathBuf,
    repo: PathBuf,
    home: PathBuf,
    project: store::Project,
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.root);
    }
}
impl Fixture {
    fn new(name: &str) -> Self {
        let root = std::env::temp_dir().join(format!("rvow-fix-export-{}-{name}", std::process::id()));
        let repo = root.join("repo");
        let home = root.join("home");
        std::fs::create_dir_all(&repo).unwrap();
        let git = |args: &[&str]| {
            let out = Command::new("git")
                .args(args)
                .current_dir(&repo)
                .env_remove("GIT_DIR")
                .env_remove("GIT_WORK_TREE")
                .env_remove("GIT_INDEX_FILE")
                .env_remove("GIT_COMMON_DIR")
                .env("GIT_CONFIG_GLOBAL", "/dev/null")
                .env("GIT_CONFIG_NOSYSTEM", "1")
                .env("GIT_AUTHOR_NAME", "test")
                .env("GIT_AUTHOR_EMAIL", "test@example.invalid")
                .env("GIT_COMMITTER_NAME", "test")
                .env("GIT_COMMITTER_EMAIL", "test@example.invalid")
                .output()
                .unwrap();
            assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
        };
        git(&["init", "-q", "-b", "main"]);
        std::fs::write(repo.join("initial"), "initial").unwrap();
        git(&["add", "initial"]);
        git(&["commit", "-q", "-m", "initial"]);
        let project = store::init(&repo, &home, store::Mode::Home).unwrap();
        Self { root, repo, home, project }
    }
    fn put(&self, rel: &str, text: &str) {
        let path = self.project.root.join(rel);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, text).unwrap();
    }
    fn cli(&self, args: &[&str]) -> Output {
        Command::new(env!("CARGO_BIN_EXE_rvow"))
            .args(args)
            .current_dir(&self.repo)
            .env("HOME", &self.root)
            .env("RVOW_HOME", &self.home)
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .output()
            .unwrap()
    }
    fn seed(&self) {
        self.put(
            "specs/core/spec.md",
            "# core\n\n## REQ-k3f9m2 Keep the tail\nWHEN a run fails, rvow SHALL keep the last 20 lines.\n\n### Scenario: A failure keeps a tail\n- GIVEN a command that prints 30 lines\n- WHEN it exits 1\n- THEN 20 lines are kept\n- AND the lines are the last ones\n\n## REQ-q8w2c7 Old format\nstatus: deprecated\n\nrvow SHALL print the old format.\n\n### Scenario: Old\n- WHEN a run ends\n- THEN the old format prints\n",
        );
        self.put("specs/core/design.md", "# Purpose\n\nKeep proofs honest.\n");
        self.put(
            "specs/other/spec.md",
            "# other\n\n## REQ-h4t6v9 Name the branch\nrvow SHALL name the branch in the brief.\n\n### Scenario: Branch shown\n- WHEN the brief prints\n- THEN the branch is named\n",
        );
        let c = "active/CHG-7k2mq8-output-tail";
        self.put(&format!("{c}/proposal.md"), "branch: main\n\n# Output tail\n\n## Why\nTails help.\n\n## What changes\nKeep 5 lines.\n\n## Out of scope\nColors.\n\n## Assumptions\nUTF-8.\n");
        self.put(&format!("{c}/design.md"), "# Design\n\nD1.\n");
        self.put(
            &format!("{c}/tasks.md"),
            "# Tasks\n\n## Build\n\n- TSK-q8w2c7 Keep the tail\n  covers: REQ-new1\n  verify: cargo test tail\n  files: src/**\n- TSK-h4t6v9 Print it\n  verify: cargo test print\n  files: src/**, tests/**\n",
        );
        self.put(
            &format!("{c}/specs/core.md"),
            "## ADD REQ-new1 Keep five lines\nrvow SHALL keep 5 lines.\n### Scenario: Five\n- WHEN a run fails\n- THEN 5 lines are kept\n\n## CHANGE REQ-k3f9m2 Keep the output tail\nWHEN a run fails, rvow SHALL keep the last 5 lines.\n### Scenario: A failure keeps a tail\n- WHEN it exits 1\n- THEN 5 lines are kept\n",
        );
        self.put(&format!("{c}/specs/other.md"), "## REMOVE REQ-h4t6v9\n");
        let d = "archive/done/2026-09-01-CHG-p3x8a1-first-cut";
        self.put(
            &format!("{d}/proposal.md"),
            "branch: first\n\n# First cut\n\n## Why\nStart.\n\n## What changes\nBase.\n\n## Out of scope\nNone.\n",
        );
        self.put(&format!("{d}/tasks.md"), "# Tasks\n\n- TSK-m4n5p6 Base\n  verify: cargo test base\n  files: **\n");
        self.put("archive/dropped/CHG-3z7y2x-nope/proposal.md", "branch: x\n");
    }
}

fn text(o: &Output) -> String {
    format!("{}{}", String::from_utf8_lossy(&o.stdout), String::from_utf8_lossy(&o.stderr))
}

fn read(p: &Path) -> String {
    std::fs::read_to_string(p).unwrap_or_else(|e| panic!("{}: {e}", p.display()))
}

#[test]
fn export_writes_the_openspec_tree() {
    let f = Fixture::new("tree");
    f.seed();
    let to = f.root.join("out");
    let o = f.cli(&["export", "openspec", "--to", to.to_str().unwrap(), "--human"]);
    assert!(o.status.success(), "{}", text(&o));
    let out = text(&o);
    assert!(out.contains("specs: 2") && out.contains("requirements: 3") && out.contains("open changes: 1"), "{out}");
    assert!(out.contains("archived changes: 1") && out.contains("1 dropped"), "{out}");
    let os = to.join("openspec");
    for rel in [
        "config.yaml",
        "specs/core/spec.md",
        "specs/other/spec.md",
        "changes/output-tail/proposal.md",
        "changes/output-tail/design.md",
        "changes/output-tail/tasks.md",
        "changes/output-tail/specs/core/spec.md",
        "changes/output-tail/specs/other/spec.md",
        "changes/archive/2026-09-01-first-cut/proposal.md",
        "changes/archive/2026-09-01-first-cut/tasks.md",
    ] {
        assert!(os.join(rel).is_file(), "missing {rel}");
    }
    assert!(!os.join("changes/nope").exists());

    let core = read(&os.join("specs/core/spec.md"));
    assert!(core.contains("## Purpose\n\nKeep proofs honest.\n"), "{core}");
    assert!(core.contains("### Requirement: Keep the tail\n<!-- rvow: REQ-k3f9m2 -->\nWHEN a run fails"), "{core}");
    assert!(core.contains("#### Scenario: A failure keeps a tail\n- **GIVEN** a command that prints 30 lines\n- **WHEN** it exits 1\n- **THEN** 20 lines are kept\n- **AND** the lines are the last ones\n"), "{core}");
    assert!(core.contains("<!-- rvow: REQ-q8w2c7 deprecated -->\n(deprecated)\n"), "{core}");

    let tasks = read(&os.join("changes/output-tail/tasks.md"));
    assert!(tasks.contains("- [ ] 1.1 Keep the tail; verify `cargo test tail`\n  - covers: REQ-new1\n  - files: src/**\n"), "{tasks}");
    let done = read(&os.join("changes/archive/2026-09-01-first-cut/tasks.md"));
    assert!(done.contains("- [x] 1.1 Base; verify `cargo test base`"), "{done}");
    let prop = read(&os.join("changes/output-tail/proposal.md"));
    assert!(
        prop.contains("## What Changes") && prop.contains("## Non-goals\n\nColors.") && prop.contains("## Assumptions\n\nUTF-8."),
        "{prop}"
    );

    let delta = read(&os.join("changes/output-tail/specs/core/spec.md"));
    assert!(delta.contains("## ADDED Requirements\n\n### Requirement: Keep five lines\n<!-- rvow: REQ-new1 -->"), "{delta}");
    assert!(delta.contains("## MODIFIED Requirements\n\n### Requirement: Keep the output tail\n<!-- rvow: REQ-k3f9m2 -->"), "{delta}");
    assert!(delta.contains("- FROM: `### Requirement: Keep the tail`\n- TO: `### Requirement: Keep the output tail`"), "{delta}");
    let removed = read(&os.join("changes/output-tail/specs/other/spec.md"));
    assert!(removed.contains("## REMOVED Requirements\n\n### Requirement: Name the branch\n<!-- rvow: REQ-h4t6v9 -->"), "{removed}");
}

#[test]
fn export_refuses_an_existing_target() {
    let f = Fixture::new("exists");
    f.seed();
    let to = f.root.join("out");
    std::fs::create_dir_all(to.join("openspec")).unwrap();
    std::fs::write(to.join("openspec/keep.txt"), "mine").unwrap();
    let o = f.cli(&["export", "openspec", "--to", to.to_str().unwrap()]);
    assert!(!o.status.success());
    assert!(text(&o).contains("--to"), "{}", text(&o));
    assert_eq!(read(&to.join("openspec/keep.txt")), "mine");
    assert!(!to.join("openspec/specs").exists());
}

#[test]
fn dry_run_writes_nothing_and_lists_the_files() {
    let f = Fixture::new("dry");
    f.seed();
    let to = f.root.join("out");
    let o = f.cli(&["export", "openspec", "--to", to.to_str().unwrap(), "--dry-run", "--toon"]);
    assert!(o.status.success(), "{}", text(&o));
    assert!(!to.exists());
    let out = text(&o);
    assert!(out.contains("openspec/specs/core/spec.md") && out.contains("openspec/changes/archive/2026-09-01-first-cut/tasks.md"), "{out}");
    assert!(out.contains("dry_run: true"), "{out}");
}

#[test]
fn round_trip_keeps_titles_text_and_verify() {
    let f = Fixture::new("trip");
    f.seed();
    let to = f.root.join("out");
    assert!(f.cli(&["export", "openspec", "--to", to.to_str().unwrap()]).status.success());

    let g = Fixture::new("trip-fresh");
    let o = g.cli(&["import", "openspec", "--from", to.join("openspec").to_str().unwrap(), "--human"]);
    assert!(o.status.success(), "{}", text(&o));

    let (before, _) = change::load_living(&f.project).unwrap();
    let (after, _) = change::load_living(&g.project).unwrap();
    for (cap, spec) in &before {
        let (_, imported) = after.iter().find(|(c, _)| c == cap).unwrap_or_else(|| panic!("no {cap}"));
        assert_eq!(imported.requirements.len(), spec.requirements.len());
        for r in &spec.requirements {
            let got = imported.requirements.iter().find(|i| i.title == r.title).unwrap_or_else(|| panic!("no {}", r.title));
            assert_eq!(got.text, r.text, "the id comment and the deprecated marker stay out of the text");
            assert_eq!(got.scenarios.len(), r.scenarios.len());
            for (a, b) in got.scenarios.iter().zip(&r.scenarios) {
                assert_eq!(a.title, b.title);
                let steps = |s: &rvow_core::spec::Scenario| s.steps.iter().map(|x| (x.kind.word(), x.text.clone())).collect::<Vec<_>>();
                assert_eq!(steps(a), steps(b));
            }
        }
    }

    let (changes, _) = change::list(&g.project).unwrap();
    assert_eq!(changes.len(), 1);
    let (parsed, _) = tasks::parse_tasks(&read(&changes[0].path.join("tasks.md")));
    let verifies: Vec<_> = parsed.iter().map(|t| t.verify.clone().unwrap()).collect();
    assert_eq!(verifies, ["cargo test tail", "cargo test print"]);
    let history = read(&g.project.root.join("archive/openspec/2026-09-01-first-cut/tasks.md"));
    assert!(history.contains("verify `cargo test base`"), "{history}");
}
