//! Regression tests for the enforcement holes the audits found. Each test
//! drives the real hook or the public rvow_core function the way the audit did.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use rvow_core::hook::{bash_denial, script_denial};
use rvow_core::{gate, install, ledger, store};
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
    fn new(name: &str) -> Fx {
        let root = std::env::temp_dir().join(format!("rvow-fixenf-{}-{name}", std::process::id()));
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

    fn home(&self) -> PathBuf {
        self.root.join("home")
    }

    fn rvow_home(&self) -> PathBuf {
        self.root.join("rh")
    }

    fn cmd(&self, program: &str) -> Command {
        let mut c = Command::new(program);
        c.current_dir(self.repo())
            .env("HOME", self.home())
            .env("RVOW_HOME", self.rvow_home())
            .env("PATH", "/usr/bin:/bin:/usr/sbin:/sbin")
            .env_remove("XDG_CONFIG_HOME")
            .env_remove("XDG_DATA_HOME")
            .env_remove("PI_CODING_AGENT_DIR")
            .env_remove("CLAUDE_CONFIG_DIR")
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

    /// An initialized project.
    fn init(&self) -> store::Project {
        std::fs::create_dir_all(self.home()).unwrap();
        store::init(&self.repo(), &self.rvow_home(), store::Mode::Home).unwrap()
    }

    /// A project with one accepted change of one task; returns the change id and the change folder.
    fn accepted(&self) -> (String, PathBuf) {
        std::fs::create_dir_all(self.home()).unwrap();
        self.rvow(&["init"]);
        let out = self.rvow(&["new", "--spec", "Out"]);
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
        self.rvow(&["accept", &id, "--yes"]);
        (id, dir)
    }

    /// The deny reason of one hook call, or `None` when the call is allowed.
    fn hook(&self, tool: &str, input: Value, env: &[(&str, &Path)]) -> Option<String> {
        let payload = json!({"cwd": self.repo(), "tool_name": tool, "tool_input": input});
        let mut c = self.cmd(RVOW);
        c.args(["hook", "pre-tool-use"]).stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped());
        for (k, v) in env {
            c.env(k, v);
        }
        let mut child = c.spawn().unwrap();
        child.stdin.take().unwrap().write_all(payload.to_string().as_bytes()).unwrap();
        let out = child.wait_with_output().unwrap();
        assert_eq!(String::from_utf8_lossy(&out.stderr), "", "{tool} {input}");
        let text = String::from_utf8(out.stdout).unwrap();
        if text.trim().is_empty() {
            return None;
        }
        let v: Value = serde_json::from_str(&text).unwrap();
        assert_eq!(v["hookSpecificOutput"]["permissionDecision"], "deny", "{text}");
        Some(v["hookSpecificOutput"]["permissionDecisionReason"].as_str().unwrap().to_string())
    }

    fn write(&self, file: &Path) -> Option<String> {
        self.hook("Write", json!({"file_path": file}), &[])
    }

    /// An allowed Write call of session `s1`, which puts the file in the session's record.
    fn write_as_session(&self, file: &Path) {
        let payload = json!({"cwd": self.repo(), "tool_name": "Write", "tool_input": {"file_path": file}, "session_id": "s1"});
        let send = || {
            let mut child = self.cmd(RVOW).args(["hook", "pre-tool-use"]).stdin(Stdio::piped()).stdout(Stdio::piped()).spawn().unwrap();
            child.stdin.take().unwrap().write_all(payload.to_string().as_bytes()).unwrap();
            String::from_utf8(child.wait_with_output().unwrap().stdout).unwrap()
        };
        let mut out = send();
        // The first source edit is denied once for the baseline, and an agent retries it.
        if out.contains("has no baseline yet") {
            out = send();
        }
        assert_eq!(out.trim(), "", "the write of {} was held back", file.display());
    }

    fn bash(&self, command: &str) -> Option<String> {
        self.hook("Bash", json!({"command": command}), &[])
    }
}

// 1. The installed pi extension folder is protected.

#[test]
fn pi_extension_folder_is_protected_in_the_default_and_a_custom_dir() {
    let fx = Fx::new("pi");
    fx.init();
    let custom = fx.root.join("custom-pi");
    let default = fx.home().join(".pi/agent");
    for (dir, env) in [(&default, vec![]), (&custom, vec![("PI_CODING_AGENT_DIR", custom.as_path())])] {
        for file in ["extensions/rvow/index.ts", "extensions/rvow/tools.ts", "extensions/rvow.ts"] {
            let path = dir.join(file);
            for tool in ["Write", "Edit"] {
                let reason = fx.hook(tool, json!({"file_path": path}), &env);
                assert!(reason.is_some_and(|r| r.contains("enforces rvow")), "{tool} {}", path.display());
            }
        }
        let other = dir.join("extensions/other/index.ts");
        assert_eq!(fx.hook("Write", json!({"file_path": other}), &env), None, "{}", other.display());
    }
}

