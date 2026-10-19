//! `rvow docs --out <dir>` writes the command reference from the CLI itself,
//! and the committed `docs/reference` stays the same as what it writes.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::Command;

const RVOW: &str = env!("CARGO_BIN_EXE_rvow");

fn generate(name: &str) -> PathBuf {
    let out = std::env::temp_dir().join(format!("rvow-docs-ref-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&out);
    let run = Command::new(RVOW).args(["docs", "--out"]).arg(&out).output().unwrap();
    assert!(run.status.success(), "rvow docs: {}", String::from_utf8_lossy(&run.stdout));
    out
}

/// Every file under `dir`, by its path relative to `dir`.
fn files(dir: &Path) -> BTreeMap<String, String> {
    let mut out = BTreeMap::new();
    let mut stack = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        for e in std::fs::read_dir(&d).unwrap().flatten() {
            let p = e.path();
            if p.is_dir() {
                stack.push(p);
            } else {
                let rel = p.strip_prefix(dir).unwrap().to_string_lossy().into_owned();
                out.insert(rel, std::fs::read_to_string(&p).unwrap());
            }
        }
    }
    out
}

#[test]
fn docs_reference_writes_a_page_per_command_and_an_index() {
    let out = generate("pages");
    let prove = std::fs::read_to_string(out.join("commands/prove.md")).unwrap();
    assert!(prove.starts_with("# rvow prove\n"), "{prove}");
    assert!(prove.contains("## Usage\n\n```sh\nrvow prove"), "{prove}");
    assert!(prove.contains("## Options") && prove.contains("`--stale`"), "{prove}");
    let index = std::fs::read_to_string(out.join("commands/index.md")).unwrap();
    for page in ["prove.md", "accept.md"] {
        assert!(index.contains(&format!("]({page})")), "the index links {page}:\n{index}");
    }
    assert!(!out.join("commands/reprove.md").exists(), "a hidden command has no page");
    assert!(!out.join("commands/docs.md").exists(), "the hidden docs command has no page");
    for (rel, text) in files(&out) {
        assert!(text.lines().next().is_some_and(|l| l.starts_with("# ")), "{rel} starts with a heading");
    }
    let _ = std::fs::remove_dir_all(&out);
}

#[test]
fn docs_reference_in_the_repository_is_current() {
    let out = generate("current");
    let committed = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../docs/reference");
    let (want, have) = (files(&out), if committed.is_dir() { files(&committed) } else { BTreeMap::new() });
    let _ = std::fs::remove_dir_all(&out);
    let stale: Vec<&String> = want.keys().chain(have.keys()).filter(|k| want.get(*k) != have.get(*k)).collect();
    assert!(stale.is_empty(), "docs/reference is stale ({stale:?}); run `rvow docs --out docs/reference`");
}
