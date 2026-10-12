//! A committed test file is not read for owner command text (CHG-yxa55h).
//! Each test drives the hook entry on a script in a throwaway git repository.

use std::path::PathBuf;
use std::process::Command;

use rvow_core::hook::{self, Denial};
use rvow_core::store::{self, Project};
use serde_json::json;

const SCRIPT: &str = "t/deny.test.js";
const TEXT: &str = "rvow accept CHG-x\nrvow confirm CHG-x\n";
const CMD: &str = "node --test t/deny.test.js";

struct Fx {
    root: PathBuf,
}

impl Drop for Fx {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

fn git_command(repo: &std::path::Path) -> Command {
    let mut c = Command::new("git");
    c.current_dir(repo)
        .env_remove("GIT_DIR")
        .env_remove("GIT_WORK_TREE")
        .env_remove("GIT_INDEX_FILE")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_AUTHOR_NAME", "t")
        .env("GIT_AUTHOR_EMAIL", "t@t")
        .env("GIT_COMMITTER_NAME", "t")
        .env("GIT_COMMITTER_EMAIL", "t@t");
    c
}

impl Fx {
    /// A git repository with one commit and no test script yet.
    fn new(name: &str) -> Fx {
        let root = std::env::temp_dir().join(format!("rvow-committed-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("repo")).unwrap();
        let fx = Fx { root: root.canonicalize().unwrap() };
        let repo = fx.repo();
        std::fs::write(repo.join("a.txt"), "a").unwrap();
        for args in [vec!["init", "-q", "-b", "main"], vec!["add", "."], vec!["commit", "-q", "-m", "init"]] {
            assert!(git_command(&repo).args(&args).status().unwrap().success(), "git {args:?}");
        }
        fx
    }

    fn repo(&self) -> PathBuf {
        self.root.join("repo")
    }

    fn home(&self) -> PathBuf {
        self.root.join("rh")
    }

    fn git(&self, args: &[&str]) {
        assert!(git_command(&self.repo()).args(args).status().unwrap().success(), "git {args:?}");
    }

    fn rvow(&self, args: &[&str]) -> String {
        let out = Command::new(env!("CARGO_BIN_EXE_rvow"))
            .args(args)
            .current_dir(self.repo())
            .env("HOME", self.root.join("home"))
            .env("RVOW_HOME", self.home())
            .env("PATH", "/usr/bin:/bin:/usr/sbin:/sbin")
            .env_remove("GIT_DIR")
            .env_remove("GIT_WORK_TREE")
            .env_remove("GIT_INDEX_FILE")
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .output()
            .unwrap();
        assert!(out.status.success(), "rvow {args:?}: {}", String::from_utf8_lossy(&out.stdout));
        String::from_utf8_lossy(&out.stdout).into_owned()
    }

    /// Commits `t/deny.test.js` with the given text and writes it back, so the
    /// file has no change against HEAD.
    fn commit_script(&self) {
        std::fs::write(self.repo().join(SCRIPT), TEXT).unwrap();
        self.git(&["add", SCRIPT]);
        self.git(&["commit", "-q", "-m", "script"]);
        std::fs::write(self.repo().join(SCRIPT), TEXT).unwrap();
    }

    /// Commits `t/deny.test.js` clean, then appends the given lines, so the
    /// added lines hold exactly `text`.
    fn commit_then_append(&self, text: &str) {
        std::fs::write(self.repo().join(SCRIPT), "base line\n").unwrap();
        self.git(&["add", SCRIPT]);
        self.git(&["commit", "-q", "-m", "script"]);
        let mut full = String::from("base line\n");
        full.push_str(text);
        std::fs::write(self.repo().join(SCRIPT), full).unwrap();
    }

    fn project(&self) -> Project {
        store::find(&self.repo(), &self.home()).unwrap().unwrap()
    }

    fn run(&self, command: &str) -> Option<Denial> {
        self.run_in(&self.repo(), command)
    }

    fn run_in(&self, cwd: &std::path::Path, command: &str) -> Option<Denial> {
        let payload = json!({"cwd": cwd, "tool_name": "Bash", "tool_input": {"command": command}});
        hook::pre_tool_use_denial(&self.project(), &payload).unwrap()
    }
}

#[test]
fn committed_tests_committed_test_file_runs() {
    let fx = Fx::new("committed");
    std::fs::create_dir_all(fx.repo().join("t")).unwrap();
    std::fs::write(fx.repo().join(SCRIPT), TEXT).unwrap();
    fx.rvow(&["init"]);
    fx.commit_script();
    assert_eq!(fx.run(CMD), None, "a committed test file is not read");
}

#[test]
fn committed_tests_changed_test_file_is_denied() {
    let fx = Fx::new("changed");
    std::fs::create_dir_all(fx.repo().join("t")).unwrap();
    fx.rvow(&["init"]);
    std::fs::write(fx.repo().join(SCRIPT), "rvow prove CHG-x\n").unwrap();
    fx.git(&["add", SCRIPT]);
    fx.git(&["commit", "-q", "-m", "clean"]);
    std::fs::write(fx.repo().join(SCRIPT), TEXT).unwrap();
    let denial = fx.run(CMD);
    assert!(denial.as_ref().is_some_and(|d| d.reason.contains("deny.test.js")), "{denial:?}");
}

#[test]
fn committed_tests_untracked_test_file_is_denied() {
    let fx = Fx::new("untracked");
    std::fs::create_dir_all(fx.repo().join("t")).unwrap();
    fx.rvow(&["init"]);
    std::fs::write(fx.repo().join(SCRIPT), TEXT).unwrap();
    let denial = fx.run(CMD);
    assert!(denial.as_ref().is_some_and(|d| d.reason.contains("deny.test.js")), "{denial:?}");
}

#[test]
fn committed_tests_committed_non_test_script_is_denied() {
    let fx = Fx::new("non-test");
    fx.rvow(&["init"]);
    std::fs::write(fx.repo().join("run.py"), TEXT).unwrap();
    fx.git(&["add", "run.py"]);
    fx.git(&["commit", "-q", "-m", "script"]);
    std::fs::write(fx.repo().join("run.py"), TEXT).unwrap();
    let denial = fx.run("python3 run.py");
    assert!(denial.as_ref().is_some_and(|d| d.reason.contains("run.py")), "{denial:?}");
}

#[test]
fn committed_tests_subfolder_uses_the_file_that_runs() {
    let fx = Fx::new("subfolder-wrong");
    // The old code joined the file name to the top, so a clean tracked
    // x.test.js there let the untracked sub/x.test.js pass.
    std::fs::write(fx.repo().join("x.test.js"), "rvow prove CHG-x\n").unwrap();
    std::fs::create_dir_all(fx.repo().join("tests")).unwrap();
    std::fs::write(fx.repo().join("tests/x.test.js"), "rvow prove CHG-x\n").unwrap();
    fx.rvow(&["init"]);
    fx.git(&["add", "x.test.js", "tests/x.test.js"]);
    fx.git(&["commit", "-q", "-m", "clean"]);
    std::fs::create_dir_all(fx.repo().join("sub")).unwrap();
    std::fs::write(fx.repo().join("sub/x.test.js"), TEXT).unwrap();
    let denial = fx.run_in(&fx.repo().join("sub"), "node --test x.test.js");
    assert!(denial.is_some(), "the file that runs is checked, not the one at the top");
}

#[test]
fn committed_tests_subfolder_committed_file_runs() {
    let fx = Fx::new("subfolder-committed");
    std::fs::create_dir_all(fx.repo().join("tests")).unwrap();
    std::fs::write(fx.repo().join("tests/x.test.js"), TEXT).unwrap();
    fx.rvow(&["init"]);
    fx.git(&["add", "tests/x.test.js"]);
    fx.git(&["commit", "-q", "-m", "clean"]);
    std::fs::write(fx.repo().join("tests/x.test.js"), TEXT).unwrap();
    let denial = fx.run_in(&fx.repo().join("tests"), "node --test x.test.js");
    assert_eq!(denial, None, "a committed test file in a subfolder is not read");
}

#[test]
fn committed_tests_added_plain_line_runs() {
    let fx = Fx::new("added-plain");
    std::fs::create_dir_all(fx.repo().join("t")).unwrap();
    fx.rvow(&["init"]);
    fx.commit_script();
    std::fs::write(fx.repo().join(SCRIPT), format!("{TEXT}just a plain line\n")).unwrap();
    assert_eq!(fx.run(CMD), None, "only the lines added against HEAD are read");
}

#[test]
fn committed_tests_added_owner_line_is_denied() {
    let fx = Fx::new("added-owner");
    std::fs::create_dir_all(fx.repo().join("t")).unwrap();
    fx.rvow(&["init"]);
    fx.commit_then_append("rvow accept CHG-x\n");
    let denial = fx.run(CMD);
    assert!(denial.as_ref().is_some_and(|d| d.reason.contains("deny.test.js")), "{denial:?}");
}

#[test]
fn committed_tests_slash_name_runs() {
    let fx = Fx::new("slash-only");
    std::fs::create_dir_all(fx.repo().join("t")).unwrap();
    fx.rvow(&["init"]);
    std::fs::write(fx.repo().join(SCRIPT), "/rvow-accept CHG-a1\n").unwrap();
    assert_eq!(fx.run(CMD), None, "a slash command name cannot run from a script");
}

#[test]
fn committed_tests_slash_name_with_owner_call_is_denied() {
    let fx = Fx::new("slash-with-owner");
    std::fs::create_dir_all(fx.repo().join("t")).unwrap();
    fx.rvow(&["init"]);
    std::fs::write(fx.repo().join(SCRIPT), "/rvow-accept CHG-a1\nrvow accept CHG-a1\n").unwrap();
    let denial = fx.run(CMD);
    assert!(denial.as_ref().is_some_and(|d| d.reason.contains("deny.test.js")), "{denial:?}");
}

#[test]
fn committed_tests_symlink_is_checked() {
    let fx = Fx::new("symlink");
    fx.rvow(&["init"]);
    std::fs::write(fx.repo().join("evil.txt"), TEXT).unwrap();
    std::os::unix::fs::symlink("evil.txt", fx.repo().join("link.test.js")).unwrap();
    fx.git(&["add", "link.test.js"]);
    fx.git(&["commit", "-q", "-m", "link"]);
    let denial = fx.run("node --test link.test.js");
    assert!(denial.is_some(), "a symlink is not a committed test file");
}

#[test]
fn committed_tests_added_line_keeps_the_file_shebang() {
    let fx = Fx::new("added-shebang");
    std::fs::create_dir_all(fx.repo().join("t")).unwrap();
    fx.rvow(&["init"]);
    std::fs::write(fx.repo().join(SCRIPT), "#!/usr/bin/env node\nconsole.log(1)\n").unwrap();
    fx.git(&["add", SCRIPT]);
    fx.git(&["commit", "-q", "-m", "clean"]);
    std::fs::write(
        fx.repo().join(SCRIPT),
        "#!/usr/bin/env node\nconsole.log(1)\nrequire('child_process').execSync('rvow ' + 'accept CHG-x')\n",
    )
    .unwrap();
    let denial = fx.run("./t/deny.test.js");
    assert!(denial.as_ref().is_some_and(|d| d.reason.contains("deny.test.js")), "{denial:?}");
}
