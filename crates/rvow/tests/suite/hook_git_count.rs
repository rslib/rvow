//! One Edit call costs at most 7 git processes: the hooks of a call share the
//! answers of one `rev-parse` and hash only the file the call names.
use std::io::Write;
use std::path::PathBuf;
use std::process::{Command, Stdio};

use serde_json::{Value, json};

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
    /// A repository with an rvow store and a committed `a.txt`.
    fn new(name: &str) -> Fx {
        let root = std::env::temp_dir().join(format!("rvow-hook-git-count-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("repo")).unwrap();
        std::fs::create_dir_all(root.join("home")).unwrap();
        let fx = Fx { root: root.canonicalize().unwrap() };
        std::fs::write(fx.repo().join("a.txt"), "a").unwrap();
        for args in [&["init", "-q", "-b", "main"][..], &["add", "a.txt"], &["commit", "-q", "-m", "init"]] {
            assert!(fx.cmd("git").args(args).status().unwrap().success(), "git {args:?}");
        }
        assert!(fx.cmd(RVOW).args(["init", "--in-repo"]).output().unwrap().status.success());
        fx
    }

    fn repo(&self) -> PathBuf {
        self.root.join("repo")
    }

    fn count_file(&self) -> PathBuf {
        self.root.join("git-count.txt")
    }

    /// A git that counts itself: the hook runs it through `RVOW_GIT`, so every
    /// git process of the two hooks adds one line before the real git runs.
    fn counting_git(&self) -> PathBuf {
        let out = Command::new("sh").args(["-c", "command -v git"]).output().unwrap();
        let real = String::from_utf8_lossy(&out.stdout).trim().to_string();
        assert!(!real.is_empty(), "the test needs a git on PATH");
        let path = self.root.join("counting-git.sh");
        let script = format!("#!/bin/sh\nprintf 'git\\n' >> '{}'\nexec '{}' \"$@\"\n", self.count_file().display(), real);
        std::fs::write(&path, script).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        path
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

    /// Runs `rvow hook claude <event>` on `payload` through the counting git.
    fn hook(&self, event: &str, payload: Value) {
        let mut child = self
            .cmd(RVOW)
            .arg("hook")
            .args(["claude", event])
            .env("RVOW_GIT", self.counting_git())
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .spawn()
            .unwrap();
        child.stdin.take().unwrap().write_all(payload.to_string().as_bytes()).unwrap();
        let out = child.wait_with_output().unwrap();
        assert!(out.status.success(), "{event} exited {}: {}", out.status, String::from_utf8_lossy(&out.stderr));
    }

    /// The number of git processes the two hooks of one Edit of `a.txt` started.
    fn edit_git_count(&self) -> usize {
        std::fs::write(self.count_file(), "").unwrap();
        let call = json!({"cwd": self.repo(), "session_id": "s1", "tool_use_id": "e1"});
        let mut pre = call.clone();
        pre["tool_name"] = json!("Edit");
        pre["tool_input"] = json!({"file_path": self.repo().join("a.txt"), "old_string": "a", "new_string": "bb"});
        self.hook("pre-tool-use", pre);
        std::fs::write(self.repo().join("a.txt"), "bb").unwrap();
        self.hook("post-tool-use", call);
        std::fs::read_to_string(self.count_file()).unwrap_or_default().lines().count()
    }
}

#[test]
fn hook_git_count_one_edit_call_starts_at_most_seven_git_processes() {
    let fx = Fx::new("edit");
    let count = fx.edit_git_count();
    assert!(count > 0, "the counting git never ran");
    assert!(count <= 7, "one Edit call started {count} git processes, at most 7 allowed");
}

impl Fx {
    /// A repository with a home-mode rvow store, where the project lookup reads
    /// the home index, and a committed `a.txt`.
    fn new_home(name: &str) -> Fx {
        let root = std::env::temp_dir().join(format!("rvow-hook-git-count-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("repo")).unwrap();
        std::fs::create_dir_all(root.join("home")).unwrap();
        let fx = Fx { root: root.canonicalize().unwrap() };
        std::fs::write(fx.repo().join("a.txt"), "a").unwrap();
        for args in [&["init", "-q", "-b", "main"][..], &["add", "a.txt"], &["commit", "-q", "-m", "init"], &["gc", "-q"]] {
            assert!(fx.cmd("git").args(args).status().unwrap().success(), "git {args:?}");
        }
        assert!(fx.cmd(RVOW).arg("init").output().unwrap().status.success());
        fx
    }
}

#[test]
fn hook_git_count_a_home_store_finds_its_project_with_one_git_process() {
    let fx = Fx::new_home("home");
    // The first call walks the home and writes the index; the next ones read it.
    fx.edit_git_count();
    let count = fx.edit_git_count();
    assert!(count > 0, "the counting git never ran");
    assert!(count <= 7, "one Edit call on a home store started {count} git processes, at most 7 allowed");
}