#[test]
fn install_writes_the_pi_extension_into_pi_coding_agent_dir() {
    let fx = Fx::new("pidir");
    fx.init();
    let pi = fx.root.join("custom-pi");
    std::fs::create_dir_all(&pi).unwrap();
    let out = fx.cmd(RVOW).args(["install", "--claude", "settings"]).env("PI_CODING_AGENT_DIR", &pi).output().unwrap();
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stdout));
    assert!(pi.join("extensions/rvow/index.ts").is_file());
    assert!(!fx.home().join(".pi").exists(), "the default dir stays untouched");
}

// 2. `rvow ingest` cannot forge proofs.

#[test]
fn ingest_is_denied_for_agents_and_checks_the_run_against_the_plan() {
    let reason = bash_denial("rvow ingest run.jsonl").unwrap();
    assert!(reason.contains("! rvow ingest"), "{reason}");
    assert!(bash_denial("sh -c 'rvow ingest run.jsonl'").is_some());
    assert_eq!(bash_denial("rvow show x"), None);

    let fx = Fx::new("ingest");
    let (id, _) = fx.accepted();
    std::fs::write(fx.repo().join("out.txt"), "ok").unwrap();
    fx.git(&["add", "out.txt"]);
    fx.git(&["commit", "-q", "-m", "ok"]);
    let gate = fx.cmd(RVOW).args(["gate", "--strict", "--rerun", "--out", "run.jsonl"]).output().unwrap();
    assert!(gate.status.success(), "{gate:?}");
    let file = std::fs::read_to_string(fx.repo().join("run.jsonl")).unwrap();
    let run: ledger::Run = serde_json::from_str(file.lines().next().unwrap()).unwrap();
    assert_eq!(run.change, id);
    let root = fx.rvow_home().join(&run.project);

    let forged = |edit: &dyn Fn(&mut ledger::Run)| {
        let mut r = run.clone();
        edit(&mut r);
        ledger::ingest_run(&root, &r).unwrap_err()
    };
    let err = forged(&|r| {
        r.data.insert("task".into(), "TSK-zzzzzz".into());
    });
    assert!(err.message.contains("TSK-zzzzzz"), "{err:?}");
    let err = forged(&|r| {
        r.data.insert("task_hash".into(), "0".repeat(64).into());
    });
    assert!(err.message.contains("task_hash"), "{err:?}");
    let err = forged(&|r| {
        r.data.insert("requirements".into(), json!({"REQ-new1": "bogus"}));
    });
    assert!(err.message.contains("requirements"), "{err:?}");
    let err = forged(&|r| {
        r.data.remove("task_hash");
    });
    assert!(err.message.contains("task_hash"), "{err:?}");

    let ingested = fx.rvow(&["ingest", "run.jsonl"]);
    assert!(ingested.contains("count: 1") && ingested.contains("skipped: 0"), "{ingested}");
}

// 3. The signing trust file and the hidden checks.

#[test]
fn the_allowed_signers_file_is_an_enforcement_path() {
    let fx = Fx::new("signers");
    let project = fx.init();
    let signers = fx.root.join("trust/allowed_signers");
    std::fs::create_dir_all(signers.parent().unwrap()).unwrap();
    std::fs::write(&signers, "alice ssh-ed25519 AAAA\n").unwrap();
    std::fs::write(project.root.join("config.toml"), format!("allowed_signers = \"{}\"\n", signers.display())).unwrap();
    for tool in ["Write", "Edit"] {
        let reason = fx.hook(tool, json!({"file_path": signers}), &[]);
        assert!(reason.is_some_and(|r| r.contains("enforces rvow")), "{tool}");
    }
    let reason = fx.bash(&format!("echo 'evil ssh-ed25519 BBBB' >> {}", signers.display()));
    assert!(reason.is_some_and(|r| r.contains("enforces rvow")));
    assert_eq!(fx.bash(&format!("cat {}", signers.display())), None, "reading the trust file is fine");
    assert_eq!(fx.write(&fx.root.join("trust/notes.txt")), None);
}

#[test]
fn the_hidden_checks_are_denied_for_every_tool_and_for_shell_commands() {
    let fx = Fx::new("hidden");
    fx.init();
    let hidden = fx.rvow_home().join("hidden-checks/local/repo/TSK-x7k2q4.sh");
    for (tool, key) in [
        ("Write", "file_path"),
        ("Edit", "file_path"),
        ("Read", "file_path"),
        ("Grep", "path"),
        ("Glob", "path"),
        ("mcp__fs__read_file", "path"),
    ] {
        let reason = fx.hook(tool, json!({key: hidden}), &[]);
        assert!(reason.is_some_and(|r| r.contains("hidden")), "{tool}");
    }
    for command in [
        format!("cat {}", hidden.display()),
        format!("ls {}", hidden.parent().unwrap().display()),
        "cat \"$RVOW_HOME\"/hidden-checks/x".to_string(),
        "grep -r secret hidden\"\"-checks".to_string(),
    ] {
        let reason = fx.bash(&command);
        assert!(reason.is_some_and(|r| r.contains("hidden")), "{command}");
    }
    assert_eq!(fx.hook("Read", json!({"file_path": fx.repo().join("a.txt")}), &[]), None);
}

// 4. Every deny reason names a concrete next step.

