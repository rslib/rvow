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
        let dir = std::env::temp_dir().join(format!("rvow-friction-{}-{name}", std::process::id()));
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
}

fn hook_project(name: &str) -> (Repo, rvow_hook::store::Project) {
    let r = Repo::new(name);
    let home = r.0.join("home");
    let p = rvow_hook::store::init(&r.repo(), &home, rvow_hook::store::Mode::Home).unwrap();
    (r, p)
}

fn bash_denial(p: &rvow_hook::store::Project, cwd: &Path, cmd: &str) -> Option<rvow_hook::hook::Denial> {
    let input = serde_json::json!({
        "cwd": cwd.to_str().unwrap(),
        "tool_name": "Bash",
        "tool_input": {"command": cmd},
    });
    rvow_hook::hook::pre_tool_use_denial(p, &input).unwrap()
}

fn ledger_store(r: &Repo) -> PathBuf {
    let change = r.0.join("store/active/CHG-abcdef");
    std::fs::create_dir_all(&change).unwrap();
    std::fs::write(change.join("ledger.jsonl"), "{\"prove\":1}\n").unwrap();
    r.0.join("store")
}

#[test]
fn glob_read_grep_through_ledger_glob_is_allowed() {
    let _guard = crate::env_lock::lock();
    let (r, p) = hook_project("glob-read-allow");
    let store = ledger_store(&r);
    let cmd = format!("grep -c prove {}/active/CHG-a*/ledger.jsonl", store.display());
    assert_eq!(bash_denial(&p, &r.repo(), &cmd), None);
}

#[test]
fn glob_read_write_through_ledger_glob_stays_denied() {
    let _guard = crate::env_lock::lock();
    let (r, p) = hook_project("glob-read-write");
    let store = ledger_store(&r);
    let cmd = format!("echo x >> {}/active/CHG-a*/ledger.jsonl", store.display());
    let found = bash_denial(&p, &r.repo(), &cmd).unwrap();
    assert_eq!(found.rule, rvow_hook::hook::Rule::Ledger);
}

#[test]
fn glob_read_hidden_checks_glob_stays_denied() {
    let _guard = crate::env_lock::lock();
    let (r, p) = hook_project("glob-read-secret");
    let home = r.0.join("rvowhome");
    std::fs::create_dir_all(home.join("hidden-checks")).unwrap();
    std::fs::write(home.join("hidden-checks").join("gatekey"), "key").unwrap();
    let old = std::env::var_os("RVOW_HOME");
    unsafe { std::env::set_var("RVOW_HOME", &home) };
    let cmd = format!("cat {}/hidden-check*/gatekey", home.display());
    let found = bash_denial(&p, &r.repo(), &cmd);
    match old {
        Some(v) => unsafe { std::env::set_var("RVOW_HOME", v) },
        None => unsafe { std::env::remove_var("RVOW_HOME") },
    }
    assert_eq!(found.unwrap().rule, rvow_hook::hook::Rule::HiddenChecks);
}

#[test]
fn glob_read_sort_output_through_glob_is_denied() {
    let _guard = crate::env_lock::lock();
    let (r, p) = hook_project("glob-read-sort");
    let store = ledger_store(&r);
    let cmd = format!("sort -o {}/active/CHG-a*/ledger.jsonl /dev/null", store.display());
    let found = bash_denial(&p, &r.repo(), &cmd).unwrap();
    assert_eq!(found.rule, rvow_hook::hook::Rule::Ledger);
}

#[test]
fn glob_read_uniq_output_through_glob_is_denied() {
    let _guard = crate::env_lock::lock();
    let (r, p) = hook_project("glob-read-uniq");
    let store = ledger_store(&r);
    let cmd = format!("uniq /dev/null {}/active/CHG-a*/ledger.jsonl", store.display());
    let found = bash_denial(&p, &r.repo(), &cmd).unwrap();
    assert_eq!(found.rule, rvow_hook::hook::Rule::Ledger);
}

#[test]
fn glob_read_sort_output_literal_path_is_denied() {
    let _guard = crate::env_lock::lock();
    let (r, p) = hook_project("glob-read-sort-literal");
    let store = ledger_store(&r);
    let cmd = format!("sort -o {}/active/CHG-abcdef/ledger.jsonl /dev/null", store.display());
    let found = bash_denial(&p, &r.repo(), &cmd).unwrap();
    assert_eq!(found.rule, rvow_hook::hook::Rule::Ledger);
}

#[test]
fn glob_read_sort_abbrev_output_through_glob_is_denied() {
    let _guard = crate::env_lock::lock();
    let (r, p) = hook_project("glob-read-sort-abbrev");
    let store = ledger_store(&r);
    let cmd = format!("sort --out={}/active/CHG-a*/ledger.jsonl /dev/null", store.display());
    let found = bash_denial(&p, &r.repo(), &cmd).unwrap();
    assert_eq!(found.rule, rvow_hook::hook::Rule::Ledger);
}

#[test]
fn glob_read_plain_sort_through_glob_stays_allowed() {
    let _guard = crate::env_lock::lock();
    let (r, p) = hook_project("glob-read-sort-plain");
    let store = ledger_store(&r);
    let cmd = format!("sort {}/active/CHG-a*/ledger.jsonl", store.display());
    assert_eq!(bash_denial(&p, &r.repo(), &cmd), None);
}
