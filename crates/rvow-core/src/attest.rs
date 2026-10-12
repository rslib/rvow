//! `rvow attest`: export the current proofs of a change as in-toto
//! attestations, one JSON statement per line.

use std::path::{Path, PathBuf};

use serde_json::{Value, json};

use crate::amend;
use crate::change;
use crate::config;
use crate::drift;
use crate::error::{Error, Result};
use crate::fingerprint;
use crate::git;
use crate::id;
use crate::ledger::Kind;
use crate::proof;
use crate::store::Project;
use crate::vow::sha256_hex;

/// The `_type` of every line `rvow attest` writes.
pub const STATEMENT: &str = "https://in-toto.io/Statement/v1";
/// The predicate of one task's line.
pub const TEST_RESULT: &str = "https://in-toto.io/attestation/test-result/v0.1";
/// The predicate of the last line, which sums up the change.
pub const CHANGE_SUMMARY: &str = "https://github.com/rslib/rvow/attestation/change/v0.1";

pub struct Attested {
    pub lines: usize,
    pub signed: bool,
}

fn no_change(input: &str) -> Error {
    Error::new("change", format!("{input}: no change with that id"), "run rvow ls to see the changes")
}

fn find_change(project: &Project, input: &str) -> Result<change::ChangeDir> {
    let (dirs, _) = change::list(project)?;
    let typed = if input.contains('-') { input.to_owned() } else { format!("CHG-{input}") };
    let resolved = id::resolve(&typed, dirs.iter().map(|d| &d.id)).map_err(|_| no_change(input))?;
    dirs.into_iter().find(|d| d.id == resolved).ok_or_else(|| no_change(input))
}

fn last_prove<'a>(events: &'a [crate::ledger::Event], task: &str) -> Option<&'a crate::ledger::Event> {
    events.iter().rfind(|e| e.kind == Kind::Prove && e.data.get("task").and_then(Value::as_str) == Some(task))
}

/// Each tracked file in the task's scope that exists, with its sha256.
fn subjects(top: &Path, files: &[String]) -> Result<Vec<Value>> {
    let set = fingerprint::globset(files)?;
    let tracked = git::run(top, &["ls-files"])?;
    let mut out = Vec::new();
    for name in tracked.lines().filter(|f| set.is_match(f)) {
        let path = top.join(name);
        if !path.is_file() {
            continue;
        }
        let bytes = std::fs::read(&path).map_err(|e| Error::io(&path, e))?;
        out.push(json!({"name": name, "digest": {"sha256": sha256_hex(&bytes)}}));
    }
    Ok(out)
}

fn statement(task: &str, verify: &str, tests: u64, result: &str, subjects: Vec<Value>) -> Value {
    let subjects =
        if subjects.is_empty() { vec![json!({"name": task, "digest": {"sha256": sha256_hex(verify.as_bytes())}})] } else { subjects };
    json!({
        "_type": STATEMENT,
        "subject": subjects,
        "predicateType": TEST_RESULT,
        "predicate": {
            "result": result,
            "configuration": [{
                "name": "rvow verify",
                "digest": {"sha256": sha256_hex(verify.as_bytes())},
                "annotations": {"task": task, "verify": verify, "tests": tests},
            }],
            "passedTests": [],
            "failedTests": [],
        },
    })
}

fn summary(id: &str, vow: &str, confirm: Option<&str>, gate: &str, unproved: &[String]) -> Value {
    json!({
        "_type": STATEMENT,
        "subject": [{"name": format!("{id} vow"), "digest": {"sha256": sha256_hex(vow.as_bytes())}}],
        "predicateType": CHANGE_SUMMARY,
        "predicate": {
            "change": id,
            "vow": vow,
            "confirm": confirm.map_or(Value::Null, |h| Value::String(h.to_owned())),
            "gate": gate,
            "unproved": unproved,
        },
    })
}

/// Writes one Test Result statement per task with a current proof, then one
/// summary statement, as JSON lines. The file appears only after every line
/// is built. With `sign`, the configured `signing_key` signs the file in the
/// `rvow` namespace and the signature lands next to it with `.sig` added.
pub fn attest(project: &Project, home: &Path, cwd: &Path, input: &str, out: &Path, sign: bool) -> Result<Attested> {
    let dir = find_change(project, input)?;
    let top = git::toplevel(cwd)?;
    let (living, _) = change::load_living(project)?;
    let loaded = change::load(&dir.path, &living)?;
    let (events, _) = crate::ledger::read(&dir.path.join(change::LEDGER))?;
    let config = config::load(project, home)?;
    if sign && config.signing_key.is_none() {
        return Err(Error::new(
            "sign",
            "attest --sign needs a signing key but the config sets none",
            "set signing_key in the global config to the path of an ssh signing key",
        ));
    }
    let active = proof::Active { dir, loaded, events, ledger_diags: Vec::new() };
    let now = proof::Now { top: &top, config: &config, env: fingerprint::env(&top, &config)?, living: &living };
    let mut lines = Vec::new();
    let mut unproved = Vec::new();
    for task in &active.loaded.tasks {
        let state = proof::state(&active, task, &now)?;
        let result = match state {
            proof::State::Proved => "PASSED",
            proof::State::Failed => "FAILED",
            _ => {
                unproved.push(task.id.to_string());
                continue;
            }
        };
        let id = task.id.to_string();
        let event = last_prove(&active.events, &id);
        let verify = event.and_then(|e| e.data.get("verify")).and_then(Value::as_str).or(task.verify.as_deref()).unwrap_or("");
        let tests = event.and_then(|e| e.data.get("tests_run")).and_then(Value::as_u64).unwrap_or(0);
        lines.push(statement(&id, verify, tests, result, subjects(&top, &task.files)?));
    }
    let blocking = drift::drift(project, &top, &config, &active)?.into_iter().any(|i| i.class.blocks());
    let pending = !amend::pending(std::slice::from_ref(&active)).is_empty();
    let all_proved = unproved.is_empty();
    let gate = if all_proved && !blocking && !pending { "pass" } else { "fail" };
    let vow = active.vow().map(|e| e.hash.as_str()).unwrap_or("");
    let confirm = active.events.iter().rfind(|e| e.kind == Kind::Confirm).map(|e| e.hash.as_str());
    lines.push(summary(&active.dir.id.to_string(), vow, confirm, gate, &unproved));
    let mut text = String::new();
    for line in &lines {
        text.push_str(&serde_json::to_string(line).map_err(|e| Error::internal(format!("attestation does not serialize: {e}")))?);
        text.push('\n');
    }
    let signature = if sign {
        let key = config.signing_key.as_deref().unwrap_or("");
        Some(crate::gate::sign(Path::new(key), &text)?)
    } else {
        None
    };
    std::fs::write(out, &text).map_err(|e| Error::io(out, e))?;
    if let Some(signature) = signature {
        let path = PathBuf::from(format!("{}.sig", out.display()));
        std::fs::write(&path, signature).map_err(|e| Error::io(&path, e))?;
    }
    Ok(Attested { lines: lines.len(), signed: sign })
}
