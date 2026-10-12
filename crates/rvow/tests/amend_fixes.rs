use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicU32, Ordering};

use rvow_core::{amend, change, config, drift, git, proof, store, vow};

static COUNTER: AtomicU32 = AtomicU32::new(0);

struct Setup {
    _tmp: PathBuf,
    home: PathBuf,
    repo: PathBuf,
    project: store::Project,
    minted: Vec<(String, rvow_core::id::Id)>,
}

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

fn setup(name: &str, tasks: &str) -> Setup {
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    let tmp = std::env::temp_dir().join(format!("rvow-amendfixes-{}-{n}-{name}", std::process::id()));
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

    let project = store::init(&repo, &home, store::Mode::Home).unwrap();
    let c = change::new(&project, "Out", "main", false).unwrap();
    std::fs::write(c.path.join("tasks.md"), tasks).unwrap();
    std::fs::write(
        c.path.join("specs/out.md"),
        "## ADD REQ-new1 Out\nrvow SHALL write ok.\n### Scenario: s\n- WHEN it runs\n- THEN out.txt holds ok\n",
    )
    .unwrap();
    let prepared = vow::prepare(&project, &home, c, &repo, &mut std::io::sink()).unwrap();
    let vowed = vow::commit(&project, prepared).unwrap();
    Setup { _tmp: tmp, home, repo, project, minted: vowed.minted }
}

fn failing_task() -> String {
    "- TSK-new1 Write ok\n  covers: REQ-new1\n  verify: grep -q ok out.txt && echo 'test result: ok. 1 passed; 0 failed;'\n  files: out.txt\n".to_string()
}

fn real_id(s: &Setup, placeholder: &str) -> String {
    s.minted.iter().find(|(from, _)| from == placeholder).unwrap().1.to_string()
}

fn active_dir(project: &store::Project) -> PathBuf {
    std::fs::read_dir(project.root.join("active")).unwrap().next().unwrap().unwrap().path()
}

fn pending_of(s: &Setup) -> Vec<amend::Pending> {
    let (living, _) = change::load_living(&s.project).unwrap();
    let changes = proof::active(&s.project, &living).unwrap();
    amend::pending(&changes)
}

fn confirm_first(s: &Setup) {
    let pend = pending_of(s);
    assert_eq!(pend.len(), 1);
    let id = pend[0].id.to_string();
    amend::confirm(&s.project, &[id[4..8].to_string()], amend::Decision::Accept, None).unwrap();
}

#[test]
fn base_commit_amend_after_confirmed_amend() {
    let s = setup("after-confirm", &failing_task());
    let dir = active_dir(&s.project);
    let tasks = dir.join("tasks.md");
    let text = std::fs::read_to_string(&tasks).unwrap();
    std::fs::write(&tasks, text.replace("Write ok", "Write ok clearly")).unwrap();
    amend::amend(&s.project, &s.home, &s.repo, None, "clearer title", None).unwrap();
    confirm_first(&s);

    let text = std::fs::read_to_string(&tasks).unwrap();
    std::fs::write(
        &tasks,
        text.replace(
            "grep -q ok out.txt && echo 'test result: ok. 1 passed; 0 failed;'",
            "grep -q ok other.txt && echo 'test result: ok. 1 passed; 0 failed;'",
        ),
    )
    .unwrap();
    let out = amend::amend(&s.project, &s.home, &s.repo, None, "changed check", None)
        .unwrap_or_else(|e| panic!("amend after a confirmed amend failed: {e:?}"));
    assert_eq!(out.len(), 1);
    let pend = pending_of(&s);
    assert_eq!(pend.len(), 1);
    assert!(!pend[0].baseline.is_empty(), "changed check runs on the base commit");
}

#[test]
fn base_commit_mutants_diff_after_amend() {
    let tasks = "- TSK-new1 Write ok\n  covers: REQ-new1\n  verify: echo 'test result: ok. 1 passed; 0 failed;'\n  files: out.txt\n  baseline: pass\n  mutants: cat \"$RVOW_DIFF\" > seen.diff\n";
    let s = setup("mutants-after", tasks);
    let dir = active_dir(&s.project);
    let tasks_path = dir.join("tasks.md");
    let text = std::fs::read_to_string(&tasks_path).unwrap();
    std::fs::write(&tasks_path, text.replace("Write ok", "Write ok clearly")).unwrap();
    amend::amend(&s.project, &s.home, &s.repo, None, "clearer title", None).unwrap();
    confirm_first(&s);

    std::fs::write(s.repo.join("out.txt"), "hello\n").unwrap();
    let task_id = real_id(&s, "TSK-new1");
    let r = proof::prove(&s.project, &s.home, &s.repo, &task_id, &mut std::io::sink()).unwrap();
    assert!(r.proved, "{:?}", r.outcome);
    let diff = std::fs::read_to_string(s.repo.join("seen.diff")).unwrap();
    assert!(diff.contains("out.txt"), "RVOW_DIFF holds the diff from the vow base: {diff:?}");
}

