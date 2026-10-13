//! Scoped `setup` and `checks` entries, and a gate that every cached task answers.

use std::path::PathBuf;
use std::process::Command;

use rvow_core::config::{self, Step};
use rvow_core::store;

const RVOW: &str = env!("CARGO_BIN_EXE_rvow");

struct Fx {
    root: PathBuf,
    task: String,
    log: PathBuf,
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
    /// A repository with one accepted, committed and proved task. `config` may
    /// name `$LOG`, the file that the commands append to.
    fn new(name: &str, config: &str, task_files: &str) -> Fx {
        let root = std::env::temp_dir().join(format!("rvow-scope-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("repo/src")).unwrap();
        std::fs::create_dir_all(root.join("repo/python")).unwrap();
        let root = root.canonicalize().unwrap();
        let log = root.join("log");
        let mut fx = Fx { root, task: String::new(), log };
        let repo = fx.root.join("repo");
        std::fs::write(repo.join("src/a.txt"), "a").unwrap();
        std::fs::write(repo.join("python/a.py"), "a").unwrap();
        for args in [vec!["init", "-q", "-b", "main"], vec!["add", "."], vec!["commit", "-q", "-m", "init"], vec!["branch", "base0"]] {
            assert!(git_command(&repo).args(&args).status().unwrap().success(), "git {args:?}");
        }
        fx.rvow(&["init"]);
        std::fs::write(
            fx.root.join("rh/config.toml"),
            format!("base = \"base0\"\nrerun_proven = true\n{}", config.replace("$LOG", &fx.log.display().to_string())),
        )
        .unwrap();
        let out = fx.rvow(&["new", "--spec", "Out"]);
        let dir = PathBuf::from(out.lines().find_map(|l| l.strip_prefix("path: ")).unwrap());
        std::fs::write(
            dir.join("tasks.md"),
            format!(
                "- TSK-new1 Write ok\n  covers: REQ-new1\n  verify: grep -q ok out.txt && echo 'test result: ok. 1 passed; 0 failed;'\n  files: {task_files}\n"
            ),
        )
        .unwrap();
        std::fs::write(
            dir.join("specs/out.md"),
            "## ADD REQ-new1 Out\nrvow SHALL write ok.\n### Scenario: s\n- WHEN it runs\n- THEN out.txt holds ok\n",
        )
        .unwrap();
        let id = out.lines().find_map(|l| l.strip_prefix("created: ")).unwrap().to_string();
        fx.rvow(&["accept", &id, "--yes"]);
        let located = fx.rvow(&["path", &id]);
        let tasks =
            std::fs::read_to_string(PathBuf::from(located.lines().find_map(|l| l.strip_prefix("path: ")).unwrap().trim()).join("tasks.md"))
                .unwrap();
        fx.task = tasks.lines().find_map(|l| l.strip_prefix("- ")?.split_whitespace().next().map(String::from)).unwrap();
        fx.commit("out.txt", "ok");
        fx.rvow(&["prove", &fx.task]);
        let _ = std::fs::remove_file(&fx.log);
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

    fn gate(&self) -> String {
        self.rvow(&["gate", "--rerun"])
    }

    fn commit(&self, file: &str, text: &str) {
        std::fs::write(self.root.join("repo").join(file), text).unwrap();
        for args in [vec!["add", file], vec!["commit", "-q", "-m", "change"]] {
            assert!(git_command(&self.root.join("repo")).args(&args).status().unwrap().success(), "git {args:?}");
        }
    }

    fn log(&self) -> Vec<String> {
        std::fs::read_to_string(&self.log).map(|t| t.lines().map(String::from).collect()).unwrap_or_default()
    }
}

#[test]
fn scope_all_cached_creates_no_worktree_and_runs_no_setup() {
    let fx = Fx::new("cached", "setup = [\"echo setup >> $LOG\"]\nchecks = [\"echo check:$(pwd) >> $LOG\"]\n", "out.txt");
    fx.gate();
    let first = fx.log();
    assert_eq!(first.len(), 2, "{first:?}");
    assert_eq!(first[0], "setup");
    assert!(first[1].contains("rvow-rerun-"), "the first run checks in the rerun tree: {first:?}");
    std::fs::remove_file(&fx.log).unwrap();
    let again = fx.gate();
    assert!(again.contains("cached[1]"), "{again}");
    let second = fx.log();
    assert_eq!(second.len(), 1, "setup ran or the check was skipped: {second:?}");
    assert!(second[0].starts_with("check:") && !second[0].contains("rvow-rerun-"), "no rerun tree was made: {second:?}");
}

#[test]
fn scope_a_python_task_skips_cpp_setup_and_checks() {
    let config = "setup = [\"echo plain-setup >> $LOG\", { run = \"echo cpp-setup >> $LOG\", files = [\"src/**\"] }, { run = \"echo py-setup >> $LOG\", files = [\"python/**\"] }]\nchecks = [{ run = \"echo cpp-check >> $LOG\", files = [\"src/**\"] }, { run = \"echo py-check >> $LOG\", files = [\"python/**\"] }, \"echo plain-check >> $LOG\"]\n";
    let fx = Fx::new("python", config, "out.txt, python/**");
    let out = fx.gate();
    assert!(out.contains("passed: true"), "{out}");
    assert_eq!(fx.log(), ["plain-setup", "py-setup", "py-check", "plain-check"]);
}

#[test]
fn scope_a_changed_file_of_the_branch_selects_entries() {
    let config = "setup = [{ run = \"echo cpp-setup >> $LOG\", files = [\"src/**\"] }, { run = \"echo py-setup >> $LOG\", files = [\"python/**\"] }]\n";
    let fx = Fx::new("changed", config, "out.txt, python/**");
    fx.commit("src/a.txt", "changed");
    fx.gate();
    assert_eq!(fx.log(), ["cpp-setup", "py-setup"]);
}

#[test]
fn scope_entries_parse_as_strings_or_tables_and_keep_the_vow_text_for_strings() {
    let fx = Fx::new("shape", "setup = [\"make\", { run = \"cmake\", files = [\"src/**\"] }]\nchecks = [\"lint\"]\n", "out.txt");
    let project = store::find(&fx.root.join("repo"), &fx.root.join("rh")).unwrap().unwrap();
    let cfg = config::load(&project, &fx.root.join("rh")).unwrap();
    assert_eq!(cfg.setup[0], Step::Plain("make".into()));
    assert_eq!(cfg.setup[1].run(), "cmake");
    assert_eq!(cfg.setup[1].files(), Some(&["src/**".to_string()][..]));
    let json = serde_json::to_string(&cfg).unwrap();
    assert!(json.contains("\"checks\":[\"lint\"]"), "a plain entry is still a string: {json}");

    std::fs::write(fx.root.join("rh/config.toml"), "setup = [{ run = \"x\" }]\n").unwrap();
    assert!(config::load(&project, &fx.root.join("rh")).is_err(), "a table needs files");
    std::fs::write(fx.root.join("rh/config.toml"), "setup = [{ run = \"x\", files = [\"a\"], extra = 1 }]\n").unwrap();
    assert!(config::load(&project, &fx.root.join("rh")).is_err(), "unknown keys are refused");
}
