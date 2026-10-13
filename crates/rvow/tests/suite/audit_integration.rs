use std::path::{Path, PathBuf};
use std::process::Command;

/// The repository root (crates/rvow/../../..).
fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..").canonicalize().unwrap()
}

/// Every top-level module declared in the core library must resolve to a real
/// source file or directory. This catches the "module declared but missing"
/// class of build breakage independent of git index state.
#[test]
fn audit_integration_declared_core_modules_resolve_to_sources() {
    let lib = std::fs::read_to_string(root().join("crates/rvow-core/src/lib.rs")).unwrap();
    let mut found = 0;
    for line in lib.lines() {
        let Some(name) = line.trim().strip_prefix("pub mod") else { continue };
        let name = name.trim().trim_end_matches(';').trim();
        if name.is_empty() {
            continue;
        }
        let dir = root().join("crates/rvow-core/src").join(name);
        let as_rs = root().join(format!("crates/rvow-core/src/{name}.rs"));
        assert!(as_rs.is_file() || dir.is_dir(), "declared module {name} has neither {name}.rs nor a {name}/ folder");
        found += 1;
    }
    assert!(found >= 30, "sanity: expected 30+ declared modules, found {found}");
}

/// The pi runtime modules that the typecheck and typed-tool tests import must
/// all exist. This mirrors the audit_pi_tools install check without performing
/// a clean-checkout copy.
#[test]
fn audit_integration_pi_runtime_modules_present() {
    let dir = root().join("extensions/pi/rvow");
    for file in ["index.ts", "typed-tools.ts", "tools.ts", "types.ts", "formats.ts", "prompts.ts", "refresh.ts"] {
        assert!(dir.join(file).is_file(), "missing pi module {file}");
    }
    let index = std::fs::read_to_string(dir.join("index.ts")).unwrap();
    assert!(index.contains("registerTool"), "pi index must register tools");
    assert!(index.contains("rvow_ask"), "pi must keep the rvow_ask tool for open questions");
    let loader = std::fs::read_to_string(root().join("extensions/pi/rvow.ts")).unwrap();
    assert!(loader.contains("rvow/index.ts"), "the flat adapter must load the installed modules");
}

/// The CLI binary links and answers --version, so the release artifact is not a
/// stub. The version comes from the workspace package and is embedded at build.
#[test]
fn audit_integration_binary_links_and_reports_version() {
    let bin = root().join("target/debug/rvow");
    if !bin.is_file() {
        eprintln!("skip: build the debug binary first (cargo build)");
        return;
    }
    let out = Command::new(&bin).arg("--version").output().unwrap();
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    let text = String::from_utf8_lossy(&out.stdout);
    assert!(!text.trim().is_empty(), "--version printed nothing");
}

/// The extension npm scripts that the plan's verify commands reference must be
/// declared, so `npm --prefix extensions run check` resolves every step.
#[test]
fn audit_integration_extension_check_scripts_exist() {
    let pkg = std::fs::read_to_string(root().join("extensions/package.json")).unwrap();
    let value: serde_json::Value = serde_json::from_str(&pkg).unwrap();
    let scripts = value["scripts"].as_object().expect("package.json scripts");
    for name in ["check", "typecheck:pi", "test:pi", "typecheck:claude", "validate:claude", "test:claude", "drift", "lint"] {
        assert!(scripts.contains_key(name), "missing npm script {name}");
    }
    let check = scripts["check"].as_str().expect("check script");
    for step in ["drift", "lint", "typecheck:pi", "test:pi", "typecheck:claude", "validate:claude", "test:claude"] {
        assert!(check.contains(step), "the check script must run {step}, was: {check}");
    }
}

/// A release-notes guard: the changelog must carry an Unreleased section, since
/// the audit added commands and no release is cut yet (the plan's W28).
#[test]
fn audit_integration_changelog_keeps_unreleased() {
    let changelog = std::fs::read_to_string(root().join("CHANGELOG.md")).unwrap();
    assert!(changelog.contains("## [Unreleased]"), "CHANGELOG.md must keep an [Unreleased] section while no release is cut");
}
