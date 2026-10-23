use std::path::{Path, PathBuf};
use std::process::Command;

struct Scratch(PathBuf);

impl Scratch {
    fn new(name: &str) -> Self {
        let path = std::env::temp_dir().join(format!("rvow-audit-build-{}-{name}", std::process::id()));
        std::fs::create_dir_all(&path).unwrap();
        Self(path)
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..").canonicalize().unwrap()
}

/// The literal paths after each `marker` in `source`.
fn literal_paths<'a>(source: &'a str, marker: &str) -> Vec<&'a str> {
    source.match_indices(marker).filter_map(|(at, _)| source[at + marker.len()..].split('"').next()).collect()
}

/// A clean checkout builds only when every file the crates embed or name as
/// a module is tracked: an untracked one exists here and nowhere else.
#[test]
fn audit_build_every_embedded_file_is_tracked() {
    let root = root();
    let tracked = Command::new("git").args(["ls-files", "-z"]).current_dir(&root).output().unwrap();
    assert!(tracked.status.success(), "{}", String::from_utf8_lossy(&tracked.stderr));
    let tracked: std::collections::BTreeSet<PathBuf> =
        tracked.stdout.split(|b| *b == 0).filter(|p| !p.is_empty()).map(|p| root.join(std::str::from_utf8(p).unwrap())).collect();
    let mut checked = 0;
    for source in tracked.iter().filter(|p| p.starts_with(root.join("crates")) && p.extension().is_some_and(|e| e == "rs")) {
        let text = std::fs::read_to_string(source).unwrap();
        let text: String = text.lines().filter(|l| !l.trim_start().starts_with("//")).map(|l| format!("{l}\n")).collect();
        let dir = source.parent().unwrap();
        for marker in ["include_str!(\"", "include_bytes!(\"", "#[path = \""] {
            for rel in literal_paths(&text, marker) {
                let target = dir.join(rel);
                let Ok(target) = target.canonicalize() else {
                    panic!("{} names {rel}, which does not exist", source.display());
                };
                assert!(tracked.contains(&target), "{} names {rel}, which git does not track", source.display());
                checked += 1;
            }
        }
    }
    assert!(checked > 100, "only {checked} embedded paths found");
}

/// The file of each `mod name;` in `source` that no `#[path]` names.
fn module_files(source: &Path, text: &str) -> Vec<(String, [PathBuf; 2])> {
    let dir = source.parent().unwrap();
    let stem = source.file_stem().unwrap().to_str().unwrap();
    let base = if matches!(stem, "main" | "lib" | "mod") { dir.to_path_buf() } else { dir.join(stem) };
    let mut out = Vec::new();
    let mut after_path = false;
    for line in text.lines().map(str::trim) {
        let decl = line.strip_prefix("pub(crate) ").or_else(|| line.strip_prefix("pub ")).unwrap_or(line);
        if let Some(name) = decl.strip_prefix("mod ").and_then(|r| r.strip_suffix(';'))
            && !after_path
        {
            out.push((name.to_string(), [base.join(format!("{name}.rs")), base.join(name).join("mod.rs")]));
        }
        after_path = line.starts_with("#[path");
    }
    out
}

/// A module file that git does not track exists here and not in a clean checkout.
#[test]
fn audit_build_every_module_file_is_tracked() {
    let root = root();
    let tracked = Command::new("git").args(["ls-files", "-z"]).current_dir(&root).output().unwrap();
    assert!(tracked.status.success(), "{}", String::from_utf8_lossy(&tracked.stderr));
    let tracked: std::collections::BTreeSet<PathBuf> =
        tracked.stdout.split(|b| *b == 0).filter(|p| !p.is_empty()).map(|p| root.join(std::str::from_utf8(p).unwrap())).collect();
    let mut checked = 0;
    for source in tracked.iter().filter(|p| p.starts_with(root.join("crates")) && p.extension().is_some_and(|e| e == "rs")) {
        let text = std::fs::read_to_string(source).unwrap();
        for (name, candidates) in module_files(source, &text) {
            assert!(
                candidates.iter().any(|c| tracked.contains(c)),
                "{} declares mod {name}, but git tracks none of {candidates:?}",
                source.display()
            );
            checked += 1;
        }
    }
    assert!(checked > 50, "only {checked} module files found");
}

#[test]
fn audit_build_linux_environment_helpers_compile() {
    let scratch = Scratch::new("linux-helpers");
    let source = std::fs::read_to_string(root().join("crates/rvow-plan/src/fingerprint.rs")).unwrap();
    let mut helpers = String::new();
    for name in ["linux_cpu_model", "linux_cores", "linux_memory"] {
        let start = source.find(&format!("fn {name}(")).unwrap();
        let body = &source[start..];
        let end = body.find("\n}").unwrap() + 2;
        helpers.push_str(&body[..end]);
        helpers.push('\n');
    }
    let input = scratch.0.join("helpers.rs");
    std::fs::write(&input, helpers).unwrap();
    let output = Command::new("rustc")
        .args(["--crate-type", "lib", "--emit", "metadata"])
        .arg(&input)
        .arg("-o")
        .arg(scratch.0.join("helpers.rmeta"))
        .output()
        .unwrap();
    assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
}

