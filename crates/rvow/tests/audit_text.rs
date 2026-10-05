use rvow_core::{brief, change, diag::Rule, ledger, store, vow};
use std::path::PathBuf;
use std::process::Command;

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
        let root = std::env::temp_dir().join(format!("rvow-audit-text-{}-{name}", std::process::id()));
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
    fn cli(&self, args: &[&str]) -> std::process::Output {
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
}
fn bounded(text: &str) {
    assert!(text.len() <= 1500, "{} bytes", text.len());
    assert!(text.lines().count() <= 25, "{} lines", text.lines().count());
    assert!(
        text.contains("rules: rvow accept and confirm belong to the user") || text.contains("rules: only the user runs rvow accept"),
        "{text}"
    );
    assert!(text.contains("next:"), "{text}");
}
#[test]
fn audit_text_final_brief_keeps_next_and_rules_with_large_notes() {
    let f = Fixture::new("notes");
    let c = change::new(&f.project, "Output", "main", true).unwrap();
    std::fs::write(c.path.join("tasks.md"), "- TSK-new1 Output\n  verify: test -f out.txt\n  files: out.txt\n").unwrap();
    let prepared = vow::prepare(&f.project, &f.home, c, &f.repo, &mut std::io::sink()).unwrap();
    let accepted = vow::commit(&f.project, prepared).unwrap();
    let mut data = serde_json::Map::new();
    data.insert("note".into(), "handoff".into());
    data.insert("text".into(), "long note\n".repeat(500).into());
    ledger::append(&accepted.path.join(change::LEDGER), &f.project.lock().unwrap(), ledger::Kind::Note, data).unwrap();
    bounded(&brief::brief(&f.project, &f.home, &f.repo).unwrap());
}
#[test]
fn audit_text_planning_brief_is_bounded_with_many_plans() {
    let f = Fixture::new("plans");
    for n in 0..40 {
        change::new(&f.project, &format!("Plan {n}"), "main", true).unwrap();
    }
    bounded(&brief::brief(&f.project, &f.home, &f.repo).unwrap());
}
#[test]
fn audit_text_design_warning_uses_the_documented_hundred_line_limit() {
    let f = Fixture::new("design");
    let c = change::new(&f.project, "Design", "main", true).unwrap();
    std::fs::write(c.path.join("design.md"), "Decision\n".repeat(101)).unwrap();
    let loaded = change::load(&c.path, &[]).unwrap();
    assert!(loaded.findings.iter().any(|(path, diag)| path == "design.md" && diag.rule == Rule::Size), "{:?}", loaded.findings);
}
#[test]
fn audit_text_neutral_hooks_keep_the_claude_alias() {
    let f = Fixture::new("hooks");
    for event in ["pre-tool-use", "session-start", "stop", "status", "view"] {
        let neutral = f.cli(&["hook", event]);
        let old = f.cli(&["hook", "claude", event]);
        assert!(neutral.status.success(), "{}", String::from_utf8_lossy(&neutral.stdout));
        assert_eq!(neutral.stdout, old.stdout);
        assert_eq!(neutral.status.code(), old.status.code());
    }
    let out = f.cli(&["hook", "unknown-event"]);
    assert_eq!(out.status.code(), Some(2));
}
