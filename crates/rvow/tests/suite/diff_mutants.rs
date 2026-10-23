use std::path::{Path, PathBuf};
use std::process::Command;

struct Repo(PathBuf);

impl Drop for Repo {
    fn drop(&mut self) {
        rvow_core::git::testrepo::remove_tree(&self.0);
    }
}

impl Repo {
    fn new(name: &str) -> Repo {
        let dir = std::env::temp_dir().join(format!("rvow-diff-{}-{name}", std::process::id()));
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

    /// Runs rvow with stdout on a pipe, as an agent does: stdout and exit code.
    fn rvow(&self, args: &[&str]) -> (String, i32) {
        let (out, _, code) = self.rvow_full(args);
        (out, code)
    }

    /// Like `rvow`, but keeps stderr: `prove` streams the command output there.
    fn rvow_full(&self, args: &[&str]) -> (String, String, i32) {
        let out = self.cmd(env!("CARGO_BIN_EXE_rvow")).args(args).output().unwrap();
        (String::from_utf8(out.stdout).unwrap(), String::from_utf8(out.stderr).unwrap(), out.status.code().unwrap())
    }

    /// Runs `prove`, which streams the verify output to stderr.
    fn prove(&self, task: &str) -> (String, i32) {
        let (out, _, code) = self.rvow_full(&["prove", task]);
        (out, code)
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

const VERIFY_OK: &str = "echo 'test result: ok. 1 passed; 0 failed;'";
const SPEC: &str = "## ADD REQ-new1 Out\nrvow SHALL write ok.\n### Scenario: s\n- WHEN it runs\n- THEN out.txt holds ok\n";

/// An accepted change whose task runs `mutants`, returning the repo, the
/// project root and the minted task id.
fn accepted_with(name: &str, tasks: &str) -> (Repo, PathBuf, String) {
    let r = Repo::new(name);
    let root = PathBuf::from(field(&r.ok(&["init"]), "created"));
    // Fixtures that exist at the base commit, so later edits are tracked changes.
    std::fs::create_dir_all(r.repo().join("src")).unwrap();
    std::fs::write(r.repo().join("src/a.rs"), "x\n").unwrap();
    std::fs::create_dir_all(r.repo().join("docs")).unwrap();
    std::fs::write(r.repo().join("docs/x.md"), "x\n").unwrap();
    r.git(&["add", "."]);
    r.git(&["commit", "-q", "-m", "fixtures"]);
    let out = r.ok(&["new", "--spec", "Out"]);
    let id = field(&out, "created").to_owned();
    let dir = Path::new(field(&out, "path")).to_owned();
    std::fs::write(dir.join("tasks.md"), tasks).unwrap();
    std::fs::write(dir.join("specs/out.md"), SPEC).unwrap();
    let accept = r.cmd(env!("CARGO_BIN_EXE_rvow")).args(["accept", &id, "--yes"]).output().unwrap();
    let text = String::from_utf8(accept.stdout).unwrap();
    assert!(accept.status.success(), "{text}");
    let active = std::fs::read_dir(root.join("active"))
        .unwrap()
        .map(|e| e.unwrap().path())
        .find(|p| p.to_string_lossy().contains(id.as_str()))
        .unwrap();
    let md = std::fs::read_to_string(active.join("tasks.md")).unwrap();
    let task = md.lines().find_map(|l| l.strip_prefix("- ")).unwrap().split_whitespace().next().unwrap().to_string();
    (r, root, task)
}

fn env_task(mutants: &str) -> String {
    format!("- TSK-new1 Diff env\n  covers: REQ-new1\n  verify: {VERIFY_OK}\n  files: src/**\n  baseline: pass\n  mutants: {mutants}\n")
}

fn capture_mutants() -> &'static str {
    "cat \"$RVOW_DIFF\" > seen.diff && printf '%s' \"$RVOW_CHANGED_FILES\" > seen.files"
}

#[test]
fn diff_mutants_env_command_reads_diff() {
    let (r, _root, task) = accepted_with("envreads", &env_task(capture_mutants()));
    // src/a.rs exists at the base commit, so this is an edit since the base.
    std::fs::write(r.repo().join("src/a.rs"), "y\n").unwrap();
    let (out, code) = r.prove(&task);
    assert_eq!(code, 0, "{out}");
    let diff = std::fs::read_to_string(r.repo().join("seen.diff")).unwrap();
    assert!(diff.contains("src/a.rs"), "{diff}");
    assert!(diff.contains("+y"), "{diff}");
    let files = std::fs::read_to_string(r.repo().join("seen.files")).unwrap();
    assert_eq!(files, "src/a.rs", "{files}");
}

#[test]
fn diff_mutants_env_new_file_counts() {
    let (r, _root, task) = accepted_with("envnew", &env_task(capture_mutants()));
    std::fs::create_dir_all(r.repo().join("src")).unwrap();
    std::fs::write(r.repo().join("src/b.rs"), "new\n").unwrap();
    let (out, code) = r.prove(&task);
    assert_eq!(code, 0, "{out}");
    let diff = std::fs::read_to_string(r.repo().join("seen.diff")).unwrap();
    assert!(diff.contains("src/b.rs"), "{diff}");
    assert!(diff.contains("new file") || diff.contains("+new"), "{diff}");
    let files = std::fs::read_to_string(r.repo().join("seen.files")).unwrap();
    assert_eq!(files, "src/b.rs", "{files}");
}

#[test]
fn diff_mutants_env_outside_files_stay_out() {
    let (r, _root, task) = accepted_with("envoutside", &env_task(capture_mutants()));
    // docs/x.md exists at the base commit but is outside the task's files.
    std::fs::write(r.repo().join("docs/x.md"), "changed\n").unwrap();
    let (out, code) = r.prove(&task);
    assert_eq!(code, 0, "{out}");
    let diff = std::fs::read_to_string(r.repo().join("seen.diff")).unwrap();
    assert!(!diff.contains("docs/x.md"), "{diff}");
    let files = std::fs::read_to_string(r.repo().join("seen.files")).unwrap();
    assert!(!files.contains("docs/x.md"), "{files}");
}

#[test]
fn diff_mutants_env_nonascii_name_in_changed_files() {
    let (r, _root, task) = accepted_with("envnonascii", &env_task(capture_mutants()));
    // A non-ASCII name: git quotes it without -z, which would hide it from the globs.
    let name = "src/caf\u{e9}.rs".to_string();
    std::fs::write(r.repo().join(&name), "new\n").unwrap();
    let (out, code) = r.prove(&task);
    assert_eq!(code, 0, "{out}");
    let files = std::fs::read_to_string(r.repo().join("seen.files")).unwrap();
    assert!(files.lines().any(|l| l == name), "{files}");
    let diff = std::fs::read_to_string(r.repo().join("seen.diff")).unwrap();
    assert!(diff.contains(&name), "{diff}");
}

#[test]
fn diff_mutants_env_empty_without_change() {
    let mutants = "echo \"$RVOW_DIFF\" > diff.path && cat \"$RVOW_DIFF\" > seen.diff && printf '%s' \"$RVOW_CHANGED_FILES\" > seen.files";
    let (r, _root, task) = accepted_with("envempty", &env_task(mutants));
    let (out, code) = r.prove(&task);
    assert_eq!(code, 0, "{out}");
    let diff = std::fs::read_to_string(r.repo().join("seen.diff")).unwrap();
    assert!(diff.is_empty(), "{diff}");
    let files = std::fs::read_to_string(r.repo().join("seen.files")).unwrap();
    assert!(files.is_empty(), "{files}");
    // The diff file is a temp file: it is gone after the run.
    let path = std::fs::read_to_string(r.repo().join("diff.path")).unwrap();
    assert!(!Path::new(path.trim()).exists(), "{path}");
}

fn prove_event(root: &Path) -> serde_json::Value {
    // Each test repository holds exactly one active change.
    let active = std::fs::read_dir(root.join("active")).unwrap().map(|e| e.unwrap().path()).next().expect("no active change");
    let text = std::fs::read_to_string(active.join("ledger.jsonl")).unwrap();
    let mut last = None;
    for line in text.lines() {
        let v: serde_json::Value = serde_json::from_str(line).unwrap();
        if v.get("kind").and_then(|k| k.as_str()).is_some_and(|k| k.eq_ignore_ascii_case("prove")) {
            last = Some(v);
        }
    }
    last.expect("no prove event in the ledger")
}

#[test]
fn diff_mutants_survived_summary_counted() {
    let mutants = "printf '12 mutants tested in 4s: 2 missed, 9 caught, 1 unviable\\n'; exit 2";
    let (r, root, task) = accepted_with("survcount", &env_task(mutants));
    let (out, code) = r.prove(&task);
    assert_ne!(code, 0, "a failing mutants run is not a proof: {out}");
    assert!(out.contains("mutation check"), "{out}");
    let event = prove_event(&root);
    assert_eq!(event["mutants"]["survived"], serde_json::json!(2), "{event}");
    let shown = r.ok(&["show", &task]);
    assert!(shown.contains("mutants: 2 survived"), "{shown}");
}

#[test]
fn diff_mutants_survived_other_output_no_count() {
    let (r, root, task) = accepted_with("survnone", &env_task("echo ok"));
    let (out, code) = r.prove(&task);
    assert_eq!(code, 0, "{out}");
    let event = prove_event(&root);
    assert!(event["mutants"].get("survived").is_none(), "{event}");
    let shown = r.ok(&["show", &task]);
    assert!(!shown.contains("survived"), "{shown}");
}
