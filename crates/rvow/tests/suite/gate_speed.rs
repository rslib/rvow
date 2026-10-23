//! The gate runs each configured check once per commit: a passed check is
//! cached by command, HEAD tree and tool fingerprint, and a failed grammar
//! fetch is not tried again for an hour. One test per spec scenario.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

const RVOW: &str = env!("CARGO_BIN_EXE_rvow");

struct Fx {
    root: PathBuf,
    repo: PathBuf,
    home: PathBuf,
    count: PathBuf,
    task: String,
    change: String,
}

impl Drop for Fx {
    fn drop(&mut self) {
        rvow_core::git::testrepo::remove_tree(&self.root);
    }
}

fn git_command(repo: &Path) -> Command {
    let mut command = Command::new("git");
    command
        .current_dir(repo)
        .env_remove("GIT_DIR")
        .env_remove("GIT_WORK_TREE")
        .env_remove("GIT_INDEX_FILE")
        .env_remove("GIT_COMMON_DIR")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_AUTHOR_NAME", "test")
        .env("GIT_AUTHOR_EMAIL", "test@example.invalid")
        .env("GIT_COMMITTER_NAME", "test")
        .env("GIT_COMMITTER_EMAIL", "test@example.invalid");
    command
}

impl Fx {
    /// A repository with one accepted, committed and proved task, and a global
    /// config whose `checks` list is given by `checks`.
    fn new(name: &str, checks: &[&str]) -> Fx {
        let root = std::env::temp_dir().join(format!("rvow-gate-speed-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("repo/src")).unwrap();
        std::fs::create_dir_all(root.join("home")).unwrap();
        let root = root.canonicalize().unwrap();
        let repo = root.join("repo");
        let home = root.join("home");
        let mut fx = Fx {
            root: root.clone(),
            repo: repo.clone(),
            home: home.clone(),
            count: root.join("count"),
            task: String::new(),
            change: String::new(),
        };
        fx.git(&["init", "-q", "-b", "main"]);
        std::fs::write(repo.join("src/a.txt"), "a").unwrap();
        fx.git(&["add", "."]);
        fx.git(&["commit", "-q", "-m", "init"]);
        fx.rvow(&["init"]);
        let out = fx.rvow(&["new", "--spec", "Out"]);
        let dir = PathBuf::from(out.lines().find_map(|l| l.strip_prefix("path: ")).unwrap());
        std::fs::write(
            dir.join("tasks.md"),
            "- TSK-new1 Write ok\n  covers: REQ-new1\n  verify: grep -q ok out.txt && echo 'test result: ok. 1 passed; 0 failed;'\n  files: out.txt, src/**\n",
        )
        .unwrap();
        std::fs::write(
            dir.join("specs/out.md"),
            "## ADD REQ-new1 Out\nrvow SHALL write ok.\n### Scenario: s\n- WHEN it runs\n- THEN out.txt holds ok\n",
        )
        .unwrap();
        let id = out.lines().find_map(|l| l.strip_prefix("created: ")).unwrap().to_string();
        fx.change = id.clone();
        fx.rvow(&["accept", &id, "--yes"]);
        let active = fx.rvow(&["path", &id]);
        let tasks =
            std::fs::read_to_string(PathBuf::from(active.lines().find_map(|l| l.strip_prefix("path: ")).unwrap().trim()).join("tasks.md"))
                .unwrap();
        fx.task = tasks.lines().find_map(|l| l.strip_prefix("- ")?.split_whitespace().next().map(String::from)).unwrap();
        fx.commit("out.txt", "ok");
        fx.set_config(&checks.iter().map(|c| format!("checks = [{c:?}]\n")).collect::<String>());
        fx
    }

    fn cmd(&self) -> Command {
        let mut c = Command::new(RVOW);
        c.current_dir(&self.repo)
            .env("HOME", &self.home)
            .env("RVOW_HOME", &self.home)
            .env("PATH", "/usr/bin:/bin:/usr/sbin:/sbin")
            .env_remove("GIT_DIR")
            .env_remove("GIT_WORK_TREE")
            .env_remove("GIT_INDEX_FILE")
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_AUTHOR_NAME", "t")
            .env("GIT_AUTHOR_EMAIL", "t@t")
            .env("GIT_COMMITTER_NAME", "t")
            .env("GIT_COMMITTER_EMAIL", "t@t")
            .env_remove("RVOW_GRAMMAR_URL")
            .env_remove("RVOW_GRAMMAR_OFFLINE");
        c
    }

    fn run(&self, args: &[&str]) -> (String, String, bool) {
        let out = self.cmd().args(args).output().unwrap();
        (String::from_utf8_lossy(&out.stdout).into_owned(), String::from_utf8_lossy(&out.stderr).into_owned(), out.status.success())
    }

    fn rvow(&self, args: &[&str]) -> String {
        let (stdout, stderr, ok) = self.run(args);
        assert!(ok, "rvow {args:?} failed\nstdout: {stdout}\nstderr: {stderr}");
        stdout
    }

    fn git(&self, args: &[&str]) {
        let output = git_command(&self.repo).args(args).output().unwrap();
        assert!(output.status.success(), "git {args:?}: {}", String::from_utf8_lossy(&output.stderr));
    }

    fn commit(&self, file: &str, text: &str) {
        std::fs::write(self.repo.join(file), text).unwrap();
        self.git(&["add", file]);
        self.git(&["commit", "-q", "-m", "change"]);
    }

    fn set_config(&self, text: &str) {
        std::fs::write(self.home.join("config.toml"), text).unwrap();
    }

    fn proved(&self) {
        self.rvow(&["prove", &self.task]);
    }

    /// `rvow gate` and its echo on stderr.
    fn gate(&self) -> String {
        let (_, stderr, _) = self.run(&["gate"]);
        stderr
    }

    fn runs(&self) -> usize {
        std::fs::read_to_string(&self.count).map_or(0, |t| t.lines().count())
    }

    /// The number of check entries in the shared cache file.
    fn entries(&self) -> usize {
        fn walk(dir: &Path, out: &mut Vec<PathBuf>) {
            let Ok(read) = std::fs::read_dir(dir) else { return };
            for e in read.flatten() {
                let p = e.path();
                if p.is_dir() {
                    walk(&p, out);
                } else if p.file_name().is_some_and(|n| n == rvow_core::gate::cache::CACHE_FILE) {
                    out.push(p);
                }
            }
        }
        let mut files = Vec::new();
        walk(&self.home, &mut files);
        files.iter().map(|p| std::fs::read_to_string(p).unwrap().lines().count()).sum()
    }
}

fn counter(fx: &Fx) -> String {
    format!("echo run >> {}", fx.count.display())
}

#[test]
fn gate_speed_check_archive_after_a_gate_runs_the_check_once() {
    let fx = Fx::new("archive", &[]);
    let cmd = counter(&fx);
    fx.set_config(&format!("checks = [{cmd:?}]\n"));
    fx.proved();
    assert!(fx.gate().contains("check: echo run"), "the first gate runs the check");
    assert_eq!(fx.runs(), 1);
    let (stdout, stderr, _) = fx.run(&["archive", &fx.change]);
    assert!(stderr.contains("cached check: echo run"), "archive skips the cached check\nstdout: {stdout}\nstderr: {stderr}");
    assert_eq!(fx.runs(), 1, "the check ran again during archive");
}

#[test]
fn gate_speed_check_a_new_commit_runs_the_check_again() {
    let fx = Fx::new("commit", &[]);
    let cmd = counter(&fx);
    fx.set_config(&format!("checks = [{cmd:?}]\n"));
    fx.proved();
    assert!(fx.gate().contains("check: echo run"));
    assert_eq!(fx.runs(), 1);
    fx.commit("src/a.txt", "b");
    fx.proved();
    let second = fx.gate();
    assert!(second.contains("check: echo run"), "a new commit runs the check again: {second}");
    assert!(!second.contains("cached check:"), "{second}");
    assert_eq!(fx.runs(), 2);
}

#[test]
fn gate_check_dirty_uncommitted_change_runs_once_then_is_cached_by_content() {
    let fx = Fx::new("dirty", &[]);
    let cmd = counter(&fx);
    fx.set_config(&format!("checks = [{cmd:?}]\n"));
    fx.proved();
    fx.gate();
    assert_eq!(fx.runs(), 1);
    let entries = fx.entries();
    std::fs::write(fx.repo.join("src/a.txt"), "uncommitted").unwrap();
    assert!(fx.gate().contains("check: echo run"), "a dirty tree runs the check");
    assert_eq!(fx.runs(), 2);
    assert_ne!(fx.entries(), entries, "a dirty tree records its pass under its own content");
    assert!(fx.gate().contains("cached check: echo run"), "a retry on the same files skips the check");
    assert_eq!(fx.runs(), 2);
    std::fs::write(fx.repo.join("src/a.txt"), "another edit").unwrap();
    assert!(fx.gate().contains("check: echo run"), "another content runs it again");
    assert_eq!(fx.runs(), 3);
}

#[test]
fn gate_check_dirty_untracked_file_runs_once_then_is_cached_by_content() {
    let fx = Fx::new("untracked", &[]);
    let cmd = counter(&fx);
    fx.set_config(&format!("checks = [{cmd:?}]\n"));
    fx.proved();
    fx.gate();
    assert_eq!(fx.runs(), 1);
    let entries = fx.entries();
    std::fs::write(fx.repo.join("src/new.txt"), "not added yet").unwrap();
    assert!(fx.gate().contains("check: echo run"), "an untracked file runs the check");
    assert_eq!(fx.runs(), 2);
    assert_ne!(fx.entries(), entries, "an untracked file records its pass under its own content");
    assert!(fx.gate().contains("cached check: echo run"), "a retry on the same files skips the check");
    assert_eq!(fx.runs(), 2);
}

#[test]
fn gate_speed_check_the_cache_policy_off_runs_the_check_always() {
    let fx = Fx::new("off", &[]);
    let cmd = counter(&fx);
    fx.set_config(&format!("checks = [{cmd:?}]\nrerun_cache = \"off\"\n"));
    fx.proved();
    assert!(fx.gate().contains("check: echo run"));
    let second = fx.gate();
    assert!(second.contains("check: echo run"), "{second}");
    assert!(!second.contains("cached check:"), "{second}");
    assert_eq!(fx.runs(), 2);
}

// Grammar fetch retries.

/// A local server that counts its hits and answers 404, so a fetch fails at
/// once and the test can prove a second run made no request.
struct FailServer {
    port: u16,
    hits: Arc<AtomicUsize>,
}

impl FailServer {
    fn start() -> FailServer {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let hits = Arc::new(AtomicUsize::new(0));
        let counter = hits.clone();
        std::thread::spawn(move || {
            for mut stream in listener.incoming().filter_map(Result::ok) {
                counter.fetch_add(1, Ordering::SeqCst);
                let _ = stream.write_all(b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n");
            }
        });
        FailServer { port, hits }
    }

    fn url(&self) -> String {
        format!("http://127.0.0.1:{}", self.port)
    }

    fn hits(&self) -> usize {
        self.hits.load(Ordering::SeqCst)
    }
}

fn grammar_fixtures() -> String {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/grammars").display().to_string()
}

struct GrammarFx {
    root: PathBuf,
    repo: PathBuf,
    home: PathBuf,
    task: String,
}

impl Drop for GrammarFx {
    fn drop(&mut self) {
        rvow_core::git::testrepo::remove_tree(&self.root);
    }
}

impl GrammarFx {
    /// A repository whose vow baseline holds `.cargo/config.toml`, changed
    /// after accept so a prove fetches the toml grammar.
    fn new(name: &str) -> GrammarFx {
        let root = std::env::temp_dir().join(format!("rvow-gate-speed-grammar-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("repo/.cargo")).unwrap();
        std::fs::create_dir_all(root.join("repo/src")).unwrap();
        std::fs::create_dir_all(root.join("home")).unwrap();
        let root = root.canonicalize().unwrap();
        let repo = root.join("repo");
        let home = root.join("home");
        let mut fx = GrammarFx { root: root.clone(), repo: repo.clone(), home: home.clone(), task: String::new() };
        let run = |args: &[&str]| assert!(git_command(&repo).args(args).status().unwrap().success(), "git {args:?}");
        run(&["init", "-q", "-b", "main"]);
        std::fs::write(repo.join(".cargo/config.toml"), "# build notes\nkey_po = \"value\"\n\nflags = []\n").unwrap();
        std::fs::write(repo.join("src/a.txt"), "a").unwrap();
        run(&["add", "."]);
        run(&["commit", "-q", "-m", "init"]);
        let cmd = |args: &[&str]| fx.run(args);
        assert!(cmd(&["init"]).2);
        let out = cmd(&["new", "--spec", "Out"]).0;
        let dir = PathBuf::from(out.lines().find_map(|l| l.strip_prefix("path: ")).unwrap());
        std::fs::write(
            dir.join("tasks.md"),
            "- TSK-new1 Write ok\n  covers: REQ-new1\n  verify: grep -q ok out.txt && echo 'test result: ok. 1 passed; 0 failed;'\n  files: out.txt, src/**, .cargo/**\n",
        )
        .unwrap();
        std::fs::write(
            dir.join("specs/out.md"),
            "## ADD REQ-new1 Out\nrvow SHALL write ok.\n### Scenario: s\n- WHEN it runs\n- THEN out.txt holds ok\n",
        )
        .unwrap();
        let id = out.lines().find_map(|l| l.strip_prefix("created: ")).unwrap().to_string();
        let (accept_out, accept_err, accepted) = cmd(&["accept", &id, "--yes"]);
        assert!(accepted, "accept failed\nstdout: {accept_out}\nstderr: {accept_err}");
        let active = cmd(&["path", &id]).0;
        let tasks =
            std::fs::read_to_string(PathBuf::from(active.lines().find_map(|l| l.strip_prefix("path: ")).unwrap().trim()).join("tasks.md"))
                .unwrap();
        fx.task = tasks.lines().find_map(|l| l.strip_prefix("- ")?.split_whitespace().next().map(String::from)).unwrap();
        // The task's file lands after accept; the protected file changes too,
        // so the prove fetches the toml grammar.
        std::fs::write(repo.join("out.txt"), "ok").unwrap();
        run(&["add", "out.txt"]);
        run(&["commit", "-q", "-m", "out"]);
        std::fs::write(repo.join(".cargo/config.toml"), "# build notes\nkey_po = \"value\"\nflags = []\n").unwrap();
        fx
    }

    fn cmd(&self) -> Command {
        let mut c = Command::new(RVOW);
        c.current_dir(&self.repo)
            .env("HOME", &self.home)
            .env("RVOW_HOME", &self.home)
            .env("PATH", "/usr/bin:/bin:/usr/sbin:/sbin")
            .env_remove("RVOW_GRAMMAR_OFFLINE")
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

    fn run(&self, args: &[&str]) -> (String, String, bool) {
        let out = self.cmd().args(args).output().unwrap();
        (String::from_utf8_lossy(&out.stdout).into_owned(), String::from_utf8_lossy(&out.stderr).into_owned(), out.status.success())
    }

    fn with_url(&self, args: &[&str], url: &str) -> bool {
        self.cmd().args(args).env("RVOW_GRAMMAR_URL", url).output().unwrap().status.success()
    }

    fn cache(&self) -> PathBuf {
        self.home.join("grammars/tree-sitter-toml.wasm")
    }
}

#[test]
fn gate_speed_grammar_a_failed_fetch_is_not_tried_again_for_an_hour() {
    let fx = GrammarFx::new("retry");
    let server = FailServer::start();
    let _ = fx.with_url(&["prove", &fx.task], &server.url());
    assert!(server.hits() > 0, "the first prove fetches the missing grammar");
    assert!(!fx.cache().is_file(), "the failed download was not cached");
    let after_first = server.hits();
    let _ = fx.with_url(&["prove", &fx.task], &server.url());
    assert_eq!(server.hits(), after_first, "the second prove makes no fetch request");
}

#[test]
fn gate_speed_grammar_explicit_fetch_always_tries_and_a_success_clears_the_record() {
    let fx = GrammarFx::new("explicit");
    let server = FailServer::start();
    let _ = fx.with_url(&["prove", &fx.task], &server.url());
    assert!(server.hits() > 0, "prove records the failure");
    let after_prove = server.hits();
    assert!(!fx.with_url(&["grammar", "fetch", "toml"], &server.url()), "the failing fetch still fails");
    assert!(server.hits() > after_prove, "rvow grammar fetch tries again despite the record");

    assert!(fx.with_url(&["grammar", "fetch", "toml"], &grammar_fixtures()), "the fixture fetch succeeds");
    assert!(fx.cache().is_file());
    std::fs::remove_file(fx.cache()).unwrap();
    let after_success = server.hits();
    let _ = fx.with_url(&["prove", &fx.task], &server.url());
    assert!(server.hits() > after_success, "a successful fetch cleared the record");
}
