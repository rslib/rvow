//! The `rvow grammar` command, offline through the fixture folder: fetch
//! fills the checked cache, list shows each pinned language and its cache
//! state, and a missing or wrong download fails without cache writes.

use std::path::{Path, PathBuf};
use std::process::Command;

use rvow_core::vow::sha256_hex;

struct Home(PathBuf);

impl Drop for Home {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

impl Home {
    fn new(name: &str) -> Home {
        let dir = std::env::temp_dir().join(format!("rvow-syntax-cli-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("repo")).unwrap();
        Home(dir.canonicalize().unwrap())
    }

    fn cmd(&self, program: &str) -> Command {
        let mut c = Command::new(program);
        c.current_dir(self.0.join("repo"))
            .env("RVOW_HOME", self.0.join("home"))
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_NOSYSTEM", "1");
        c
    }

    fn rvow(&self, args: &[&str], base: &str) -> (String, i32) {
        let out = self.cmd(env!("CARGO_BIN_EXE_rvow")).args(args).env("RVOW_GRAMMAR_URL", base).output().unwrap();
        (String::from_utf8(out.stdout).unwrap(), out.status.code().unwrap())
    }

    fn ok(&self, args: &[&str], base: &str) -> String {
        let (out, code) = self.rvow(args, base);
        assert_eq!(code, 0, "rvow {args:?}: {out}");
        out
    }

    fn cache(&self, name: &str) -> PathBuf {
        self.0.join("home").join("grammars").join(format!("tree-sitter-{name}.wasm"))
    }
}

fn base() -> String {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/grammars").display().to_string()
}

fn row(out: &str, lang: &str) -> String {
    out.lines().find_map(|l| l.strip_prefix(&format!("  {lang},"))).unwrap_or_else(|| panic!("no {lang} row in {out}")).to_string()
}

#[test]
fn syntax_cli_fetch_caches_and_list_shows_cached() {
    let h = Home::new("fetch");
    let pinned = rvow_core::syntax::grammar::by_name("json").unwrap().sha256.clone();
    let out = h.ok(&["grammar", "fetch", "json", "--toon"], &base());
    assert!(out.contains("json,fetched"), "{out}");
    let file = h.cache("json");
    assert!(file.is_file(), "{}", file.display());
    assert_eq!(sha256_hex(&std::fs::read(&file).unwrap()), pinned);
    let list = h.ok(&["grammar", "list", "--toon"], &base());
    let row_json = row(&list, "json");
    assert!(row_json.contains(",cached,"), "{row_json}");
    assert!(!row(&list, "toml").contains("cached"), "{list}");
}

#[test]
fn syntax_cli_fetch_again_reports_cached() {
    let h = Home::new("again");
    let _ = h.ok(&["grammar", "fetch", "json", "--toon"], &base());
    let out = h.ok(&["grammar", "fetch", "json", "--toon"], &base());
    assert!(out.contains("json,cached"), "{out}");
    assert!(!out.contains("fetched"), "{out}");
}

#[test]
fn syntax_cli_unknown_language_is_refused() {
    let h = Home::new("unknown");
    let (out, code) = h.rvow(&["grammar", "fetch", "norwegian"], &base());
    assert_eq!(code, 1);
    assert!(out.contains("no pinned grammar named norwegian"), "{out}");
    assert!(out.contains("run rvow grammar list"), "{out}");
    assert!(!h.cache("norwegian").is_file());
}

#[test]
fn syntax_cli_missing_download_is_refused_without_cache() {
    let empty = crate::temp::TempDir::new("syntax-cli-empty");
    let h = Home::new("offline");
    let (out, code) = h.rvow(&["grammar", "fetch", "json", "--toon"], empty.path().to_str().unwrap());
    assert_eq!(code, 1);
    assert!(out.contains("grammar is missing"), "{out}");
    assert!(!h.cache("json").is_file());
    let list = h.ok(&["grammar", "list", "--toon"], &base());
    assert!(!row(&list, "json").contains("cached"), "{list}");
}

#[test]
fn syntax_cli_fetch_takes_only_the_named_targets() {
    let h = Home::new("two");
    let out = h.ok(&["grammar", "fetch", "json", "toml", "--toon"], &base());
    assert!(out.contains("json,fetched"), "{out}");
    assert!(out.contains("toml,fetched"), "{out}");
    let list = h.ok(&["grammar", "list", "--toon"], &base());
    assert!(row(&list, "json").contains("cached"), "{list}");
    assert!(row(&list, "toml").contains("cached"), "{list}");
    assert!(row(&list, "bash").contains("missing"), "{list}");
}
