//! The amend baseline: removed tasks and new guard flags are weaken drift,
//! `rvow amend` runs added and changed checks on the base commit, and
//! `rvow confirm` lists weaken items first with each baseline result. One
//! test per spec scenario, with small shell verify commands that print a
//! cargo-style `test result:` line so the baseline stays quick.

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
    /// Placeholder to real id, in mint order.
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

/// An accepted change of `tasks` on a fresh repository. Each task entry is
/// `(placeholder, title, verify, extra fields)`.
fn setup(name: &str, tasks: &[(&str, &str, &str, &str)]) -> Setup {
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    let tmp = std::env::temp_dir().join(format!("rvow-baseline-{}-{n}-{name}", std::process::id()));
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
    let mut text = String::new();
    for (id, title, verify, extra) in tasks {
        text.push_str(&format!("- {id} {title}\n  covers: REQ-new1\n  verify: {verify}\n  files: out.txt\n{extra}"));
    }
    std::fs::write(c.path.join("tasks.md"), &text).unwrap();
    std::fs::write(
        c.path.join("specs/out.md"),
        "## ADD REQ-new1 Out\nrvow SHALL write ok.\n### Scenario: s\n- WHEN it runs\n- THEN out.txt holds ok\n",
    )
    .unwrap();
    let prepared = vow::prepare(&project, &home, c, &repo, &mut std::io::sink()).unwrap();
    let vowed = vow::commit(&project, prepared).unwrap();
    Setup { _tmp: tmp, home, repo, project, minted: vowed.minted }
}

fn failing() -> (&'static str, &'static str, &'static str, &'static str) {
    ("TSK-new1", "Write ok", "grep -q ok out.txt && echo 'test result: ok. 1 passed; 0 failed;'", "")
}

fn real_id(s: &Setup, placeholder: &str) -> String {
    s.minted.iter().find(|(from, _)| from == placeholder).unwrap().1.to_string()
}

fn active_dir(project: &store::Project) -> PathBuf {
    std::fs::read_dir(project.root.join("active")).unwrap().next().unwrap().unwrap().path()
}

/// The task id of the entry whose title contains `title`, read back from
/// tasks.md after amend minted the placeholders.
fn task_id_with_title(dir: &Path, title: &str) -> String {
    let text = std::fs::read_to_string(dir.join("tasks.md")).unwrap();
    for line in text.lines().filter(|l| l.starts_with("- ")) {
        if line.contains(title) {
            let id = line[2..].split_whitespace().next().unwrap().to_string();
            assert!(id.starts_with("TSK-"), "no TSK- id in {line}");
            return id;
        }
    }
    panic!("no {title} in {text}");
}

fn drift_items(s: &Setup) -> Vec<(drift::Class, String)> {
    let (living, _) = change::load_living(&s.project).unwrap();
    let changes = proof::active(&s.project, &living).unwrap();
    let top = git::toplevel(&s.repo).unwrap();
    let cfg = config::load(&s.project, &s.home).unwrap();
    drift::drift(&s.project, &top, &cfg, &changes[0]).unwrap().into_iter().map(|i| (i.class, i.what)).collect()
}

fn pending_of(s: &Setup) -> Vec<amend::Pending> {
    let (living, _) = change::load_living(&s.project).unwrap();
    let changes = proof::active(&s.project, &living).unwrap();
    amend::pending(&changes)
}

/// Keeps only the entry of `id` (and its fields) from a tasks file.
fn keep_task(text: &str, id: &str) -> String {
    let mut out = String::new();
    let mut keep = false;
    for line in text.lines() {
        if line.starts_with("- ") {
            keep = line.starts_with(&format!("- {id} "));
        }
        if keep {
            out.push_str(line);
            out.push('\n');
        }
    }
    assert!(out.contains(id), "no {id} in {text}");
    out
}

#[test]
fn drift_removed_task_is_weaken() {
    let s = setup(
        "removed",
        &[
            ("TSK-new1", "Write a", "grep -q ok a.txt && echo 'test result: ok. 1 passed; 0 failed;'", ""),
            ("TSK-new2", "Write b", "grep -q ok b.txt && echo 'test result: ok. 1 passed; 0 failed;'", ""),
        ],
    );
    let a = real_id(&s, "TSK-new1");
    let b = real_id(&s, "TSK-new2");
    let dir = active_dir(&s.project);
    let text = std::fs::read_to_string(dir.join("tasks.md")).unwrap();
    std::fs::write(dir.join("tasks.md"), keep_task(&text, &a)).unwrap();
    let got = drift_items(&s);
    assert_eq!(got, [(drift::Class::Weaken, format!("{b} removed"))], "{got:?}");
}