#[test]
fn base_commit_missing_names_workable_step() {
    let s = setup("missing-base", &failing_task());
    let dir = active_dir(&s.project);
    let ledger = dir.join("ledger.jsonl");
    let text = std::fs::read_to_string(&ledger).unwrap();
    let mut out_lines = Vec::new();
    for line in text.lines() {
        let mut v: serde_json::Value = serde_json::from_str(line).unwrap();
        if v.get("kind").and_then(|k| k.as_str()).is_some_and(|k| k.eq_ignore_ascii_case("vow")) {
            v.as_object_mut().unwrap().remove("base_commit");
        }
        out_lines.push(serde_json::to_string(&v).unwrap());
    }
    std::fs::write(&ledger, out_lines.join("\n") + "\n").unwrap();

    let tasks = dir.join("tasks.md");
    let text = std::fs::read_to_string(&tasks).unwrap();
    std::fs::write(
        &tasks,
        text.replace(
            "grep -q ok out.txt && echo 'test result: ok. 1 passed; 0 failed;'",
            "grep -q ok other.txt && echo 'test result: ok. 1 passed; 0 failed;'",
        ),
    )
    .unwrap();
    let err = amend::amend(&s.project, &s.home, &s.repo, None, "changed check", None).unwrap_err();
    assert_eq!(err.code, "baseline");
    assert!(!err.fix.to_lowercase().contains("accept"), "help must not say accept: {:?}", err.fix);
    assert!(err.fix.contains("rvow block"), "help names a step for an active change: {:?}", err.fix);
}

#[test]
fn amend_mints_new_task_id_at_amend() {
    let s = setup("mint-new", &failing_task());
    let dir = active_dir(&s.project);
    let tasks = dir.join("tasks.md");
    let mut text = std::fs::read_to_string(&tasks).unwrap();
    text.push_str("- TSK-new8 Extra\n  verify: grep -q ok extra.txt && echo 'test result: ok. 1 passed; 0 failed;'\n  files: out.txt\n");
    std::fs::write(&tasks, text).unwrap();
    amend::amend(&s.project, &s.home, &s.repo, None, "add a task", None).unwrap();
    let text = std::fs::read_to_string(&tasks).unwrap();
    assert!(!text.contains("TSK-new8"), "plan names a real id: {text}");
    let pend = pending_of(&s);
    assert_eq!(pend.len(), 1);
    let rendered = format!("{:?}", pend[0].items);
    assert!(!rendered.contains("TSK-new8"), "amendment names a real id: {rendered}");
    let id = pend[0].id.to_string();
    amend::confirm(&s.project, &[id[4..8].to_string()], amend::Decision::Accept, None).unwrap();
    let (living, _) = change::load_living(&s.project).unwrap();
    let changes = proof::active(&s.project, &living).unwrap();
    let top = git::toplevel(&s.repo).unwrap();
    let cfg = config::load(&s.project, &s.home).unwrap();
    let drift = drift::drift(&s.project, &top, &cfg, &changes[0]).unwrap();
    assert!(drift.is_empty(), "no drift after confirm: {drift:?}");
}

#[test]
fn amend_mints_ids_after_confirmed_amend_is_not_drift() {
    let s = setup("mint-nodrift", &failing_task());
    let dir = active_dir(&s.project);
    let tasks = dir.join("tasks.md");
    let mut text = std::fs::read_to_string(&tasks).unwrap();
    text.push_str("- TSK-new8 Extra\n  verify: grep -q ok extra.txt && echo 'test result: ok. 1 passed; 0 failed;'\n  files: out.txt\n");
    std::fs::write(&tasks, text).unwrap();
    amend::amend(&s.project, &s.home, &s.repo, None, "add a task", None).unwrap();
    let pend = pending_of(&s);
    let id = pend[0].id.to_string();
    amend::confirm(&s.project, &[id[4..8].to_string()], amend::Decision::Accept, None).unwrap();
    let minted = vow::mint_active(&s.project, &s.repo, None).unwrap();
    assert!(minted.is_empty(), "ids mints nothing: {minted:?}");
    let st = proof::status(&s.project, &s.home, &s.repo, false).unwrap();
    assert!(st[0].drift.is_empty(), "no drift: {:?}", st[0].drift);
}
