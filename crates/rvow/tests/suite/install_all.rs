//! `rvow install --all`: the gates that `rvow install` remembered, refreshed
//! from any folder, and the repositories it leaves alone.

use std::path::{Path, PathBuf};
use std::process::Command;

use rvow_core::store::GATE_REPOS_FILE;
use rvow_core::vow::sha256_hex;

const RECORD: &str = "rvow-pre-push-installed";
const MARK: &str = "# rvow pre-push gate";

struct Fixture(PathBuf);

impl Fixture {
    fn new(name: &str) -> Fixture {
        let dir = std::env::temp_dir().join(format!("rvow-install-all-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("home")).unwrap();
        Fixture(dir.canonicalize().unwrap())
    }

    fn home(&self) -> PathBuf {
        self.0.join("home")
    }

    fn cmd(&self, cwd: &Path, program: &str) -> Command {
        let mut c = Command::new(program);
        c.current_dir(cwd)
            .env("HOME", &self.0)
            .env("RVOW_HOME", self.home())
            .env("CLAUDE_CONFIG_DIR", self.0.join(".claude"))
            .env("XDG_CONFIG_HOME", self.0.join("config"))
            .env("XDG_DATA_HOME", self.0.join("data"))
            .env("PI_CODING_AGENT_DIR", self.0.join("pi"))
            .env_remove("RVOW_AGENT")
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_AUTHOR_NAME", "t")
            .env("GIT_AUTHOR_EMAIL", "t@t")
            .env("GIT_COMMITTER_NAME", "t")
            .env("GIT_COMMITTER_EMAIL", "t@t");
        c
    }

    fn git(&self, dir: &Path, args: &[&str]) {
        let out = self.cmd(dir, "git").args(args).output().unwrap();
        assert!(out.status.success(), "git {args:?}: {}", String::from_utf8_lossy(&out.stderr));
    }

    fn rvow(&self, cwd: &Path, args: &[&str]) -> (String, i32) {
        let out = self.cmd(cwd, env!("CARGO_BIN_EXE_rvow")).args(args).output().unwrap();
        assert_eq!(String::from_utf8_lossy(&out.stderr), "", "rvow {args:?}");
        (String::from_utf8_lossy(&out.stdout).into_owned(), out.status.code().unwrap())
    }

    fn ok(&self, cwd: &Path, args: &[&str]) -> String {
        let (out, code) = self.rvow(cwd, args);
        assert_eq!(code, 0, "rvow {args:?}: {out}");
        out
    }

    /// A repository with one commit, initialized as an in-repo rvow project.
    fn in_repo(&self, name: &str) -> PathBuf {
        let repo = self.repo(name);
        self.ok(&repo, &["init", "--in-repo"]);
        self.ok(&repo, &["install", "--claude", "settings"]);
        repo
    }

    fn repo(&self, name: &str) -> PathBuf {
        let dir = self.0.join(name);
        std::fs::create_dir_all(&dir).unwrap();
        self.git(&dir, &["init", "-q", "-b", "main"]);
        std::fs::write(dir.join("a.txt"), name).unwrap();
        self.git(&dir, &["add", "a.txt"]);
        self.git(&dir, &["commit", "-q", "-m", name]);
        dir
    }

    /// A folder that is not a repository.
    fn plain(&self) -> PathBuf {
        let dir = self.0.join("plain");
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn gates(&self) -> Vec<PathBuf> {
        serde_json::from_str(&std::fs::read_to_string(self.home().join(GATE_REPOS_FILE)).unwrap()).unwrap()
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        rvow_core::git::testrepo::remove_tree(&self.0);
    }
}

fn record(repo: &Path) -> PathBuf {
    repo.join(".git/hooks").join(RECORD)
}

fn binary() -> PathBuf {
    std::fs::canonicalize(env!("CARGO_BIN_EXE_rvow")).unwrap()
}

#[test]
fn install_all_remembers_an_in_repo_store_once() {
    let f = Fixture::new("once");
    let repo = f.in_repo("r");
    f.ok(&repo, &["install", "--claude", "settings"]);
    assert_eq!(f.gates(), [repo.join(".git")], "a second install records the repository once");
}

#[test]
fn install_all_refreshes_every_gate_from_any_folder() {
    let f = Fixture::new("refresh");
    let (a, b) = (f.in_repo("a"), f.in_repo("b"));
    for repo in [&a, &b] {
        let old = format!("{MARK}\nsha256:{}\n", "0".repeat(64));
        std::fs::write(record(repo), old).unwrap();
    }
    let (out, code) = f.rvow(&f.plain(), &["install", "--all", "--claude", "settings"]);
    assert_eq!(code, 0, "{out}");
    let want = format!("{MARK}\nsha256:{}\n", sha256_hex(&std::fs::read(binary()).unwrap()));
    for repo in [&a, &b] {
        assert_eq!(std::fs::read_to_string(record(repo)).unwrap(), want, "{}", repo.display());
    }
    assert!(out.contains("pre-push gates: 2 refreshed, 0 skipped"), "{out}");
}

#[test]
fn install_all_refreshes_none_without_remembered_repos() {
    let f = Fixture::new("fresh");
    let (out, code) = f.rvow(&f.plain(), &["install", "--all", "--claude", "settings"]);
    assert_eq!(code, 0, "{out}");
    assert!(out.contains("pre-push gates: 0 refreshed, 0 skipped"), "{out}");
}

#[test]
fn install_all_skips_a_gone_repo_and_a_hook_manager() {
    let f = Fixture::new("skip");
    let gone = f.in_repo("gone");
    let managed = f.repo("managed");
    std::fs::create_dir_all(managed.join(".husky")).unwrap();
    let hook = managed.join(".husky/pre-push");
    std::fs::write(&hook, "#!/bin/sh\necho husky\n").unwrap();
    f.git(&managed, &["config", "core.hooksPath", ".husky"]);
    f.ok(&managed, &["init", "--in-repo"]);
    let installed = f.ok(&managed, &["install", "--claude", "settings"]);
    assert!(installed.contains("another hook manager owns"), "{installed}");
    std::fs::remove_dir_all(&gone).unwrap();
    std::fs::write(record(&managed), format!("{MARK}\nsha256:{}\n", "0".repeat(64))).unwrap();

    let out = f.ok(&f.plain(), &["install", "--all", "--claude", "settings"]);
    let want = format!("{MARK}\nsha256:{}\n", sha256_hex(&std::fs::read(binary()).unwrap()));
    assert_eq!(std::fs::read_to_string(record(&managed)).unwrap(), want, "the hook manager's gate trusts the new binary");
    assert!(out.contains(&format!("{} is gone", gone.display())), "the gone repository is named: {out}");
    let line = rvow_core::install::pre_push_line(&binary());
    assert!(out.contains(&line), "the line for the hook manager is printed: {out}");
    assert!(out.contains(&managed.display().to_string()), "the hook manager's repository is named: {out}");
    assert_eq!(std::fs::read_to_string(&hook).unwrap(), "#!/bin/sh\necho husky\n", "the other hook manager's hook stays");
    assert!(out.contains("pre-push gates: 0 refreshed, 2 skipped"), "{out}");
}

#[test]
fn install_all_lists_a_known_repo_without_a_gate() {
    let f = Fixture::new("known");
    let repo = f.repo("known");
    f.ok(&repo, &["init"]);
    f.ok(&repo, &["status"]);
    assert!(!repo.join(".git/hooks/pre-push").exists(), "install never ran here");
    let out = f.ok(&f.plain(), &["install", "--all", "--claude", "settings"]);
    assert!(out.contains(&format!("cd {} && rvow install", repo.display())), "{out}");
    assert!(!repo.join(".git/hooks/pre-push").exists(), "a listed repository gets no gate");
}

#[test]
fn install_all_refreshes_a_known_old_gate() {
    let f = Fixture::new("known-old");
    let repo = f.repo("known");
    f.ok(&repo, &["init"]);
    f.ok(&repo, &["status"]);
    std::fs::create_dir_all(repo.join(".git/hooks")).unwrap();
    std::fs::write(record(&repo), format!("{MARK}\nsha256:{}\n", "0".repeat(64))).unwrap();
    assert!(!f.home().join(GATE_REPOS_FILE).exists(), "no install recorded the repository");

    let out = f.ok(&f.plain(), &["install", "--all", "--claude", "settings"]);
    let want = format!("{MARK}\nsha256:{}\n", sha256_hex(&std::fs::read(binary()).unwrap()));
    assert_eq!(std::fs::read_to_string(record(&repo)).unwrap(), want, "the old gate pins the running binary");
    assert!(out.contains("pre-push gates: 1 refreshed, 0 skipped"), "{out}");
    assert!(!out.contains(&format!("cd {} && rvow install", repo.display())), "a gated repository is not listed: {out}");
    assert_eq!(f.gates(), [repo.join(".git")], "the refreshed repository is recorded");

    let again = f.ok(&f.plain(), &["install", "--all", "--claude", "settings"]);
    assert!(again.contains("pre-push gates: 1 refreshed, 0 skipped"), "the second run refreshes it from the record: {again}");
}
