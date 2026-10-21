use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicU32, Ordering};

static COUNTER: AtomicU32 = AtomicU32::new(0);

struct Repo {
    tmp: PathBuf,
    home: PathBuf,
    repo: PathBuf,
    root: PathBuf,
}

impl Drop for Repo {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.tmp);
    }
}

impl Repo {
    fn fresh(name: &str) -> Repo {
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        let tmp = std::env::temp_dir().join(format!("rvow-attest-{}-{n}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp);
        std::fs::create_dir_all(tmp.join("repo")).unwrap();
        let tmp = tmp.canonicalize().unwrap();
        let mut r = Repo { tmp: tmp.clone(), home: tmp.join("home"), repo: tmp.join("repo"), root: PathBuf::new() };
        r.git(&["init", "-q", "-b", "main"]);
        r.git(&["config", "user.name", "t"]);
        r.git(&["config", "user.email", "t@t"]);
        std::fs::write(r.repo.join("a.txt"), "a").unwrap();
        r.git(&["add", "a.txt"]);
        r.git(&["commit", "-q", "-m", "init"]);
        let out = r.ok(&["init"]);
        r.root = PathBuf::from(field(&out, "created").to_owned());
        r
    }

    fn git(&self, args: &[&str]) {
        assert!(self.cmd("git").args(args).status().unwrap().success(), "git {args:?}");
    }

    fn cmd(&self, program: &str) -> Command {
        let mut c = Command::new(program);
        c.current_dir(&self.repo)
            .env("HOME", &self.tmp)
            .env("RVOW_HOME", &self.home)
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_AUTHOR_NAME", "t")
            .env("GIT_AUTHOR_EMAIL", "t@t")
            .env("GIT_COMMITTER_NAME", "t")
            .env("GIT_COMMITTER_EMAIL", "t@t");
        c
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

    fn active_dir(&self, id: &str) -> PathBuf {
        std::fs::read_dir(self.root.join("active"))
            .unwrap()
            .map(|e| e.unwrap().path())
            .find(|p| p.to_string_lossy().contains(id))
            .unwrap_or_else(|| panic!("no active dir for {id}"))
    }

    fn commit(&self, rel: &str, content: &str) {
        let path = self.repo.join(rel);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, content).unwrap();
        self.git(&["add", rel]);
        self.git(&["commit", "-q", "-m", rel]);
    }
}

fn field<'a>(out: &'a str, key: &str) -> &'a str {
    out.lines().find_map(|l| l.strip_prefix(&format!("{key}: "))).unwrap_or_else(|| panic!("no {key} in {out}"))
}

const SPEC: &str = "## ADD REQ-new1 Out\nrvow SHALL write ok.\n### Scenario: s\n- WHEN it runs\n- THEN out.txt holds ok\n";

fn tasks_two() -> String {
    "- TSK-new1 First\n  covers: REQ-new1\n  verify: grep -q ok one.txt && echo 'test result: ok. 1 passed; 0 failed;'\n  files: one.txt\n\n- TSK-new2 Second\n  covers: REQ-new1\n  verify: grep -q ok two.txt && echo 'test result: ok. 1 passed; 0 failed;'\n  files: two.txt\n".to_string()
}

/// An accepted change; returns the change id and its two minted task ids.
/// The task files do not exist yet, so each check fails at accept time.
fn accepted_two(r: &Repo) -> (String, String, String) {
    let out = r.ok(&["new", "--spec", "Out"]);
    let id = field(&out, "created").to_owned();
    let dir = PathBuf::from(field(&out, "path").to_owned());
    std::fs::write(dir.join("tasks.md"), tasks_two()).unwrap();
    std::fs::write(dir.join("specs/out.md"), SPEC).unwrap();
    accept(r, &id);
    let tasks = std::fs::read_to_string(r.active_dir(&id).join("tasks.md")).unwrap();
    let mut ids = tasks.lines().filter_map(|l| l.strip_prefix("- ").and_then(|t| t.split_whitespace().next()));
    (id, ids.next().unwrap().to_owned(), ids.next().unwrap().to_owned())
}

fn write_ok(r: &Repo) {
    r.commit("one.txt", "ok\n");
    r.commit("two.txt", "ok\n");
}

fn prove(r: &Repo, task: &str) -> (String, i32) {
    let out = r.cmd(env!("CARGO_BIN_EXE_rvow")).args(["prove", task]).output().unwrap();
    (String::from_utf8(out.stdout).unwrap(), out.status.code().unwrap())
}