/// A command in backticks, a slash command, or a `run rvow ...` / `run git ...` phrase.
fn has_concrete_step(reason: &str) -> bool {
    let ticks: Vec<&str> = reason.split('`').collect();
    let backticked = ticks.len() >= 3 && ticks.iter().skip(1).step_by(2).any(|c| !c.trim().is_empty());
    let lower = reason.to_lowercase();
    backticked || lower.contains("/rvow-") || lower.contains("run rvow ") || lower.contains("run git ")
}

#[test]
fn every_deny_reason_of_the_production_functions_names_a_concrete_step() {
    let fx = Fx::new("texts");
    std::fs::create_dir_all(fx.repo().join("tests")).unwrap();
    std::fs::write(fx.repo().join("tests/t.sh"), "exit 0").unwrap();
    let (id, dir) = fx.accepted();
    let project_root = dir.parent().unwrap().parent().unwrap().to_path_buf();
    let claude = fx.home().join(".claude/settings.json");

    let mut reasons: Vec<(String, String)> = Vec::new();
    let mut edit_reasons: Vec<(String, String)> = Vec::new();
    let mut edit = |what: &str, file: &Path| {
        let r = fx.write(file).unwrap_or_else(|| panic!("{what} was allowed"));
        reasons.push((what.to_string(), r));
    };
    edit("enforcement file", &claude);
    edit("ledger", &dir.join("ledger.jsonl"));
    edit("ids", &project_root.join("ids.jsonl"));
    edit("archive", &project_root.join("archive/x/tasks.md"));
    edit("living spec", &project_root.join("specs/cap/spec.md"));
    edit("git dir", &fx.repo().join(".git/hooks/pre-push"));
    edit("hooks config", &fx.repo().join(".claude/settings.json"));
    // These two rules are in audit mode, so the hook lets the call through; the finding still holds the text.
    for (what, file) in [("protected test", "tests/t.sh"), ("outside scope", "docs/readme.md")] {
        let project = store::find(&fx.repo(), &fx.rvow_home()).unwrap().unwrap();
        let payload = json!({"cwd": fx.repo(), "tool_name": "Write", "tool_input": {"file_path": fx.repo().join(file)}});
        let found = rvow_core::hook::pre_tool_use_denial(&project, &payload).unwrap();
        assert_eq!(fx.write(&fx.repo().join(file)), None, "{what} is audited, not denied");
        edit_reasons.push((what.to_string(), found.unwrap_or_else(|| panic!("{what} was not found")).reason));
    }
    edit("hidden check", &fx.rvow_home().join("hidden-checks/x.sh"));

    let bash = [
        "rvow accept x",
        "rvow confirm --accept",
        "rvow ingest f",
        "rvow answer CHG-x:proposal.md:3 yes --via dialog",
        "git push --no-verify",
        "git commit -qn -m x",
        "git config core.hooksPath /tmp/x",
        "rm .git/hooks/pre-push",
        "echo x >> ids.jsonl",
        "echo aGk= | base64 -d | sh",
    ];
    for command in bash {
        let r = fx.bash(command).unwrap_or_else(|| panic!("{command} was allowed"));
        reasons.push((command.to_string(), r));
    }
    let r = fx.bash(&format!("echo {{}} > {}", claude.display())).unwrap();
    reasons.push(("write to enforcement file by shell".into(), r));
    let script = fx.repo().join("go.sh");
    std::fs::write(&script, "#!/bin/sh\nrvow accept x\n").unwrap();
    reasons.push(("script".into(), script_denial("sh go.sh", &fx.repo()).unwrap()));

    // A backlog plan blocks code edits.
    let other = Fx::new("texts-backlog");
    other.rvow(&["init"]);
    other.rvow(&["new", "--spec", "Later"]);
    reasons.push(("unaccepted plan".into(), other.write(&other.repo().join("src/lib.rs")).unwrap()));

    // Stop: an open task, then drift.
    let stop = |fx: &Fx| {
        let mut child = fx.cmd(RVOW).args(["hook", "stop"]).stdin(Stdio::piped()).stdout(Stdio::piped()).spawn().unwrap();
        child.stdin.take().unwrap().write_all(json!({"cwd": fx.repo(), "session_id": "s1"}).to_string().as_bytes()).unwrap();
        let out = String::from_utf8(child.wait_with_output().unwrap().stdout).unwrap();
        serde_json::from_str::<Value>(&out).unwrap()["reason"].as_str().unwrap().to_string()
    };
    std::fs::write(fx.repo().join("out.txt"), "touched").unwrap();
    fx.write_as_session(&fx.repo().join("out.txt"));
    reasons.push(("open task".into(), stop(&fx)));
    // A change to a protected test file needs the user's confirm.
    std::fs::write(fx.repo().join("tests/t.sh"), "exit 1").unwrap();
    fx.write_as_session(&fx.repo().join("tests/t.sh"));
    let drift = stop(&fx);
    assert!(drift.contains("rvow amend --reason") && !drift.contains("/rvow-confirm"), "{drift}");
    reasons.push(("drift".into(), drift));
    let _ = id;

    reasons.extend(edit_reasons);
    for (what, reason) in &reasons {
        let text = reason.strip_prefix("rvow: ").unwrap_or(reason);
        assert!(!text.starts_with("rvow: "), "{what}: doubled prefix: {reason}");
        assert!(!text.starts_with("Never "), "{what}: state the rule in the positive form: {reason}");
        assert!(has_concrete_step(reason), "{what}: no concrete next step: {reason}");
    }
}

