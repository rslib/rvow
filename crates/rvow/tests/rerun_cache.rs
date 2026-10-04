//! The rerun cache: signed entries, keys on HEAD content, and the hook rules
//! that keep agents away from the file and its key.

use std::collections::BTreeMap;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use rvow_core::config;
use rvow_core::error::Result;
use rvow_core::fingerprint;
use rvow_core::gate::cache::{CACHE_FILE, Cache, cache_key};
use rvow_core::ledger::short_hash;
use rvow_core::runner::Outcome;
use rvow_core::store;
use rvow_core::tasks::{Task, parse_tasks};
use serde_json::{Value, json};

const RVOW: &str = env!("CARGO_BIN_EXE_rvow");
const HOUR: u64 = 3_600_000;

struct Fixture {
    dir: PathBuf,
    repo: PathBuf,
    home: PathBuf,
    project: store::Project,
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
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

impl Fixture {
    fn new(name: &str, config: &str) -> Self {
        let dir = std::env::temp_dir().join(format!("rvow-rerun-cache-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let repo = dir.join("repo");
        let home = dir.join("home");
        std::fs::create_dir_all(repo.join("src")).unwrap();
        std::fs::create_dir_all(repo.join("docs")).unwrap();
        let placeholder =
            store::Project { root: home.join("none"), meta: store::Meta { roots: vec![], remote: None, mode: store::Mode::Home } };
        let mut f = Self { dir: dir.clone(), repo: repo.clone(), home: home.clone(), project: placeholder };
        f.git(&["init", "-q", "-b", "main"]);
        f.write("src/a.txt", "alpha");
        f.write("src/b.txt", "beta");
        f.write("docs/out.txt", "outside");
        f.git(&["add", "."]);
        f.git(&["commit", "-q", "-m", "initial"]);
        f.project = store::init(&repo, &home, store::Mode::Home).unwrap();
        std::fs::write(f.project.root.join("config.toml"), config).unwrap();
        f
    }

    fn git(&self, args: &[&str]) {
        let output = git_command(&self.repo).args(args).output().unwrap();
        assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
    }

    fn write(&self, rel: &str, text: &str) {
        std::fs::write(self.repo.join(rel), text).unwrap();
    }

    fn commit_all(&self, message: &str) {
        self.git(&["add", "-A"]);
        self.git(&["commit", "-q", "-m", message]);
    }

    fn set_config(&self, text: &str) {
        std::fs::write(self.project.root.join("config.toml"), text).unwrap();
    }

    fn task(&self, files: &str) -> Task {
        let src = format!("- TSK-abc123 Work\n  covers: REQ-abc123\n  verify: true\n  files: {files}\n");
        parse_tasks(&src).0.remove(0)
    }

    fn key(&self, task: &Task) -> Result<Option<String>> {
        let config = config::load(&self.project, &self.home).unwrap();
        let env = fingerprint::env(&self.repo, &config).unwrap();
        let reqs = BTreeMap::from([("REQ-abc123".to_string(), "h".to_string())]);
        cache_key(&self.project, &self.home, &self.repo, &config, task, &reqs, &env)
    }

    fn cache_path(&self) -> PathBuf {
        self.project.root.join(CACHE_FILE)
    }

    fn put(&self, unit: &str, key: &str) {
        let lock = self.project.lock().unwrap();
        let mut cache = Cache::open(&self.project, &self.home).unwrap();
        cache.record(&lock, unit, key, Some(3), 40, "m").unwrap();
    }

    fn lines(&self) -> Vec<String> {
        std::fs::read_to_string(self.cache_path()).unwrap().lines().map(String::from).collect()
    }
}

fn passed() -> Outcome {
    Outcome { exit: Some(0), timed_out: false, tests_run: Some(2), duration_ms: 5, tail: vec![] }
}

// Entries: write, read, age, forgery.

#[test]
fn cache_record_round_trip() {
    let f = Fixture::new("round", "");
    f.put("TSK-abc123", "k1");
    let cache = Cache::open(&f.project, &f.home).unwrap();
    let hit = cache.lookup("TSK-abc123", "k1", u64::MAX / 2, u64::MAX / 2).unwrap();
    assert_eq!((hit.tests_run, hit.duration_ms), (Some(3), 40));
    assert!(cache.lookup("TSK-abc123", "other", u64::MAX / 2, u64::MAX / 2).is_none());
    assert!(cache.lookup("TSK-zzzzzz", "k1", u64::MAX / 2, u64::MAX / 2).is_none());
    assert!(cache.warnings().is_empty(), "{:?}", cache.warnings());
    let key_file = f.home.join("hidden-checks/.cache-key");
    assert_eq!(std::fs::metadata(&key_file).unwrap().len(), 32);
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(std::fs::metadata(&key_file).unwrap().permissions().mode() & 0o777, 0o600);
    }
}

#[test]
fn cache_record_age_filter() {
    let f = Fixture::new("age", "");
    f.put("TSK-abc123", "k1");
    let cache = Cache::open(&f.project, &f.home).unwrap();
    let time = cache.lookup("TSK-abc123", "k1", u64::MAX / 2, u64::MAX / 2).unwrap().time;
    assert!(cache.lookup("TSK-abc123", "k1", time + 24 * HOUR, 24 * HOUR).is_some());
    assert!(cache.lookup("TSK-abc123", "k1", time + 25 * HOUR, 24 * HOUR).is_none());
}

#[test]
fn cache_record_edited_line_is_ignored_with_a_warning() {
    let f = Fixture::new("edited", "");
    f.put("TSK-abc123", "k1");
    f.put("TSK-def456", "k2");
    let text = std::fs::read_to_string(f.cache_path()).unwrap().replacen("\"duration_ms\":40", "\"duration_ms\":1", 1);
    std::fs::write(f.cache_path(), text).unwrap();
    let cache = Cache::open(&f.project, &f.home).unwrap();
    assert!(cache.lookup("TSK-abc123", "k1", u64::MAX / 2, u64::MAX / 2).is_none());
    assert!(cache.lookup("TSK-def456", "k2", u64::MAX / 2, u64::MAX / 2).is_none(), "an entry after a broken link is untrusted");
    assert!(!cache.warnings().is_empty());
}

#[test]
fn cache_record_deleted_middle_line_breaks_the_chain() {
    let f = Fixture::new("deleted", "");
    f.put("TSK-aaa111", "k1");
    f.put("TSK-bbb222", "k2");
    f.put("TSK-ccc333", "k3");
    let lines = f.lines();
    std::fs::write(f.cache_path(), format!("{}\n{}\n", lines[0], lines[2])).unwrap();
    let cache = Cache::open(&f.project, &f.home).unwrap();
    assert!(cache.lookup("TSK-aaa111", "k1", u64::MAX / 2, u64::MAX / 2).is_some());
    assert!(cache.lookup("TSK-ccc333", "k3", u64::MAX / 2, u64::MAX / 2).is_none());
    assert!(!cache.warnings().is_empty());
}

#[test]
fn cache_record_forged_line_without_the_key_is_ignored() {
    let f = Fixture::new("forged", "");
    f.put("TSK-aaa111", "k1");
    let last = f.lines().pop().unwrap();
    let forged = json!({
        "unit": "TSK-evil00", "key": "kx", "tests_run": 9, "duration_ms": 1, "rvow": "0", "machine": "m",
        "time": 1u64, "prev": short_hash(last.as_bytes()), "mac": "00".repeat(32),
    });
    let mut text = std::fs::read_to_string(f.cache_path()).unwrap();
    text.push_str(&format!("{forged}\n"));
    std::fs::write(f.cache_path(), text).unwrap();
    let cache = Cache::open(&f.project, &f.home).unwrap();
    assert!(cache.lookup("TSK-evil00", "kx", u64::MAX / 2, u64::MAX / 2).is_none());
    assert!(cache.lookup("TSK-aaa111", "k1", u64::MAX / 2, u64::MAX / 2).is_some());
    assert!(!cache.warnings().is_empty());
}

#[test]
fn cache_record_writes_after_a_broken_chain_are_trusted_again() {
    let f = Fixture::new("heal", "");
    f.put("TSK-aaa111", "k1");
    std::fs::write(f.cache_path(), "garbage\n").unwrap();
    f.put("TSK-bbb222", "k2");
    let cache = Cache::open(&f.project, &f.home).unwrap();
    assert!(cache.lookup("TSK-bbb222", "k2", u64::MAX / 2, u64::MAX / 2).is_some());
    assert!(cache.warnings().is_empty(), "{:?}", cache.warnings());
}

#[test]
fn cache_record_never_writes_a_failure() {
    let f = Fixture::new("fail", "");
    let lock = f.project.lock().unwrap();
    let mut cache = Cache::open(&f.project, &f.home).unwrap();
    let failing = Outcome { exit: Some(1), ..passed() };
    let timeout = Outcome { timed_out: true, ..passed() };
    let none = Outcome { tests_run: Some(0), ..passed() };
    for bad in [failing, timeout, none] {
        assert!(!cache.record_pass(&lock, "TSK-abc123", "k1", &bad, "m").unwrap());
    }
    assert!(!f.cache_path().exists() || f.lines().is_empty());
    assert!(cache.record_pass(&lock, "TSK-abc123", "k1", &passed(), "m").unwrap());
    assert_eq!(f.lines().len(), 1);
}

// Keys.

#[test]
fn cache_record_key_changes_with_a_byte_of_scope_content() {
    let f = Fixture::new("byte", "");
    let task = f.task("src/**");
    let before = f.key(&task).unwrap().unwrap();
    assert_eq!(before, f.key(&task).unwrap().unwrap());
    f.write("src/a.txt", "alphb");
    f.commit_all("edit");
    assert_ne!(before, f.key(&task).unwrap().unwrap());
}

#[test]
fn cache_record_key_changes_when_a_scope_file_is_added_deleted_or_renamed() {
    let f = Fixture::new("shape", "");
    let task = f.task("src/**");
    let base = f.key(&task).unwrap().unwrap();
    f.write("src/c.txt", "gamma");
    f.commit_all("add");
    let added = f.key(&task).unwrap().unwrap();
    assert_ne!(base, added);
    f.git(&["rm", "-q", "src/c.txt"]);
    f.commit_all("delete");
    assert_eq!(base, f.key(&task).unwrap().unwrap(), "back to the same tree");
    f.git(&["mv", "src/b.txt", "src/renamed.txt"]);
    f.commit_all("rename");
    assert_ne!(base, f.key(&task).unwrap().unwrap());
}

#[test]
fn cache_record_key_changes_with_the_mode() {
    let f = Fixture::new("mode", "");
    let task = f.task("src/**");
    let base = f.key(&task).unwrap().unwrap();
    f.git(&["update-index", "--chmod=+x", "src/a.txt"]);
    f.git(&["commit", "-q", "-m", "mode"]);
    assert_ne!(base, f.key(&task).unwrap().unwrap());
}

#[test]
fn cache_record_key_ignores_history_branch_names_and_out_of_scope_files() {
    let f = Fixture::new("stable", "");
    let task = f.task("src/**");
    let base = f.key(&task).unwrap().unwrap();
    f.git(&["commit", "-q", "--amend", "-m", "reworded"]);
    f.git(&["branch", "-m", "feature"]);
    assert_eq!(base, f.key(&task).unwrap().unwrap(), "amend and branch name");
    f.git(&["commit", "-q", "--allow-empty", "-m", "empty"]);
    f.git(&["rebase", "-q", "--root", "--force-rebase"]);
    assert_eq!(base, f.key(&task).unwrap().unwrap(), "rebase");
    f.write("docs/out.txt", "changed");
    f.commit_all("outside");
    assert_eq!(base, f.key(&task).unwrap().unwrap(), "out of scope");
}

#[test]
fn cache_record_key_follows_the_env_output_and_fails_closed() {
    let f = Fixture::new("env", "env = [\"cat envfile.txt\"]\n");
    let task = f.task("src/**");
    f.write("envfile.txt", "one\n");
    let one = f.key(&task).unwrap().unwrap();
    f.write("envfile.txt", "two\n");
    assert_ne!(one, f.key(&task).unwrap().unwrap());
    f.set_config("env = [\"false\"]\n");
    assert_eq!(f.key(&task).unwrap(), None);
}

#[test]
fn cache_record_key_follows_the_tool_binaries() {
    let f = Fixture::new("tools", "");
    let task = f.task("src/**");
    let tools = f.dir.join("tools");
    std::fs::create_dir_all(&tools).unwrap();
    let (one, two, link) = (tools.join("cc-one"), tools.join("cc-two"), tools.join("cc"));
    std::fs::write(&one, "build one").unwrap();
    std::fs::write(&two, "build two").unwrap();
    #[cfg(unix)]
    for t in [&one, &two] {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(t, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
    f.set_config(&format!("tools = [\"{}\"]\n", one.display()));
    let first = f.key(&task).unwrap().unwrap();
    std::fs::write(&one, "build three").unwrap();
    assert_ne!(first, f.key(&task).unwrap().unwrap(), "replaced executable");

    #[cfg(unix)]
    {
        f.set_config(&format!("tools = [\"{}\"]\n", link.display()));
        std::os::unix::fs::symlink(&one, &link).unwrap();
        let through = f.key(&task).unwrap().unwrap();
        std::fs::remove_file(&link).unwrap();
        std::os::unix::fs::symlink(&two, &link).unwrap();
        assert_ne!(through, f.key(&task).unwrap().unwrap(), "symlink target");
    }

    f.set_config("tools = [\"rvow-no-such-tool-xyz\"]\n");
    assert_eq!(f.key(&task).unwrap(), None);
}

#[test]
fn cache_record_key_refuses_a_files_glob_that_matches_nothing() {
    let f = Fixture::new("empty", "");
    let err = f.key(&f.task("src/**, nothing/**")).unwrap_err();
    assert!(err.message.contains("TSK-abc123") || err.fix.contains("TSK-abc123"), "{err:?}");
}

// Protection from agents.

struct Fx {
    root: PathBuf,
    project: store::Project,
}

impl Drop for Fx {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

impl Fx {
    fn new(name: &str) -> Fx {
        let root = std::env::temp_dir().join(format!("rvow-cache-protect-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("repo")).unwrap();
        let root = root.canonicalize().unwrap();
        let run = |args: &[&str]| assert!(git_command(&root.join("repo")).args(args).status().unwrap().success());
        run(&["init", "-q", "-b", "main"]);
        std::fs::write(root.join("repo/a.txt"), "a").unwrap();
        run(&["add", "a.txt"]);
        run(&["commit", "-q", "-m", "init"]);
        let project = store::init(&root.join("repo"), &root.join("rh"), store::Mode::Home).unwrap();
        Fx { root, project }
    }

    fn hook(&self, tool: &str, input: Value) -> Option<String> {
        let payload = json!({"cwd": self.root.join("repo"), "tool_name": tool, "tool_input": input});
        let mut c = Command::new(RVOW);
        c.args(["hook", "pre-tool-use"])
            .current_dir(self.root.join("repo"))
            .env("HOME", self.root.join("home"))
            .env("RVOW_HOME", self.root.join("rh"))
            .env("PATH", "/usr/bin:/bin:/usr/sbin:/sbin")
            .env_remove("XDG_CONFIG_HOME")
            .env_remove("XDG_DATA_HOME")
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        let mut child = c.spawn().unwrap();
        child.stdin.take().unwrap().write_all(payload.to_string().as_bytes()).unwrap();
        let out = child.wait_with_output().unwrap();
        assert_eq!(String::from_utf8_lossy(&out.stderr), "", "{tool} {input}");
        let text = String::from_utf8(out.stdout).unwrap();
        if text.trim().is_empty() {
            return None;
        }
        let v: Value = serde_json::from_str(&text).unwrap();
        assert_eq!(v["hookSpecificOutput"]["permissionDecision"], "deny", "{text}");
        Some(v["hookSpecificOutput"]["permissionDecisionReason"].as_str().unwrap().to_string())
    }

    fn bash(&self, command: &str) -> Option<String> {
        self.hook("Bash", json!({"command": command}))
    }
}

fn has_next_step(reason: &str) -> bool {
    ["rvow ", "git ", "run ", "ask the user", "only the user", "instead", "Use "].iter().any(|n| reason.contains(n))
}

#[test]
fn cache_protect_edit_and_write_of_the_cache_file_are_denied() {
    let fx = Fx::new("edit");
    let cache = fx.project.root.join(CACHE_FILE);
    for tool in ["Edit", "Write"] {
        let reason = fx.hook(tool, json!({"file_path": cache})).unwrap_or_else(|| panic!("{tool} allowed"));
        assert!(reason.contains(CACHE_FILE) && has_next_step(&reason), "{reason}");
    }
}

#[test]
fn cache_protect_shell_writes_to_the_cache_file_are_denied() {
    let fx = Fx::new("shell");
    let cache = fx.project.root.join(CACHE_FILE).display().to_string();
    for command in [
        format!("echo x >> {cache}"),
        format!("sed -i s/a/b/ {cache}"),
        format!("echo x | tee {cache}"),
        format!("mv /tmp/x {cache}"),
        format!("rm {cache}"),
        format!("cp /tmp/x {cache}"),
    ] {
        let reason = fx.bash(&command).unwrap_or_else(|| panic!("allowed: {command}"));
        assert!(reason.contains(CACHE_FILE) && has_next_step(&reason), "{reason}");
    }
}

#[test]
fn cache_protect_the_key_file_is_denied_for_every_tool() {
    let fx = Fx::new("key");
    let key = fx.root.join("rh/hidden-checks/.cache-key");
    for (tool, field) in [("Read", "file_path"), ("Write", "file_path"), ("Edit", "file_path"), ("Grep", "path"), ("Glob", "path")] {
        let reason = fx.hook(tool, json!({field: key})).unwrap_or_else(|| panic!("{tool} allowed"));
        assert!(reason.contains("hidden") && has_next_step(&reason), "{reason}");
    }
    for command in
        [format!("cat {}", key.display()), format!("echo x > {}", key.display()), "cat \"$RVOW_HOME\"/hidden-checks/.cache-key".to_string()]
    {
        let reason = fx.bash(&command).unwrap_or_else(|| panic!("allowed: {command}"));
        assert!(reason.contains("hidden") && has_next_step(&reason), "{reason}");
    }
}

#[test]
fn cache_protect_an_unrelated_file_next_to_the_cache_is_allowed() {
    let fx = Fx::new("other");
    let other = fx.project.root.join("notes.txt");
    assert_eq!(fx.hook("Write", json!({"file_path": other})), None);
    assert_eq!(fx.bash(&format!("echo x > {}", other.display())), None);
}
