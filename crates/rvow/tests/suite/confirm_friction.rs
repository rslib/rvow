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

    fn rvow(&self, args: &[&str]) -> (String, i32) {
        let out = self.cmd(env!("CARGO_BIN_EXE_rvow")).args(args).output().unwrap();
        assert_eq!(String::from_utf8_lossy(&out.stderr), "", "rvow {args:?}");
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

fn accepted(name: &str) -> (Repo, PathBuf, String) {
    let r = Repo::new(name);
    let root = PathBuf::from(field(&r.ok(&["init"]), "created"));
    let out = r.ok(&["new", "--spec", "Out"]);
    let id = field(&out, "created").to_owned();
    let dir = Path::new(field(&out, "path")).to_owned();
    std::fs::write(
        dir.join("tasks.md"),
        "- TSK-new1 Write ok\n  covers: REQ-new1\n  verify: grep -q ok out.txt && echo 'test result: ok. 1 passed; 0 failed;'\n  files: out.txt\n",
    )
    .unwrap();
    std::fs::write(
        dir.join("specs/out.md"),
        "## ADD REQ-new1 Out\nrvow SHALL write ok.\n### Scenario: s\n- WHEN it runs\n- THEN out.txt holds ok\n",
    )
    .unwrap();
    let accept = r.cmd(env!("CARGO_BIN_EXE_rvow")).args(["accept", &id, "--yes"]).output().unwrap();
    let text = String::from_utf8(accept.stdout).unwrap();
    assert!(accept.status.success(), "{text}");
    (r, root, id)
}

fn amend_one(r: &Repo, root: &Path, id: &str) -> String {
    let dir =
        std::fs::read_dir(root.join("active")).unwrap().map(|e| e.unwrap().path()).find(|p| p.to_string_lossy().contains(id)).unwrap();
    let tasks = std::fs::read_to_string(dir.join("tasks.md")).unwrap();
    std::fs::write(dir.join("tasks.md"), tasks.replace("Write ok", "Write one")).unwrap();
    let amended = r.ok(&["amend", "--reason", "edit one"]);
    amended.split_whitespace().find(|w| w.starts_with("AMD-")).unwrap().trim_matches(|c: char| !c.is_alphanumeric() && c != '-').to_owned()
}

#[test]
fn decided_accepted_names_the_decision() {
    let (r, root, id) = accepted("decided-accepted");
    let amd = amend_one(&r, &root, &id);
    r.ok(&["confirm", &amd, "--accept"]);
    let (out, code) = r.rvow(&["confirm", "--accept", &amd]);
    assert_eq!(code, 1, "{out}");
    assert!(out.contains("error[decided]"), "{out}");
    assert!(out.contains(&format!("{amd} is already accepted")), "{out}");
}

#[test]
fn decided_rejected_names_the_rejection() {
    let (r, root, id) = accepted("decided-rejected");
    let amd = amend_one(&r, &root, &id);
    r.ok(&["confirm", &amd, "--reject"]);
    let (out, code) = r.rvow(&["confirm", "--accept", &amd]);
    assert_eq!(code, 1, "{out}");
    assert!(out.contains("error[decided]"), "{out}");
    assert!(out.contains(&format!("{amd} is already rejected")), "{out}");
}

fn hook_project(name: &str) -> (Repo, rvow_core::store::Project) {
    let r = Repo::new(name);
    let home = r.0.join("home");
    let p = rvow_core::store::init(&r.repo(), &home, rvow_core::store::Mode::Home).unwrap();
    (r, p)
}

fn bash_denial(p: &rvow_core::store::Project, cwd: &Path, cmd: &str) -> Option<rvow_core::hook::Denial> {
    let input = serde_json::json!({
        "cwd": cwd.to_str().unwrap(),
        "tool_name": "Bash",
        "tool_input": {"command": cmd},
    });
    rvow_core::hook::pre_tool_use_denial(p, &input).unwrap()
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
    assert_eq!(found.rule, rvow_core::hook::Rule::Ledger);
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
    assert_eq!(found.unwrap().rule, rvow_core::hook::Rule::HiddenChecks);
}

#[test]
fn glob_read_sort_output_through_glob_is_denied() {
    let _guard = crate::env_lock::lock();
    let (r, p) = hook_project("glob-read-sort");
    let store = ledger_store(&r);
    let cmd = format!("sort -o {}/active/CHG-a*/ledger.jsonl /dev/null", store.display());
    let found = bash_denial(&p, &r.repo(), &cmd).unwrap();
    assert_eq!(found.rule, rvow_core::hook::Rule::Ledger);
}

#[test]
fn glob_read_uniq_output_through_glob_is_denied() {
    let _guard = crate::env_lock::lock();
    let (r, p) = hook_project("glob-read-uniq");
    let store = ledger_store(&r);
    let cmd = format!("uniq /dev/null {}/active/CHG-a*/ledger.jsonl", store.display());
    let found = bash_denial(&p, &r.repo(), &cmd).unwrap();
    assert_eq!(found.rule, rvow_core::hook::Rule::Ledger);
}

#[test]
fn glob_read_sort_output_literal_path_is_denied() {
    let _guard = crate::env_lock::lock();
    let (r, p) = hook_project("glob-read-sort-literal");
    let store = ledger_store(&r);
    let cmd = format!("sort -o {}/active/CHG-abcdef/ledger.jsonl /dev/null", store.display());
    let found = bash_denial(&p, &r.repo(), &cmd).unwrap();
    assert_eq!(found.rule, rvow_core::hook::Rule::Ledger);
}

#[test]
fn glob_read_sort_abbrev_output_through_glob_is_denied() {
    let _guard = crate::env_lock::lock();
    let (r, p) = hook_project("glob-read-sort-abbrev");
    let store = ledger_store(&r);
    let cmd = format!("sort --out={}/active/CHG-a*/ledger.jsonl /dev/null", store.display());
    let found = bash_denial(&p, &r.repo(), &cmd).unwrap();
    assert_eq!(found.rule, rvow_core::hook::Rule::Ledger);
}

#[test]
fn glob_read_plain_sort_through_glob_stays_allowed() {
    let _guard = crate::env_lock::lock();
    let (r, p) = hook_project("glob-read-sort-plain");
    let store = ledger_store(&r);
    let cmd = format!("sort {}/active/CHG-a*/ledger.jsonl", store.display());
    assert_eq!(bash_denial(&p, &r.repo(), &cmd), None);
}

#[test]
fn decided_unknown_keeps_amendment_code() {
    let (r, _root, _id) = accepted("decided-unknown");
    let (out, code) = r.rvow(&["confirm", "--accept", "AMD-zzzzzz"]);
    assert_eq!(code, 1, "{out}");
    assert!(out.contains("error[amendment]"), "{out}");
}
