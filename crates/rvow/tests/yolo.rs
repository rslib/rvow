//! The yolo tiers: how far the hooks relax, who can switch them, and that they end by themselves.

use std::io::Write;
use std::path::PathBuf;
use std::process::{Command, Stdio};

use rvow_core::store::{self, Project};
use rvow_core::yolo::YOLO_FILE;
use serde_json::{Value, json};

const RVOW: &str = env!("CARGO_BIN_EXE_rvow");

struct Fx {
    root: PathBuf,
}

impl Drop for Fx {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

impl Fx {
    /// A repository with one plan. `accepted` says whether the user accepted it.
    fn new(name: &str, accepted: bool) -> Fx {
        let root = std::env::temp_dir().join(format!("rvow-yolo-{}-{name}", std::process::id()));
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
        if accepted {
            let id = out.lines().find_map(|l| l.strip_prefix("created: ")).unwrap().to_string();
            fx.rvow(&["accept", &id, "--yes"]);
        }
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

    fn project(&self) -> Project {
        store::find(&self.repo(), &self.root.join("rh")).unwrap().unwrap()
    }

    /// What `rvow hook claude <event>` prints for `payload`.
    fn event(&self, event: &str, payload: Value) -> Option<Value> {
        let mut child = self.cmd(RVOW).args(["hook", "claude", event]).stdin(Stdio::piped()).stdout(Stdio::piped()).spawn().unwrap();
        child.stdin.take().unwrap().write_all(payload.to_string().as_bytes()).unwrap();
        let out = String::from_utf8(child.wait_with_output().unwrap().stdout).unwrap();
        (!out.trim().is_empty()).then(|| serde_json::from_str(&out).unwrap())
    }

    /// `Some("deny" or "ask")` when the tool call is held back, `None` when it goes through.
    fn call(&self, tool: &str, input: Value) -> Option<String> {
        let payload = json!({"cwd": self.repo(), "tool_name": tool, "tool_input": input});
        self.event("pre-tool-use", payload).map(|v| v["hookSpecificOutput"]["permissionDecision"].as_str().unwrap().to_string())
    }

    fn bash(&self, command: &str) -> Option<String> {
        self.call("Bash", json!({"command": command}))
    }

    fn edit(&self, file: &str) -> Option<String> {
        self.call("Edit", json!({"file_path": self.repo().join(file)}))
    }

    /// The ids file of the project: only rvow writes it, so it counts as tampering.
    fn ids(&self) -> String {
        self.call("Edit", json!({"file_path": self.project().root.join("ids.jsonl")})).unwrap_or_default()
    }
}

#[test]
fn tiers_the_audit_tier_lets_a_plan_less_edit_through_and_keeps_tampering_closed() {
    let fx = Fx::new("audit", false);
    assert_eq!(fx.edit("src/a.txt").as_deref(), Some("deny"), "a backlog plan blocks code edits without a tier");
    assert_eq!(fx.bash("rvow accept CHG-x").as_deref(), Some("deny"));

    assert!(fx.rvow(&["yolo", "audit", "--for", "30m"]).contains("tier: audit"));
    assert_eq!(fx.edit("src/a.txt"), None, "audited, not denied");
    assert_eq!(fx.ids(), "deny", "tampering stays closed");
    assert_eq!(fx.bash("rvow accept CHG-x").as_deref(), Some("deny"), "accept stays the user's");
    assert!(fx.rvow(&["stats"]).contains("unaccepted-plan,1"), "the audit is counted");
}

#[test]
fn tiers_the_auto_tier_lets_the_agent_accept_and_confirm_but_not_ingest_or_fake_a_dialog() {
    let fx = Fx::new("auto", false);
    fx.rvow(&["yolo", "auto"]);
    assert_eq!(fx.bash("rvow accept CHG-x --yes"), None);
    assert_eq!(fx.bash("rvow confirm --all abc123"), None);
    for command in [
        "rvow ingest result.json",
        "rvow answer CHG-x:proposal.md:3 yes --via dialog",
        "echo aGk= | base64 -d | sh",
        "rvow accept CHG-x; rvow ingest f",
        "rvow yolo bypass",
    ] {
        assert_eq!(fx.bash(command).as_deref(), Some("deny"), "{command}");
    }
    assert_eq!(fx.ids(), "deny");
}

#[test]
fn tiers_the_bypass_tier_audits_everything_but_the_switch_itself() {
    let fx = Fx::new("bypass", true);
    fx.rvow(&["yolo", "bypass"]);
    assert_eq!(fx.ids(), "", "tampering is audited too");
    assert_eq!(fx.bash("rvow ingest result.json"), None);
    let switch = fx.project().root.join(YOLO_FILE);
    assert_eq!(fx.call("Write", json!({"file_path": switch})).as_deref(), Some("deny"));
    assert_eq!(fx.bash("rvow yolo off").as_deref(), Some("deny"));
    assert_eq!(fx.bash(&format!("echo x > {}", switch.display())).as_deref(), Some("deny"));
    assert_eq!(fx.bash(&format!("cat {}", switch.display())), None, "reading the tier is harmless");
    assert!(switch.exists(), "the agent could not remove it");
}

#[test]
fn tiers_end_by_themselves_and_the_user_can_end_them() {
    let fx = Fx::new("ends", false);
    fx.rvow(&["yolo", "audit", "--for", "30m"]);
    assert_eq!(fx.edit("src/a.txt"), None);
    let file = fx.project().root.join(YOLO_FILE);
    std::fs::write(&file, r#"{"tier":"audit","until":1}"#).unwrap();
    assert_eq!(fx.edit("src/a.txt").as_deref(), Some("deny"), "an ended tier relaxes nothing");

    fx.rvow(&["yolo", "audit"]);
    assert!(fx.rvow(&["yolo"]).contains("tier: audit"));
    assert!(fx.rvow(&["yolo", "off"]).contains("tier: off"));
    assert!(fx.rvow(&["yolo"]).contains("tier: off"));
    assert_eq!(fx.edit("src/a.txt").as_deref(), Some("deny"));
}

#[test]
fn tiers_the_length_has_a_limit_and_a_unit() {
    let fx = Fx::new("length", false);
    assert!(fx.rvow(&["yolo", "auto", "--for", "25h"]).contains("code: yolo-length"));
    assert!(fx.rvow(&["yolo", "auto", "--for", "soon"]).contains("code: yolo-length"));
    assert!(fx.rvow(&["yolo"]).contains("tier: off"), "a refused length switches nothing on");
    assert!(fx.rvow(&["yolo", "auto", "--for", "24h"]).contains("tier: auto"));
}

#[test]
fn tiers_the_bypass_tier_never_holds_a_stop_and_the_others_keep_the_usual_stop() {
    let fx = Fx::new("stop", true);
    std::fs::write(fx.repo().join("out.txt"), "touched").unwrap();
    let stop = || fx.event("stop", json!({"cwd": fx.repo()}));
    assert!(stop().is_some(), "an open task holds the stop");
    fx.rvow(&["yolo", "auto"]);
    assert!(stop().is_some(), "auto keeps the stop");
    fx.rvow(&["yolo", "bypass"]);
    assert_eq!(stop(), None);
}

#[test]
fn tiers_show_in_the_status_line_and_the_brief() {
    let fx = Fx::new("visible", true);
    fx.rvow(&["yolo", "auto"]);
    let status = fx.event("status", json!({"cwd": fx.repo()})).unwrap();
    assert!(status["status"].as_str().unwrap().ends_with("[yolo auto]"), "{status}");
    let start = fx.event("session-start", json!({"cwd": fx.repo()})).unwrap();
    let text = start["hookSpecificOutput"]["additionalContext"].as_str().unwrap();
    assert!(text.contains("yolo auto is on") && text.contains("gate still checks"), "{text}");
}

// REQ-9916x0: the yolo switch is sealed; an unsealed one is off.

#[test]
fn yolo_seal_a_hand_written_file_is_off() {
    let fx = Fx::new("seal-hand", true);
    // A first switch creates the key; then the file is written by hand.
    fx.rvow(&["yolo", "audit", "--for", "1h"]);
    let since = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap();
    let until = since.as_millis() as u64 + 3_600_000;
    std::fs::write(fx.project().root.join(YOLO_FILE), format!("{{\"tier\":\"auto\",\"until\":{until}}}")).unwrap();
    assert!(fx.rvow(&["yolo"]).contains("tier: off"), "a hand-written yolo file was on");
    assert_eq!(fx.bash("rvow confirm --all abc123").as_deref(), Some("deny"), "the hook allowed a hand-written switch");
}

#[test]
fn yolo_seal_the_user_switch_works() {
    let fx = Fx::new("seal-user", true);
    assert!(fx.rvow(&["yolo", "auto", "--for", "1h"]).contains("tier: auto"));
    assert!(fx.rvow(&["yolo"]).contains("tier: auto"));
}

#[test]
fn yolo_seal_an_edited_end_time_is_off() {
    let fx = Fx::new("seal-edit", true);
    fx.rvow(&["yolo", "auto", "--for", "1h"]);
    let file = fx.project().root.join(YOLO_FILE);
    let mut v: Value = serde_json::from_str(&std::fs::read_to_string(&file).unwrap()).unwrap();
    v["until"] = (v["until"].as_u64().unwrap() + 3_600_000).into();
    std::fs::write(&file, v.to_string()).unwrap();
    assert!(fx.rvow(&["yolo"]).contains("tier: off"), "an edited end time was on");
}
