//! A stale proof is kept without a verify run when the runner's build leaves every artifact as the
//! last passing proof recorded it.

use std::path::PathBuf;
use std::process::Command;

const RVOW: &str = env!("CARGO_BIN_EXE_rvow");

const CONFIG: &str = "[runners.t]\nrun = \"cp src.bin built.bin && grep -q ok out.txt && echo ran >> ran.log && echo 'test result: ok. 1 passed; 0 failed;'\"\nbuild = \"cp src.bin built.bin\"\nartifacts = [\"built.bin\"]\n";

struct Fx {
    root: PathBuf,
    id: String,
}

impl Drop for Fx {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

impl Fx {
    fn new(name: &str) -> Fx {
        let root = std::env::temp_dir().join(format!("rvow-revalidate-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("repo")).unwrap();
        std::fs::create_dir_all(root.join("home")).unwrap();
        let mut fx = Fx { root: root.canonicalize().unwrap(), id: String::new() };
        std::fs::write(fx.repo().join("README"), "x").unwrap();
        fx.run("git", &["init", "-q", "-b", "main"]);
        fx.run("git", &["add", "."]);
        fx.run("git", &["commit", "-q", "-m", "init"]);
        fx.run(RVOW, &["init"]);
        let out = fx.run(RVOW, &["new", "--spec", "Out"]);
        let dir = PathBuf::from(out.lines().find_map(|l| l.strip_prefix("path: ")).unwrap());
        fx.id = out.lines().find_map(|l| l.strip_prefix("created: ")).unwrap().to_string();
        std::fs::write(dir.parent().unwrap().parent().unwrap().join("config.toml"), CONFIG).unwrap();
        std::fs::write(
            dir.join("tasks.md"),
            "- TSK-new1 Write ok\n  covers: REQ-new1\n  verify: @t\n  files: out.txt, src.bin, notes.txt\n",
        )
        .unwrap();
        std::fs::write(
            dir.join("specs/out.md"),
            "## ADD REQ-new1 Out\nrvow SHALL write ok.\n### Scenario: s\n- WHEN it runs\n- THEN out.txt holds ok\n",
        )
        .unwrap();
        fx.run(RVOW, &["accept", &fx.id, "--yes"]);
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

    fn run(&self, program: &str, args: &[&str]) -> String {
        let out = self.cmd(program).args(args).output().unwrap();
        assert!(out.status.success(), "{program} {args:?}: {}", String::from_utf8_lossy(&out.stdout));
        String::from_utf8(out.stdout).unwrap()
    }

    fn task(&self) -> String {
        let status = self.run(RVOW, &["status"]);
        let at = status.find("TSK-").unwrap();
        status[at..].chars().take_while(|c| c.is_ascii_alphanumeric() || *c == '-').collect()
    }

    fn write(&self, file: &str, text: &str) {
        std::fs::write(self.repo().join(file), text).unwrap();
    }

    fn runs(&self) -> usize {
        std::fs::read_to_string(self.repo().join("ran.log")).map_or(0, |t| t.lines().count())
    }

    fn proved(&self) -> Fx {
        let task = self.task();
        let out = self.cmd(RVOW).args(["prove", &task]).output().unwrap();
        assert!(String::from_utf8_lossy(&out.stdout).contains("baseline"), "the first run is the red baseline");
        self.write("out.txt", "ok");
        self.write("src.bin", "A");
        self.run(RVOW, &["prove", &task]);
        assert_eq!(self.runs(), 1);
        Fx { root: self.root.clone(), id: self.id.clone() }
    }
}

#[test]
fn revalidate_flow_prove_stale_keeps_a_proof_when_the_artifacts_match() {
    let fx = Fx::new("stale");
    let fx = fx.proved();
    fx.write("notes.txt", "a comment");
    let out = fx.run(RVOW, &["prove", "--stale"]);
    assert!(out.contains("revalidated"), "{out}");
    assert_eq!(fx.runs(), 1, "the verify did not run again");
    assert!(fx.run(RVOW, &["status"]).contains("proved"));
    std::mem::forget(fx);
}

#[test]
fn revalidate_flow_prove_stale_runs_the_verify_when_an_artifact_changed() {
    let fx = Fx::new("changed");
    let fx = fx.proved();
    fx.write("src.bin", "B");
    let out = fx.run(RVOW, &["prove", "--stale"]);
    assert!(!out.contains("revalidated"), "{out}");
    assert_eq!(fx.runs(), 2, "a changed artifact runs the verify");
    std::mem::forget(fx);
}

#[test]
fn revalidate_flow_reprove_keeps_a_proof_when_the_artifacts_match() {
    let fx = Fx::new("reprove");
    let fx = fx.proved();
    fx.write("notes.txt", "a comment");
    let out = fx.cmd(RVOW).arg("reprove").output().unwrap();
    let text = format!("{}{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr));
    assert!(text.contains("revalidated"), "{text}");
    assert_eq!(fx.runs(), 1, "the verify did not run again");
    std::mem::forget(fx);
}
