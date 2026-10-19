//! What rvow knows about a task's baseline without running its verify command.

use std::path::Path;
use std::process::Command;

use crate::config::Config;
use crate::error::{Error, Result};
use crate::git;
use crate::tasks::Task;

/// Flags of `cargo test` and `cargo nextest run` that take no value.
const BARE_FLAGS: &[&str] = &[
    "--locked",
    "--frozen",
    "--offline",
    "--release",
    "--all-features",
    "--no-default-features",
    "--workspace",
    "--all",
    "--lib",
    "--bins",
    "--tests",
    "--benches",
    "--examples",
    "--all-targets",
    "--doc",
    "--no-fail-fast",
    "-q",
    "--quiet",
    "-v",
    "--verbose",
];

/// Flags of `cargo test` and `cargo nextest run` that take one value.
const VALUE_FLAGS: &[&str] = &[
    "-p",
    "--package",
    "--test",
    "--bin",
    "--example",
    "--bench",
    "--features",
    "-F",
    "--target",
    "--target-dir",
    "--manifest-path",
    "-j",
    "--jobs",
    "--profile",
    "--exclude",
    "--color",
];

/// The test filter of a verify that is one `cargo test` or `cargo nextest run`
/// call with exactly one filter and only known flags, else `None`. Any shell
/// syntax, environment prefix, unknown flag or argument after `--` gives `None`,
/// so a flag value is never taken for a filter.
pub fn cargo_filter(cmd: &str) -> Option<&str> {
    if cmd.chars().any(|c| "&|;<>$`()'\"\\*?[]{}~\n".contains(c)) {
        return None;
    }
    let mut words = cmd.split_whitespace();
    let rest: Vec<&str> = match (words.next(), words.next()) {
        (Some("cargo"), Some("test")) => words.collect(),
        (Some("cargo"), Some("nextest")) if words.next() == Some("run") => words.collect(),
        _ => return None,
    };
    let mut filters = Vec::new();
    let mut i = 0;
    while i < rest.len() {
        let word = rest[i];
        if word == "--" {
            return None;
        }
        if VALUE_FLAGS.contains(&word) {
            i += 1;
            rest.get(i)?;
        } else if let Some((flag, _)) = word.split_once('=') {
            if !VALUE_FLAGS.contains(&flag) {
                return None;
            }
        } else if word.starts_with('-') {
            if !BARE_FLAGS.contains(&word) {
                return None;
            }
        } else {
            filters.push(word);
        }
        i += 1;
    }
    match filters[..] {
        [one] => Some(one),
        _ => None,
    }
}

/// Whether `cmd` must fail at `commit` because its lone cargo filter names a
/// test that does not exist there: no tracked Rust file at `commit` holds the
/// last path segment of the filter. A name in a comment counts as present.
pub fn known_red(top: &Path, commit: &str, cmd: &str) -> Result<bool> {
    let Some(filter) = cargo_filter(cmd) else { return Ok(false) };
    let needle = filter.rsplit("::").next().unwrap_or(filter);
    if needle.is_empty() {
        return Ok(false);
    }
    let out = Command::new(git::bin())
        .arg("-C")
        .arg(top)
        .args(["grep", "-q", "-F", "-e", needle, commit, "--", "*.rs"])
        .env_remove("GIT_DIR")
        .env_remove("GIT_WORK_TREE")
        .env_remove("GIT_INDEX_FILE")
        .output()
        .map_err(|e| Error::new("git", format!("cannot run git: {e}"), "install git"))?;
    match out.status.code() {
        Some(0) => Ok(false),
        Some(1) => Ok(true),
        _ => Err(Error::new(
            "git",
            format!("git grep for {needle} at {commit} failed: {}", String::from_utf8_lossy(&out.stderr).trim()),
            "run rvow accept in a git repository with the base commit fetched",
        )),
    }
}

/// The files in `task`'s scope that hold work: those that differ from `base`,
/// untracked ones included, other than test files (the `test_globs`) and files in
/// `done`, the scope of the change's proved tasks. Empty means the verify
/// still sees the code as it was before the work.
pub fn work_files(top: &Path, config: &Config, base: &str, task: &Task, done: &[String]) -> Result<Vec<String>> {
    let changed = git::task_diff(top, base, &task.files)?.files;
    let tests = crate::fingerprint::globset(&config.test_globs)?;
    let done = crate::fingerprint::globset(done)?;
    Ok(changed.into_iter().filter(|f| !tests.is_match(f) && !done.is_match(f)).collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_lone_filter_is_found() {
        assert_eq!(cargo_filter("cargo test --locked -p rvow tail::keeps"), Some("tail::keeps"));
        assert_eq!(cargo_filter("cargo nextest run --features=x keeps"), Some("keeps"));
    }

    #[test]
    fn anything_unsure_gives_no_filter() {
        for cmd in [
            "cargo test",
            "cargo test a b",
            "cargo build && cargo test a",
            "RUST_LOG=1 cargo test a",
            "cargo test --partition 1/2 a",
            "cargo test a -- --exact",
            "cargo test -p",
            "cargo nextest list a",
            "cargo test 'a'",
        ] {
            assert_eq!(cargo_filter(cmd), None, "{cmd}");
        }
    }
}