fn attest(r: &Repo, args: &[&str]) -> (String, i32) {
    r.rvow(args)
}

fn accept(r: &Repo, id: &str) {
    let out = r.cmd(env!("CARGO_BIN_EXE_rvow")).args(["accept", id, "--yes"]).output().unwrap();
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stdout));
}

fn lines(path: &Path) -> Vec<serde_json::Value> {
    std::fs::read_to_string(path)
        .unwrap_or_else(|_| panic!("no file at {}", path.display()))
        .lines()
        .map(|l| serde_json::from_str(l).unwrap())
        .collect::<Vec<_>>()
}

fn vow_hash(r: &Repo, id: &str) -> String {
    let (events, _) = rvow_core::ledger::read(&r.active_dir(id).join("ledger.jsonl")).unwrap();
    events.into_iter().find(|e| e.kind == rvow_core::ledger::Kind::Vow).expect("no vow").hash
}

fn assert_subjects(line: &serde_json::Value) {
    let subjects = line["subject"].as_array().unwrap_or_else(|| panic!("no subject list in {line}"));
    assert!(!subjects.is_empty(), "empty subject in {line}");
    for s in subjects {
        assert!(s["digest"]["sha256"].is_string(), "subject without digest.sha256 in {line}");
    }
}

#[test]
fn attest_task_statements_proved_task_passes() {
    let r = Repo::fresh("proved");
    r.commit("src/a.rs", "other\n");
    let out = r.ok(&["new", "--spec", "Out"]);
    let id = field(&out, "created").to_owned();
    let dir = PathBuf::from(field(&out, "path").to_owned());
    let verify = "grep -q content src/a.rs && echo 'test result: ok. 4 passed; 0 failed;'";
    std::fs::write(dir.join("tasks.md"), format!("- TSK-new1 Write a\n  covers: REQ-new1\n  verify: {verify}\n  files: src/a.rs\n"))
        .unwrap();
    std::fs::write(dir.join("specs/out.md"), SPEC).unwrap();
    accept(&r, &id);
    let tasks = std::fs::read_to_string(r.active_dir(&id).join("tasks.md")).unwrap();
    let task = tasks.lines().find_map(|l| l.strip_prefix("- ").and_then(|t| t.split_whitespace().next())).unwrap().to_owned();
    r.commit("src/a.rs", "content\n");
    assert_eq!(prove(&r, &task).1, 0);
    let dest = r.repo.join("att.jsonl");
    let (out, code) = attest(&r, &["attest", &id, "--out", &dest.to_string_lossy()]);
    assert_eq!(code, 0, "{out}");
    let got = lines(&dest);
    assert_eq!(got.len(), 2, "{got:?}");
    let first = &got[0];
    assert_eq!(first["_type"], "https://in-toto.io/Statement/v1");
    assert_eq!(first["predicateType"], "https://in-toto.io/attestation/test-result/v0.1");
    assert_eq!(first["predicate"]["result"], "PASSED");
    let config = &first["predicate"]["configuration"];
    assert_eq!(config.as_array().map(Vec::len), Some(1));
    assert_eq!(config[0]["name"], "rvow verify");
    assert_eq!(config[0]["digest"], serde_json::json!({"sha256": rvow_core::vow::sha256_hex(verify.as_bytes())}));
    assert_eq!(config[0]["annotations"]["task"], serde_json::json!(task));
    assert_eq!(config[0]["annotations"]["verify"], verify);
    assert_eq!(config[0]["annotations"]["tests"], 4);
    assert_subjects(first);
    assert_eq!(first["predicate"]["passedTests"], serde_json::json!([]));
    assert_eq!(first["predicate"]["failedTests"], serde_json::json!([]));
    let want = rvow_core::vow::sha256_hex(b"content\n");
    assert_eq!(first["subject"], serde_json::json!([{"name": "src/a.rs", "digest": {"sha256": want}}]));
}

