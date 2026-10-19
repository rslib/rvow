//! The push hook and CI run each check that both have with the same flags,
//! as AGENTS.md says, apart from a short list of flags only one side needs.

use std::collections::BTreeSet;
use std::path::Path;

/// The checks that both the hook and CI run, each named by the words that start it.
const SHARED: &[&str] = &[
    "cargo fmt",
    "cargo clippy",
    "cargo nextest run",
    "run lint",
    "run drift",
    "run typecheck:pi",
    "run test:pi",
    "run typecheck:claude",
    "run validate:claude",
    "run test:claude",
];

/// Flags only one side needs: CI keeps going and runs the ignored pi audit tests,
/// and the hook prints only failures.
const ONE_SIDED: &[&str] = &["--no-fail-fast", "--run-ignored", "all", "--status-level", "fail"];

fn read(rel: &str) -> String {
    std::fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join("../..").join(rel)).unwrap()
}

/// The words of the command that starts with `check` in `line`, up to the end of that command.
fn command(line: &str, check: &str) -> Option<BTreeSet<String>> {
    let at = line.find(check)?;
    let rest = &line[at..];
    let end = rest.find(['\'', ';', '"', '|', '&']).unwrap_or(rest.len());
    Some(rest[..end].split_whitespace().filter(|w| !ONE_SIDED.contains(w)).map(String::from).collect())
}

fn find(text: &str, check: &str, prefix: &str) -> Option<BTreeSet<String>> {
    text.lines().filter(|l| l.trim_start().starts_with(prefix)).find_map(|l| command(l, check))
}

#[test]
fn audit_hook_ci_shared_checks_have_the_same_flags() {
    let hook = read(".pre-commit-config.yaml");
    let ci = read(".github/workflows/ci.yml");
    for check in SHARED {
        let in_hook = find(&hook, check, "entry:").unwrap_or_else(|| panic!("the push hook has no `{check}`"));
        let in_ci = find(&ci, check, "- run:").or_else(|| find(&ci, check, "run:")).unwrap_or_else(|| panic!("CI has no `{check}`"));
        assert_eq!(in_hook, in_ci, "`{check}` differs between .pre-commit-config.yaml and .github/workflows/ci.yml");
    }
}

#[test]
fn audit_hook_ci_hook_comments_match_the_hooks() {
    let hook = read(".pre-commit-config.yaml");
    let stale = "the claude hooks skip (exit 0) when the claude CLI is missing";
    assert!(!hook.contains(stale), "a hook comment says the claude hooks skip, on hooks that are not claude hooks");
}
