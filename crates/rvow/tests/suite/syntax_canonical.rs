//! The canonical syntax form: a file whose only edit removes a blank line or
//! a comment keeps the form, a changed token does not, and a parse error has
//! no form. One test per spec scenario, offline through the fixture folder.

use std::path::{Path, PathBuf};

use rvow_core::syntax::canonical::canonical;
use rvow_core::syntax::grammar;

struct Home(PathBuf);

impl Drop for Home {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

impl Home {
    fn new(name: &str) -> Home {
        let dir = std::env::temp_dir().join(format!("rvow-canonical-h-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        Home(dir.canonicalize().unwrap())
    }
}

fn fixtures() -> String {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/grammars").display().to_string()
}

fn form(home: &Home, name: &str, text: &str) -> Option<String> {
    let entry = grammar::by_name(name).unwrap();
    let mut session = grammar::Session::new().unwrap();
    canonical(&mut session, &home.0, entry, Some(&fixtures()), text).unwrap()
}

#[test]
fn syntax_canonical_blank_line_removed_keeps_the_form() {
    let home = Home::new("blank");
    let before = "{\n  \"a\": 1\n}\n";
    let after = "{\n\n  \"a\": 1\n}\n";
    assert_eq!(form(&home, "json", before), form(&home, "json", after));
}

#[test]
fn syntax_canonical_kept_comment_added_directive_changes_the_form() {
    let home = Home::new("kept-added");
    let before = "[t]\nk = 1\n";
    let after = "[t]\nk = 1 # noqa\n";
    assert_ne!(form(&home, "toml", before), form(&home, "toml", after));
}

#[test]
fn syntax_canonical_kept_comment_respaced_keeps_the_form() {
    let home = Home::new("kept-space");
    let before = "[t]\nk = 1 # noqa\n";
    let after = "[t]\nk = 1   #   noqa\n";
    assert_eq!(form(&home, "toml", before), form(&home, "toml", after));
}

#[test]
fn syntax_canonical_kept_comment_blank_line_removed_keeps_the_form() {
    let home = Home::new("kept-blank");
    let before = "[t]\nk = 1\n\n[t2]\n";
    let after = "[t]\nk = 1\n[t2]\n";
    assert_eq!(form(&home, "toml", before), form(&home, "toml", after));
}

#[test]
fn syntax_canonical_comment_removed_changes_the_form() {
    let home = Home::new("comment");
    let before = "# head\n[t]\nk = 1 # trailing\n\n[t2]\n";
    let after = "[t]\nk = 1\n\n[t2]\n";
    assert_ne!(form(&home, "toml", before), form(&home, "toml", after));
}

#[test]
fn syntax_canonical_changed_token_changes_the_form() {
    let home = Home::new("token");
    assert_ne!(form(&home, "json", "{\"assert\": \"a\"}"), form(&home, "json", "{\"assert\": \"b\"}"));
    assert_ne!(form(&home, "json", "{\"n\": 1}"), form(&home, "json", "{\"n\": 2}"));
}

#[test]
fn syntax_canonical_parse_error_has_no_form() {
    let home = Home::new("error");
    assert_eq!(form(&home, "json", "{,}"), None);
}

#[test]
fn syntax_canonical_form_names_kinds_and_token_text() {
    let home = Home::new("shape");
    let form_shape = form(&home, "json", "[1]").unwrap();
    assert_eq!(form_shape, "(document(array([[1][)(number[1]1)(][1]])))");
}
