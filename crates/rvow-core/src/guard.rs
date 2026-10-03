//! Tests that a passing proof relied on. A proof saves the test files in the
//! task's scope and the `#[cfg(test)]` modules of its Rust files; later edits
//! that remove or change a saved line are weaken drift, while added lines stay
//! free, so a later task can add tests but not weaken earlier ones.

use std::collections::{BTreeMap, HashSet};
use std::path::Path;

use serde_json::{Map, Value};

use crate::config::Config;
use crate::drift::{Class, Item};
use crate::error::Result;
use crate::fingerprint::{candidate_files, globset};
use crate::ledger::Kind;
use crate::proof::Active;
use crate::store::Project;
use crate::tasks::Task;
use crate::vow::put_object;

/// The key suffix for the test modules of a Rust file.
pub const RUST_TESTS: &str = "#cfg(test)";

/// The `#[cfg(test)]` modules of a Rust source, joined. Braces are counted
/// naively, so a brace inside a string can end a module early; that only makes
/// the saved text shorter, never wider.
pub fn rust_test_modules(src: &str) -> String {
    let lines: Vec<&str> = src.lines().collect();
    let mut out = String::new();
    let mut i = 0;
    while i < lines.len() {
        if lines[i].trim() != "#[cfg(test)]" {
            i += 1;
            continue;
        }
        let mut depth = 0i64;
        let mut opened = false;
        while i < lines.len() {
            let line = lines[i];
            out.push_str(line);
            out.push('\n');
            depth += line.matches('{').count() as i64 - line.matches('}').count() as i64;
            opened |= line.contains('{');
            i += 1;
            if opened && depth <= 0 {
                break;
            }
        }
    }
    out
}

/// Saves the guarded text for `task` and returns key to object hash. Keys are
/// test file paths, and `<path>#cfg(test)` for Rust test modules.
pub fn capture(project: &Project, top: &Path, config: &Config, task: &Task) -> Result<Map<String, Value>> {
    let scope = globset(&task.files)?;
    let tests: Vec<String> = config.test_globs.iter().chain(&config.protect_infra).cloned().collect();
    let tests = globset(&tests)?;
    let mut out = Map::new();
    for rel in candidate_files(top)?.into_iter().filter(|f| scope.is_match(f)) {
        let Ok(text) = std::fs::read_to_string(top.join(&rel)) else {
            continue;
        };
        if tests.is_match(&rel) {
            out.insert(rel.clone(), put_object(project, text.as_bytes())?.into());
        } else if rel.ends_with(".rs") {
            let modules = rust_test_modules(&text);
            if !modules.is_empty() {
                out.insert(format!("{rel}{RUST_TESTS}"), put_object(project, modules.as_bytes())?.into());
            }
        }
    }
    Ok(out)
}

/// The guarded text since the change's baseline: for each key, the version
/// saved by the first passing proof after the latest accepted amendment (or the
/// vow).
pub fn guarded(change: &Active) -> BTreeMap<String, String> {
    let start = change
        .events
        .iter()
        .rposition(|e| e.kind == Kind::Vow || (e.kind == Kind::Confirm && e.data.get("decision").and_then(Value::as_str) == Some("accept")))
        .unwrap_or(0);
    let mut out = BTreeMap::new();
    for e in &change.events[start..] {
        if e.kind != Kind::Prove || e.data.get("proved").and_then(Value::as_bool) != Some(true) {
            continue;
        }
        if let Some(map) = e.data.get("tests").and_then(Value::as_object) {
            for (k, v) in map {
                if let Some(h) = v.as_str() {
                    out.entry(k.clone()).or_insert_with(|| h.to_string());
                }
            }
        }
    }
    out
}

fn current(top: &Path, key: &str) -> Option<String> {
    match key.strip_suffix(RUST_TESTS) {
        Some(rel) => std::fs::read_to_string(top.join(rel)).ok().map(|t| rust_test_modules(&t)),
        None => std::fs::read_to_string(top.join(key)).ok(),
    }
}

