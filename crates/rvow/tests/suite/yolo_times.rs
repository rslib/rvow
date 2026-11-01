//! `rvow yolo` prints the exact `at` and `until` it sealed into the switch file,
//! so the mod and pi record those values and not a clock of their own.

use std::path::PathBuf;
use std::process::Command;

use rvow_core::store;
use rvow_core::yolo::YOLO_FILE;

const RVOW: &str = env!("CARGO_BIN_EXE_rvow");

struct Fx {
    root: PathBuf,
}

impl Drop for Fx {
    fn drop(&mut self) {
        rvow_core::git::testrepo::remove_tree(&self.root);
    }
}

impl Fx {
    /// A repository with one accepted plan, so that only the yolo output matters.
    fn new(name: &str) -> Fx {
        let root = std::env::temp_dir().join(format!("rvow-yolo-times-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("repo/src")).unwrap();
        let fx = Fx { root: root.canonicalize().unwrap() };
        std::fs::write(fx.repo().join("src/a.txt"), "a").unwrap();
        for args in [vec!["init", "-q", "-b", "main"], vec!["add", "."], vec!["commit", "-q", "-m", "init"]] {
            assert!(fx.cmd("git").args(&args).status().unwrap().success(), "git {args:?}");
        }
        fx.rvow(&["init"]);
        let out = fx.rvow(&["new", "--spec", "Out"]);
        let dir = PathBuf::from(out.lines().find_map(|l| l.strip_prefix("path: ")).unwrap());
        std::fs::write(
            dir.join("tasks.md"),
            "- TSK-new1 Write ok\n  covers: REQ-new1\n  verify: grep -q ok out.txt && echo 'test result: ok. 1 passed; 0 failed;'\n  files: out.txt, src/**\n",
        )
        .unwrap();
        std::fs::write(
            dir.join("specs/out.md"),
            "## ADD REQ-new1 Out\nrvow SHALL write ok.\n### Scenario: s\n- WHEN it runs\n- THEN out.txt holds ok\n",
        )
        .unwrap();
        let id = out.lines().find_map(|l| l.strip_prefix("created: ")).unwrap().to_string();
        fx.rvow(&["accept", &id, "--yes"]);
        fx
    }

    fn repo(&self) -> PathBuf {
        self.root.join("repo")
    }

    fn cmd(&self, program: &str) -> Command {
        let mut c = Command::new(program);
        c.current_dir(self.repo())
            .env("HOME", self.root.join("home"))
            .env("RVOW_HOME", self.root.join("rh"))
            .env("PATH", "/usr/bin:/bin:/usr/sbin:/sbin")
            .env_remove("GIT_DIR")
            .env_remove("GIT_WORK_TREE")
            .env_remove("GIT_INDEX_FILE")
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_AUTHOR_NAME", "t")
            .env("GIT_AUTHOR_EMAIL", "t@t")
            .env("GIT_COMMITTER_NAME", "t")
            .env("GIT_COMMITTER_EMAIL", "t@t");
        c
    }

    fn rvow(&self, args: &[&str]) -> String {
        let out = self.cmd(RVOW).args(args).output().unwrap();
        String::from_utf8_lossy(&out.stdout).into_owned()
    }

    fn switch(&self) -> PathBuf {
        store::find(&self.repo(), &self.root.join("rh")).unwrap().unwrap().root.join(YOLO_FILE)
    }
}

/// The `key: n` line of TOON output, or `None`.
fn field(out: &str, key: &str) -> Option<u64> {
    out.lines().find_map(|l| l.strip_prefix(key)?.strip_prefix(": ")?.parse().ok())
}

fn at(out: &str) -> u64 {
    field(out, "at").unwrap_or_else(|| panic!("no at in {out:?}"))
}

fn until(out: &str) -> u64 {
    field(out, "until").unwrap_or_else(|| panic!("no until in {out:?}"))
}

#[test]
fn yolo_times_a_switch_prints_the_at_and_until_it_sealed() {
    let fx = Fx::new("switch");
    let out = fx.rvow(&["yolo", "audit", "--for", "30m"]);
    assert!(out.contains("tier: audit"), "{out}");
    assert!(out.contains("minutes: 29") || out.contains("minutes: 30"), "{out}");
    assert_eq!(until(&out) - at(&out), 1_800_000, "half an hour in ms: {out}");

    // The printed values are the ones sealed into the switch file.
    let file: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(fx.switch()).unwrap()).unwrap();
    assert_eq!(file["at"].as_u64(), Some(at(&out)), "{file}");
    assert_eq!(file["until"].as_u64(), Some(until(&out)), "{file}");

    // And `rvow yolo` with no tier prints the same two values.
    let shown = fx.rvow(&["yolo"]);
    assert!(shown.contains("tier: audit"), "{shown}");
    assert_eq!(at(&shown), at(&out), "{shown}");
    assert_eq!(until(&shown), until(&out), "{shown}");
}

#[test]
fn yolo_times_off_prints_neither_at_nor_until() {
    let fx = Fx::new("off");
    fx.rvow(&["yolo", "auto", "--for", "1h"]);
    let out = fx.rvow(&["yolo", "off"]);
    assert!(out.contains("tier: off"), "{out}");
    assert_eq!(field(&out, "at"), None, "{out}");
    assert_eq!(field(&out, "until"), None, "{out}");
    let shown = fx.rvow(&["yolo"]);
    assert!(shown.contains("tier: off"), "{shown}");
    assert_eq!(field(&shown, "at"), None, "{shown}");
    assert_eq!(field(&shown, "until"), None, "{shown}");
}
