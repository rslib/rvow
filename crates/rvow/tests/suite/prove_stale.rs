//! `rvow prove --stale` proves every stale task, also after one that rvow refuses to run.

use std::path::PathBuf;
use std::process::{Command, Output};

const RVOW: &str = env!("CARGO_BIN_EXE_rvow");

const CONFIG: &str = "[runners.t]\nrun = \"grep -q ok a.txt && echo 'test result: ok. 1 passed; 0 failed;'\"\nneeds = [\"builddir\"]\n";

struct Fx {
    root: PathBuf,
}

impl Drop for Fx {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

impl Fx {
    fn new(name: &str) -> Fx {
        let root = std::env::temp_dir().join(format!("rvow-prove-stale-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("repo")).unwrap();
        std::fs::create_dir_all(root.join("home")).unwrap();
        let fx = Fx { root: root.canonicalize().unwrap() };
        std::fs::write(fx.repo().join("README"), "x").unwrap();
        fx.run("git", &["init", "-q", "-b", "main"]);
        fx.run("git", &["add", "."]);
        fx.run("git", &["commit", "-q", "-m", "init"]);
        fx.run(RVOW, &["init"]);
        let out = fx.run(RVOW, &["new", "--spec", "Out"]);
        let dir = PathBuf::from(out.lines().find_map(|l| l.strip_prefix("path: ")).unwrap());
        let id = out.lines().find_map(|l| l.strip_prefix("created: ")).unwrap().to_string();
        std::fs::write(dir.parent().unwrap().parent().unwrap().join("config.toml"), CONFIG).unwrap();
        std::fs::write(
            dir.join("tasks.md"),
            "- TSK-new1 Write a\n  covers: REQ-new1\n  verify: @t\n  files: a.txt, notes.txt\n- TSK-new2 Write b\n  covers: REQ-new2\n  verify: grep -q ok b.txt && echo 'test result: ok. 1 passed; 0 failed;'\n  files: b.txt, notes.txt\n",
        )
        .unwrap();
        std::fs::write(
            dir.join("specs/out.md"),
            "## ADD REQ-new1 A\nrvow SHALL write a.\n### Scenario: a\n- WHEN it runs\n- THEN a.txt holds ok\n\n## ADD REQ-new2 B\nrvow SHALL write b.\n### Scenario: b\n- WHEN it runs\n- THEN b.txt holds ok\n",
        )
        .unwrap();
        std::fs::create_dir(fx.repo().join("builddir")).unwrap();
        fx.run(RVOW, &["accept", &id, "--yes"]);
        fx
    }

    fn repo(&self) -> PathBuf {
        self.root.join("repo")
    }

    fn cmd(&self, program: &str) -> Command {
        let mut c = Command::new(program);
        c.current_dir(self.repo())
            .env("HOME", self.root.join("home"))
            .env("RVOW_HOME", self.root.join("rh"))
            .env_remove("CLAUDE_CONFIG_DIR")
            .env("PATH", "/usr/bin:/bin:/usr/sbin:/sbin")
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_AUTHOR_NAME", "t")
            .env("GIT_AUTHOR_EMAIL", "t@t")
            .env("GIT_COMMITTER_NAME", "t")
            .env("GIT_COMMITTER_EMAIL", "t@t");
        c
    }

    fn output(&self, program: &str, args: &[&str]) -> Output {
        self.cmd(program).args(args).output().unwrap()
    }

    fn run(&self, program: &str, args: &[&str]) -> String {
        let out = self.output(program, args);
        assert!(out.status.success(), "{program} {args:?}: {}", String::from_utf8_lossy(&out.stdout));
        String::from_utf8(out.stdout).unwrap()
    }

    fn tasks(&self) -> Vec<String> {
        let status = self.run(RVOW, &["status"]);
        status
            .split("TSK-")
            .skip(1)
            .map(|t| format!("TSK-{}", t.chars().take_while(|c| c.is_ascii_alphanumeric()).collect::<String>()))
            .fold(Vec::new(), |mut seen, id| {
                if !seen.contains(&id) {
                    seen.push(id);
                }
                seen
            })
    }
}

#[test]
fn prove_stale_goes_on_after_a_refused_task_and_exits_non_zero() {
    let fx = Fx::new("refused");
    let tasks = fx.tasks();
    assert_eq!(tasks.len(), 2, "{tasks:?}");
    for task in &tasks {
        fx.output(RVOW, &["prove", task]);
    }
    std::fs::write(fx.repo().join("a.txt"), "ok").unwrap();
    std::fs::write(fx.repo().join("b.txt"), "ok").unwrap();
    for task in &tasks {
        fx.run(RVOW, &["prove", task]);
    }
    std::fs::write(fx.repo().join("notes.txt"), "a comment").unwrap();
    std::fs::remove_dir(fx.repo().join("builddir")).unwrap();
    let out = fx.output(RVOW, &["prove", "--stale"]);
    let text = String::from_utf8_lossy(&out.stdout).to_string();
    assert!(!out.status.success(), "a refused task fails the command: {text}");
    assert!(text.contains(&tasks[0]) && text.contains("needs"), "the refusal is reported: {text}");
    let status = fx.run(RVOW, &["status"]);
    let second = status.lines().find(|l| l.contains(&tasks[1])).unwrap();
    assert!(second.contains("proved"), "the later stale task was proved: {second}");
}