#[test]
fn the_step_matcher_is_strict() {
    assert!(!has_concrete_step("rvow: only the user edits it"));
    assert!(!has_concrete_step("run the tests and ask the user"));
    assert!(has_concrete_step("use `rvow block <task>`"));
    assert!(has_concrete_step("ask the user to run /rvow-accept CHG-x"));
}

// 5. Command-text bypasses.

#[test]
fn command_text_bypasses_are_denied_and_lookalikes_are_allowed() {
    let denied = [
        "sh -c 'rvow accept CHG-x'",
        "bash -c \"rvow confirm --accept\"",
        "bash -lc 'rvow accept x'",
        "eval 'rvow accept x'",
        "r=rvow; $r accept x",
        "r=rvow\n${r} accept x",
        "python3 -c \"subprocess.run(['rvow','accept'])\"",
        "echo cnZvdyBhY2NlcHQgeA== | base64 -d | sh",
        "sh -c \"$(echo cnZvdyBhY2NlcHQ= | base64 -d)\"",
        "xargs rvow accept < ids",
        "echo x | xargs -I{} rvow confirm {}",
        "env FOO=1 /usr/bin/rvow accept",
        "find . -exec rvow accept {} \\;",
        "r\"\"vow accept x",
        "\\rvow accept x",
        ">/dev/null rvow accept x",
        "rvow 2>/dev/null accept x",
        "bash <<EOF\nrvow accept x\nEOF",
        "if true; then rvow accept x; fi",
        "git commit -qn -m x",
        "git commit -anm 'msg'",
        "git commit -n",
        "git commit -m x --no-verify",
        "git commit --no-ver -m x",
        "git -C . commit -qn",
        "git push --no-verify",
        "git config core.hooks\\Path /tmp/x",
        "git config core.hooks\"\"Path /tmp/x",
        "mv .git/hoo\"\"ks x",
        "GIT_CONFIG_COUNT=1 GIT_CONFIG_KEY_0=core.hooks\"\"Path GIT_CONFIG_VALUE_0=/tmp git push",
        "cd .git && rm -r hooks",
        "rvow ingest run.jsonl",
        "echo x >> ledger.jsonl",
        "echo '{}' >> ~/.local/share/rvow/x/active/c/ledger.jsonl",
    ];
    for command in denied {
        assert!(bash_denial(command).is_some(), "not denied: {command:?}");
    }
    let allowed = [
        "rvow prove x7k",
        "rvow status",
        "rvow amend --reason 'accept the new name'",
        "cargo test confirm_flow",
        "git commit -m 'confirm accept'",
        "git commit -m 'use -n here'",
        "git commit -m x && head -n 5 log",
        "export -n FOO && git commit -m x",
        "git commit -m x | head -n 5",
        "git commit -am x; git log -n 3",
        "git log --grep commit -n 3",
        "git push",
        "git push -n",
        "rvow show ledger.jsonl",
        "grep -rn accept src/",
        "echo \"run rvow accept X\"",
        "echo 'run rvow confirm X'",
        "cat <<'EOF'\nrvow accept CHG-x\nEOF",
        "python3 -c \"print('accept')\"",
        "base64 -d cfg > x && bash build.sh",
        "echo $(date)",
        "sh -c 'echo hello'",
        "eval 'ls -la'",
        "r=ls; $r accept",
    ];
    for command in allowed {
        assert_eq!(bash_denial(command), None, "denied: {command:?}");
    }
}

#[test]
fn a_script_that_holds_accept_or_confirm_does_not_run_from_the_agent() {
    let fx = Fx::new("scripts");
    fx.init();
    let dir = fx.repo();
    std::fs::write(dir.join("run.sh"), "#!/bin/sh\necho hi\nrvow accept CHG-x\n").unwrap();
    std::fs::write(dir.join("safe.sh"), "#!/bin/sh\nrvow prove x\n").unwrap();
    std::fs::write(dir.join("go.py"), "import subprocess\nsubprocess.run(['rvow', 'confirm'])\n").unwrap();
    std::fs::write(dir.join("indirect.sh"), "#!/bin/sh\nr=rvow\n$r accept x\n").unwrap();
    for command in ["bash ./run.sh", "sh run.sh", "./run.sh", "source run.sh", ". ./run.sh", "python3 go.py", "bash indirect.sh"] {
        assert_eq!(bash_denial(command), None, "the command text alone looks harmless: {command}");
        let reason = script_denial(command, &dir);
        assert!(reason.is_some_and(|r| has_concrete_step(&r)), "{command}");
    }
    for command in ["bash safe.sh", "bash missing.sh", "echo run.sh", "cat run.sh"] {
        assert_eq!(script_denial(command, &dir), None, "{command}");
    }
    let reason = fx.hook("Bash", json!({"command": "bash ./run.sh"}), &[]);
    assert!(reason.is_some_and(|r| r.contains("run.sh")));
    // Known limit: a script written by the same command is not on disk yet.
    assert_eq!(script_denial("printf 'rvow accept x' > new.sh && sh new.sh", &dir), None);
}

