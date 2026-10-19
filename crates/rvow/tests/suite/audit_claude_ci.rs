//! CI runs the Claude mod checks for real: it installs the pinned Claude Code,
//! and in CI the check script fails when the `claude` CLI is missing.

use std::path::{Path, PathBuf};
use std::process::Command;

fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..").canonicalize().unwrap()
}

/// The folder of the `node` on PATH, or `None` when node is missing.
fn node_dir() -> Option<PathBuf> {
    std::env::split_paths(&std::env::var_os("PATH")?).find(|d| d.join("node").is_file())
}

/// Runs `node extensions/scripts/claude.mjs <mode>` with a PATH that has node and no `claude`.
fn script(mode: &str, ci: bool) -> Option<(i32, String)> {
    let node = node_dir()?;
    let empty = std::env::temp_dir().join(format!("rvow-claude-ci-{}-{mode}-{ci}", std::process::id()));
    std::fs::create_dir_all(&empty).unwrap();
    let mut c = Command::new(node.join("node"));
    c.arg(root().join("extensions/scripts/claude.mjs")).arg(mode).env_clear().env("PATH", &node).env("HOME", &empty);
    if ci {
        c.env("CI", "true");
    }
    let out = c.output().unwrap();
    let text = format!("{}{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr));
    let _ = std::fs::remove_dir_all(&empty);
    // Another `claude` in the node folder would make the check meaningless.
    if node.join("claude").exists() {
        return None;
    }
    Some((out.status.code().unwrap_or(-1), text))
}

#[test]
fn script_fails_in_ci_without_claude_and_skips_elsewhere() {
    let Some((code, text)) = script("test", true) else {
        eprintln!("node is missing or sits beside claude; the script check cannot run here");
        return;
    };
    assert_ne!(code, 0, "in CI a missing claude CLI fails: {text}");
    assert!(text.contains("claude CLI not found"), "{text}");
    for mode in ["test", "validate", "tsc"] {
        let (code, text) = script(mode, false).unwrap();
        assert_eq!(code, 0, "outside CI a missing claude CLI skips {mode}: {text}");
    }
}

#[test]
fn script_names_the_login_when_it_skips_the_type_check() {
    let source = std::fs::read_to_string(root().join("extensions/scripts/claude.mjs")).unwrap();
    assert!(source.contains("ANTHROPIC_API_KEY"), "the type check skip names the login it needs");
}

#[test]
fn workflow_installs_the_pinned_claude_and_runs_the_mod_checks() {
    let ci = std::fs::read_to_string(root().join(".github/workflows/ci.yml")).unwrap();
    assert!(ci.contains("@anthropic-ai/claude-code@"), "CI installs Claude Code:\n{ci}");
    assert!(ci.contains("claudeCodeTypes"), "CI installs the version the types are pinned to:\n{ci}");
    assert!(ci.contains("secrets.ANTHROPIC_API_KEY"), "CI passes the optional key to the type check:\n{ci}");
    assert!(!ci.contains("skip themselves"), "the comment that the steps skip is gone:\n{ci}");
    for step in ["npm run drift", "npm run typecheck:claude", "npm run validate:claude", "npm run test:claude"] {
        assert!(ci.contains(step), "CI runs `{step}`");
    }
    let agents = std::fs::read_to_string(root().join("AGENTS.md")).unwrap();
    assert!(!agents.contains("so a skip proves nothing"), "AGENTS.md says CI fails on a missing claude CLI");
}
