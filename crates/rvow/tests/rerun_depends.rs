//! `depends_on:` joins a dependency's cache key into a task's key.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::process::Command;

use rvow_core::gate::cache::dependent_keys;
use rvow_core::tasks::parse_tasks;

const RVOW: &str = env!("CARGO_BIN_EXE_rvow");

struct Fx {
    root: PathBuf,
    ids: Vec<String>,
    tasks_file: PathBuf,
    count: PathBuf,
}

impl Drop for Fx {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

fn git_command(repo: &std::path::Path) -> Command {
    let mut c = Command::new("git");
    c.current_dir(repo)
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

impl Fx {
    /// Tasks A (`a/**`), B (`b/**`, depends on A) and C (`c/**`), all proved.
    fn new(name: &str) -> Fx {
        let root = std::env::temp_dir().join(format!("rvow-depends-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        for d in ["a", "b", "c"] {
            std::fs::create_dir_all(root.join("repo").join(d)).unwrap();
            std::fs::write(root.join("repo").join(d).join("x"), d).unwrap();
        }
        let root = root.canonicalize().unwrap();
        let count = root.join("count");
        let mut fx = Fx { root, ids: Vec::new(), tasks_file: PathBuf::new(), count };
        let repo = fx.root.join("repo");
        for args in [vec!["init", "-q", "-b", "main"], vec!["add", "."], vec!["commit", "-q", "-m", "init"]] {
            assert!(git_command(&repo).args(&args).status().unwrap().success(), "git {args:?}");
        }
        fx.rvow(&["init"]);
        // These tests are about the cache, so a proven task reruns here.
        std::fs::write(fx.root.join("rh/config.toml"), "rerun_proven = true\n").unwrap();
        let out = fx.rvow(&["new", "--spec", "Out"]);
        let dir = PathBuf::from(out.lines().find_map(|l| l.strip_prefix("path: ")).unwrap());
        let task = |n: u32, title: &str, files: &str, extra: &str| {
            format!(
                "- TSK-new{n} {title}\n  covers: REQ-new1\n  verify: echo {title} >> {} && echo 'test result: ok. 1 passed; 0 failed;'\n  files: {files}\n  baseline: pass\n{extra}\n",
                fx.count.display()
            )
        };
        std::fs::write(
            dir.join("tasks.md"),
            format!("{}{}{}", task(1, "A", "a/**", ""), task(2, "B", "b/**", "  depends_on: TSK-new1\n"), task(3, "C", "c/**", "")),
        )
        .unwrap();
        std::fs::write(
            dir.join("specs/out.md"),
            "## ADD REQ-new1 Out\nrvow SHALL write.\n### Scenario: s\n- WHEN it runs\n- THEN it writes\n",
        )
        .unwrap();
        let id = out.lines().find_map(|l| l.strip_prefix("created: ")).unwrap().to_string();
        fx.rvow(&["accept", &id, "--yes"]);
        let located = fx.rvow(&["path", &id]);
        fx.tasks_file = PathBuf::from(located.lines().find_map(|l| l.strip_prefix("path: ")).unwrap().trim()).join("tasks.md");
        let text = std::fs::read_to_string(&fx.tasks_file).unwrap();
        fx.ids = text.lines().filter_map(|l| l.strip_prefix("- ")?.split_whitespace().next().map(String::from)).collect();
        for id in fx.ids.clone() {
            fx.rvow(&["prove", &id]);
        }
        let _ = std::fs::remove_file(&fx.count);
        fx
    }

    fn rvow(&self, args: &[&str]) -> String {
        let out = Command::new(RVOW)
            .args(args)
            .current_dir(self.root.join("repo"))
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
            .env("GIT_COMMITTER_EMAIL", "t@t")
            .output()
            .unwrap();
        String::from_utf8_lossy(&out.stdout).into_owned()
    }

    fn commit(&self, file: &str, text: &str) {
        std::fs::write(self.root.join("repo").join(file), text).unwrap();
        for args in [vec!["add", file], vec!["commit", "-q", "-m", "change"]] {
            assert!(git_command(&self.root.join("repo")).args(&args).status().unwrap().success(), "git {args:?}");
        }
    }

    /// The titles that ran since the last call, sorted.
    fn ran(&self) -> Vec<String> {
        let mut lines: Vec<String> =
            std::fs::read_to_string(&self.count).map(|t| t.lines().map(String::from).collect()).unwrap_or_default();
        let _ = std::fs::remove_file(&self.count);
        lines.sort();
        lines
    }
}

#[test]
fn depends_a_change_in_the_dependency_reruns_the_dependent_only() {
    let fx = Fx::new("rerun");
    let first = fx.rvow(&["gate", "--rerun"]);
    assert!(first.contains("passed: true"), "{first}");
    assert_eq!(fx.ran(), ["A", "B", "C"]);
    let second = fx.rvow(&["gate", "--rerun"]);
    assert!(second.contains("cached[3]"), "{second}");
    assert!(fx.ran().is_empty());

    fx.commit("a/x", "changed");
    fx.rvow(&["prove", &fx.ids[0]]);
    let _ = fx.ran();
    let third = fx.rvow(&["gate", "--rerun"]);
    assert!(third.contains("passed: true"), "{third}");
    assert_eq!(fx.ran(), ["A", "B"], "C has no dependency and stays cached: {third}");
    assert!(third.contains("cached[1]"), "{third}");
}

#[test]
fn depends_on_is_minted_with_the_task_ids() {
    let fx = Fx::new("mint");
    let text = std::fs::read_to_string(&fx.tasks_file).unwrap();
    assert!(text.contains(&format!("depends_on: {}", fx.ids[0])), "{text}");
    assert!(!text.contains("TSK-new"), "{text}");
}

#[test]
fn depends_field_must_name_tasks() {
    let (tasks, diags) = parse_tasks("- TSK-abc123 Work\n  covers: REQ-abc123\n  verify: true\n  files: a\n  depends_on: REQ-abc123\n");
    assert_eq!(tasks.len(), 1);
    assert!(diags.iter().any(|d| d.message.contains("depends_on")), "{diags:?}");
}

#[test]
fn depends_keys_chain_and_a_bad_chain_is_uncacheable() {
    let src = |extra_a: &str| {
        format!(
            "- TSK-aaaaaa A\n  verify: true\n  files: a\n{extra_a}\n- TSK-bbbbbb B\n  verify: true\n  files: b\n  depends_on: TSK-aaaaaa\n\n- TSK-cccccc C\n  verify: true\n  files: c\n  depends_on: TSK-bbbbbb\n\n- TSK-dddddd D\n  verify: true\n  files: d\n"
        )
    };
    let own = |a: &str| {
        BTreeMap::from([
            ("TSK-aaaaaa".to_string(), Some(a.to_string())),
            ("TSK-bbbbbb".to_string(), Some("b".to_string())),
            ("TSK-cccccc".to_string(), Some("c".to_string())),
            ("TSK-dddddd".to_string(), Some("d".to_string())),
        ])
    };
    let tasks = parse_tasks(&src("")).0;
    let one = dependent_keys(&tasks, &own("a1"));
    let two = dependent_keys(&tasks, &own("a2"));
    assert_eq!(one["TSK-dddddd"], Some("d".to_string()), "a task without depends_on keeps its own key");
    assert_ne!(one["TSK-bbbbbb"], two["TSK-bbbbbb"]);
    assert_ne!(one["TSK-cccccc"], two["TSK-cccccc"], "the chain is transitive");
    assert_eq!(one["TSK-dddddd"], two["TSK-dddddd"]);

    let mut missing = own("a1");
    missing.insert("TSK-aaaaaa".into(), None);
    let keys = dependent_keys(&tasks, &missing);
    assert_eq!((&keys["TSK-aaaaaa"], &keys["TSK-bbbbbb"], &keys["TSK-cccccc"]), (&None, &None, &None));

    let cyclic = parse_tasks(&src("  depends_on: TSK-cccccc\n")).0;
    let keys = dependent_keys(&cyclic, &own("a1"));
    assert_eq!(keys["TSK-aaaaaa"], None);
    assert_eq!(keys["TSK-bbbbbb"], None);
    assert_eq!(keys["TSK-dddddd"], Some("d".to_string()));

    let unknown = parse_tasks("- TSK-aaaaaa A\n  verify: true\n  files: a\n  depends_on: TSK-zzzzzz\n").0;
    assert_eq!(dependent_keys(&unknown, &own("a1"))["TSK-aaaaaa"], None);
}