// 6. File-writing tools other than the built-in editors.

#[test]
fn writers_with_other_file_argument_names_are_checked() {
    let fx = Fx::new("fileargs");
    fx.init();
    let target = fx.home().join(".claude/settings.json");
    let t = target.to_str().unwrap();
    let denied: Vec<(&str, Value)> = vec![
        ("mcp__fs__write_file", json!({"path": t})),
        ("mcp__fs__write_file", json!({"filename": t})),
        ("mcp__fs__write_file", json!({"file": t})),
        ("mcp__fs__write_file", json!({"dest": t})),
        ("mcp__fs__move_file", json!({"destination": t})),
        ("mcp__fs__write_file", json!({"target": t})),
        ("mcp__fs__copy", json!({"to": t})),
        ("mcp__fs__save", json!({"output": t})),
        ("mcp__fs__save", json!({"out": t})),
        ("mcp__fs__do_it", json!({"file": t})),
        ("apply_patch", json!({"file_path": t})),
        ("apply_patch", json!({"input": format!("*** Begin Patch\n*** Update File: {t}\n@@\n-a\n+b\n*** End Patch")})),
        ("custom_writer", json!({"file": t})),
    ];
    for (tool, input) in denied {
        assert!(fx.hook(tool, input.clone(), &[]).is_some(), "not denied: {tool} {input}");
    }
    let allowed: Vec<(&str, Value)> = vec![
        ("mcp__fs__read_file", json!({"path": t})),
        ("mcp__fs__read_text_file", json!({"filename": t})),
        ("mcp__fs__list_directory", json!({"path": t})),
        ("mcp__fs__search_files", json!({"path": t})),
        ("mcp__fs__get_file_info", json!({"file": t})),
        ("Read", json!({"file_path": t})),
        ("Grep", json!({"path": t})),
        ("mcp__fs__write_file", json!({"filename": fx.root.join("elsewhere.txt")})),
        ("custom_writer", json!({"unrelated": t})),
        ("custom_tool", json!({"query": "settings"})),
    ];
    for (tool, input) in allowed {
        assert_eq!(fx.hook(tool, input.clone(), &[]), None, "denied: {tool} {input}");
    }
}

// 7. The gate hook trusts only the binary that rvow install recorded.

fn executable(path: &Path) {
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).unwrap();
}

fn hook_failures(fx: &Fx, project: &store::Project) -> Vec<String> {
    gate::gate(project, &fx.rvow_home(), &fx.repo(), gate::Options::default(), &mut std::io::sink())
        .unwrap()
        .failures
        .into_iter()
        .filter(|f| f.contains("hook"))
        .collect()
}

#[test]
fn the_manager_hook_must_run_the_recorded_binary_by_an_absolute_path() {
    let fx = Fx::new("gatehook");
    let project = fx.init();
    let real = fx.root.join("rvow");
    std::fs::write(&real, "#!/bin/sh\nexit 0\n").unwrap();
    executable(&real);
    install::pre_push(&fx.repo(), &real).unwrap();
    fx.git(&["config", "core.hooksPath", ".manager"]);
    std::fs::create_dir_all(fx.repo().join(".manager")).unwrap();
    let manager = fx.repo().join(".manager/pre-push");
    let set = |line: &str| {
        std::fs::write(&manager, format!("#!/bin/sh\n{line}\n")).unwrap();
        executable(&manager);
    };

    set(&format!("'{}' gate --strict --rerun", real.display()));
    assert!(hook_failures(&fx, &project).is_empty(), "the recorded binary passes");

    std::fs::create_dir_all(fx.repo().join("x")).unwrap();
    std::fs::write(fx.repo().join("x/rvow"), "#!/bin/sh\necho inside\n").unwrap();
    executable(&fx.repo().join("x/rvow"));
    set("./x/rvow gate --strict --rerun");
    assert!(!hook_failures(&fx, &project).is_empty(), "a relative fake passed as the gate");
    set("x/rvow gate --strict --rerun");
    assert!(!hook_failures(&fx, &project).is_empty());

    let inside = fx.repo().join("x/rvow");
    set(&format!("'{}' gate --strict --rerun", inside.display()));
    assert!(!hook_failures(&fx, &project).is_empty(), "an absolute fake with other content passed");

    let other = fx.root.join("other/rvow");
    std::fs::create_dir_all(other.parent().unwrap()).unwrap();
    std::fs::write(&other, "#!/bin/sh\necho fake\n").unwrap();
    executable(&other);
    set(&format!("'{}' gate --strict --rerun", other.display()));
    assert!(!hook_failures(&fx, &project).is_empty(), "a binary that is not the recorded one passed");
}

/// A fake rvow that records its arguments in `$MARKER`, and a bare remote.
fn chain_setup(fx: &Fx) -> (PathBuf, PathBuf, PathBuf) {
    let bin = fx.root.join("bin/rvow");
    std::fs::create_dir_all(bin.parent().unwrap()).unwrap();
    std::fs::write(&bin, "#!/bin/sh\necho \"$@\" >> \"$MARKER\"\n").unwrap();
    executable(&bin);
    let remote = fx.root.join("remote.git");
    fx.git(&["init", "-q", "--bare", remote.to_str().unwrap()]);
    fx.git(&["remote", "add", "origin", remote.to_str().unwrap()]);
    (bin, remote, fx.root.join("marker"))
}

