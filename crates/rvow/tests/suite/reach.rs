//! Reachability: rvow usable from any agent (CHG-jngd0d). Each test drives
//! the built binary the way an agent would.

use std::io::Write;
use std::path::PathBuf;
use std::process::{Command, Stdio};

use rvow_core::install;

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
    fn new(name: &str) -> Fx {
        let root = std::env::temp_dir().join(format!("rvow-reach-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("repo")).unwrap();
        let fx = Fx { root: root.canonicalize().unwrap() };
        fx.git(&["init", "-q", "-b", "main"]);
        std::fs::write(fx.repo().join("a.txt"), "a").unwrap();
        fx.git(&["add", "a.txt"]);
        fx.git(&["commit", "-q", "-m", "init"]);
        fx
    }

    fn repo(&self) -> PathBuf {
        self.root.join("repo")
    }

    fn cmd_in(&self, dir: &std::path::Path) -> Command {
        let mut c = Command::new(RVOW);
        c.current_dir(dir)
            .env("HOME", self.root.join("home"))
            .env("RVOW_HOME", self.root.join("rh"))
            .env("CLAUDE_CONFIG_DIR", self.root.join("claude"))
            .env("PATH", "/usr/bin:/bin:/usr/sbin:/sbin")
            .env_remove("XDG_CONFIG_HOME")
            .env_remove("XDG_DATA_HOME")
            .env_remove("PI_CODING_AGENT_DIR")
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_AUTHOR_NAME", "t")
            .env("GIT_AUTHOR_EMAIL", "t@t")
            .env("GIT_COMMITTER_NAME", "t")
            .env("GIT_COMMITTER_EMAIL", "t@t");
        c
    }

    fn cmd(&self) -> Command {
        self.cmd_in(&self.root.clone())
    }

    fn git(&self, args: &[&str]) {
        assert!(
            Command::new("git")
                .args(args)
                .current_dir(self.repo())
                .env("GIT_CONFIG_GLOBAL", "/dev/null")
                .env("GIT_CONFIG_NOSYSTEM", "1")
                .env("GIT_AUTHOR_NAME", "t")
                .env("GIT_AUTHOR_EMAIL", "t@t")
                .env("GIT_COMMITTER_NAME", "t")
                .env("GIT_COMMITTER_EMAIL", "t@t")
                .status()
                .unwrap()
                .success(),
            "git {args:?}"
        );
    }

    fn hook_session_start(&self) -> String {
        let mut child = self
            .cmd_in(&self.repo())
            .args(["hook", "claude", "session-start"])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .spawn()
            .unwrap();
        let payload = serde_json::json!({"cwd": self.repo()});
        child.stdin.take().unwrap().write_all(payload.to_string().as_bytes()).unwrap();
        let out = child.wait_with_output().unwrap();
        assert!(out.status.success(), "hook session-start failed");
        String::from_utf8(out.stdout).unwrap()
    }

    fn rvow(&self, args: &[&str]) -> String {
        let out = self.cmd_in(&self.repo()).args(args).output().unwrap();
        assert!(out.status.success(), "rvow {args:?}: {}", String::from_utf8_lossy(&out.stdout));
        String::from_utf8(out.stdout).unwrap()
    }

    /// A project with one accepted change of one missing task; returns the change id.
    fn accepted(&self) -> String {
        std::fs::create_dir_all(self.root.join("home")).unwrap();
        self.rvow(&["init"]);
        let out = self.rvow(&["new", "--spec", "Out"]);
        let field = |k: &str| out.lines().find_map(|l| l.strip_prefix(&format!("{k}: "))).unwrap().to_string();
        let (id, dir) = (field("created"), PathBuf::from(field("path")));
        std::fs::write(dir.join("tasks.md"), "- TSK-new1 Write ok\n  covers: REQ-new1\n  verify: test -f out.txt\n  files: out.txt\n")
            .unwrap();
        std::fs::write(
            dir.join("specs/out.md"),
            "## ADD REQ-new1 Out\nrvow SHALL write ok.\n### Scenario: s\n- WHEN it runs\n- THEN out.txt holds ok\n",
        )
        .unwrap();
        self.rvow(&["accept", &id, "--yes"]);
        id
    }

    fn install(&self, pi: Option<&std::path::Path>) -> String {
        let mut c = self.cmd();
        c.args(["install", "--claude", "settings"]);
        match pi {
            Some(pi) => {
                c.env("PI_CODING_AGENT_DIR", pi);
            }
            None => {
                c.env_remove("PI_CODING_AGENT_DIR");
            }
        }
        let out = c.output().unwrap();
        assert!(out.status.success(), "rvow install failed: {}", String::from_utf8_lossy(&out.stdout));
        String::from_utf8(out.stdout).unwrap()
    }
}

#[test]
fn reach_pi_skill_written_matches_the_claude_skill() {
    let fx = Fx::new("pi-written");
    let pi = fx.root.join("pi");
    std::fs::create_dir_all(&pi).unwrap();
    fx.install(Some(&pi));
    let pi_skill = std::fs::read_to_string(pi.join("skills/rvow/SKILL.md")).unwrap();
    let claude_skill = std::fs::read_to_string(fx.root.join("claude/skills/rvow/SKILL.md")).unwrap();
    assert_eq!(pi_skill, claude_skill, "the pi skill holds the same text as the Claude skill");
}

#[test]
fn reach_pi_skill_keeps_a_user_edit_and_names_it_kept() {
    let fx = Fx::new("pi-kept");
    let pi = fx.root.join("pi");
    std::fs::create_dir_all(&pi).unwrap();
    fx.install(Some(&pi));
    let path = pi.join("skills/rvow/SKILL.md");
    let current = std::fs::read_to_string(&path).unwrap();
    std::fs::write(&path, format!("{current}my note\n")).unwrap();
    let out = fx.install(Some(&pi));
    assert!(std::fs::read_to_string(&path).unwrap().ends_with("my note\n"), "the edit survives reinstall");
    assert!(out.contains("kept") && out.contains("SKILL.md"), "the output names the kept file: {out}");
    let raw = current.lines().filter(|l| !l.contains("rvow-managed skill-sha256")).collect::<Vec<_>>().join("\n") + "\n";
    let stale =
        install::stale_managed_files(&fx.root.join("claude"), &fx.root.join("home/.local/share/rvow/claude-plugin"), Some(&pi), &raw);
    assert!(stale.contains(&path), "a stale pi skill is reported: {stale:?}");
}

#[test]
fn reach_skill_description_leads_with_the_trigger() {
    let fx = Fx::new("skill-desc");
    let pi = fx.root.join("pi");
    std::fs::create_dir_all(&pi).unwrap();
    fx.install(Some(&pi));
    let text = std::fs::read_to_string(fx.root.join("claude/skills/rvow/SKILL.md")).unwrap();
    let description = text.lines().find(|l| l.starts_with("description: ")).unwrap().trim_start_matches("description: ").to_string();
    assert!(description.starts_with("Load before you"), "{description}");
    for word in ["accepted change", "CHG-", "TSK-", "denied", "gate", "stale", "ralibi"] {
        assert!(description.contains(word), "{word} missing from {description}");
    }
    assert!(description.contains("load") && description.contains("even one question about rvow"), "{description}");
    assert!(description.ends_with("ralibi"), "it ends with the neighbor skills: {description}");
    assert!(!description.contains(": "), "no colon followed by a space: {description}");
    assert!(description.len() < 600, "{} chars", description.len());
}

#[test]
fn reach_brief_names_the_work_loop_with_the_next_task() {
    let fx = Fx::new("brief-loop");
    let change = fx.accepted();
    let brief = fx.rvow(&["brief"]);
    let loops: Vec<&str> = brief.lines().filter(|l| l.trim_start().starts_with("loop:")).collect();
    assert_eq!(loops.len(), 1, "one loop line: {brief}");
    for word in ["rvow prove TSK-", "rvow amend", &format!("rvow show {change}")] {
        assert!(loops[0].contains(word), "{word} missing from {}: {brief}", loops[0]);
    }
    assert!(!loops[0].contains("rvow_confirm"), "{}", loops[0]);
    assert!(brief.lines().count() <= 25, "{brief}");
    assert!(brief.len() <= 1500, "{} chars", brief.len());
}

#[test]
fn reach_brief_planning_has_no_loop_line() {
    let fx = Fx::new("brief-planning");
    std::fs::create_dir_all(fx.root.join("home")).unwrap();
    fx.rvow(&["init"]);
    fx.rvow(&["new", "Waiting"]);
    let brief = fx.rvow(&["brief"]);
    assert!(brief.contains("waits for accept"), "{brief}");
    assert!(!brief.lines().any(|l| l.starts_with("loop:")), "no loop line in a planning brief: {brief}");
}

#[test]
fn reach_brief_empty_branch_points_to_new_and_the_guide() {
    let fx = Fx::new("brief-empty");
    std::fs::create_dir_all(fx.root.join("home")).unwrap();
    fx.rvow(&["init"]);
    let brief = fx.rvow(&["brief", "--human"]);
    assert_eq!(brief.lines().count(), 1, "exactly one line, no task rules: {brief}");
    assert!(brief.contains("rvow new") && brief.contains("rvow guide proposal"), "{brief}");
    let out = fx.hook_session_start();
    let v: serde_json::Value = serde_json::from_str(&out).unwrap();
    let context = v["hookSpecificOutput"]["additionalContext"].as_str().unwrap();
    assert!(context.contains("rvow new") && context.contains("rvow guide proposal"), "{context}");
}

#[test]
fn reach_brief_branch_without_a_change_points_to_new() {
    let fx = Fx::new("brief-other");
    fx.accepted();
    fx.git(&["checkout", "-q", "-b", "other"]);
    let brief = fx.rvow(&["brief", "--human"]);
    assert!(brief.contains("rvow new") && brief.contains("rvow guide proposal"), "{brief}");
}

fn task_id(brief: &str) -> String {
    let at = brief.find("TSK-").unwrap();
    brief[at..at + 10].to_string()
}

#[test]
fn reach_next_prints_the_next_task() {
    let fx = Fx::new("next-task");
    fx.accepted();
    let task = task_id(&fx.rvow(&["brief"]));
    let out = fx.rvow(&["next"]);
    for word in [task.as_str(), "Write ok", "missing", "test -f out.txt", "out.txt"] {
        assert!(out.contains(word), "{word} missing from {out}");
    }
}

#[test]
fn reach_next_says_so_when_nothing_is_next() {
    let fx = Fx::new("next-empty");
    fx.accepted();
    let task = task_id(&fx.rvow(&["brief"]));
    fx.rvow(&["block", &task, "--reason", "cannot pass as written"]);
    let out = fx.rvow(&["next"]);
    assert!(out.contains("no task is next"), "{out}");
}

#[test]
fn reach_pi_skill_stale_copy_is_reported() {
    let fx = Fx::new("pi-stale");
    let pi = fx.root.join("pi");
    std::fs::create_dir_all(&pi).unwrap();
    fx.install(Some(&pi));
    let path = pi.join("skills/rvow/SKILL.md");
    let other = "---\nname: rvow\ndescription: changed\n---\nChanged body\n";
    let stale =
        install::stale_managed_files(&fx.root.join("claude"), &fx.root.join("home/.local/share/rvow/claude-plugin"), Some(&pi), other);
    assert!(stale.contains(&path), "a stale pi skill is reported: {stale:?}");
    let current = std::fs::read_to_string(&path).unwrap();
    let raw = current.lines().filter(|l| !l.contains("rvow-managed skill-sha256")).collect::<Vec<_>>().join("\n") + "\n";
    let fresh =
        install::stale_managed_files(&fx.root.join("claude"), &fx.root.join("home/.local/share/rvow/claude-plugin"), Some(&pi), &raw);
    assert!(!fresh.contains(&path), "a current pi skill is not reported: {fresh:?}");
}

#[test]
fn reach_pi_skill_no_dir_writes_nothing_and_does_not_fail() {
    let fx = Fx::new("pi-absent");
    fx.install(None);
    assert!(!fx.root.join("home/.pi").exists(), "the default pi dir stays untouched");
    assert!(!fx.root.join("pi").exists(), "no pi skill is written");
}

fn install_mod(fx: &Fx, xdg_data: Option<&std::path::Path>) {
    let mut c = fx.cmd();
    c.arg("install");
    if let Some(d) = xdg_data {
        c.env("XDG_DATA_HOME", d);
    }
    let out = c.output().unwrap();
    assert!(out.status.success(), "rvow install failed: {}", String::from_utf8_lossy(&out.stdout));
}

#[test]
fn reach_mod_lands_in_the_xdg_data_folder_not_the_store() {
    let fx = Fx::new("mod-xdg");
    let data = fx.root.join("data");
    install_mod(&fx, Some(&data));
    assert!(data.join("rvow/claude-plugin/plugins/rvow/hooks/register.tsx").is_file(), "the mod is in the data folder");
    assert!(!fx.root.join("rh/claude-plugin").exists(), "the store holds no mod");
}

#[test]
fn reach_mod_lands_in_local_share_without_xdg_data() {
    let fx = Fx::new("mod-local");
    install_mod(&fx, None);
    assert!(fx.root.join("home/.local/share/rvow/claude-plugin/plugins/rvow/hooks/register.tsx").is_file());
    assert!(!fx.root.join("rh/claude-plugin").exists(), "the store holds no mod");
}

#[test]
fn reach_mod_old_copy_in_the_store_is_removed() {
    let fx = Fx::new("mod-old");
    let old = fx.root.join("rh/claude-plugin");
    install::claude_mod(&old, std::path::Path::new("/bin/rvow"), "---\nname: rvow\ndescription: a\n---\nold\n").unwrap();
    std::fs::write(fx.root.join("rh/keep.txt"), "store data").unwrap();
    install_mod(&fx, None);
    assert!(!old.exists(), "the old mod copy is gone");
    assert!(fx.root.join("rh/keep.txt").is_file(), "the store data stays");
    assert!(fx.root.join("home/.local/share/rvow/claude-plugin/plugins/rvow").is_dir());
}