#[test]
fn drift_new_guard_flag_is_weaken() {
    let s = setup("guard-new", &[failing()]);
    let a = real_id(&s, "TSK-new1");
    let dir = active_dir(&s.project);
    let text = std::fs::read_to_string(dir.join("tasks.md")).unwrap();
    std::fs::write(dir.join("tasks.md"), text.replacen("  verify:", "  baseline: pass\n  verify:", 1)).unwrap();
    let got = drift_items(&s);
    assert_eq!(got, [(drift::Class::Weaken, format!("{a} baseline pass added"))], "{got:?}");
}

#[test]
fn drift_dropped_guard_flag_is_free() {
    let s = setup("guard-drop", &[("TSK-new1", "Write ok", "echo 'test result: ok. 1 passed; 0 failed;'", "  baseline: pass\n")]);
    let a = real_id(&s, "TSK-new1");
    let dir = active_dir(&s.project);
    let text = std::fs::read_to_string(dir.join("tasks.md")).unwrap();
    assert!(text.contains("baseline: pass"), "{text}");
    std::fs::write(dir.join("tasks.md"), text.replace("  baseline: pass\n", "")).unwrap();
    let got = drift_items(&s);
    assert_eq!(got, [(drift::Class::Free, format!("{a} baseline pass removed"))], "{got:?}");
}

#[test]
fn drift_longer_timeout_is_free() {
    let s = setup(
        "timeout",
        &[("TSK-new1", "Write ok", "grep -q ok out.txt && echo 'test result: ok. 1 passed; 0 failed;'", "  timeout: 90s\n")],
    );
    let a = real_id(&s, "TSK-new1");
    let dir = active_dir(&s.project);
    let text = std::fs::read_to_string(dir.join("tasks.md")).unwrap();
    std::fs::write(dir.join("tasks.md"), text.replace("timeout: 90s", "timeout: 20m")).unwrap();
    let got = drift_items(&s);
    assert_eq!(got, [(drift::Class::Free, format!("{a} timeout changed"))], "{got:?}");
}

#[test]
fn amend_baseline_passing_check_is_refused() {
    let s = setup("refuse-pass", &[failing()]);
    let dir = active_dir(&s.project);
    let mut text = std::fs::read_to_string(dir.join("tasks.md")).unwrap();
    text.push_str("- TSK-new2 Extra check\n  verify: echo 'test result: ok. 2 passed; 0 failed;'\n  files: out.txt\n");
    std::fs::write(dir.join("tasks.md"), text).unwrap();
    let err = amend::amend(&s.project, &s.home, &s.repo, None, "add a task", None).unwrap_err();
    assert_eq!(err.code, "baseline", "{err:?}");
    assert!(err.message.contains("passes on the base commit"), "{err:?}");
    assert!(!err.message.contains("TSK-new2"), "amend mints before the baseline: {err:?}");
    let minted = task_id_with_title(&dir, "Extra check");
    assert_ne!(minted, "TSK-new2", "the new task keeps no placeholder: {minted}");
    assert!(err.message.contains(&minted), "the message names the minted id {minted}: {err:?}");
    assert!(pending_of(&s).is_empty(), "a refused amendment records nothing");
}

#[test]
fn amend_baseline_failing_check_is_recorded() {
    let s = setup("record-fail", &[failing()]);
    let a = real_id(&s, "TSK-new1");
    let dir = active_dir(&s.project);
    let text = std::fs::read_to_string(dir.join("tasks.md")).unwrap();
    std::fs::write(
        dir.join("tasks.md"),
        text.replace(
            "grep -q ok out.txt && echo 'test result: ok. 1 passed; 0 failed;'",
            "echo 'test result: ok. 2 passed; 0 failed;'; exit 101",
        ),
    )
    .unwrap();
    amend::amend(&s.project, &s.home, &s.repo, None, "name the real test", None).unwrap();
    let pend = pending_of(&s);
    assert_eq!(pend.len(), 1, "{pend:?}");
    assert_eq!(
        pend[0].baseline,
        [amend::BaselineRun { task: a.clone(), exit: Some(101), tests_run: Some(2), timed_out: false }],
        "{:?}",
        pend[0].baseline
    );
}

