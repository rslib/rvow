//! The three tamper holes the hooks close: a denial that must stay a denial,
//! a shell glob that reaches a protected file, and a stop held for the tasks
//! that no pending amendment names.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use rvow_core::store::{self, Project};
use rvow_core::{change, ledger};
use serde_json::{Value, json};

const RVOW: &str = env!("CARGO_BIN_EXE_rvow");

struct Fx {
    root: PathBuf,
    change: PathBuf,
    task: String,
}

impl Drop for Fx {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

impl Fx {
    /// A repository with one accepted change of one task that reads `out.txt`.
    fn new(name: &str) -> Fx {
        let root = std::env::temp_dir().join(format!("rvow-tamper-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("repo/src")).unwrap();
        std::fs::create_dir_all(root.join("home")).unwrap();
        let mut fx = Fx { root: root.canonicalize().unwrap(), change: PathBuf::new(), task: String::new() };
        fx.git(&["init", "-q", "-b", "main"]);
        std::fs::write(fx.repo().join("src/a.txt"), "a").unwrap();
        std::fs::write(fx.repo().join("src/b.rs"), "fn a() {}").unwrap();
        fx.git(&["add", "."]);
        fx.git(&["commit", "-q", "-m", "init"]);
        fx.rvow(&["init"]);
        fx.accepted_change("Out");
        fx
    }

    /// Adds an accepted change on the current branch and points the fixture at it.
    fn accepted_change(&mut self, title: &str) -> String {
        let out = self.rvow(&["new", "--spec", title]);
        let field = |k: &str| out.lines().find_map(|l| l.strip_prefix(&format!("{k}: "))).unwrap().to_string();
        let id = field("created");
        let dir = PathBuf::from(field("path"));
        std::fs::write(
            dir.join("tasks.md"),
            "- TSK-new1 Write ok\n  covers: REQ-new1\n  verify: grep -q ok out.txt && echo 'test result: ok. 1 passed; 0 failed;'\n  files: src/a.txt, out.txt\n",
        )
        .unwrap();
        std::fs::write(
            dir.join("specs/out.md"),
            "## ADD REQ-new1 Out\nrvow SHALL write ok.\n### Scenario: s\n- WHEN it runs\n- THEN out.txt holds ok\n",
        )
        .unwrap();
        self.rvow(&["accept", &id, "--yes"]);
        let path = self.rvow(&["path", &id]);
        self.change = PathBuf::from(path.lines().find_map(|l| l.strip_prefix("path: ")).unwrap());
        let tasks = std::fs::read_to_string(self.change.join("tasks.md")).unwrap();
        self.task = tasks.lines().find_map(|l| l.strip_prefix("- ")?.split_whitespace().next().map(String::from)).unwrap();
        id
    }

    fn repo(&self) -> PathBuf {
        self.root.join("repo")
    }

    fn home(&self) -> PathBuf {
        self.root.join("rh")
    }

    fn cmd(&self, program: &str) -> Command {
        let mut c = Command::new(program);
        c.current_dir(self.repo())
            .env("HOME", self.root.join("home"))
            .env("RVOW_HOME", self.home())
            .env("PATH", "/usr/bin:/bin:/usr/sbin:/sbin")
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_AUTHOR_NAME", "t")
            .env("GIT_AUTHOR_EMAIL", "t@t")
            .env("GIT_COMMITTER_NAME", "t")
            .env("GIT_COMMITTER_EMAIL", "t@t");
        c
    }

    fn git(&self, args: &[&str]) {
        assert!(self.cmd("git").args(args).status().unwrap().success(), "git {args:?}");
    }

    fn rvow(&self, args: &[&str]) -> String {
        let out = self.cmd(RVOW).args(args).output().unwrap();
        assert!(out.status.success(), "rvow {args:?}: {}", String::from_utf8_lossy(&out.stdout));
        String::from_utf8(out.stdout).unwrap()
    }

    fn project(&self) -> Project {
        store::find(&self.repo(), &self.home()).unwrap().unwrap()
    }

    /// The JSON that `rvow hook claude <event>` printed, or `None` when it printed nothing.
    fn event(&self, event: &str, payload: Value) -> Option<Value> {
        self.event_in(event, payload, None)
    }

    /// Like `event`, with HOME set for the hook process when `home` is given.
    fn event_in(&self, event: &str, mut payload: Value, home: Option<&Path>) -> Option<Value> {
        payload["cwd"] = self.repo().to_str().unwrap().into();
        let mut c = self.cmd(RVOW);
        if let Some(home) = home {
            c.env("HOME", home);
        }
        let mut child = c.args(["hook", "claude", event]).stdin(Stdio::piped()).stdout(Stdio::piped()).spawn().unwrap();
        child.stdin.take().unwrap().write_all(payload.to_string().as_bytes()).unwrap();
        let out = String::from_utf8(child.wait_with_output().unwrap().stdout).unwrap();
        (!out.trim().is_empty()).then(|| serde_json::from_str(&out).unwrap())
    }

    fn bash_event(&self, command: &str, session: &str) -> Option<Value> {
        self.event("pre-tool-use", json!({"tool_name": "Bash", "tool_input": {"command": command}, "session_id": session}))
    }

    fn bash_event_home(&self, command: &str, session: &str, home: &Path) -> Option<Value> {
        self.event_in("pre-tool-use", json!({"tool_name": "Bash", "tool_input": {"command": command}, "session_id": session}), Some(home))
    }

    fn decision(out: &Value) -> &str {
        out["hookSpecificOutput"]["permissionDecision"].as_str().unwrap_or("")
    }

    /// The parsed outcome lines of the stats file, with no duration line.
    fn stats(&self) -> Vec<Value> {
        std::fs::read_to_string(self.project().root.join(rvow_core::stats::STATS_FILE))
            .map(|t| t.lines().map(|l| serde_json::from_str::<Value>(l).unwrap()).filter(|l| l.get("duration_ms").is_none()).collect())
            .unwrap_or_default()
    }

    /// The reason of one Stop hook call, or `None` when the stop is allowed.
    fn stop(&self, session: &str) -> Option<String> {
        let out = self.event("stop", json!({"session_id": session}));
        out.map(|v| v["reason"].as_str().unwrap().to_string())
    }

    fn touch(&self, rel: &str) {
        std::fs::write(self.repo().join(rel), "changed").unwrap();
    }

    /// An allowed Write call of `session` on `rel`, which puts the file in the session's record.
    fn edit(&self, session: &str, rel: &str) {
        let input = json!({"file_path": self.repo().join(rel)});
        let call = json!({"tool_name": "Write", "tool_input": input, "session_id": session});
        let mut out = self.event("pre-tool-use", call.clone());
        // The first source edit is denied once for the baseline, and an agent retries it.
        if out.as_ref().is_some_and(|v| v.to_string().contains("has no baseline yet")) {
            out = self.event("pre-tool-use", call);
        }
        assert_eq!(out, None);
    }
}

#[test]
fn backstop_a_tampering_denial_is_never_an_ask() {
    let fx = Fx::new("tamper-deny");
    for _ in 0..3 {
        let out = fx.bash_event("rvow accept CHG-x", "s1").unwrap();
        assert_eq!(Fx::decision(&out), "deny", "{out}");
    }
    let out = fx.bash_event("mv .git/hooks .git/h2", "s1").unwrap();
    assert_eq!(Fx::decision(&out), "deny", "{out}");
    let last = fx.stats().pop().unwrap();
    assert_eq!(last["rule"], "git-hooks", "{last}");
    assert_eq!(last["outcome"], "deny", "{last}");
    assert_eq!(fx.stats().iter().filter(|l| l["outcome"] == "ask").count(), 0, "a tampering denial is never an ask");
}

#[test]
fn backstop_an_owner_command_still_reaches_the_user() {
    let fx = Fx::new("owner-ask");
    for _ in 0..3 {
        fx.bash_event("rvow accept CHG-x", "s1");
    }
    let out = fx.bash_event("rvow accept CHG-x", "s1").unwrap();
    assert_eq!(Fx::decision(&out), "ask", "{out}");
    let last = fx.stats().pop().unwrap();
    assert_eq!(last["rule"], "owner-command", "{last}");
    assert_eq!(last["outcome"], "ask", "{last}");
}

#[test]
fn backstop_a_tampering_denial_keeps_the_streak() {
    let fx = Fx::new("streak");
    for _ in 0..3 {
        fx.bash_event("rvow accept CHG-x", "s1");
    }
    let out = fx.bash_event("echo x >> ledger.jsonl", "s1").unwrap();
    assert_eq!(Fx::decision(&out), "deny", "{out}");
    assert_eq!(fx.stats().pop().unwrap()["rule"], "ledger");
    let out = fx.bash_event("rvow confirm --accept --all x", "s1").unwrap();
    assert_eq!(Fx::decision(&out), "ask", "{out}");
}

#[test]
fn glob_a_glob_on_the_ledger_is_denied() {
    let fx = Fx::new("glob-ledger");
    let target = fx.change.join("ledg?r.jsonl");
    let out = fx.bash_event(&format!("echo x >> {}", target.display()), "s1").unwrap();
    assert_eq!(Fx::decision(&out), "deny", "{out}");
    assert_eq!(fx.stats().pop().unwrap()["rule"], "ledger");
}

#[test]
fn glob_a_glob_on_the_cache_key_is_denied() {
    let fx = Fx::new("glob-key");
    let hidden = fx.home().join("hidden-checks");
    std::fs::create_dir_all(&hidden).unwrap();
    std::fs::write(hidden.join(".cache-key"), "k").unwrap();
    let target = fx.home().join("hidden-chec*/.cache-key");
    let out = fx.bash_event(&format!("cat {}", target.display()), "s1").unwrap();
    assert_eq!(Fx::decision(&out), "deny", "{out}");
    assert_eq!(fx.stats().pop().unwrap()["rule"], "hidden-checks");
}

#[test]
fn glob_an_ordinary_glob_is_allowed() {
    let fx = Fx::new("glob-ok");
    assert_eq!(fx.bash_event("grep -n fn src/*.rs", "s1"), None);
}

#[test]
fn glob_a_glob_that_matches_nothing_is_allowed() {
    let fx = Fx::new("glob-none");
    assert_eq!(fx.bash_event("ls *.zzz", "s1"), None);
}

#[test]
fn glob_a_home_var_glob_on_the_cache_key_is_denied() {
    let fx = Fx::new("glob-home-var");
    let hidden = fx.home().join("hidden-checks");
    std::fs::create_dir_all(&hidden).unwrap();
    std::fs::write(hidden.join(".cache-key"), "k").unwrap();
    let out = fx.bash_event_home("cat $HOME/hidden-ch*/.cache-key", "s1", &fx.home()).unwrap();
    assert_eq!(Fx::decision(&out), "deny", "{out}");
    assert_eq!(fx.stats().pop().unwrap()["rule"], "hidden-checks");
}

#[test]
fn glob_a_braced_home_var_glob_on_the_cache_key_is_denied() {
    let fx = Fx::new("glob-braced-home");
    let hidden = fx.home().join("hidden-checks");
    std::fs::create_dir_all(&hidden).unwrap();
    std::fs::write(hidden.join(".cache-key"), "k").unwrap();
    let out = fx.bash_event_home("cat ${HOME}/hidden-ch*/.cache-key", "s1", &fx.home()).unwrap();
    assert_eq!(Fx::decision(&out), "deny", "{out}");
    assert_eq!(fx.stats().pop().unwrap()["rule"], "hidden-checks");
}

#[test]
fn glob_a_tilde_glob_on_the_cache_key_is_denied() {
    let fx = Fx::new("glob-tilde");
    let hidden = fx.home().join("hidden-checks");
    std::fs::create_dir_all(&hidden).unwrap();
    std::fs::write(hidden.join(".cache-key"), "k").unwrap();
    let out = fx.bash_event_home("cat ~/hidden-ch*/.cache-key", "s1", &fx.home()).unwrap();
    assert_eq!(Fx::decision(&out), "deny", "{out}");
    assert_eq!(fx.stats().pop().unwrap()["rule"], "hidden-checks");
}

#[test]
fn stop_hold_a_design_amendment_does_not_release_a_task() {
    let fx = Fx::new("hold-design");
    fx.touch("src/a.txt");
    fx.edit("s1", "src/a.txt");
    fx.edit("s2", "src/a.txt");
    let first = fx.stop("s1").expect("the unproved task holds the stop");
    assert!(first.contains(&fx.task), "{first}");
    std::fs::write(fx.change.join("design.md"), "# Design\n\nSomething.\n").unwrap();
    fx.rvow(&["amend", "--reason", "design"]);
    assert_eq!(fx.stop("s2"), None, "any pending amendment of the change releases the stop");
}

#[test]
fn stop_hold_an_amendment_that_names_a_task_releases_it() {
    let fx = Fx::new("hold-named");
    fx.touch("src/a.txt");
    fx.edit("s2", "src/a.txt");
    let tasks = std::fs::read_to_string(fx.change.join("tasks.md")).unwrap();
    std::fs::write(fx.change.join("tasks.md"), tasks.replace("grep -q ok", "grep -q changed")).unwrap();
    fx.rvow(&["amend", "--reason", "verify"]);
    assert_eq!(fx.stop("s2"), None, "the amendment names the task, so the stop is allowed");
}

#[test]
fn stop_hold_an_old_amendment_without_task_names_releases() {
    let fx = Fx::new("hold-old");
    fx.touch("src/a.txt");
    let lock = fx.project().lock().unwrap();
    fx.edit("s2", "src/a.txt");
    let mut data = serde_json::Map::new();
    data.insert("amendment".into(), "AMD-old111".into());
    data.insert("reason".into(), "old".into());
    data.insert("items".into(), json!([{"class": "confirm", "what": "proposal changed", "before": null, "after": null}]));
    ledger::append(&fx.change.join(change::LEDGER), &lock, ledger::Kind::Amend, data).unwrap();
    drop(lock);
    assert_eq!(fx.stop("s2"), None, "an old amendment without task names releases the whole change");
}

#[test]
fn stop_hold_an_amendment_on_another_branch_releases_nothing() {
    let mut fx = Fx::new("hold-branch");
    let x_task = fx.task.clone();
    fx.touch("src/a.txt");
    fx.edit("s2", "src/a.txt");
    fx.git(&["checkout", "-q", "-b", "feat/y"]);
    fx.accepted_change("Other");
    let tasks = std::fs::read_to_string(fx.change.join("tasks.md")).unwrap();
    std::fs::write(fx.change.join("tasks.md"), tasks.replace("grep -q ok", "grep -q y")).unwrap();
    fx.rvow(&["amend", "--reason", "y"]);
    fx.git(&["checkout", "-q", "main"]);
    let held = fx.stop("s2").expect("an amendment on another branch releases nothing");
    assert!(held.contains(&x_task), "{held}");
}
