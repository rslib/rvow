//! The amend baseline: removed tasks and new guard flags are weaken drift,
//! `rvow amend` runs added and changed checks on the base commit, and
//! `rvow confirm` lists weaken items first with each baseline result. One
//! test per spec scenario, with small shell verify commands that print a
//! cargo-style `test result:` line so the baseline stays quick.

use std::path::{Path, PathBuf};
use std::process::Command;

use rvow_gate::{amend, change, config, drift, git, proof, store, vow};

struct Setup {
    _tmp: crate::temp::TempDir,
    home: PathBuf,
    repo: PathBuf,
    project: store::Project,
    /// Placeholder to real id, in mint order.
    minted: Vec<(String, rvow_gate::id::Id)>,
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
    setup_with(name, tasks, |_| {})
}

/// The same, with `before` run on the repository before the accept.
fn setup_with(name: &str, tasks: &[(&str, &str, &str, &str)], before: impl Fn(&Path)) -> Setup {
    let guard = crate::temp::TempDir::new(&format!("baseline-{name}"));
    let tmp = guard.path().to_path_buf();
    let home = tmp.join("home");
    let repo = tmp.join("r");
    std::fs::create_dir_all(&repo).unwrap();
    git_cmd(&repo, &["init", "-q", "-b", "main"]);
    std::fs::write(repo.join("a.txt"), "a").unwrap();
    git_cmd(&repo, &["add", "a.txt"]);
    git_cmd(&repo, &["commit", "-q", "-m", "init"]);

    before(&repo);
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
    Setup { _tmp: guard, home, repo, project, minted: vowed.minted }
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
fn confirm_list_weaken_first_with_digest() {
    let s = setup("order", &[failing()]);
    let dir = active_dir(&s.project);
    let text = std::fs::read_to_string(dir.join("tasks.md")).unwrap();
    let text = text.replace("Write ok", "Write ok clearly");
    std::fs::write(dir.join("tasks.md"), text.replace("grep -q ok out.txt", "grep -q ok other.txt")).unwrap();
    amend::amend(&s.project, &s.home, &s.repo, None, "retitle and recheck", None).unwrap();
    let pend = pending_of(&s);
    assert_eq!(pend.len(), 1, "{pend:?}");
    let lines = amend::listing(&pend, amend::Detail::Full);
    let weaken = lines.iter().position(|l| l.starts_with("  weaken:")).unwrap();
    let free = lines.iter().position(|l| l.starts_with("  free:")).unwrap();
    assert!(weaken < free, "weaken items lead: {lines:?}");
    assert_eq!(lines.last().unwrap(), &format!("digest {}", amend::digest(&pend)), "{lines:?}");
}

/// An amendment runs no check and marks no task pending: the baseline is the code at accept.
#[test]
fn amend_baseline_none_runs_nothing_and_marks_nothing_pending() {
    let s = setup("none", &[failing()]);
    let dir = active_dir(&s.project);
    let mut text = std::fs::read_to_string(dir.join("tasks.md")).unwrap();
    text.push_str("- TSK-new2 Marker\n  verify: touch marker && echo 'test result: ok. 1 passed; 0 failed;'\n  files: out.txt\n");
    std::fs::write(dir.join("tasks.md"), text).unwrap();
    amend::amend(&s.project, &s.home, &s.repo, None, "add a task", None).unwrap();
    assert!(!s.repo.join("marker").exists(), "amend ran the new verify command");
    let pend = pending_of(&s);
    assert_eq!(pend.len(), 1, "{pend:?}");
    let lines = amend::listing(&pend, amend::Detail::Full);
    assert!(lines.iter().all(|l| !l.contains("baseline")), "{lines:?}");
    let marker = task_id_with_title(&dir, "Marker");
    let first = proof::prove(&s.project, &s.home, &s.repo, &marker, &mut std::io::sink()).unwrap();
    assert!(first.proved, "the first prove of an added task is an ordinary proof");
}

/// Widens the task's scope to `files` and records an amendment of it.
fn widen(s: &Setup, files: &str, reason: &str) -> String {
    let dir = active_dir(&s.project);
    let text = std::fs::read_to_string(dir.join("tasks.md")).unwrap();
    let start = text.find("  files: ").unwrap();
    let end = start + text[start..].find('\n').unwrap();
    std::fs::write(dir.join("tasks.md"), format!("{}  files: {files}{}", &text[..start], &text[end..])).unwrap();
    let amended = amend::amend(&s.project, &s.home, &s.repo, None, reason, None).unwrap();
    amended[0].id.to_string()
}

#[test]
fn amend_baseline_newest_snapshot_wins_a_batch_confirm() {
    let s = setup("batch", &[failing()]);
    let older = widen(&s, "out.txt, b.txt", "add b");
    let newer = widen(&s, "out.txt, b.txt, c.txt", "add c");
    // The older amendment's confirm is written last.
    amend::confirm(&s.project, &[newer, older], amend::Decision::Accept, None).unwrap();
    let got = drift_items(&s);
    assert!(got.is_empty(), "the newest snapshot is the baseline: {got:?}");
}

#[test]
fn amend_baseline_newest_rejected_keeps_the_older_snapshot() {
    let s = setup("declined", &[failing()]);
    let older = widen(&s, "out.txt, b.txt", "add b");
    let newer = widen(&s, "out.txt, b.txt, c.txt", "add c");
    amend::confirm(&s.project, &[older], amend::Decision::Accept, None).unwrap();
    amend::confirm(&s.project, &[newer], amend::Decision::Reject, None).unwrap();
    let a = real_id(&s, "TSK-new1");
    let got = drift_items(&s);
    assert_eq!(got.iter().map(|(_, w)| w.clone()).collect::<Vec<_>>(), [format!("{a} scope made wider")], "{got:?}");
}

/// REQ-ahdyae: an unlock recorded while a scope change is pending does not
/// bring the old scope back as drift once both are accepted.
#[test]
fn an_unlock_keeps_the_confirmed_scope() {
    let s = setup_with("unlock-scope", &[failing()], |repo| {
        std::fs::create_dir_all(repo.join("tests")).unwrap();
        std::fs::write(repo.join("tests/check.rs"), "// a test\n").unwrap();
        git_cmd(repo, &["add", "tests/check.rs"]);
        git_cmd(repo, &["commit", "-q", "-m", "test"]);
    });
    widen(&s, "out.txt, more.txt", "widen the scope");
    amend::amend(&s.project, &s.home, &s.repo, None, "the test is wrong", Some("tests/check.rs")).unwrap();
    let ids: Vec<String> = pending_of(&s).iter().map(|p| p.id.to_string()).collect();
    assert_eq!(ids.len(), 2, "{ids:?}");
    amend::confirm(&s.project, &ids, amend::Decision::Accept, None).unwrap();
    let items = drift_items(&s);
    assert!(items.iter().all(|(_, what)| !what.contains("scope")), "the confirmed scope shows as drift again: {items:?}");
}