#[test]
fn amend_baseline_guard_must_pass() {
    let s = setup("refuse-guard", &[failing()]);
    let dir = active_dir(&s.project);
    let mut text = std::fs::read_to_string(dir.join("tasks.md")).unwrap();
    text.push_str("- TSK-new2 Guard\n  verify: false\n  files: out.txt\n  baseline: pass\n");
    std::fs::write(dir.join("tasks.md"), text).unwrap();
    let err = amend::amend(&s.project, &s.home, &s.repo, None, "add a guard", None).unwrap_err();
    assert_eq!(err.code, "baseline", "{err:?}");
    assert!(!err.message.contains("TSK-new2"), "amend mints before the baseline: {err:?}");
    let minted = task_id_with_title(&dir, "Guard");
    assert_ne!(minted, "TSK-new2", "the new task keeps no placeholder: {minted}");
    assert!(err.message.contains(&minted), "the message names the minted id {minted}: {err:?}");
    assert!(pending_of(&s).is_empty(), "a refused amendment records nothing");
}

#[test]
fn amend_baseline_title_edit_runs_nothing() {
    let s = setup("title-only", &[failing()]);
    let dir = active_dir(&s.project);
    let text = std::fs::read_to_string(dir.join("tasks.md")).unwrap();
    std::fs::write(dir.join("tasks.md"), text.replace("Write ok", "Write ok clearly")).unwrap();
    amend::amend(&s.project, &s.home, &s.repo, None, "clearer title", None).unwrap();
    let pend = pending_of(&s);
    assert_eq!(pend.len(), 1, "{pend:?}");
    assert!(pend[0].baseline.is_empty(), "a title edit runs nothing: {:?}", pend[0].baseline);
}

#[test]
fn confirm_list_weaken_first_with_digest() {
    let s = setup("order", &[failing()]);
    let dir = active_dir(&s.project);
    let text = std::fs::read_to_string(dir.join("tasks.md")).unwrap();
    let text = text.replace("Write ok", "Write ok clearly");
    std::fs::write(dir.join("tasks.md"), text.replace("grep -q ok out.txt", "grep -q ok other.txt")).unwrap();
    amend::amend(&s.project, &s.home, &s.repo, None, "retitle and recheck", None).unwrap();
    let pend = pending_of(&s);
    assert_eq!(pend.len(), 1, "{pend:?}");
    let lines = amend::listing(&pend);
    let weaken = lines.iter().position(|l| l.starts_with("  weaken:")).unwrap();
    let free = lines.iter().position(|l| l.starts_with("  free:")).unwrap();
    assert!(weaken < free, "weaken items lead: {lines:?}");
    assert_eq!(lines.last().unwrap(), &format!("digest {}", amend::digest(&pend)), "{lines:?}");
}

#[test]
fn confirm_list_shows_the_baseline_result() {
    let s = setup("baseline-line", &[failing()]);
    let dir = active_dir(&s.project);
    let mut text = std::fs::read_to_string(dir.join("tasks.md")).unwrap();
    text.push_str("- TSK-new2 Extra check\n  verify: echo 'test result: ok. 3 passed; 0 failed;'; exit 1\n  files: out.txt\n");
    std::fs::write(dir.join("tasks.md"), text).unwrap();
    amend::amend(&s.project, &s.home, &s.repo, None, "add a check", None).unwrap();
    let pend = pending_of(&s);
    assert_eq!(pend.len(), 1, "{pend:?}");
    let lines = amend::listing(&pend);
    assert!(lines.iter().any(|l| l.contains("fails before the work") && l.contains("exit 1") && l.contains("3 tests")), "{lines:?}");
    assert!(lines.iter().all(|l| !l.contains("TSK-new2")), "amend mints before the snapshot: {lines:?}");
    let minted = &pend[0].baseline[0].task;
    assert_ne!(minted, "TSK-new2", "the baseline names no placeholder: {lines:?}");
    assert!(minted.starts_with("TSK-"), "the baseline names the minted id: {lines:?}");
    assert!(lines.iter().any(|l| l.contains(minted)), "the listing names the minted id {minted}: {lines:?}");
}