fn set_pre_push(fx: &Fx, text: &str) -> PathBuf {
    let hook = fx.repo().join(".git/hooks/pre-push");
    std::fs::write(&hook, text).unwrap();
    executable(&hook);
    hook
}

fn push(fx: &Fx, marker: &Path, prev_out: &Path) -> bool {
    fx.cmd("git").args(["push", "-q", "origin", "main"]).env("MARKER", marker).env("PREV_OUT", prev_out).status().unwrap().success()
}

const BASH_HOOK: &str = "#!/usr/bin/env bash\nset -euo pipefail\n# managed hook\n{ echo \"args: $*\"; cat; } > \"$PREV_OUT\"\n";

#[test]
fn install_chains_an_existing_pre_push_hook_and_the_gate_passes() {
    let fx = Fx::new("chain");
    let project = fx.init();
    let (bin, remote, marker) = chain_setup(&fx);
    let hook = set_pre_push(&fx, BASH_HOOK);
    assert!(matches!(install::pre_push(&fx.repo(), &bin).unwrap(), install::Outcome::Chained { .. }));
    let prev = fx.repo().join(".git/hooks/pre-push.rvow-prev");
    assert_eq!(std::fs::read_to_string(&prev).unwrap(), BASH_HOOK);
    assert_eq!(std::fs::read_to_string(&hook).unwrap(), install::pre_push_script(&bin).unwrap());
    {
        use std::os::unix::fs::PermissionsExt;
        assert_ne!(std::fs::metadata(&prev).unwrap().permissions().mode() & 0o111, 0, "the old hook keeps its mode");
    }
    assert!(hook_failures(&fx, &project).is_empty(), "{:?}", hook_failures(&fx, &project));

    let prev_out = fx.root.join("prev-out");
    assert!(push(&fx, &marker, &prev_out));
    let seen = std::fs::read_to_string(&prev_out).unwrap();
    assert!(seen.starts_with(&format!("args: origin {}\n", remote.display())), "{seen}");
    assert!(
        seen.lines().nth(1).is_some_and(|l| l.starts_with("refs/heads/main ") && l.contains(" refs/heads/main ")),
        "the previous hook gets the pushed refs: {seen}"
    );
    assert_eq!(std::fs::read_to_string(&marker).unwrap(), "gate --strict --rerun\n");
}

#[test]
fn a_failing_previous_hook_stops_the_push_before_the_gate() {
    let fx = Fx::new("chain-fail");
    fx.init();
    let (bin, _, marker) = chain_setup(&fx);
    set_pre_push(&fx, "#!/usr/bin/env bash\ncat > /dev/null\nexit 1\n");
    assert!(matches!(install::pre_push(&fx.repo(), &bin).unwrap(), install::Outcome::Chained { .. }));
    assert!(!push(&fx, &marker, &fx.root.join("prev-out")), "the push passed");
    assert!(!marker.exists(), "the gate ran after the previous hook failed");
}

#[test]
fn a_previous_hook_that_rewrites_the_rvow_hook_cannot_skip_the_gate() {
    let fx = Fx::new("chain-rewrite-tail");
    fx.init();
    let (bin, _, marker) = chain_setup(&fx);
    set_pre_push(
        &fx,
        "#!/bin/sh\ncat > /dev/null\nhook=\"${0%.rvow-prev}\"\nat=$(grep -b 'have=' \"$hook\" | cut -d: -f1)\nprintf 'exit 0\\n' | dd of=\"$hook\" bs=1 seek=\"$at\" conv=notrunc 2>/dev/null\n",
    );
    assert!(matches!(install::pre_push(&fx.repo(), &bin).unwrap(), install::Outcome::Chained { .. }));
    push(&fx, &marker, &fx.root.join("prev-out"));
    assert!(std::fs::read_to_string(fx.repo().join(".git/hooks/pre-push")).unwrap().contains("exit 0\n"), "the hook was not rewritten");
    assert_eq!(std::fs::read_to_string(&marker).unwrap_or_default(), "gate --strict --rerun\n", "the gate did not run");
}

#[test]
fn install_keeps_a_previous_hook_that_is_not_text() {
    let fx = Fx::new("chain-binary");
    fx.init();
    let (bin, _, _) = chain_setup(&fx);
    let hook = fx.repo().join(".git/hooks/pre-push");
    std::fs::write(&hook, b"\x7fELF\xff\xfe").unwrap();
    executable(&hook);
    assert!(matches!(install::pre_push(&fx.repo(), &bin).unwrap(), install::Outcome::Chained { .. }));
    assert_eq!(std::fs::read(fx.repo().join(".git/hooks/pre-push.rvow-prev")).unwrap(), b"\x7fELF\xff\xfe");
}

