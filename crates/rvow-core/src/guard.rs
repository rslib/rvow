//! Tests that a passing proof relied on. A proof saves the test files in the
//! task's scope and the `#[cfg(test)]` modules of its Rust files; later edits
//! that change protected order, multiplicity or lexical context are weaken
//! drift. New Rust test items and standalone non-Rust checks stay free.
//! Existing Rust items stay exact at the token level. This is conservative
//! lexical protection, not a claim of semantic equivalence.

use std::collections::BTreeMap;

#[path = "guard_rust.rs"]
mod rust;
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

/// The `#[cfg(test)]` modules of a Rust source, joined. Unrecognized or
/// unterminated lexical context protects the whole source rather than truncating it.
pub fn rust_test_modules(src: &str) -> String {
    rust::modules(src).unwrap_or_else(|| src.to_string())
}

/// The names of `#[cfg(test)] mod <name>;` submodule includes in a Rust source.
/// These pull a separate test file into the crate under test.
pub fn rust_test_submodules(src: &str) -> Vec<String> {
    rust::test_submodules(src)
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
            // A `#[cfg(test)] mod x;` include pulls a separate test file into the
            // crate under test. Guard that file the same way we guard a test file,
            // whether or not it matches a test glob. Inline test modules are already
            // covered above; the pointed-at file is what bodyless includes actually test.
            let dir = Path::new(&rel).parent().map(|p| p.to_path_buf()).unwrap_or_default();
            for name in rust_test_submodules(&text) {
                for candidate in [dir.join(format!("{name}.rs")), dir.join(name).join("mod.rs")] {
                    let key = candidate.to_string_lossy().into_owned();
                    if !out.contains_key(&key)
                        && let Ok(sub) = std::fs::read_to_string(top.join(&candidate))
                    {
                        out.insert(key, put_object(project, sub.as_bytes())?.into());
                        break;
                    }
                }
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
        Some(rel) => std::fs::read_to_string(top.join(rel)).ok().and_then(|t| rust::modules(&t)),
        None => std::fs::read_to_string(top.join(key)).ok(),
    }
}

fn added_lines_safe(before: &str, now: &str) -> bool {
    let mut old = before.lines().peekable();
    now.lines().all(|line| {
        if old.peek().is_some_and(|saved| *saved == line) {
            old.next();
            true
        } else {
            // Without a parser for this language, only standalone added checks are free.
            let line = line.trim();
            line.is_empty()
                || ((line.starts_with("check_") || line.starts_with("assert"))
                    && !line.contains(['{', '}', '#', '/', '\\', '\'', '"', ';', '&', '|', '<', '>', '(', ')', '$', '`']))
        }
    }) && old.next().is_none()
}

/// Weaken drift for saved content whose order, multiplicity or lexical context changed.
pub fn drift(project: &Project, top: &Path, change: &Active) -> Vec<Item> {
    let mut out = Vec::new();
    for (key, hash) in guarded(change) {
        let what = |how: &str| format!("test {key} {how} after its first passing proof");
        let before = std::fs::read_to_string(project.root.join("objects").join(&hash));
        let before = match before {
            Ok(text) if crate::vow::sha256_hex(text.as_bytes()) == hash => text,
            _ => {
                out.push(Item { class: Class::Weaken, what: what("has a missing or corrupt saved object"), before: None, after: None });
                continue;
            }
        };
        let Some(now) = current(top, &key) else {
            out.push(Item { class: Class::Weaken, what: what("was removed"), before: None, after: None });
            continue;
        };
        let preserved = if key.ends_with(RUST_TESTS) || key.ends_with(".rs") {
            rust::preserved(&before, &now)
        } else {
            let mut remaining = now.lines();
            before.lines().all(|line| remaining.by_ref().any(|candidate| candidate == line)) && added_lines_safe(&before, &now)
        };
        if !preserved {
            out.push(Item { class: Class::Weaken, what: what("lost or changed lines"), before: Some(before), after: None });
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

    #[test]
    fn gated_use_and_bodyless_mod_do_not_break_test_module_extraction() {
        // A #[cfg(test)] on a `use`, a `fn` or a bodyless `mod x;` submodule
        // include is not a guardable test module. The lexer must skip these
        // instead of failing, or a real inline test module later in the file
        // would be reported as removed.
        let src = "#[cfg(test)]\nuse crate::change::DELTAS;\n\n#[cfg(test)]\nmod helper;\n\n#[cfg(test)]\nmod tests {\n    #[test]\n    fn t() { assert!(true); }\n}\n";
        assert_eq!(rust_test_modules(src), "#[cfg(test)]\nmod tests {\n    #[test]\n    fn t() { assert!(true); }\n}\n");
        // pub(crate) visibility before the module name must be accepted.
        let vis = "#[cfg(test)]\npub(crate) mod tests {\n    #[test]\n    fn t() { assert!(true); }\n}\n";
        assert_eq!(rust_test_modules(vis), "#[cfg(test)]\npub(crate) mod tests {\n    #[test]\n    fn t() { assert!(true); }\n}\n");
        // A file with only gated non-test items has no extractable test module.
        assert_eq!(rust_test_modules("#[cfg(test)]\nuse crate::x;\n"), "");
    }

    #[test]
    fn bodyless_cfg_test_mod_includes_are_listed_for_guarding() {
        let src = "#[cfg(test)]\nmod harness;\n\n#[cfg(test)]\nmod tests {\n    #[test]\n    fn t() { assert!(true); }\n}\n";
        assert_eq!(rust_test_submodules(src), ["harness"]);
        // A bodyed module, a `use` or a plain `mod x;` without cfg(test) is not listed.
        assert_eq!(
            rust_test_submodules("#[cfg(test)]\npub(crate) mod tests { fn t(){} }\n#[cfg(test)]\nuse crate::y;\nmod plain;\n"),
            Vec::<String>::new()
        );
        // No cfg(test) attributes at all.
        assert_eq!(rust_test_submodules("mod plain;\n"), Vec::<String>::new());
    }
}
