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
    let source = std::fs::read_to_string(root().join("crates/rvow-core/src/fingerprint.rs")).unwrap();
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
