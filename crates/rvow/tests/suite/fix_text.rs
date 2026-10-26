use rvow_core::{brief, change, config, ledger, lint, proposal, store, vow};
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
        rvow_core::git::testrepo::remove_tree(&self.root);
    }
}
impl Fixture {
    fn new(name: &str) -> Self {
        let root = std::env::temp_dir().join(format!("rvow-fix-text-{}-{name}", std::process::id()));
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

fn number_after(text: &str, prefix: &str) -> usize {
    let rest = text.split(prefix).nth(1).unwrap_or_else(|| panic!("`{prefix}` is missing from the guide"));
    rest.split_whitespace().next().unwrap().trim_matches(|c: char| !c.is_ascii_digit()).parse().unwrap()
}

fn guide(f: &Fixture, name: &str) -> String {
    let out = f.cli(&["guide", name]);
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stdout));
    String::from_utf8(out.stdout).unwrap()
}

#[test]
fn the_planning_brief_prints_only_non_zero_counters_and_planning_rules() {
    let f = Fixture::new("planning");
    let c = change::new(&f.project, "T", "main", true).unwrap();
    std::fs::write(c.path.join("tasks.md"), "- TSK-new1 A\n  verify: sh -c true\n  files: a\n").unwrap();
    let prop = c.path.join("proposal.md");
    let text = std::fs::read_to_string(&prop).unwrap();
    std::fs::write(&prop, text + "[NEEDS CLARIFICATION: Which? | recommended: this]\n").unwrap();
    let b = brief::brief(&f.project, &f.home, &f.repo).unwrap();
    assert!(b.contains("waits for accept: 1 open question\n"), "{b}");
    assert!(!b.contains("0 checks") && !b.contains("1 open questions"), "{b}");
    assert!(b.ends_with("rules: only the user runs rvow accept, so call the rvow_* tools (tool search loads them when they are not listed); ask with rvow_ask; write the plan files only\n"), "{b}");
    assert!(!b.contains("stay inside files:"), "{b}");
}

#[test]
fn the_worst_case_brief_keeps_the_cap() {
    let f = Fixture::new("worst");
    let long = "long title words ".repeat(30);
    let c = change::new(&f.project, &long, "main", true).unwrap();
    let mut tasks = String::new();
    for n in 1..=40 {
        tasks.push_str(&format!(
            "- TSK-new{n} {long}\n  verify: test -f out{n}.txt\n  files: out{n}.txt, {}\n",
            "dir/very/long/path/".repeat(8)
        ));
    }
    std::fs::write(c.path.join("tasks.md"), tasks).unwrap();
    let prepared = vow::prepare(&f.project, &f.home, c, &f.repo, &mut std::io::sink()).unwrap();
    let accepted = vow::commit(&f.project, prepared).unwrap();
    for (kind, count) in [("decision", 6), ("handoff", 2)] {
        for _ in 0..count {
            let mut data = serde_json::Map::new();
            data.insert("note".into(), kind.into());
            data.insert("text".into(), "long note text ".repeat(100).into());
            ledger::append(&accepted.path.join(change::LEDGER), &f.project.lock().unwrap(), ledger::Kind::Note, data).unwrap();
        }
    }
    let b = brief::brief(&f.project, &f.home, &f.repo).unwrap();
    assert!(b.lines().count() <= 25, "{} lines:\n{b}", b.lines().count());
    assert!(b.len() <= 1500, "{} chars:\n{b}", b.len());
    assert!(b.contains("next:") && b.contains("rules:"), "{b}");
}

#[test]
fn a_blank_plan_is_not_ready_and_the_brief_says_to_check() {
    let f = Fixture::new("blank");
    let out = f.cli(&["new", "--spec", "x"]);
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stdout));
    let check = String::from_utf8(f.cli(&["check"]).stdout).unwrap();
    assert!(check.contains("no-tasks"), "{check}");
    assert!(check.contains("ready: false"), "{check}");
    assert!(!check.contains("ready: true"), "{check}");
    let b = brief::brief(&f.project, &f.home, &f.repo).unwrap();
    assert!(b.contains("next: run rvow check"), "{b}");
    assert!(!b.contains("/rvow-accept"), "{b}");
}

#[test]
fn the_guide_numbers_match_the_lint_budgets() {
    let f = Fixture::new("guides");
    let proposal_guide = guide(&f, "proposal");
    assert_eq!(number_after(&proposal_guide, "Keep it to "), proposal::MAX_WORDS);
    assert_eq!(number_after(&proposal_guide, "Keep at most "), lint::MAX_CLARIFICATIONS);
    let cap = config::load(&f.project, &f.home).unwrap().interview_cap as usize;
    assert_eq!(
        number_after(&proposal_guide, "tool when your harness has it: it shows the user each question and records the answer, at most "),
        cap
    );

    let spec_guide = guide(&f, "spec");
    assert_eq!(number_after(&spec_guide, "At most "), 60);
    assert_eq!(number_after(&spec_guide, " words and "), lint::MAX_SCENARIOS);
    assert_eq!(lint_prose_budget(&f), 60);

    let design_guide = guide(&f, "design");
    assert_eq!(number_after(&design_guide, "At most "), change::MAX_DESIGN_LINES);

    let tasks_guide = guide(&f, "tasks");
    assert!(!tasks_guide.is_empty());
}

/// The largest requirement text the linter accepts, found by running it: the budget is private to rvow-core.
fn lint_prose_budget(f: &Fixture) -> usize {
    let c = change::new(&f.project, "Budget", "other", true).unwrap();
    std::fs::create_dir_all(c.path.join("specs")).unwrap();
    let mut best = 0;
    for words in 40..=80 {
        let filler = vec!["word"; words - 6].join(" ");
        let spec = format!(
            "## ADD REQ-new1 T\n\nWHEN x happens, rvow SHALL {filler} now.\n\n### Scenario: S\n- WHEN `rvow x` runs\n- THEN the result is 1\n"
        );
        std::fs::write(c.path.join("specs/cap.md"), spec).unwrap();
        let loaded = change::load(&c.path, &[]).unwrap();
        if loaded.findings.iter().any(|(p, d)| p.starts_with("specs/") && d.rule == rvow_core::diag::Rule::Size) {
            break;
        }
        best = words;
    }
    best
}

#[test]
fn the_io_error_fix_names_the_path_and_the_kind() {
    let err = rvow_core::error::Error::io(std::path::Path::new("/no/such/file"), std::io::Error::from(std::io::ErrorKind::NotFound));
    assert!(err.fix.contains("/no/such/file") && err.fix.contains("NotFound") && err.fix.contains("exists"), "{}", err.fix);
}