#[test]
fn install_twice_chains_once() {
    let fx = Fx::new("chain-twice");
    fx.init();
    let (bin, _, _) = chain_setup(&fx);
    let hook = set_pre_push(&fx, BASH_HOOK);
    install::pre_push(&fx.repo(), &bin).unwrap();
    let first = std::fs::read_to_string(&hook).unwrap();
    assert!(matches!(install::pre_push(&fx.repo(), &bin).unwrap(), install::Outcome::Unchanged(_)));
    assert_eq!(std::fs::read_to_string(&hook).unwrap(), first);
    let hooks = fx.repo().join(".git/hooks");
    assert_eq!(std::fs::read_to_string(hooks.join("pre-push.rvow-prev")).unwrap(), BASH_HOOK);
    assert!(!hooks.join("pre-push.rvow-prev.rvow-prev").exists());
}

#[test]
fn a_pre_push_hook_rewritten_after_install_fails_the_gate_with_the_reinstall_step() {
    let fx = Fx::new("chain-rewrite");
    let project = fx.init();
    let (bin, _, _) = chain_setup(&fx);
    set_pre_push(&fx, BASH_HOOK);
    install::pre_push(&fx.repo(), &bin).unwrap();
    let hook = set_pre_push(&fx, &format!("{BASH_HOOK}'{}' gate --strict --rerun\n", bin.display()));
    assert_eq!(
        hook_failures(&fx, &project),
        [format!("installed pre-push hook {} is missing or was changed; run rvow install again", hook.display())]
    );
    assert!(matches!(install::pre_push(&fx.repo(), &bin).unwrap(), install::Outcome::Chained { .. }));
    assert!(hook_failures(&fx, &project).is_empty());
}

#[test]
fn a_hooks_path_hook_passes_with_the_printed_line_first_and_fails_with_a_skip_before_it() {
    let fx = Fx::new("first-line");
    let project = fx.init();
    let (bin, _, _) = chain_setup(&fx);
    fx.git(&["config", "core.hooksPath", ".manager"]);
    let line = match install::pre_push(&fx.repo(), &bin).unwrap() {
        install::Outcome::AddLine { line, .. } => line,
        other => panic!("{other:?}"),
    };
    std::fs::create_dir_all(fx.repo().join(".manager")).unwrap();
    let manager = fx.repo().join(".manager/pre-push");
    let set = |text: String| {
        std::fs::write(&manager, text).unwrap();
        executable(&manager);
    };
    let tail = "repo_root=\"$(git rev-parse --show-toplevel)\"\n\"$repo_root/scripts/git-hooks.sh\" run-pre-push\n";
    set(format!("#!/usr/bin/env bash\n# managed\n{line}\n{tail}"));
    assert!(hook_failures(&fx, &project).is_empty(), "{line}");
    for before in ["exit 0", "if false; then", "set -euo pipefail"] {
        set(format!("#!/usr/bin/env bash\n{before}\n{line}\n{tail}"));
        assert!(!hook_failures(&fx, &project).is_empty(), "{before} before the gate passed");
    }
    set(format!("#!/usr/bin/true\n{line}\n"));
    assert!(!hook_failures(&fx, &project).is_empty(), "an interpreter that skips the script passed");
    set(format!("#!/bin/sh\n{} gate --strict --rerun\n{tail}", bin.display()));
    assert!(!hook_failures(&fx, &project).is_empty(), "a first gate line without an exit on failure passed");
    let glued = format!("'{}'gate --strict --rerun || exit 1", bin.display());
    set(format!("#!/bin/sh\n{glued}\n{tail}"));
    assert!(!hook_failures(&fx, &project).is_empty(), "a quoted binary glued to its first argument passed");
    set(format!("#!/bin/sh\n{line}\n{tail}"));
    assert!(hook_failures(&fx, &project).is_empty(), "{line}");
}

// 8. Quoting in the installed scripts and settings.

fn quoted_binary(fx: &Fx) -> (PathBuf, PathBuf) {
    let dir = fx.root.join("it's here");
    std::fs::create_dir_all(&dir).unwrap();
    let bin = dir.join("rvow");
    let marker = fx.root.join("marker");
    std::fs::write(&bin, "#!/bin/sh\necho \"$@\" > \"$MARKER\"\n").unwrap();
    executable(&bin);
    (bin, marker)
}

#[test]
fn a_binary_path_with_a_single_quote_survives_the_pre_push_script() {
    let fx = Fx::new("quote-script");
    let (bin, marker) = quoted_binary(&fx);
    let script = install::pre_push_script(&bin).unwrap();
    let file = fx.root.join("pre-push");
    std::fs::write(&file, &script).unwrap();
    assert!(Command::new("sh").arg("-n").arg(&file).status().unwrap().success(), "{script}");
    let ran = Command::new("sh").arg(&file).env("MARKER", &marker).output().unwrap();
    assert!(ran.status.success(), "{}", String::from_utf8_lossy(&ran.stderr));
    assert_eq!(std::fs::read_to_string(&marker).unwrap().trim(), "gate --strict --rerun");

    fx.init();
    fx.git(&["config", "core.hooksPath", ".manager"]);
    let rvow_line = match install::pre_push(&fx.repo(), &bin).unwrap() {
        install::Outcome::AddLine { line, .. } => line,
        other => panic!("{other:?}"),
    };
    let _ = std::fs::remove_file(&marker);
    let ran = Command::new("sh").arg("-c").arg(&rvow_line).env("MARKER", &marker).status().unwrap();
    assert!(ran.success(), "{rvow_line}");
    assert_eq!(std::fs::read_to_string(&marker).unwrap().trim(), "gate --strict --rerun");
}