#[test]
fn attest_task_statements_failed_task_fails() {
    let r = Repo::fresh("failed");
    let out = r.ok(&["new", "--spec", "Out"]);
    let id = field(&out, "created").to_owned();
    let dir = PathBuf::from(field(&out, "path").to_owned());
    std::fs::write(
        dir.join("tasks.md"),
        "- TSK-new1 Write out\n  covers: REQ-new1\n  verify: grep -q ok out.txt && echo 'test result: ok. 1 passed; 0 failed;'\n  files: out.txt\n",
    )
    .unwrap();
    std::fs::write(dir.join("specs/out.md"), SPEC).unwrap();
    accept(&r, &id);
    let tasks = std::fs::read_to_string(r.active_dir(&id).join("tasks.md")).unwrap();
    let task = tasks.lines().find_map(|l| l.strip_prefix("- ").and_then(|t| t.split_whitespace().next())).unwrap().to_owned();
    assert_eq!(prove(&r, &task).1, 1, "a failing first prove is a failed proof");
    let dest = r.repo.join("att.jsonl");
    let (out, code) = attest(&r, &["attest", &id, "--out", &dest.to_string_lossy()]);
    assert_eq!(code, 0, "{out}");
    let got = lines(&dest);
    assert_eq!(got.len(), 2, "{got:?}");
    assert_eq!(got[0]["predicateType"], "https://in-toto.io/attestation/test-result/v0.1");
    assert_eq!(got[0]["predicate"]["result"], "FAILED");
}

#[test]
fn attest_task_statements_stale_and_missing_have_no_line() {
    let r = Repo::fresh("stalemissing");
    let (id, t1, _t2) = accepted_two(&r);
    r.commit("one.txt", "ok\n");
    r.commit("two.txt", "ok\n");
    assert_eq!(prove(&r, &t1).1, 0);
    std::fs::write(r.repo.join("one.txt"), "ok\nmore\n").unwrap();
    let dest = r.repo.join("att.jsonl");
    let (out, code) = attest(&r, &["attest", &id, "--out", &dest.to_string_lossy()]);
    assert_eq!(code, 0, "{out}");
    let got = lines(&dest);
    assert_eq!(got.len(), 1, "{got:?}");
    assert_eq!(got[0]["predicateType"], "https://github.com/rslib/rvow/attestation/change/v0.1");
}

#[test]
fn attest_task_statements_proved_task_without_tracked_files_names_the_task() {
    let r = Repo::fresh("notracked");
    let out = r.ok(&["new", "--spec", "Out"]);
    let id = field(&out, "created").to_owned();
    let dir = PathBuf::from(field(&out, "path").to_owned());
    let verify = "grep -q ok ghost.txt && echo 'test result: ok. 2 passed; 0 failed;'";
    std::fs::write(dir.join("tasks.md"), format!("- TSK-new1 Write ghost\n  covers: REQ-new1\n  verify: {verify}\n  files: ghost.txt\n"))
        .unwrap();
    std::fs::write(dir.join("specs/out.md"), SPEC).unwrap();
    accept(&r, &id);
    let tasks = std::fs::read_to_string(r.active_dir(&id).join("tasks.md")).unwrap();
    let task = tasks.lines().find_map(|l| l.strip_prefix("- ").and_then(|t| t.split_whitespace().next())).unwrap().to_owned();
    std::fs::write(r.repo.join("ghost.txt"), "ok\n").unwrap();
    assert_eq!(prove(&r, &task).1, 0);
    let dest = r.repo.join("att.jsonl");
    let (out, code) = attest(&r, &["attest", &id, "--out", &dest.to_string_lossy()]);
    assert_eq!(code, 0, "{out}");
    let got = lines(&dest);
    assert_eq!(got.len(), 2, "{got:?}");
    assert_eq!(got[0]["predicate"]["result"], "PASSED");
    let want = rvow_core::vow::sha256_hex(verify.as_bytes());
    assert_eq!(got[0]["subject"], serde_json::json!([{ "name": task, "digest": { "sha256": want } }]));
    assert_subjects(&got[0]);
}

#[test]
fn attest_task_statements_unknown_change_names_the_id() {
    let r = Repo::fresh("unknown");
    let dest = r.repo.join("att.jsonl");
    let (out, code) = attest(&r, &["attest", "CHG-zzzzzz", "--out", &dest.to_string_lossy()]);
    assert_eq!(code, 1, "{out}");
    assert!(out.contains("CHG-zzzzzz"), "{out}");
    assert!(!dest.exists(), "attest wrote a file for an unknown change");
}

