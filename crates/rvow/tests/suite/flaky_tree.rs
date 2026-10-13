use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicU32, Ordering};

use rvow_core::{change, proof, store, vow};

static COUNTER: AtomicU32 = AtomicU32::new(0);

fn git_cmd(repo: &Path, args: &[&str]) {
    let ok = Command::new("git")
        .arg("-C")
        .arg(repo)
        .args(args)
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_AUTHOR_NAME", "t")
        .env("GIT_AUTHOR_EMAIL", "t@t")
        .env("GIT_COMMITTER_NAME", "t")
        .env("GIT_COMMITTER_EMAIL", "t@t")
        .output()
        .unwrap();
    assert!(ok.status.success(), "git {args:?}: {ok:?}");
}

struct Setup {
    _tmp: PathBuf,
    home: PathBuf,
    repo: PathBuf,
    project: store::Project,
    task: String,
}

fn setup(name: &str, tasks: &str) -> Setup {
    setup_in(name, tasks, store::Mode::Home)
}

fn setup_in(name: &str, tasks: &str, mode: store::Mode) -> Setup {
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    let tmp = std::env::temp_dir().join(format!("rvow-flaky-{n}-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&tmp);
    std::fs::create_dir_all(&tmp).unwrap();
    let tmp = tmp.canonicalize().unwrap();
    let home = tmp.join("home");
    let repo = tmp.join("r");
    std::fs::create_dir_all(&repo).unwrap();
    git_cmd(&repo, &["init", "-q", "-b", "main"]);
    std::fs::write(repo.join("a.txt"), "a").unwrap();
    git_cmd(&repo, &["add", "a.txt"]);
    git_cmd(&repo, &["commit", "-q", "-m", "init"]);

    let project = store::init(&repo, &home, mode).unwrap();
    let c = change::new(&project, "Out", "main", false).unwrap();
    std::fs::write(c.path.join("tasks.md"), tasks).unwrap();
    std::fs::write(
        c.path.join("specs/out.md"),
        "## ADD REQ-new1 Out\nrvow SHALL write ok.\n### Scenario: s\n- WHEN it runs\n- THEN out.txt holds ok\n",
    )
    .unwrap();
    let prepared = vow::prepare(&project, &home, c, &repo, &mut std::io::sink()).unwrap();
    let vowed = vow::commit(&project, prepared).unwrap();
    let task = vowed.minted.iter().find(|(from, _)| from == "TSK-new1").unwrap().1.to_string();
    Setup { _tmp: tmp, home, repo, project, task }
}

fn state_name(s: &Setup) -> String {
    let st = proof::status(&s.project, &s.home, &s.repo, false).unwrap();
    st[0].tasks[0].state.name().to_string()
}

#[test]
fn flaky_fix_outside_files_clears_failure() {
    let tasks = "- TSK-new1 Write ok\n  covers: REQ-new1\n  verify: grep -q ok helper.txt && grep -q ok out.txt && echo 'test result: ok. 1 passed; 0 failed;'\n  files: out.txt\n";
    let s = setup("outside-fix", tasks);
    std::fs::write(s.repo.join("out.txt"), "ok\n").unwrap();
    std::fs::write(s.repo.join("helper.txt"), "bad\n").unwrap();
    let r = proof::prove(&s.project, &s.home, &s.repo, &s.task, &mut std::io::sink()).unwrap();
    assert!(!r.proved);
    std::fs::write(s.repo.join("helper.txt"), "ok\n").unwrap();
    let r = proof::prove(&s.project, &s.home, &s.repo, &s.task, &mut std::io::sink()).unwrap();
    assert!(r.proved, "{:?}", r.outcome);
    assert_eq!(state_name(&s), "proved", "a fix outside files clears the failure");
}

#[test]
fn flaky_same_tree_still_shows_flaky() {
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    let flag = std::env::temp_dir().join(format!("rvow-flaky-flag-{}-{n}", std::process::id()));
    let _ = std::fs::remove_file(&flag);
    let tasks = format!(
        "- TSK-new1 Write ok\n  covers: REQ-new1\n  verify: test -f {} && grep -q ok out.txt && echo 'test result: ok. 1 passed; 0 failed;'\n  files: out.txt\n",
        flag.display()
    );
    let s = setup("same-tree", &tasks);
    std::fs::write(s.repo.join("out.txt"), "ok\n").unwrap();
    let r = proof::prove(&s.project, &s.home, &s.repo, &s.task, &mut std::io::sink()).unwrap();
    assert!(!r.proved);
    std::fs::write(&flag, "go\n").unwrap();
    let r = proof::prove(&s.project, &s.home, &s.repo, &s.task, &mut std::io::sink()).unwrap();
    assert!(r.proved, "{:?}", r.outcome);
    assert_eq!(state_name(&s), "flaky", "same worktree pass and fail is flaky");
    let _ = std::fs::remove_file(&flag);
}

#[test]
fn flaky_old_failure_without_fingerprint_does_not_count() {
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    let flag = std::env::temp_dir().join(format!("rvow-flaky-old-{}-{n}", std::process::id()));
    let _ = std::fs::remove_file(&flag);
    let tasks = format!(
        "- TSK-new1 Write ok\n  covers: REQ-new1\n  verify: test -f {} && grep -q ok out.txt && echo 'test result: ok. 1 passed; 0 failed;'\n  files: out.txt\n",
        flag.display()
    );
    let s = setup("old-fail", &tasks);
    std::fs::write(s.repo.join("out.txt"), "ok\n").unwrap();
    let r = proof::prove(&s.project, &s.home, &s.repo, &s.task, &mut std::io::sink()).unwrap();
    assert!(!r.proved);
    let dir = std::fs::read_dir(s.project.root.join("active")).unwrap().next().unwrap().unwrap().path();
    let ledger = dir.join("ledger.jsonl");
    let text = std::fs::read_to_string(&ledger).unwrap();
    let mut lines = Vec::new();
    for line in text.lines() {
        let mut v: serde_json::Value = serde_json::from_str(line).unwrap();
        if v.get("kind").and_then(|k| k.as_str()).is_some_and(|k| k.eq_ignore_ascii_case("prove"))
            && v.get("proved").and_then(|b| b.as_bool()) == Some(false)
        {
            v.as_object_mut().unwrap().remove("worktree");
            v.as_object_mut().unwrap().remove("worktree_fingerprint");
            v.as_object_mut().unwrap().remove("fingerprint");
        }
        lines.push(serde_json::to_string(&v).unwrap());
    }
    std::fs::write(&ledger, lines.join("\n") + "\n").unwrap();
    std::fs::write(&flag, "go\n").unwrap();
    let r = proof::prove(&s.project, &s.home, &s.repo, &s.task, &mut std::io::sink()).unwrap();
    assert!(r.proved, "{:?}", r.outcome);
    assert_eq!(state_name(&s), "proved", "an old failure without a fingerprint does not count");
    let _ = std::fs::remove_file(&flag);
}

#[test]
fn flaky_repo_mode_same_tree_is_flaky() {
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    let flag = std::env::temp_dir().join(format!("rvow-flaky-repo-{}-{n}", std::process::id()));
    let _ = std::fs::remove_file(&flag);
    let tasks = format!(
        "- TSK-new1 Write ok\n  covers: REQ-new1\n  verify: test -f {} && grep -q ok out.txt && echo 'test result: ok. 1 passed; 0 failed;'\n  files: out.txt\n",
        flag.display()
    );
    let s = setup_in("repo-same-tree", &tasks, store::Mode::Repo);
    assert_eq!(s.project.meta.mode, store::Mode::Repo);
    std::fs::write(s.repo.join("out.txt"), "ok\n").unwrap();
    let r = proof::prove(&s.project, &s.home, &s.repo, &s.task, &mut std::io::sink()).unwrap();
    assert!(!r.proved);
    std::fs::write(&flag, "go\n").unwrap();
    let r = proof::prove(&s.project, &s.home, &s.repo, &s.task, &mut std::io::sink()).unwrap();
    assert!(r.proved, "{:?}", r.outcome);
    assert_eq!(state_name(&s), "flaky", "a repo-mode fail then pass on the same tree is flaky");
    let _ = std::fs::remove_file(&flag);
}
