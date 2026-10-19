//! The test runner setup: the nextest config, the env lock and the commands
//! that the hook, CI and AGENTS.md run.

use std::path::{Path, PathBuf};

fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..").canonicalize().unwrap()
}

fn read(rel: &str) -> String {
    std::fs::read_to_string(root().join(rel)).unwrap_or_else(|e| panic!("{rel}: {e}"))
}

/// The suite modules whose source sets a process env var.
fn env_setters() -> Vec<(String, String)> {
    let dir = root().join("crates/rvow/tests/suite");
    let mut out = Vec::new();
    for entry in std::fs::read_dir(&dir).unwrap() {
        let path = entry.unwrap().path();
        let source = std::fs::read_to_string(&path).unwrap();
        if path.file_name().is_some_and(|n| n == "audit_nextest.rs") {
            continue;
        }
        if source.contains("set_var(") || source.contains("remove_var(") {
            let module = path.file_stem().unwrap().to_str().unwrap().to_string();
            out.push((module, source));
        }
    }
    out.sort();
    out
}

#[test]
fn audit_nextest_config_flags_slow_tests_and_has_a_fast_profile() {
    let config = read(".config/nextest.toml");
    assert!(config.contains("[profile.default]"), "{config}");
    assert!(config.contains("slow-timeout = { period = \"30s\" }"), "{config}");
    assert!(config.contains("[profile.fast]"), "{config}");
    assert!(config.contains("slow-timeout = { period = \"15s\", terminate-after = 1 }"), "{config}");
}

#[test]
fn audit_nextest_config_runs_every_env_setter_in_the_serial_group() {
    let config = read(".config/nextest.toml");
    assert!(config.contains("serial = { max-threads = 1 }"), "{config}");
    let setters = env_setters();
    assert!(!setters.is_empty());
    for (module, _) in setters {
        assert!(config.contains(&format!("test(/^{module}::/)")), "{module} sets an env var but is not in the serial group:\n{config}");
    }
}

#[test]
fn audit_nextest_env_setters_hold_the_shared_lock() {
    for (module, source) in env_setters() {
        assert!(source.contains("crate::env_lock::lock()"), "{module} sets an env var without crate::env_lock::lock()");
        assert!(!source.contains("static ENV_LOCK"), "{module} keeps its own env lock, which does not stop the other modules");
    }
}

#[test]
fn audit_nextest_runner_is_nextest_in_the_hook_ci_and_agents_md() {
    let hook = read(".pre-commit-config.yaml");
    assert!(hook.contains("cargo nextest run --locked"), "{hook}");
    let ci = read(".github/workflows/ci.yml");
    assert!(ci.contains("cargo nextest run --locked"), "{ci}");
    assert!(ci.contains("--run-ignored all"), "{ci}");
    let agents = read("AGENTS.md");
    for command in ["cargo nextest run --locked", "-E 'test(/^cli::/)'"] {
        assert!(agents.contains(command), "AGENTS.md does not show `{command}`");
    }
}
