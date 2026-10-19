//! The docs name only paths that exist, and the Node version the extensions
//! need is declared once and agrees everywhere it is named.

use std::path::{Path, PathBuf};

use serde_json::Value;

fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..").canonicalize().unwrap()
}

fn read(rel: &str) -> String {
    std::fs::read_to_string(root().join(rel)).unwrap_or_else(|e| panic!("{rel}: {e}"))
}

/// Repository paths that `text` names in backticks or links. A path with a
/// placeholder, a glob or a `skills/` start (the installed skill, not the
/// source) is left out.
fn named_paths(text: &str) -> Vec<String> {
    let starts = ["crates/", "extensions/", "evals/", ".github/", "guides/", "docs/"];
    let mut out = Vec::new();
    for piece in text.split('`').skip(1).step_by(2).chain(text.split("](").skip(1).filter_map(|s| s.split(')').next())) {
        let path = piece.split([' ', '#']).next().unwrap_or("").trim_end_matches(['.', ',', '/']);
        let path = path.split(':').next().unwrap_or(path);
        if starts.iter().any(|s| path.starts_with(s)) && !path.contains(['<', '*', '{', '$']) {
            out.push(path.to_string());
        }
    }
    out
}

fn path_ok(path: &str) -> bool {
    path_ok_in(&root(), path)
}

/// Whether a path the docs name is real in the repository at `repo`: it exists, or git ignores
/// it, as for a results folder that a local run makes and a clean checkout does not have. The
/// path is also asked about as a folder, since a rule such as `results/` matches only a folder
/// and git knows a missing path is one only from its trailing slash.
fn path_ok_in(repo: &Path, path: &str) -> bool {
    let ignored = |p: &str| {
        std::process::Command::new("git")
            .arg("-C")
            .arg(repo)
            .args(["check-ignore", "-q", "--no-index", "--", p])
            .status()
            .is_ok_and(|s| s.success())
    };
    repo.join(path).exists() || ignored(path) || ignored(&format!("{path}/"))
}

#[test]
fn docs_name_only_paths_that_exist() {
    let mut pages: Vec<String> = vec!["README.md".into(), "AGENTS.md".into()];
    for e in std::fs::read_dir(root().join("docs")).unwrap().flatten() {
        if e.path().extension().is_some_and(|x| x == "md") {
            pages.push(format!("docs/{}", e.file_name().to_string_lossy()));
        }
    }
    let mut checked = 0;
    let mut missing = Vec::new();
    for page in &pages {
        for path in named_paths(&read(page)) {
            checked += 1;
            if !path_ok(&path) {
                missing.push(format!("{page}: {path}"));
            }
        }
    }
    assert!(checked > 30, "the scan found only {checked} paths");
    assert!(missing.is_empty(), "the docs name paths that do not exist:\n{}", missing.join("\n"));
}

/// The lowest version that an `engines.node` range such as `>=22.19.0` allows, as numbers.
fn floor(range: &str) -> Option<Vec<u64>> {
    range
        .split("||")
        .filter_map(|part| {
            part.trim().strip_prefix(">=").map(|v| v.trim().split('.').map(|n| n.parse().unwrap_or(0)).collect::<Vec<u64>>())
        })
        .min()
}

#[test]
fn node_floor_agrees_with_the_docs_ci_and_the_lock_file() {
    let pkg: Value = serde_json::from_str(&read("extensions/package.json")).unwrap();
    let declared = pkg["engines"]["node"].as_str().expect("extensions/package.json declares engines.node");
    let ours = floor(declared).expect("engines.node is a >= range");
    let version = ours.iter().map(u64::to_string).collect::<Vec<_>>().join(".");
    assert!(read("AGENTS.md").contains(&format!("Node {version} or newer")), "AGENTS.md names Node {version}");
    assert!(read(".github/workflows/ci.yml").contains(&version), "CI runs the extension checks on Node {version}");
    let lock: Value = serde_json::from_str(&read("extensions/package-lock.json")).unwrap();
    let mut needs = Vec::new();
    for (name, entry) in lock["packages"].as_object().unwrap() {
        if let Some(theirs) = entry["engines"]["node"].as_str().and_then(floor)
            && theirs > ours
        {
            needs.push(format!("{name} needs {}", entry["engines"]["node"]));
        }
    }
    assert!(needs.is_empty(), "a dependency needs a newer Node than {declared}:\n{}", needs.join("\n"));
}

#[test]
fn platform_build_stops_outside_macos_and_linux() {
    let lib = read("crates/rvow-core/src/lib.rs");
    assert!(
        lib.contains("#[cfg(not(any(target_os = \"macos\", target_os = \"linux\")))]\ncompile_error!("),
        "rvow-core stops the build on another platform"
    );
    let readme = read("README.md");
    assert!(readme.contains("Windows is not supported"), "the README names the unsupported platform");
}

#[test]
fn docs_paths_accept_an_ignored_path_and_refuse_a_missing_tracked_one() {
    assert!(path_ok("evals/agent/results/never-made"), "a git-ignored path counts, also when a clean checkout lacks it");
    assert!(!path_ok("crates/rvow/src/never_made.rs"), "a missing path that git would track fails");
    assert!(path_ok("crates/rvow/src/main.rs"));
}

#[test]
fn docs_paths_accept_a_folder_only_rule_when_the_folder_is_missing() {
    let repo = std::env::temp_dir().join(format!("rvow-audit-paths-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&repo);
    std::fs::create_dir_all(&repo).unwrap();
    assert!(std::process::Command::new("git").arg("-C").arg(&repo).args(["init", "-q"]).status().unwrap().success());
    std::fs::write(repo.join(".gitignore"), "results/\n").unwrap();
    let missing_folder = path_ok_in(&repo, "evals/results");
    let missing_file = path_ok_in(&repo, "evals/kept.rs");
    let _ = std::fs::remove_dir_all(&repo);
    assert!(missing_folder, "a folder-only rule counts on a clean checkout, where the folder does not exist");
    assert!(!missing_file, "a missing path that no rule ignores still fails");
}
