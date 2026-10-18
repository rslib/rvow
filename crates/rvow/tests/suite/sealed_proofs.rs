//! Seals: a proof carries an HMAC under the rerun cache key, the key never
//! reaches output, and the gate trusts a proof only when its seal checks.

use std::path::{Path, PathBuf};
use std::process::Command;

use rvow_core::change::LEDGER;
use rvow_core::ledger::{self, Kind};
use serde_json::{Map, Value};

const RVOW: &str = env!("CARGO_BIN_EXE_rvow");
const KEY_FILE: &str = "hidden-checks/.cache-key";
const BASE_PATH: &str = "/usr/bin:/bin:/usr/sbin:/sbin";
const PASS_CHECK: &str = "grep -q ok out.txt && echo 'test result: ok. 2 passed; 0 failed;'";

struct Fx {
    root: PathBuf,
    repo: PathBuf,
    home: PathBuf,
    change: PathBuf,
    task: String,
    count: PathBuf,
}

impl Drop for Fx {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

fn git_command(repo: &Path) -> Command {
    let mut c = Command::new("git");
    c.current_dir(repo)
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

impl Fx {
    /// A repository with one accepted, committed task. The verify command logs
    /// every run to a file outside the repository and, when `passing`, prints a
    /// libtest line for two passing tests.
    fn new(name: &str) -> Fx {
        Fx::with(name, true)
    }

    fn with(name: &str, passing: bool) -> Fx {
        Fx::build(name, if passing { PASS_CHECK } else { "false" }, if passing { "ok" } else { "no" }, "out.txt, src/**")
    }

    /// Like `new`, with `check` as the verify command and `out` as the content of out.txt.
    fn build(name: &str, check: &str, content: &str, files: &str) -> Fx {
        let root = std::env::temp_dir().join(format!("rvow-sealed-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("repo/src")).unwrap();
        let root = root.canonicalize().unwrap();
        let repo = root.join("repo");
        let home = root.join("rh");
        let mut fx =
            Fx { root: root.clone(), repo: repo.clone(), home, change: PathBuf::new(), task: String::new(), count: root.join("count") };
        let git = |args: &[&str]| assert!(git_command(&repo).args(args).status().unwrap().success(), "git {args:?}");
        git(&["init", "-q", "-b", "main"]);
        std::fs::write(repo.join("src/a.txt"), "a").unwrap();
        git(&["add", "."]);
        git(&["commit", "-q", "-m", "init"]);
        fx.rvow(&["init"]);
        let out = fx.rvow(&["new", "--spec", "Out"]);
        let dir = PathBuf::from(out.lines().find_map(|l| l.strip_prefix("path: ")).unwrap());
        std::fs::write(
            dir.join("tasks.md"),
            format!("- TSK-new1 Write ok\n  covers: REQ-new1\n  verify: echo run >> {}; {check}\n  files: {files}\n", fx.count.display()),
        )
        .unwrap();
        std::fs::write(
            dir.join("specs/out.md"),
            "## ADD REQ-new1 Out\nrvow SHALL write ok.\n### Scenario: s\n- WHEN it runs\n- THEN out.txt holds ok\n",
        )
        .unwrap();
        let id = out.lines().find_map(|l| l.strip_prefix("created: ")).unwrap().to_string();
        fx.rvow(&["accept", &id, "--yes"]);
        let active = fx.rvow(&["path", &id]);
        fx.change = PathBuf::from(active.lines().find_map(|l| l.strip_prefix("path: ")).unwrap().trim());
        let tasks = std::fs::read_to_string(fx.change.join("tasks.md")).unwrap();
        fx.task = tasks.lines().find_map(|l| l.strip_prefix("- ")?.split_whitespace().next().map(String::from)).unwrap();
        fx.commit("out.txt", content);
        let _ = std::fs::remove_file(&fx.count);
        fx
    }

    fn cmd(&self) -> Command {
        let mut c = Command::new(RVOW);
        c.current_dir(&self.repo)
            .env("HOME", self.root.join("home"))
            .env("RVOW_HOME", &self.home)
            .env("PATH", BASE_PATH)
            .env_remove("XDG_CONFIG_HOME")
            .env_remove("XDG_DATA_HOME")
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

    fn rvow(&self, args: &[&str]) -> String {
        let out = self.cmd().args(args).output().unwrap();
        String::from_utf8_lossy(&out.stdout).into_owned()
    }

    /// Like `rvow`, with `dir` in front of `PATH`.
    fn rvow_path(&self, args: &[&str], dir: &Path) -> String {
        let out = self.cmd().env("PATH", format!("{}:{BASE_PATH}", dir.display())).args(args).output().unwrap();
        String::from_utf8_lossy(&out.stdout).into_owned()
    }

    /// Writes an executable script `name` inside `dir`.
    fn shim(&self, dir: &Path, name: &str, body: &str) -> PathBuf {
        std::fs::create_dir_all(dir).unwrap();
        let path = dir.join(name);
        std::fs::write(&path, body).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        path
    }

    fn commit(&self, file: &str, text: &str) {
        std::fs::write(self.repo.join(file), text).unwrap();
        let git = |args: &[&str]| assert!(git_command(&self.repo).args(args).status().unwrap().success(), "git {args:?}");
        git(&["add", file]);
        git(&["commit", "-q", "-m", "change"]);
    }

    fn prove(&self) {
        let out = self.rvow(&["prove", &self.task]);
        assert!(out.contains("proved"), "{out}");
        let _ = std::fs::remove_file(&self.count);
    }

    /// The number of times the verify command ran since the last reset.
    fn runs(&self) -> usize {
        std::fs::read_to_string(&self.count).map_or(0, |t| t.lines().count())
    }

    fn reset_runs(&self) {
        let _ = std::fs::remove_file(&self.count);
    }

    fn events(&self) -> Vec<ledger::Event> {
        ledger::read(&self.change.join(LEDGER)).unwrap().0
    }

    fn key(&self) -> Vec<u8> {
        std::fs::read(self.home.join(KEY_FILE)).unwrap()
    }

    fn latest_prove(&self) -> ledger::Event {
        self.events().into_iter().rev().find(|e| e.kind == Kind::Prove).unwrap()
    }

    /// Appends an unsealed Prove event, as a hand-written line would be.
    fn append_prove(&self, data: &Map<String, Value>) {
        let project = rvow_core::store::find(&self.repo, &self.home).unwrap().unwrap();
        let lock = project.lock().unwrap();
        ledger::append(&self.change.join(LEDGER), &lock, Kind::Prove, data.clone()).unwrap();
    }

    /// Appends one raw ledger line, as a hand-written copy would be.
    fn append_raw(&self, line: &str) {
        use std::io::Write;
        let path = self.change.join(LEDGER);
        let mut f = std::fs::OpenOptions::new().append(true).open(&path).unwrap();
        f.write_all(line.as_bytes()).unwrap();
        f.write_all(b"\n").unwrap();
    }

    /// The raw ledger line of the newest prove event.
    fn prove_line(&self) -> String {
        let text = std::fs::read_to_string(self.change.join(LEDGER)).unwrap();
        text.lines().rfind(|l| l.contains("\"kind\":\"prove\"")).unwrap().to_string()
    }

    /// Removes the seal field from every prove line, as an older rvow wrote it.
    fn strip_seals(&self) {
        let path = self.change.join(LEDGER);
        let text = std::fs::read_to_string(&path).unwrap();
        let lines: Vec<String> = text
            .lines()
            .map(|line| {
                let mut v: Value = serde_json::from_str(line).unwrap();
                if v.get("kind").and_then(Value::as_str) == Some("prove") {
                    v.as_object_mut().unwrap().remove("seal");
                }
                v.to_string()
            })
            .collect();
        std::fs::write(&path, lines.join("\n") + "\n").unwrap();
    }

    fn project_root(&self) -> PathBuf {
        rvow_core::store::find(&self.repo, &self.home).unwrap().unwrap().root
    }

    /// Replaces the project's config.toml.
    fn config(&self, text: &str) {
        std::fs::write(self.project_root().join("config.toml"), text).unwrap();
    }
}

// REQ-b0eyyt: `rvow prove` and `rvow ingest` seal each proof.

#[test]
fn seal_records_a_proof_with_a_seal_that_checks() {
    let fx = Fx::new("prove");
    fx.prove();
    let event = fx.latest_prove();
    assert!(event.data.get("seal").and_then(Value::as_str).is_some(), "the prove event carries no seal");
    let key = fx.key();
    assert_eq!(key.len(), 32);
    assert!(rvow_core::seal::checks(&key, &event), "the seal does not check against the key in the rvow home");
}

#[test]
fn seal_records_a_forged_seal_is_not_trusted() {
    let fx = Fx::new("forged");
    fx.prove();
    let mut event = fx.latest_prove();
    event.data.insert("seal".into(), "00".repeat(32).into());
    assert!(!rvow_core::seal::checks(&fx.key(), &event), "a hand-written seal must not check");
}

#[test]
fn seal_records_ingest_seals_a_run() {
    let fx = Fx::new("ingest");
    let runs = fx.root.join("runs.jsonl");
    let out = fx.rvow(&["gate", "--strict", "--rerun", "--out", runs.to_str().unwrap()]);
    assert!(out.contains("passed: true"), "{out}");
    assert!(runs.is_file(), "the gate wrote no runs file");
    let ingested = fx.rvow(&["ingest", runs.to_str().unwrap()]);
    assert!(ingested.contains("count: 1"), "{ingested}");
    let event = fx.latest_prove();
    assert!(event.data.get("seal").and_then(Value::as_str).is_some(), "the ingested event carries no seal");
    assert!(rvow_core::seal::checks(&fx.key(), &event), "the ingested seal does not check");
}

#[test]
fn seal_records_show_never_prints_the_key() {
    let fx = Fx::new("show");
    fx.prove();
    let hex: String = fx.key().iter().map(|b| format!("{b:02x}")).collect();
    let out = fx.rvow(&["show", &fx.task]);
    assert!(!out.contains(&hex), "the whole key appears in rvow show");
    for i in (0..hex.len() - 8).step_by(2) {
        assert!(!out.contains(&hex[i..i + 8]), "a piece of the key appears in rvow show");
    }
}

#[test]
fn seal_records_an_unsealed_proof_still_loads() {
    let fx = Fx::new("old");
    fx.prove();
    fx.strip_seals();
    let (events, diags) = ledger::read(&fx.change.join(LEDGER)).unwrap();
    assert!(diags.is_empty(), "{diags:?}");
    let event = events.into_iter().rev().find(|e| e.kind == Kind::Prove).expect("the unsealed proof did not load");
    assert!(!rvow_core::seal::checks(&fx.key(), &event), "an unsealed proof must not check");
}

// REQ-zeds1g: the gate trusts a proven task only when its seal checks.

#[test]
fn seal_trust_a_sealed_proof_is_trusted() {
    let fx = Fx::new("trusted");
    fx.prove();
    let gate = fx.rvow(&["gate", "--rerun"]);
    assert!(gate.contains("passed: true"), "{gate}");
    assert!(gate.contains("proven"), "a sealed proof is not trusted: {gate}");
    assert!(!gate.contains("warning"), "{gate}");
    assert_eq!(fx.runs(), 0, "a trusted task ran: {gate}");
}

#[test]
fn seal_trust_a_hand_written_proof_is_rerun_and_fails() {
    let fx = Fx::with("hand", false);
    let out = fx.rvow(&["prove", &fx.task]);
    assert!(out.contains("failed"), "{out}");
    let mut data = fx.latest_prove().data.clone();
    data.insert("proved".into(), true.into());
    data.insert("key".into(), "forged".into());
    data.remove("seal");
    fx.append_prove(&data);
    fx.reset_runs();
    let gate = fx.rvow(&["gate", "--rerun"]);
    assert!(gate.contains("fail"), "the gate passed a hand-written proof: {gate}");
    assert!(gate.contains(&fx.task), "no warning names the task: {gate}");
    assert_eq!(fx.runs(), 1, "the hand-written proof was trusted: {gate}");
}

#[test]
fn seal_trust_a_copied_seal_is_rerun() {
    let fx = Fx::new("copied");
    fx.prove();
    let pass = fx.latest_prove();
    let mut failed = pass.data.clone();
    failed.insert("proved".into(), false.into());
    failed.insert("key".into(), "forged".into());
    failed.remove("seal");
    fx.append_prove(&failed);
    // A copy of the sealed pass: same seal, but its parents moved.
    fx.append_prove(&pass.data);
    fx.reset_runs();
    let gate = fx.rvow(&["gate", "--rerun"]);
    assert!(gate.contains("passed: true"), "{gate}");
    assert!(gate.contains(&fx.task), "no warning names the task: {gate}");
    assert_eq!(fx.runs(), 1, "the copied seal was trusted: {gate}");
}

#[test]
fn seal_trust_an_old_proof_reruns_once() {
    let fx = Fx::new("old-trust");
    fx.prove();
    fx.strip_seals();
    let first = fx.rvow(&["gate", "--rerun"]);
    assert!(first.contains("passed: true"), "{first}");
    assert!(first.contains(&fx.task), "the first run does not name the task: {first}");
    assert_eq!(fx.runs(), 1, "the first run did not rerun the task: {first}");
    let second = fx.rvow(&["gate", "--rerun"]);
    assert!(second.contains("cached"), "the second run is not cached: {second}");
    assert_eq!(fx.runs(), 1, "the task ran again: {second}");
}

// REQ-gt6w4v: the gate trusts a proof only with the same verify programs.

#[test]
fn programs_recorded_in_the_proof() {
    let fx = Fx::new("recorded");
    fx.prove();
    let programs = fx.latest_prove().data["programs"].clone();
    let programs = programs.as_object().expect("programs is not an object");
    assert!(programs.contains_key("grep"), "grep is not recorded: {programs:?}");
    assert!(programs["grep"].get("path").and_then(Value::as_str).is_some(), "no path for grep: {programs:?}");
    assert!(programs["grep"].get("sha256").and_then(Value::as_str).is_some(), "no hash for grep: {programs:?}");
    assert!(!programs.contains_key("echo"), "a shell builtin must be absent: {programs:?}");
}

#[test]
fn programs_a_tool_shim_on_path_is_caught() {
    let fx = Fx::build("shim", "grep -q ok out.txt && cargo && echo 'test result: ok. 2 passed; 0 failed;'", "ok", "out.txt, src/**");
    let shim = fx.root.join("shim");
    fx.shim(&shim, "cargo", "#!/bin/sh\nexit 0\n");
    let out = fx.rvow_path(&["prove", &fx.task], &shim);
    assert!(out.contains("proved"), "{out}");
    fx.reset_runs();
    let gate = fx.rvow(&["gate", "--rerun"]);
    assert!(gate.contains(&fx.task), "no warning names the task: {gate}");
    assert!(gate.contains("cargo"), "the warning does not name the program: {gate}");
    assert_eq!(fx.runs(), 1, "the shim proof was trusted: {gate}");
}

#[test]
fn programs_a_builtin_stays_trusted() {
    let fx = Fx::build("builtin", "test -f out.txt && cargo && echo 'test result: ok. 2 passed; 0 failed;'", "ok", "out.txt, src/**");
    let shim = fx.root.join("shim");
    fx.shim(&shim, "cargo", "#!/bin/sh\nexit 0\n");
    let out = fx.rvow_path(&["prove", &fx.task], &shim);
    assert!(out.contains("proved"), "{out}");
    fx.reset_runs();
    let gate = fx.rvow_path(&["gate", "--rerun"], &shim);
    assert!(gate.contains("passed: true"), "{gate}");
    assert!(gate.contains("proven"), "the builtin proof was not trusted: {gate}");
    assert_eq!(fx.runs(), 0, "a trusted task ran: {gate}");
}

// REQ-8a9jz9: an ignored file in a task's scope makes the gate rerun it.

#[test]
fn ignored_a_file_in_scope_reruns_the_task() {
    let fx = Fx::new("ignored");
    fx.commit(".gitignore", "src/local.env\n");
    fx.prove();
    std::fs::write(fx.repo.join("src/local.env"), "SECRET=1").unwrap();
    let gate = fx.rvow(&["gate", "--rerun"]);
    assert!(gate.contains("passed: true"), "{gate}");
    assert_eq!(fx.runs(), 1, "an ignored file in scope was trusted: {gate}");
}

#[test]
fn ignored_a_linked_folder_keeps_the_trust() {
    let fx = Fx::build("linked", PASS_CHECK, "ok", "out.txt, extensions/**");
    std::fs::create_dir_all(fx.repo.join("extensions")).unwrap();
    fx.commit(".gitignore", "extensions/node_modules/\n");
    fx.commit("extensions/keep.txt", "keep");
    fx.config("rerun_links = [\"extensions/node_modules\"]\ncache_with_links = true\n");
    fx.prove();
    std::fs::create_dir_all(fx.repo.join("extensions/node_modules")).unwrap();
    std::fs::write(fx.repo.join("extensions/node_modules/dep.txt"), "dep").unwrap();
    let gate = fx.rvow(&["gate", "--rerun"]);
    assert!(gate.contains("passed: true"), "{gate}");
    assert!(gate.contains("proven"), "a linked folder did not keep the trust: {gate}");
    assert_eq!(fx.runs(), 0, "a trusted task ran: {gate}");
}

#[test]
fn seal_trust_a_respaced_copy_of_a_sealed_line_is_rerun() {
    let fx = Fx::new("respaced");
    fx.prove();
    let pass = fx.prove_line();
    // A failed proof, older than the pass and with another key, so it is not
    // the newest proof and it does not make the task flaky.
    let mut failed: Value = serde_json::from_str(&pass).unwrap();
    failed["proved"] = false.into();
    failed["key"] = "forged".into();
    failed["time"] = (failed["time"].as_u64().unwrap() - 1000).into();
    failed.as_object_mut().unwrap().remove("seal");
    fx.append_raw(&failed.to_string());
    // The same sealed pass line again with added whitespace. It parses to the
    // same event and its seal still checks, but its parents are stale. The
    // copy must be the newest proof, and equal timestamps break by line hash,
    // so try whitespace until the copy wins that tie. A high pass hash needs
    // many tries: the bits of n pick a space or a tab for each position.
    let pass_hash = rvow_core::ledger::short_hash(pass.as_bytes());
    let copy = (1u32..=1_000_000)
        .map(|n| {
            let tail: String = (0..=n.ilog2()).map(|bit| if n >> bit & 1 == 1 { '\t' } else { ' ' }).collect();
            format!("{pass}{tail}")
        })
        .find(|line| rvow_core::ledger::short_hash(line.as_bytes()) > pass_hash)
        .expect("no whitespace variant made the copy the newest proof");
    fx.append_raw(&copy);
    fx.reset_runs();
    let gate = fx.rvow(&["gate", "--rerun"]);
    assert_eq!(fx.runs(), 1, "the respaced copy was trusted: {gate}");
}