/// Weaken drift for every guarded text that lost or changed a saved line.
pub fn drift(project: &Project, top: &Path, change: &Active) -> Vec<Item> {
    let mut out = Vec::new();
    for (key, hash) in guarded(change) {
        let Ok(before) = std::fs::read_to_string(project.root.join("objects").join(&hash)) else {
            continue;
        };
        let what = |how: &str| format!("test {key} {how} after its first passing proof");
        let Some(now) = current(top, &key) else {
            out.push(Item { class: Class::Weaken, what: what("was removed"), before: None, after: None });
            continue;
        };
        let kept: HashSet<&str> = now.lines().collect();
        let lost: Vec<&str> = before.lines().filter(|l| !kept.contains(l)).collect();
        if !lost.is_empty() {
            out.push(Item { class: Class::Weaken, what: what("lost or changed lines"), before: Some(lost.join("\n")), after: None });
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn setup() -> (crate::git::testrepo::TempDir, Project, std::path::PathBuf, String) {
        use crate::git::testrepo::*;
        let tmp = TempDir::new();
        let home = tmp.0.join("home");
        let repo = repo_with_commit(&tmp.0, "r");
        let p = crate::store::init(&repo, &home, crate::store::Mode::Home).unwrap();
        let c = crate::change::new(&p, "T", "main", true).unwrap();
        std::fs::write(
            c.path.join("tasks.md"),
            "- TSK-new1 T\n  verify: sh tests/t.sh && echo 'test result: ok. 1 passed; 0 failed;'\n  files: tests/**, src/**\n",
        )
        .unwrap();
        let prepared = crate::vow::prepare(&p, &home, c, &repo, &mut std::io::sink()).unwrap();
        let task = crate::vow::commit(&p, prepared).unwrap().minted[0].1.to_string();
        std::fs::create_dir_all(repo.join("tests")).unwrap();
        std::fs::create_dir_all(repo.join("src")).unwrap();
        std::fs::write(repo.join("tests/t.sh"), "check_one\ncheck_two\nexit 0\n").unwrap();
        std::fs::write(
            repo.join("src/lib.rs"),
            "pub fn f() {}\n#[cfg(test)]\nmod tests {\n    #[test]\n    fn t() { assert_eq!(1, 1); }\n}\n",
        )
        .unwrap();
        let r = crate::proof::prove(&p, &home, &repo, &task, &mut std::io::sink()).unwrap();
        assert!(r.proved, "{:?}", r.outcome);
        (tmp, p, repo, task)
    }

    fn weaken(p: &Project, repo: &Path) -> Vec<String> {
        let (living, _) = crate::change::load_living(p).unwrap();
        let changes = crate::proof::active(p, &living).unwrap();
        drift(p, repo, &changes[0]).into_iter().map(|i| i.what).collect()
    }

    #[test]
    fn additions_stay_free_and_removals_are_weaken() {
        let (_tmp, p, repo, _) = setup();
        assert_eq!(weaken(&p, &repo), Vec::<String>::new());
        std::fs::write(repo.join("tests/t.sh"), "check_one\ncheck_two\ncheck_three\nexit 0\n").unwrap();
        assert_eq!(weaken(&p, &repo), Vec::<String>::new());
        std::fs::write(repo.join("tests/t.sh"), "check_one\nexit 0\n").unwrap();
        assert_eq!(weaken(&p, &repo), ["test tests/t.sh lost or changed lines after its first passing proof"]);

        let lib = std::fs::read_to_string(repo.join("src/lib.rs")).unwrap();
        std::fs::write(repo.join("src/lib.rs"), lib.replace("assert_eq!(1, 1)", "assert!(true)")).unwrap();
        assert_eq!(weaken(&p, &repo).len(), 2);
        let lib = std::fs::read_to_string(repo.join("src/lib.rs")).unwrap();
        std::fs::write(repo.join("src/lib.rs"), lib.replace("pub fn f() {}", "pub fn f() { let _ = 1; }")).unwrap();
        assert_eq!(weaken(&p, &repo).len(), 2, "code outside test modules is free");
    }

    #[test]
    fn the_hook_protects_tests_after_their_first_proof() {
        let (_tmp, p, repo, _) = setup();
        let input = serde_json::json!({
            "cwd": repo.to_str().unwrap(),
            "tool_name": "Edit",
            "tool_input": {"file_path": "tests/t.sh"},
        });
        assert!(crate::hook::claude_pre_tool_use(&p, &input).unwrap().is_some());
        let src = serde_json::json!({
            "cwd": repo.to_str().unwrap(),
            "tool_name": "Edit",
            "tool_input": {"file_path": "src/lib.rs"},
        });
        assert_eq!(crate::hook::claude_pre_tool_use(&p, &src).unwrap(), None);
    }

    #[test]
    fn extracts_rust_test_modules() {
        let src = "fn a() {}\n\n#[cfg(test)]\nmod tests {\n    #[test]\n    fn t() { assert!(true); }\n}\n\nfn b() {}\n";
        assert_eq!(rust_test_modules(src), "#[cfg(test)]\nmod tests {\n    #[test]\n    fn t() { assert!(true); }\n}\n");
        assert_eq!(rust_test_modules("fn a() {}\n"), "");
    }
}
