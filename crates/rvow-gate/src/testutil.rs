//! Fixtures that the tests of this crate and of the crates above it build; only
//! a dev-dependency turns the module on.

use std::path::PathBuf;

use crate::git::testrepo::*;
use crate::store::{Mode, Project, init};

/// A repository with one accepted change whose single task checks that
/// `out.txt` holds "ok", with libtest-style output.
pub fn accepted() -> (TempDir, Project, PathBuf, String) {
    let tmp = TempDir::new();
    let home = tmp.0.join("home");
    let repo = repo_with_commit(&tmp.0, "r");
    let p = init(&repo, &home, Mode::Home).unwrap();
    let c = crate::change::new(&p, "Out", "main", false).unwrap();
    std::fs::write(
        c.path.join("tasks.md"),
        "- TSK-new1 Write ok\n  covers: REQ-new1\n  verify: grep -q ok out.txt && echo 'test result: ok. 1 passed; 0 failed;'\n  files: out.txt\n",
    )
    .unwrap();
    std::fs::write(
        c.path.join("specs/out.md"),
        "## ADD REQ-new1 Out\nrvow SHALL write ok.\n### Scenario: s\n- WHEN it runs\n- THEN out.txt holds ok\n",
    )
    .unwrap();
    let prepared = crate::vow::prepare(&p, &home, c, &repo, &mut std::io::sink()).unwrap();
    let vowed = crate::vow::commit(&p, prepared).unwrap();
    let task = vowed.minted.iter().find(|(from, _)| from == "TSK-new1").unwrap().1.to_string();
    (tmp, p, repo, task)
}

/// The `accepted` change, proved once: its test file and Rust source are
/// protected, and the task's proof is the latest event.
pub fn setup() -> (TempDir, Project, PathBuf, String) {
    let tmp = TempDir::new();
    let home = tmp.0.join("home");
    let repo = repo_with_commit(&tmp.0, "r");
    let p = init(&repo, &home, Mode::Home).unwrap();
    let c = crate::change::new(&p, "T", "main", true).unwrap();
    std::fs::write(
        c.path.join("tasks.md"),
        "- TSK-new1 T\n  verify: sh tests/t.sh && echo 'test result: ok. 1 passed; 0 failed;'\n  files: tests/**, src/**\n",
    )
    .unwrap();
    let prepared = crate::vow::prepare(&p, &home, c, &repo, &mut std::io::sink()).unwrap();
    let task = crate::vow::commit(&p, prepared).unwrap().minted[0].1.to_string();
    std::fs::create_dir_all(repo.join("tests")).unwrap();
    std::fs::create_dir_all(repo.join("src")).unwrap();
    std::fs::write(repo.join("tests/t.sh"), "check_one\ncheck_two\nexit 0\n").unwrap();
    std::fs::write(repo.join("src/lib.rs"), "pub fn f() {}\n#[cfg(test)]\nmod tests {\n    #[test]\n    fn t() { assert_eq!(1, 1); }\n}\n")
        .unwrap();
    let r = crate::proof::prove(&p, &home, &repo, &task, &mut std::io::sink()).unwrap();
    assert!(r.proved, "{:?}", r.outcome);
    (tmp, p, repo, task)
}
