//! rvow runs the git that `RVOW_GIT` names. Each test drives the built binary
//! in a throwaway repository with a wrapper script that logs its arguments
//! and then runs the real git.

use std::path::{Path, PathBuf};
use std::process::Command;

const RVOW: &str = env!("CARGO_BIN_EXE_rvow");

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
        let root = std::env::temp_dir().join(format!("rvow-gitbin-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        Fx { root: root.canonicalize().unwrap() }
    }

    fn env(&self, c: &mut Command) {
        c.env("HOME", self.root.join("home"))
            .env("RVOW_HOME", self.root.join("rh"))
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_AUTHOR_NAME", "t")
            .env("GIT_AUTHOR_EMAIL", "t@t")
            .env("GIT_COMMITTER_NAME", "t")
            .env("GIT_COMMITTER_EMAIL", "t@t");
    }

    fn git(&self, dir: &Path, args: &[&str]) {
        let mut c = Command::new("git");
        c.current_dir(dir);
        self.env(&mut c);
        let out = c.args(args).output().unwrap();
        assert!(out.status.success(), "git {args:?}: {}", String::from_utf8_lossy(&out.stderr));
    }

    /// An initialized repository: one commit and an in-repo rvow tree, so
    /// `rvow brief` exits 0.
    fn repo(&self) -> PathBuf {
        let dir = self.root.join("work");
        std::fs::create_dir_all(&dir).unwrap();
        self.git(&dir, &["init", "-q", "-b", "main"]);
        std::fs::write(dir.join("a.txt"), "a").unwrap();
        self.git(&dir, &["add", "a.txt"]);
        self.git(&dir, &["commit", "-q", "-m", "init"]);
        let mut c = Command::new(RVOW);
        c.current_dir(&dir).args(["init", "--in-repo"]);
        self.env(&mut c);
        let out = c.output().unwrap();
        assert!(out.status.success(), "rvow init: {}", String::from_utf8_lossy(&out.stdout));
        dir
    }

    /// A wrapper script that logs its arguments to `log` and then runs the
    /// real git. Returns the script path and the log path.
    fn wrapper(&self) -> (PathBuf, PathBuf) {
        let script = self.root.join("gitwrap.sh");
        let log = self.root.join("git.log");
        std::fs::write(&script, format!("#!/bin/sh\nprintf '%s\\n' \"$*\" >> \"{}\"\nexec git \"$@\"\n", log.display())).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        (script, log)
    }

    fn brief(&self, dir: &Path, rvow_git: Option<&Path>) -> std::process::Output {
        self.brief_with(dir, rvow_git, None)
    }

    fn brief_with(&self, dir: &Path, rvow_git: Option<&Path>, eval_git: Option<&Path>) -> std::process::Output {
        let mut c = Command::new(RVOW);
        c.current_dir(dir).args(["brief"]);
        self.env(&mut c);
        match rvow_git {
            Some(bin) => {
                c.env("RVOW_GIT", bin);
            }
            None => {
                c.env_remove("RVOW_GIT");
            }
        }
        match eval_git {
            Some(bin) => {
                c.env("EVAL_RVOW_GIT", bin);
            }
            None => {
                c.env_remove("EVAL_RVOW_GIT");
            }
        }
        c.output().unwrap()
    }
}

#[test]
fn git_bin_wrapper_is_used() {
    let fx = Fx::new("wrap");
    let dir = fx.repo();
    let (script, log) = fx.wrapper();
    let out = fx.brief(&dir, Some(&script));
    assert_eq!(out.status.code(), Some(0), "rvow brief: {}", String::from_utf8_lossy(&out.stdout));
    let logged = std::fs::read_to_string(&log).unwrap();
    assert!(logged.contains("rev-parse"), "the wrapper log must hold the rev-parse call: {logged}");
}

#[test]
fn git_bin_eval_fallback_is_used() {
    let fx = Fx::new("eval");
    let dir = fx.repo();
    let (script, log) = fx.wrapper();
    let out = fx.brief_with(&dir, None, Some(&script));
    assert_eq!(out.status.code(), Some(0), "rvow brief: {}", String::from_utf8_lossy(&out.stdout));
    let logged = std::fs::read_to_string(&log).unwrap();
    assert!(logged.contains("rev-parse"), "the wrapper log must hold the rev-parse call: {logged}");
}

#[test]
fn git_bin_default_is_path_git() {
    let fx = Fx::new("default");
    let dir = fx.repo();
    let (_script, log) = fx.wrapper();
    std::fs::write(&log, "").unwrap();
    let out = fx.brief(&dir, None);
    assert_eq!(out.status.code(), Some(0), "rvow brief: {}", String::from_utf8_lossy(&out.stdout));
    let logged = std::fs::read_to_string(&log).unwrap();
    assert!(logged.is_empty(), "without RVOW_GIT the wrapper must never run: {logged}");
}