#[test]
fn attest_summary_finished_change_passes() {
    let r = Repo::fresh("finished");
    let (id, t1, t2) = accepted_two(&r);
    write_ok(&r);
    assert_eq!(prove(&r, &t1).1, 0);
    assert_eq!(prove(&r, &t2).1, 0);
    let dest = r.repo.join("att.jsonl");
    let (out, code) = attest(&r, &["attest", &id, "--out", &dest.to_string_lossy()]);
    assert_eq!(code, 0, "{out}");
    let got = lines(&dest);
    assert_eq!(got.len(), 3, "{got:?}");
    let last = got.last().unwrap();
    assert_eq!(last["predicateType"], "https://github.com/rslib/rvow/attestation/change/v0.1");
    assert_eq!(last["predicate"]["change"], serde_json::json!(id));
    assert_eq!(last["predicate"]["vow"], serde_json::json!(vow_hash(&r, &id)));
    assert!(last["predicate"]["confirm"].is_null(), "{last}");
    assert_eq!(last["predicate"]["gate"], serde_json::json!("pass"));
    assert_eq!(last["predicate"]["unproved"], serde_json::json!([]));
    for line in &got {
        assert_subjects(line);
    }
}

#[test]
fn attest_summary_unfinished_change_fails() {
    let r = Repo::fresh("unfinished");
    let (id, t1, t2) = accepted_two(&r);
    write_ok(&r);
    assert_eq!(prove(&r, &t1).1, 0);
    assert_eq!(prove(&r, &t2).1, 0);
    std::fs::write(r.repo.join("two.txt"), "ok\nmore\n").unwrap();
    let dest = r.repo.join("att.jsonl");
    let (out, code) = attest(&r, &["attest", &id, "--out", &dest.to_string_lossy()]);
    assert_eq!(code, 0, "{out}");
    let got = lines(&dest);
    let last = got.last().unwrap();
    assert_eq!(last["predicate"]["gate"], serde_json::json!("fail"));
    assert_eq!(last["predicate"]["unproved"], serde_json::json!([t2]));
}

#[test]
fn attest_sign_signature_verifies() {
    let r = Repo::fresh("sign");
    let (id, t1, t2) = accepted_two(&r);
    write_ok(&r);
    assert_eq!(prove(&r, &t1).1, 0);
    assert_eq!(prove(&r, &t2).1, 0);
    let key = r.tmp.join("key");
    let made = Command::new("ssh-keygen")
        .args(["-t", "ed25519", "-f", &key.to_string_lossy(), "-N", "", "-q"])
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .output()
        .unwrap();
    assert!(made.status.success(), "{made:?}");
    std::fs::write(r.home.join("config.toml"), format!("signing_key = \"{}\"\n", key.display())).unwrap();
    let dest = r.repo.join("att.jsonl");
    let (out, code) = attest(&r, &["attest", &id, "--out", &dest.to_string_lossy(), "--sign"]);
    assert_eq!(code, 0, "{out}");
    let sig = PathBuf::from(format!("{}.sig", dest.display()));
    assert!(sig.exists(), "attest --sign wrote no signature");
    for line in lines(&dest) {
        assert_subjects(&line);
    }
    let pubkey = std::fs::read_to_string(format!("{}.pub", key.display())).unwrap();
    let allowed = r.tmp.join("allowed_signers");
    std::fs::write(&allowed, format!("tester@example.com {}\n", pubkey.trim())).unwrap();
    let input = std::fs::read(&dest).unwrap();
    let mut child = Command::new("ssh-keygen")
        .args(["-Y", "verify", "-f", &allowed.to_string_lossy(), "-I", "tester@example.com", "-n", "rvow", "-s", &sig.to_string_lossy()])
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .unwrap();
    use std::io::Write;
    child.stdin.take().unwrap().write_all(&input).unwrap();
    assert!(child.wait().unwrap().success(), "ssh-keygen -Y verify refused the signature");
}

#[test]
fn attest_sign_no_key_writes_no_file() {
    let r = Repo::fresh("nokey");
    let (id, t1, t2) = accepted_two(&r);
    write_ok(&r);
    assert_eq!(prove(&r, &t1).1, 0);
    assert_eq!(prove(&r, &t2).1, 0);
    let dest = r.repo.join("att.jsonl");
    let (out, code) = attest(&r, &["attest", &id, "--out", &dest.to_string_lossy(), "--sign"]);
    assert_eq!(code, 1, "{out}");
    assert!(out.contains("signing_key"), "{out}");
    assert!(!dest.exists(), "attest --sign with no key wrote the file");
    assert!(!PathBuf::from(format!("{}.sig", dest.display())).exists(), "attest --sign with no key wrote a signature");
}
