//! The grammar cache: a pinned entry is fetched once, checked by sha256 and
//! reused from the cache, and a download with the wrong hash is refused and
//! never cached. One test per spec scenario, offline through the fixture
//! folder and the download base override.

use std::path::{Path, PathBuf};

use rvow_core::syntax::grammar;
use rvow_core::vow::sha256_hex;

struct Home(PathBuf);

impl Drop for Home {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

impl Home {
    fn new(name: &str) -> Home {
        let dir = std::env::temp_dir().join(format!("rvow-grammar-h-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        Home(dir.canonicalize().unwrap())
    }

    fn cache(&self, entry: &grammar::Entry) -> PathBuf {
        grammar::cache_file(&self.0, entry)
    }
}

fn base() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests").join("fixtures").join("grammars")
}

fn base_str() -> String {
    base().display().to_string()
}

fn json_entry() -> &'static grammar::Entry {
    grammar::by_ext("json").unwrap()
}

#[test]
fn syntax_grammar_first_use_caches_the_pinned_hash() {
    let home = Home::new("first");
    let entry = json_entry();
    let bytes = grammar::acquire(&home.0, entry, Some(&base_str())).unwrap();
    assert_eq!(sha256_hex(&bytes), entry.sha256);
    let file = home.cache(entry);
    assert!(file.is_file(), "{}", file.display());
    assert_eq!(sha256_hex(&std::fs::read(file).unwrap()), entry.sha256);
}

#[test]
fn syntax_grammar_cache_wins_a_matching_file() {
    let home = Home::new("cache");
    let entry = json_entry();
    let file = home.cache(entry);
    std::fs::create_dir_all(file.parent().unwrap()).unwrap();
    std::fs::write(&file, b"stale bytes").unwrap();
    // A cache file with a stale hash must be replaced, not served.
    let bytes = grammar::acquire(&home.0, entry, Some(&base_str())).unwrap();
    assert_eq!(sha256_hex(&bytes), entry.sha256);

    let file = home.cache(entry);
    let fixture = std::fs::read(base().join("tree-sitter-json.wasm")).unwrap();
    std::fs::write(&file, &fixture).unwrap();
    // A matching cache file wins without touching the base again.
    let bytes = grammar::acquire(&home.0, entry, Some("file:///no-such-base/.")).unwrap();
    assert_eq!(sha256_hex(&bytes), entry.sha256);
}

#[test]
fn syntax_grammar_refuses_a_wrong_download() {
    let home = Home::new("wrong");
    let entry = json_entry();
    let dir = std::env::temp_dir().join(format!("rvow-grammar-bad-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("tree-sitter-json.wasm"), b"not a tree-sitter wasm module").unwrap();
    let err = grammar::acquire(&home.0, entry, Some(dir.to_str().unwrap())).unwrap_err();
    assert_eq!(err.code, "grammar");
    assert!(err.message.contains("is missing"), "{}", err.message);
    assert!(err.message.contains("in place of the pinned"), "{}", err.message);
    assert!(!home.cache(entry).is_file(), "a wrong download was cached");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn syntax_grammar_missing_download_reports_missing() {
    let home = Home::new("nogone");
    let entry = json_entry();
    let err = grammar::acquire(&home.0, entry, Some("/nonexistent/rvow-grammars")).unwrap_err();
    assert_eq!(err.code, "grammar");
    assert!(err.message.contains("is missing: "), "{}", err.message);
    assert!(err.fix.contains("rvow grammar fetch"), "{}", err.fix);
    assert!(!home.cache(entry).is_file());
}

#[test]
fn syntax_grammar_unknown_language_is_refused() {
    assert!(grammar::by_name("brainless").is_none());
}

#[test]
fn syntax_grammar_session_parses_offline() {
    let home = Home::new("session");
    let entry = json_entry();
    let mut session = grammar::Session::new().unwrap();
    let tree = session.parse(&home.0, entry, Some(&base_str()), "{\"a\": 1}").unwrap();
    assert_eq!(tree.root_node().kind(), "document");
    assert!(!tree.root_node().has_error());
    // The grammar is in the cache after a session parse.
    assert!(home.cache(entry).is_file());
    assert_eq!(sha256_hex(&std::fs::read(home.cache(entry)).unwrap()), entry.sha256);
}