#[test]
fn audit_build_dev_profile_optimizes_every_dependency() {
    let manifest = std::fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join("../../Cargo.toml")).unwrap();
    assert!(manifest.contains("[profile.dev.package.\"*\"]\nopt-level = 2\n"), "a dev build optimizes every dependency:\n{manifest}");
    let others: Vec<&str> =
        manifest.lines().filter(|l| l.starts_with("[profile.dev.package.") && !l.starts_with("[profile.dev.package.\"*\"]")).collect();
    assert!(others.is_empty(), "the \"*\" entry covers {others:?}");
}

/// The trimmed body of the `[name]` table in `manifest`, without the header.
fn toml_section(manifest: &str, name: &str) -> String {
    let header = format!("[{name}]");
    let mut body = String::new();
    let mut inside = false;
    for line in manifest.lines().map(str::trim) {
        if line.starts_with('[') {
            if inside {
                break;
            }
            inside = line == header;
            continue;
        }
        if inside {
            body.push_str(line);
            body.push('\n');
        }
    }
    assert!(!body.is_empty(), "Cargo.toml has no {header} section");
    body
}

#[test]
fn audit_build_release_profile_is_thin_with_16_codegen_units() {
    let manifest = std::fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join("../../Cargo.toml")).unwrap();
    let release = toml_section(&manifest, "profile.release");
    assert!(release.contains("strip = \"symbols\"\n"), "a release build strips symbols:\n{release}");
    assert!(release.contains("lto = \"thin\"\n"), "a release build uses thin LTO:\n{release}");
    assert!(release.contains("codegen-units = 16\n"), "a release build uses 16 codegen units:\n{release}");
}

#[test]
fn audit_build_release_profile_dist_keeps_fat_lto() {
    let manifest = std::fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join("../../Cargo.toml")).unwrap();
    let dist = toml_section(&manifest, "profile.dist");
    assert!(dist.contains("inherits = \"release\"\n"), "the dist profile inherits release:\n{dist}");
    assert!(dist.contains("lto = \"fat\"\n"), "a dist build uses fat LTO:\n{dist}");
    assert!(dist.contains("codegen-units = 1\n"), "a dist build uses 1 codegen unit:\n{dist}");
}

#[test]
fn audit_build_release_profile_ships_dist_binaries() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    for path in [".github/workflows/release.yml", ".github/scripts/build-musl.sh"] {
        let text = std::fs::read_to_string(root.join(path)).unwrap();
        assert!(text.contains("--profile dist"), "{path} does not build with --profile dist:\n{text}");
        assert!(text.contains("/dist/rvow"), "{path} does not read the dist binary:\n{text}");
        assert!(!text.contains("--release"), "{path} still builds with --release:\n{text}");
        assert!(!text.contains("/release/rvow"), "{path} still reads the release binary:\n{text}");
    }
}

/// The workspace crates bottom up; a crate links only to crates before it. AGENTS.md maps the same order.
const CRATE_ORDER: [&str; 9] =
    ["rvow-base", "rvow-git", "rvow-syntax", "rvow-shell", "rvow-store", "rvow-plan", "rvow-core", "rvow-hook", "rvow"];

/// The `rvow-*` and other crate names a manifest section lists, one per `name = ...` line.
fn section_deps(manifest: &str, section: &str) -> Vec<String> {
    let header = format!("[{section}]");
    let mut out = Vec::new();
    let mut inside = false;
    for line in manifest.lines().map(str::trim) {
        if line.starts_with('[') {
            inside = line == header;
            continue;
        }
        if inside && let Some((name, _)) = line.split_once('=') {
            out.push(name.trim().to_string());
        }
    }
    out
}

fn crate_manifest(name: &str) -> String {
    std::fs::read_to_string(root().join("crates").join(name).join("Cargo.toml")).unwrap()
}

#[test]
fn audit_build_crates_link_only_to_the_crates_below_them() {
    let workspace = std::fs::read_to_string(root().join("Cargo.toml")).unwrap();
    for name in CRATE_ORDER {
        assert!(workspace.contains(&format!("\"crates/{name}\"")), "{name} is a workspace member");
    }
    let members = workspace.matches("\"crates/").count();
    assert_eq!(members, CRATE_ORDER.len(), "every workspace member has a place in CRATE_ORDER and in AGENTS.md");
    for (at, name) in CRATE_ORDER.iter().enumerate() {
        let manifest = crate_manifest(name);
        for section in ["dependencies", "dev-dependencies"] {
            for dep in section_deps(&manifest, section).into_iter().filter(|d| d.starts_with("rvow")) {
                let to = CRATE_ORDER.iter().position(|c| *c == dep).unwrap_or_else(|| panic!("{name} links to unknown crate {dep}"));
                assert!(to < at, "{name} links up to {dep} in [{section}]; a crate links only to the crates below it");
            }
        }
    }
}

#[test]
fn audit_build_crates_only_rvow_syntax_pulls_in_tree_sitter() {
    for name in CRATE_ORDER {
        let deps = section_deps(&crate_manifest(name), "dependencies");
        let heavy: Vec<&String> = deps.iter().filter(|d| matches!(d.as_str(), "tree-sitter" | "ureq")).collect();
        if name == "rvow-syntax" {
            assert_eq!(heavy.len(), 2, "rvow-syntax holds tree-sitter and ureq");
        } else {
            assert!(heavy.is_empty(), "{name} names {heavy:?}; only rvow-syntax may, so the other crates build without wasmtime");
        }
    }
}
