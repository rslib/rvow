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

#[test]
fn audit_build_tracked_sources_compile_without_claim_manifest() {
    let root = root();
    let scratch = Scratch::new("checkout");
    let checkout = scratch.0.join("repo");
    std::fs::create_dir_all(&checkout).unwrap();
    let tracked = Command::new("git").args(["ls-files", "-z"]).current_dir(&root).output().unwrap();
    assert!(tracked.status.success(), "{}", String::from_utf8_lossy(&tracked.stderr));
    for path in tracked.stdout.split(|b| *b == 0).filter(|p| !p.is_empty()) {
        let relative = Path::new(std::str::from_utf8(path).unwrap());
        let source = root.join(relative);
        if !source.is_file() {
            continue;
        }
        let destination = checkout.join(relative);
        std::fs::create_dir_all(destination.parent().unwrap()).unwrap();
        std::fs::copy(source, destination).unwrap();
    }
    assert!(!checkout.join("PLAN-CLAIMS.md").exists());
    let output = Command::new(env!("CARGO"))
        .args(["test", "--no-run", "--locked", "--offline"])
        .current_dir(&checkout)
        .env("CARGO_TARGET_DIR", scratch.0.join("target"))
        .output()
        .unwrap();
    assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
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
