//! The `rvow prove` verdict of a runner task: it names the junit report, and a
//! non-zero exit wins over that report (CHG-vq0f01).

use std::path::{Path, PathBuf};
use std::process::Command;

use rvow_core::{change, store, vow};

const SPEC: &str = "## ADD REQ-new1 Out\nrvow SHALL run the runner command.\n### Scenario: s\n- WHEN a task names a runner\n- THEN rvow runs the filled command\n";

struct Fx {
    _tmp: crate::temp::TempDir,
    home: PathBuf,
    repo: PathBuf,
    project: store::Project,
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

/// A project with one commit, and the given project config.toml.
fn fx(name: &str, config_text: &str) -> Fx {
    let guard = crate::temp::TempDir::new(&format!("runners-{name}"));
    let tmp = guard.path().to_path_buf();
    let home = tmp.join("home");
    let repo = tmp.join("r");
    std::fs::create_dir_all(&repo).unwrap();
    git_cmd(&repo, &["init", "-q", "-b", "main"]);
    std::fs::write(repo.join("a.txt"), "a").unwrap();
    git_cmd(&repo, &["add", "a.txt"]);
    git_cmd(&repo, &["commit", "-q", "-m", "init"]);
    let project = store::init(&repo, &home, store::Mode::Home).unwrap();
    if !config_text.is_empty() {
        std::fs::write(project.root.join("config.toml"), config_text).unwrap();
    }
    Fx { _tmp: guard, home, repo, project }
}

/// A backlog change with one task that uses `verify` and covers REQ-new1.
fn backlog(f: &Fx, verify: &str, fields: &str) -> change::ChangeDir {
    let c = change::new(&f.project, "Run", "main", false).unwrap();
    std::fs::write(
        c.path.join("tasks.md"),
        format!("- TSK-new1 Run the runner\n  covers: REQ-new1\n  verify: {verify}\n  files: out.txt\n{fields}"),
    )
    .unwrap();
    std::fs::write(c.path.join("specs/out.md"), SPEC).unwrap();
    std::fs::write(c.path.join("proposal.md"), "branch: main\n\n# Run\n\n## Why\nw\n\n## What changes\nw\n\n## Out of scope\nNone.\n")
        .unwrap();
    c
}

/// Accepts the change and returns the minted task id.
fn accept(f: &Fx, c: change::ChangeDir) -> String {
    let prepared = vow::prepare(&f.project, &f.home, c, &f.repo, &mut std::io::sink()).unwrap();
    accept_prepared(f, prepared)
}

fn accept_prepared(f: &Fx, prepared: vow::Prepared) -> String {
    let vowed = vow::commit(&f.project, prepared).unwrap();
    vowed.minted.iter().find(|(from, _)| from == "TSK-new1").unwrap().1.to_string()
}

const RUNNER_JUNIT: &str = "\
[runners.t]
run = 'if grep -q pass out.txt; then cp pass.xml {junit}; else cp fail.xml {junit}; exit 1; fi'
";
const XML_PASS: &str = "<testsuite name=\"t\"><testcase classname=\"c\" name=\"Seek\"></testcase><testcase classname=\"c\" name=\"Open\"></testcase></testsuite>\n";
const XML_FAIL: &str = "<testsuite name=\"t\"><testcase classname=\"c\" name=\"Seek\"><failure>boom</failure></testcase><testcase classname=\"c\" name=\"Open\"></testcase></testsuite>\n";

/// A repo with the two JUnit fixtures committed.
fn fx_junit(name: &str, cfg: &str) -> Fx {
    let f = fx(name, cfg);
    std::fs::write(f.repo.join("pass.xml"), XML_PASS).unwrap();
    std::fs::write(f.repo.join("fail.xml"), XML_FAIL).unwrap();
    git_cmd(&f.repo, &["add", "pass.xml", "fail.xml"]);
    git_cmd(&f.repo, &["commit", "-q", "-m", "fixtures"]);
    f
}

#[test]
fn runners_fix_show_prove_verdict_names_junit() {
    // The command passed but the junit check failed: the verdict names the
    // report, it does not claim 0 tests ran.
    let f = fx_junit("show-verdict", RUNNER_JUNIT);
    let task = accept(&f, backlog(&f, "@t", ""));
    std::fs::write(f.repo.join("pass.xml"), "<testsuite></testsuite>\n").unwrap();
    std::fs::write(f.repo.join("out.txt"), "pass\n").unwrap();
    let out = std::process::Command::new(env!("CARGO_BIN_EXE_rvow"))
        .current_dir(&f.repo)
        .env("HOME", f._tmp.path())
        .env("RVOW_HOME", &f.home)
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .args(["prove", &task])
        .output()
        .unwrap();
    let stdout = String::from_utf8(out.stdout).unwrap();
    assert!(stdout.contains("failed: junit report: the JUnit report names no test cases"), "{stdout}");
    assert!(!stdout.contains("ran 0 tests"), "{stdout}");
}

#[test]
fn runners_fix_show_prove_verdict_exit_beats_junit() {
    // A non-zero exit keeps "failed: exit N" even when the junit check also
    // failed; the junit verdict shows only when the command passed.
    let f = fx_junit("show-exit", RUNNER_JUNIT);
    let task = accept(&f, backlog(&f, "@t", ""));
    let run_prove = |fx: &Fx| {
        let out = std::process::Command::new(env!("CARGO_BIN_EXE_rvow"))
            .current_dir(&fx.repo)
            .env("HOME", fx._tmp.path())
            .env("RVOW_HOME", &fx.home)
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .args(["prove", &task])
            .output()
            .unwrap();
        String::from_utf8(out.stdout).unwrap()
    };
    let stdout = run_prove(&f);
    assert!(stdout.contains("failed: exit 1"), "{stdout}");
    assert!(!stdout.contains("junit report"), "{stdout}");

    std::fs::write(f.repo.join("pass.xml"), "<testsuite></testsuite>\n").unwrap();
    std::fs::write(f.repo.join("out.txt"), "pass\n").unwrap();
    let stdout = run_prove(&f);
    assert!(stdout.contains("failed: junit report: the JUnit report names no test cases"), "{stdout}");
}
