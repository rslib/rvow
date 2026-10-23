//! The test runner setup: the nextest config, the env lock and the commands
//! that the hook, CI and AGENTS.md run.

use std::path::{Path, PathBuf};

fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..").canonicalize().unwrap()
}

fn read(rel: &str) -> String {
    std::fs::read_to_string(root().join(rel)).unwrap_or_else(|e| panic!("{rel}: {e}"))
}

/// The test modules of every crate whose source sets a process env var.
fn env_setters() -> Vec<(String, String)> {
    let mut out = Vec::new();
    for dir in test_dirs() {
        collect_setters(&dir, &mut out);
    }
    out.sort();
    out
}

/// Every `tests` folder under `crates/`, one per crate.
fn test_dirs() -> Vec<PathBuf> {
    let crates = root().join("crates");
    let mut out: Vec<PathBuf> =
        std::fs::read_dir(&crates).unwrap().map(|entry| entry.unwrap().path().join("tests")).filter(|dir| dir.is_dir()).collect();
    out.sort();
    out
}

fn collect_setters(dir: &Path, out: &mut Vec<(String, String)>) {
    let mut paths: Vec<PathBuf> = std::fs::read_dir(dir).unwrap().map(|entry| entry.unwrap().path()).collect();
    paths.sort();
    for path in paths {
        if path.is_dir() {
            collect_setters(&path, out);
            continue;
        }
        if path.extension().is_none_or(|ext| ext != "rs") {
            continue;
        }
        if path.file_name().is_some_and(|n| n == "audit_nextest.rs") {
            continue;
        }
        let source = std::fs::read_to_string(&path).unwrap();
        if source.contains("set_var(") || source.contains("remove_var(") {
            let module = path.file_stem().unwrap().to_str().unwrap().to_string();
            out.push((module, source));
        }
    }
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

/// Under `cargo test` the modules of one main.rs share a process, so a module
/// that sets PATH or HOME breaks every test that starts a tool. It builds as
/// its own test binary instead.
#[test]
fn audit_nextest_main_binaries_leave_path_and_home_alone() {
    let mut checked = 0;
    for dir in test_dirs() {
        let Ok(main) = std::fs::read_to_string(dir.join("main.rs")) else { continue };
        for rel in main.lines().filter_map(|l| l.trim().strip_prefix("#[path = \"")?.split('"').next()) {
            let source = std::fs::read_to_string(dir.join(rel)).unwrap();
            for var in ["PATH", "HOME"] {
                assert!(
                    !source.contains(&format!("set_var(\"{var}\"")) && !source.contains(&format!("remove_var(\"{var}\"")),
                    "{} sets {var} but builds into {}; give it its own test binary",
                    dir.join(rel).display(),
                    dir.join("main.rs").display()
                );
            }
            checked += 1;
        }
    }
    assert!(checked > 100, "only {checked} test modules found");
}
