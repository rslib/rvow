//! The hooks give each reason once and stay out of the way of subagents and git operations.

use std::io::Write;
use std::path::PathBuf;
use std::process::{Command, Stdio};

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

/// The one change folder under an `active` folder of the rvow home.
fn active_change(home: &std::path::Path) -> PathBuf {
    let mut stack = vec![home.to_path_buf()];
    while let Some(dir) = stack.pop() {
        for e in std::fs::read_dir(&dir).unwrap().flatten() {
            let path = e.path();
            if path.is_dir() && path.parent().is_some_and(|p| p.ends_with("active")) {
                return path;
            }
            if path.is_dir() {
                stack.push(path);
            }
        }
    }
    panic!("no active change under {}", home.display());
}

impl Fx {
    /// A repository with one accepted change of one task that writes `out.txt`.
    fn new(name: &str) -> Fx {
        let root = std::env::temp_dir().join(format!("rvow-quiet-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("repo")).unwrap();
        std::fs::create_dir_all(root.join("home")).unwrap();
        let mut fx = Fx { root: root.canonicalize().unwrap(), change: PathBuf::new(), task: String::new() };
        fx.git(&["init", "-q", "-b", "main"]);
        std::fs::write(fx.repo().join("a.txt"), "a").unwrap();
        fx.git(&["add", "a.txt"]);
        fx.git(&["commit", "-q", "-m", "init"]);
        fx.rvow(&["init"]);
        let out = fx.rvow(&["new", "--spec", "Out"]);
        let field = |k: &str| out.lines().find_map(|l| l.strip_prefix(&format!("{k}: "))).unwrap().to_string();
        let (id, dir) = (field("created"), PathBuf::from(field("path")));
        std::fs::write(
            dir.join("tasks.md"),
            "- TSK-new1 Write ok\n  covers: REQ-new1\n  verify: grep -q ok out.txt && echo 'test result: ok. 1 passed; 0 failed;'\n  files: out.txt, tests/**\n",
        )
        .unwrap();
        std::fs::write(
            dir.join("specs/out.md"),
            "## ADD REQ-new1 Out\nrvow SHALL write ok.\n### Scenario: s\n- WHEN it runs\n- THEN out.txt holds ok\n",
        )
        .unwrap();
        fx.rvow(&["accept", &id, "--yes"]);
        let _ = dir;
        fx.change = active_change(&fx.root.join("rh"));
        let tasks = std::fs::read_to_string(fx.change.join("tasks.md")).unwrap();
        fx.task = tasks.lines().find_map(|l| l.strip_prefix("- ")?.split_whitespace().next().map(String::from)).unwrap();
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

    /// The reason of one Stop hook call, or `None` when the stop is allowed.
    fn stop(&self, extra: Value) -> Option<String> {
        let mut payload = json!({"cwd": self.repo()});
        for (k, v) in extra.as_object().unwrap() {
            payload[k] = v.clone();
        }
        let mut child = self.cmd(RVOW).args(["hook", "claude", "stop"]).stdin(Stdio::piped()).stdout(Stdio::piped()).spawn().unwrap();
        child.stdin.take().unwrap().write_all(payload.to_string().as_bytes()).unwrap();
        let out = String::from_utf8(child.wait_with_output().unwrap().stdout).unwrap();
        (!out.trim().is_empty()).then(|| serde_json::from_str::<Value>(&out).unwrap()["reason"].as_str().unwrap().to_string())
    }

    /// The deny reason of one Bash call, or `None` when it is allowed.
    fn bash(&self, command: &str) -> Option<String> {
        let payload = json!({"cwd": self.repo(), "tool_name": "Bash", "tool_input": {"command": command}});
        let mut child =
            self.cmd(RVOW).args(["hook", "claude", "pre-tool-use"]).stdin(Stdio::piped()).stdout(Stdio::piped()).spawn().unwrap();
        child.stdin.take().unwrap().write_all(payload.to_string().as_bytes()).unwrap();
        let out = String::from_utf8(child.wait_with_output().unwrap().stdout).unwrap();
        (!out.trim().is_empty()).then(|| {
            serde_json::from_str::<Value>(&out).unwrap()["hookSpecificOutput"]["permissionDecisionReason"].as_str().unwrap().to_string()
        })
    }

    fn touch(&self, text: &str) {
        std::fs::write(self.repo().join("out.txt"), text).unwrap();
    }
}

#[test]
fn stop_second_stop_in_a_session_passes() {
    let fx = Fx::new("twice");
    fx.touch("ok");
    let s = json!({"session_id": "s1"});
    assert!(fx.stop(s.clone()).unwrap().contains(&fx.task));
    assert_eq!(fx.stop(s), None);
}

#[test]
fn stop_a_new_reason_blocks_once() {
    let fx = Fx::new("new-reason");
    fx.touch("ok");
    let s = json!({"session_id": "s1"});
    assert!(fx.stop(s.clone()).unwrap().contains("has no proof"));
    fx.rvow(&["prove", &fx.task.clone()]);
    assert_eq!(fx.stop(s.clone()), None);
    fx.touch("ok changed");
    assert!(fx.stop(s.clone()).unwrap().contains("stale"));
    assert_eq!(fx.stop(s), None);
}

#[test]
fn stop_another_session_is_told_again() {
    let fx = Fx::new("sessions");
    fx.touch("ok");
    assert!(fx.stop(json!({"session_id": "s1"})).is_some());
    assert_eq!(fx.stop(json!({"session_id": "s1"})), None);
    assert!(fx.stop(json!({"session_id": "s2"})).is_some());
}

#[test]
fn stop_without_a_session_id_keeps_blocking() {
    let fx = Fx::new("anon");
    fx.touch("ok");
    assert!(fx.stop(json!({})).is_some());
    assert!(fx.stop(json!({})).is_some());
}

#[test]
fn stop_a_subagent_or_a_continued_stop_passes() {
    let fx = Fx::new("sub");
    fx.touch("ok");
    assert_eq!(fx.stop(json!({"agent_id": "a1", "agent_type": "general-purpose"})), None);
    assert_eq!(fx.stop(json!({"stop_hook_active": true})), None);
    assert!(fx.stop(json!({"stop_hook_active": false})).is_some());
}

#[test]
fn stop_during_a_git_operation_passes() {
    let fx = Fx::new("rebase");
    fx.touch("ok");
    let marker = fx.repo().join(".git/rebase-merge");
    std::fs::create_dir(&marker).unwrap();
    assert_eq!(fx.stop(json!({})), None);
    std::fs::remove_dir(&marker).unwrap();
    assert!(fx.stop(json!({})).unwrap().contains(&fx.task));
}

#[test]
fn stop_plan_changed_message_lists_three_items_and_a_count() {
    let fx = Fx::new("drift");
    let tasks = fx.change.join("tasks.md");
    let original = std::fs::read_to_string(&tasks).unwrap();
    let proposal = fx.change.join("proposal.md");
    let proposal_text = std::fs::read_to_string(&proposal).unwrap();

    let id = &fx.task;
    std::fs::write(
        &tasks,
        format!("- {id} Write it\n  covers: REQ-new1\n  verify: true\n  files: out.txt, tests/**\n\n- TSK-new9 Another\n  covers: REQ-new1\n  verify: true\n  files: **\n"),
    )
    .unwrap();
    std::fs::write(&proposal, format!("{proposal_text}\nmore\n")).unwrap();
    let reason = fx.stop(json!({})).unwrap();
    assert!(reason.contains("the plan changed"), "{reason}");
    assert!(reason.contains("; and ") && reason.contains(" more)"), "{reason}");
    assert!(reason.contains("covers changed; and 1 more)"), "{reason}");

    std::fs::write(&proposal, proposal_text).unwrap();
    std::fs::write(&tasks, original.replace("Write ok", "Write it")).unwrap();
    // A title change is free drift, so it is not a reason to hold the stop.
    let one = fx.stop(json!({})).unwrap_or_default();
    assert!(!one.contains("the plan changed"), "{one}");
}

#[test]
fn shell_reads_of_the_hooks_folder_are_allowed_and_writes_are_not() {
    let fx = Fx::new("hooks-dir");
    for command in [
        "ls .git/hooks",
        "cat .git/hooks/pre-push",
        "ls .git/hooks 2>&1 | head -5",
        "grep -n rvow .git/hooks/pre-push",
        // Running the hook while a writer targets another file.
        ".git/hooks/pre-push origin url 2>&1 | tee out.log",
        ".git/hooks/pre-push origin url > out.log 2>&1",
    ] {
        assert_eq!(fx.bash(command), None, "{command}");
    }
    for command in [
        "echo x >> .git/hooks/pre-push",
        "rm .git/hooks/pre-push",
        "sed -i s/a/b/ .git/hooks/pre-push",
        "chmod -x .git/hooks/pre-push",
        "echo x | tee .git/hooks/pre-push",
        "cp /tmp/copy .git/hooks/pre-push",
    ] {
        let reason = fx.bash(command).unwrap_or_else(|| panic!("allowed: {command}"));
        assert!(reason.contains("rvow install"), "{command}: {reason}");
    }
}

#[test]
fn shell_reads_of_an_enforcement_file_are_allowed_and_writes_are_not() {
    let fx = Fx::new("enforcement");
    let settings = fx.root.join("home/.claude/settings.json");
    std::fs::create_dir_all(settings.parent().unwrap()).unwrap();
    std::fs::write(&settings, "{}").unwrap();
    let s = settings.display();
    for command in [format!("cat {s} 2>&1"), format!("grep rvow {s} 2>/dev/null"), format!("sed -n 1,5p {s}"), format!("ls {s} | head")] {
        assert_eq!(fx.bash(&command), None, "{command}");
    }
    for command in [format!("echo x > {s}"), format!("sed -i s/a/b/ {s}"), format!("rm {s}")] {
        assert!(fx.bash(&command).is_some(), "allowed: {command}");
    }
}

#[test]
fn shell_the_hidden_checks_stay_closed() {
    let fx = Fx::new("hidden");
    let file = fx.root.join("rh/hidden-checks/x.sh");
    for command in [format!("cat {}", file.display()), format!("ls {} 2>&1", file.parent().unwrap().display())] {
        assert!(fx.bash(&command).unwrap_or_else(|| panic!("allowed: {command}")).contains("hidden"), "{command}");
    }
}

#[test]
fn shell_text_that_names_a_protected_action_is_allowed() {
    let fx = Fx::new("text");
    for command in [
        "git commit -m \"explain core.hooksPath\"",
        "git commit -m \"read .git/hooks first\"",
        "echo \"core.hooksPath and .git/hooks\"",
        "cat <<'EOF'\ncore.hooksPath\ngit config core.hooksPath /x\nEOF",
        "git config core.hooksPath",
        "git config --get core.hooksPath",
    ] {
        assert_eq!(fx.bash(command), None, "{command}");
    }
    for command in [
        "git config core.hooksPath /tmp/h",
        "git -c core.hooksPath=/tmp/h commit -m x",
        "git config --unset core.hooksPath",
        "bash <<EOF\ncat > .git/hooks/pre-push\nEOF",
    ] {
        assert!(fx.bash(command).is_some(), "allowed: {command}");
    }
}

#[test]
fn shell_reads_of_the_state_files_and_text_that_names_the_secret_folder_are_allowed() {
    let fx = Fx::new("state-reads");
    let root = fx.change.parent().unwrap().parent().unwrap().to_path_buf();
    let state = ["ledger.jsonl", "ids.jsonl", "hook-stats.jsonl", "leases.jsonl", "rerun-cache.jsonl", "yolo.json"];
    for name in state {
        let file = root.join(name);
        for command in
            [format!("cat {}", file.display()), format!("grep x {} | head -3", file.display()), format!("ls -l {} 2>&1", file.display())]
        {
            assert_eq!(fx.bash(&command), None, "{command}");
        }
        for command in
            [format!("echo x >> {}", file.display()), format!("rm {}", file.display()), format!("cat a | tee {}", file.display())]
        {
            assert!(fx.bash(&command).is_some(), "allowed: {command}");
        }
    }
    let secret = fx.root.join("rh/hidden-checks/x.sh");
    for command in
        [format!("git commit -m \"never read {}\"", secret.display()), format!("echo \"the checks live in {}\"", secret.display())]
    {
        assert_eq!(fx.bash(&command), None, "{command}");
    }
    for command in
        [format!("cat {}", secret.display()), format!("echo $(cat {})", secret.display()), format!("echo x > {}", secret.display())]
    {
        assert!(fx.bash(&command).is_some(), "allowed: {command}");
    }
}