#[test]
fn a_binary_path_with_a_single_quote_survives_the_hook_settings() {
    let fx = Fx::new("quote-settings");
    let (bin, marker) = quoted_binary(&fx);
    let settings = fx.root.join("settings.json");
    install::claude_settings(&settings, &bin).unwrap();
    let v: Value = serde_json::from_str(&std::fs::read_to_string(&settings).unwrap()).unwrap();
    let command = v["hooks"]["Stop"][0]["hooks"][0]["command"].as_str().unwrap();
    assert!(Command::new("sh").arg("-n").arg("-c").arg(command).status().unwrap().success(), "{command}");
    assert!(Command::new("sh").arg("-c").arg(command).env("MARKER", &marker).status().unwrap().success());
    assert_eq!(std::fs::read_to_string(&marker).unwrap().trim(), "hook claude stop");
}

#[test]
fn the_neutral_hook_form_is_replaced_on_install_and_bad_settings_name_the_path() {
    let fx = Fx::new("neutral");
    let settings = fx.root.join("settings.json");
    let neutral =
        json!({"hooks": {"PreToolUse": [{"matcher": "Bash", "hooks": [{"type": "command", "command": "'/old/rvow' hook pre-tool-use"}]}]}});
    std::fs::write(&settings, neutral.to_string()).unwrap();
    install::claude_settings(&settings, Path::new("/new/rvow")).unwrap();
    let v: Value = serde_json::from_str(&std::fs::read_to_string(&settings).unwrap()).unwrap();
    assert_eq!(v["hooks"]["PreToolUse"].as_array().unwrap().len(), 1, "{v}");
    assert_eq!(v["hooks"]["PreToolUse"][0]["hooks"][0]["command"], "'/new/rvow' hook claude pre-tool-use");

    for (text, needle) in [("[]", "top level"), (r#"{"hooks": 1}"#, "\"hooks\""), (r#"{"hooks": {"Stop": 1}}"#, "hooks.Stop")] {
        std::fs::write(&settings, text).unwrap();
        let err = install::claude_settings(&settings, Path::new("/new/rvow")).unwrap_err();
        assert!(err.message.contains(&settings.display().to_string()) && err.message.contains(needle), "{err:?}");
        assert!(err.fix.contains("rvow install") && err.fix.contains(&settings.display().to_string()), "{err:?}");
        assert_ne!(err.fix, "fix the file");
    }
}

// 9. A stale install is noticed.

#[test]
fn a_changed_bundled_skill_is_named_as_stale() {
    let fx = Fx::new("stale-lib");
    let claude = fx.home().join(".claude");
    let old = "---\nname: rvow\ndescription: a\n---\nold text\n";
    let new = "---\nname: rvow\ndescription: a\n---\nnew text\n";
    install::skill(&claude, old).unwrap();
    let stale = |text: &str| install::stale_managed_files(&claude, &fx.home().join(".local/share/rvow/claude-plugin"), None, text);
    assert_eq!(stale(old), Vec::<PathBuf>::new());
    assert_eq!(stale(new), vec![claude.join("skills/rvow/SKILL.md")]);
    install::skill(&claude, new).unwrap();
    assert_eq!(stale(new), Vec::<PathBuf>::new());
}

#[test]
fn session_start_warns_about_a_stale_skill_mod_and_pi_file() {
    let fx = Fx::new("stale-cli");
    let repo_project = fx.init();
    let _ = repo_project;
    let claude = fx.home().join(".claude");
    let pi = fx.home().join(".pi/agent");
    install::skill(&claude, "---\nname: rvow\ndescription: a\n---\nold text\n").unwrap();
    install::pi_extension(&pi).unwrap();
    let tools = pi.join("extensions/rvow/tools.ts");
    let body = "export const old = 1;\n";
    std::fs::write(&tools, format!("// rvow-managed sha256:{}\n{body}", rvow_core::vow::sha256_hex(body.as_bytes()))).unwrap();
    let plugin = fx.home().join(".local/share/rvow/claude-plugin/plugins/rvow/hooks");
    std::fs::create_dir_all(&plugin).unwrap();
    std::fs::write(plugin.join("format.ts"), "// old\n").unwrap();

    let mut child = fx.cmd(RVOW).args(["hook", "session-start"]).stdin(Stdio::piped()).stdout(Stdio::piped()).spawn().unwrap();
    child.stdin.take().unwrap().write_all(json!({"cwd": fx.repo()}).to_string().as_bytes()).unwrap();
    let out = String::from_utf8(child.wait_with_output().unwrap().stdout).unwrap();
    let v: Value = serde_json::from_str(&out).unwrap();
    let context = v["hookSpecificOutput"]["additionalContext"].as_str().unwrap();
    for path in [claude.join("skills/rvow/SKILL.md"), tools, plugin.join("format.ts")] {
        assert!(context.contains(&path.display().to_string()), "{context}");
    }
    assert!(!context.contains("extensions/rvow/index.ts"), "a current file is not named: {context}");
}
