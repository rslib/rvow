//! Task file names reach git as literal paths, not as pathspec magic.

use std::path::{Path, PathBuf};
use std::process::Command;

struct Repo(PathBuf);

impl Drop for Repo {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

impl Repo {
    fn new(name: &str) -> Repo {
        let dir = std::env::temp_dir().join(format!("rvow-literal-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("repo")).unwrap();
        let r = Repo(dir.canonicalize().unwrap());
        r.git(&["init", "-q", "-b", "main"]);
        std::fs::write(r.repo().join("a.txt"), "a").unwrap();
        r.git(&["add", "a.txt"]);
        r.git(&["commit", "-q", "-m", "init"]);
        r
    }

    fn repo(&self) -> PathBuf {
        self.0.join("repo")
    }

    fn cmd(&self, program: &str) -> Command {
        let mut c = Command::new(program);
        c.current_dir(self.repo())
            .env("HOME", &self.0)
            .env("RVOW_HOME", self.0.join("home"))
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_AUTHOR_NAME", "t")
            .env("GIT_AUTHOR_EMAIL", "t@t")
            .env("GIT_COMMITTER_NAME", "t")
            .env("GIT_COMMITTER_EMAIL", "t@t");
        c
    }

    fn git(&self, args: &[&str]) {
        assert!(self.cmd("git").args(args).status().unwrap().success());
    }

    fn rvow(&self, args: &[&str]) -> (String, i32) {
        let out = self.cmd(env!("CARGO_BIN_EXE_rvow")).args(args).output().unwrap();
        (String::from_utf8(out.stdout).unwrap(), out.status.code().unwrap())
    }

    fn ok(&self, args: &[&str]) -> String {
        let (out, code) = self.rvow(args);
        assert_eq!(code, 0, "rvow {args:?}: {out}");
        out
    }
}

fn field<'a>(out: &'a str, key: &str) -> &'a str {
    out.lines().find_map(|l| l.strip_prefix(&format!("{key}: "))).unwrap_or_else(|| panic!("no {key} in {out}"))
}

fn next_task(r: &Repo) -> String {
    r.ok(&["brief"])
        .lines()
        .find_map(|l| l.trim_start().strip_prefix("next: "))
        .and_then(|l| l.split_whitespace().next())
        .unwrap()
        .to_owned()
}

#[test]
fn literal_paths_review_packet_holds_magic_name() {
    let r = Repo::new("packet");
    r.ok(&["init"]);
    let out = r.ok(&["new", "--spec", "Out"]);
    let id = field(&out, "created").to_owned();
    let dir = Path::new(field(&out, "path")).to_owned();
    std::fs::write(
        dir.join("tasks.md"),
        "- TSK-new1 Write ok\n  covers: REQ-new1\n  verify: grep -q ok out.txt && echo 'test result: ok. 1 passed; 0 failed;'\n  files: out.txt, :(bad\n",
    )
    .unwrap();
    std::fs::write(
        dir.join("specs/out.md"),
        "## ADD REQ-new1 Out\nrvow SHALL write ok.\n### Scenario: s\n- WHEN it runs\n- THEN out.txt holds ok\n",
    )
    .unwrap();
    std::fs::write(r.repo().join("out.txt"), "old\n").unwrap();
    std::fs::write(r.repo().join(":(bad"), "old\n").unwrap();
    r.git(&["add", "-A"]);
    r.git(&["commit", "-q", "-m", "base"]);
    let accept = r.cmd(env!("CARGO_BIN_EXE_rvow")).args(["accept", &id, "--yes"]).output().unwrap();
    assert!(accept.status.success(), "{}", String::from_utf8_lossy(&accept.stdout));
    std::fs::write(r.repo().join("out.txt"), "ok\n").unwrap();
    std::fs::write(r.repo().join(":(bad"), "new\n").unwrap();
    let task = next_task(&r);
    let proved = r.cmd(env!("CARGO_BIN_EXE_rvow")).args(["prove", &task]).output().unwrap();
    assert!(proved.status.success(), "{}", String::from_utf8_lossy(&proved.stdout));
    let (packet, code) = r.rvow(&["review", &id, "--packet"]);
    assert_eq!(code, 0, "the packet must exit 0 with a magic file name: {packet}");
    assert!(packet.contains(":(bad"), "the packet must hold the diff of :(bad: {packet}");
}

#[test]
fn literal_paths_task_diff_names_only_magic_file() {
    let r = Repo::new("diff");
    let repo = r.repo();
    std::fs::write(repo.join(":(top)x.txt"), "old\n").unwrap();
    std::fs::write(repo.join("other.txt"), "old\n").unwrap();
    r.git(&["add", "-A"]);
    r.git(&["commit", "-q", "-m", "base"]);
    std::fs::write(repo.join(":(top)x.txt"), "new\n").unwrap();
    std::fs::write(repo.join("other.txt"), "new\n").unwrap();
    let diff = rvow_core::git::task_diff(&repo, "HEAD", &[":(top)x.txt".to_string()]).unwrap();
    assert_eq!(diff.files, vec![":(top)x.txt".to_string()]);
    assert!(diff.diff.contains(":(top)x.txt"), "the diff must hold :(top)x.txt: {}", diff.diff);
    assert!(!diff.diff.contains("other.txt"), "the diff must hold no other file: {}", diff.diff);
}
