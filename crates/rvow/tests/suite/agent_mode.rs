//! Leases: one task for one subagent.

use std::path::PathBuf;
use std::process::Command;

use rvow_core::lease::{self, LEASE_FILE, TTL_MS};
use rvow_core::store::{self, Project};

const RVOW: &str = env!("CARGO_BIN_EXE_rvow");
const HOUR: u64 = 3_600_000;

struct Fx {
    root: PathBuf,
    task: String,
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
    /// A repository with one accepted task that wants `out.txt` to hold `ok`.
    fn new(name: &str) -> Fx {
        let root = std::env::temp_dir().join(format!("rvow-agent-mode-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("repo/src")).unwrap();
        let root = root.canonicalize().unwrap();
        let mut fx = Fx { root, task: String::new() };
        let repo = fx.repo();
        std::fs::write(repo.join("src/a.txt"), "a").unwrap();
        for args in [vec!["init", "-q", "-b", "main"], vec!["add", "."], vec!["commit", "-q", "-m", "init"]] {
            assert!(git_command(&repo).args(&args).status().unwrap().success(), "git {args:?}");
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
        let located = fx.rvow(&["path", &id]);
        let tasks =
            std::fs::read_to_string(PathBuf::from(located.lines().find_map(|l| l.strip_prefix("path: ")).unwrap().trim()).join("tasks.md"))
                .unwrap();
        fx.task = tasks.lines().find_map(|l| l.strip_prefix("- ")?.split_whitespace().next().map(String::from)).unwrap();
        fx
    }

    fn repo(&self) -> PathBuf {
        self.root.join("repo")
    }

    fn home(&self) -> PathBuf {
        self.root.join("rh")
    }

    fn rvow(&self, args: &[&str]) -> String {
        let out = Command::new(RVOW)
            .args(args)
            .current_dir(self.repo())
            .env("HOME", self.root.join("home"))
            .env("RVOW_HOME", self.home())
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

    fn project(&self) -> Project {
        store::find(&self.repo(), &self.home()).unwrap().unwrap()
    }

    fn lease_lines(&self) -> usize {
        std::fs::read_to_string(self.project().root.join(LEASE_FILE)).map_or(0, |t| t.lines().count())
    }

    fn prove_ok(&self) {
        std::fs::write(self.repo().join("out.txt"), "ok").unwrap();
        self.rvow(&["prove", &self.task]);
    }

    fn now(&self) -> u64 {
        std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_millis() as u64
    }
}

#[test]
fn lease_delegate_prints_the_brief_and_records_a_lease() {
    let fx = Fx::new("brief");
    let out = fx.rvow(&["delegate", &fx.task]);
    let lines: Vec<&str> = out.lines().collect();
    assert!(out.contains(&format!("lease: {}", fx.task)), "{out}");
    assert!(out.contains("out.txt, src/**") && out.contains("grep -q ok out.txt"), "{out}");
    assert!(out.contains("tmux -L") && out.contains("rvow prove") && out.contains("rvow block"), "{out}");
    assert!(lines.len() < 25, "the brief stays short: {out}");
    assert_eq!(fx.lease_lines(), 1);
    let leases = lease::leases(&fx.project()).unwrap();
    assert_eq!((leases[0].task.as_str(), leases[0].agent_id.as_deref()), (fx.task.as_str(), None));
}

#[test]
fn lease_a_proved_or_blocked_task_is_refused() {
    let fx = Fx::new("refused");
    fx.prove_ok();
    let out = fx.rvow(&["delegate", &fx.task]);
    assert!(out.contains("code: lease") && out.contains("proved") && out.contains("rvow status"), "{out}");
    assert_eq!(fx.lease_lines(), 0, "{out}");

    let blocked = Fx::new("blocked");
    blocked.rvow(&["block", &blocked.task, "--reason", "cannot pass"]);
    let out = blocked.rvow(&["delegate", &blocked.task]);
    assert!(out.contains("code: lease") && out.contains("blocked"), "{out}");
    assert_eq!(blocked.lease_lines(), 0);
}

#[test]
fn lease_an_unknown_task_is_an_error_that_names_a_next_step() {
    let fx = Fx::new("unknown");
    let out = fx.rvow(&["delegate", "TSK-zzzzzz"]);
    assert!(out.contains("rvow status"), "{out}");
    assert_eq!(fx.lease_lines(), 0);
}

#[test]
fn lease_ends_when_stopped_old_proved_or_blocked() {
    let fx = Fx::new("ends");
    let (p, home, repo) = (fx.project(), fx.home(), fx.repo());
    let hold = |agent: &str, at: u64| lease::holder(&p, &home, &repo, agent, at).unwrap();

    fx.rvow(&["delegate", &fx.task]);
    assert!(hold("a1", fx.now()).is_none(), "an unbound lease belongs to nobody");
    assert!(lease::bind(&p, &fx.task, "a1").unwrap());
    assert!(!lease::bind(&p, &fx.task, "a2").unwrap(), "a lease binds once");
    assert_eq!(hold("a1", fx.now()).unwrap().task, fx.task);
    assert!(hold("a1", fx.now() + 4 * HOUR - 60_000).is_some());
    assert!(hold("a1", fx.now() + TTL_MS + HOUR).is_none(), "older than 4 hours");

    lease::end(&p, "a1").unwrap();
    assert!(hold("a1", fx.now()).is_none(), "the subagent stopped");

    fx.rvow(&["delegate", &fx.task]);
    assert!(lease::bind(&p, &fx.task, "a3").unwrap());
    assert!(hold("a3", fx.now()).is_some());
    fx.prove_ok();
    assert!(hold("a3", fx.now()).is_none(), "the task is proved");
}

#[test]
fn lease_ends_when_the_task_is_blocked() {
    let fx = Fx::new("ends-blocked");
    let (p, home, repo) = (fx.project(), fx.home(), fx.repo());
    fx.rvow(&["delegate", &fx.task]);
    assert!(lease::bind(&p, &fx.task, "a1").unwrap());
    assert!(lease::holder(&p, &home, &repo, "a1", fx.now()).unwrap().is_some());
    fx.rvow(&["block", &fx.task, "--reason", "cannot pass"]);
    assert!(lease::holder(&p, &home, &repo, "a1", fx.now()).unwrap().is_none());
}

// The hook judges a bound subagent by its lease.

impl Fx {
    /// Two tasks: the first owns `src/**`, the second `docs/**`. Returns both ids.
    fn two(name: &str) -> (Fx, String, String) {
        let root = std::env::temp_dir().join(format!("rvow-agent-mode-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("repo/src")).unwrap();
        std::fs::create_dir_all(root.join("repo/docs")).unwrap();
        let fx = Fx { root: root.canonicalize().unwrap(), task: String::new() };
        let repo = fx.repo();
        std::fs::write(repo.join("src/a.txt"), "a").unwrap();
        std::fs::write(repo.join("docs/a.md"), "a").unwrap();
        for args in [vec!["init", "-q", "-b", "main"], vec!["add", "."], vec!["commit", "-q", "-m", "init"]] {
            assert!(git_command(&repo).args(&args).status().unwrap().success(), "git {args:?}");
        }
        fx.rvow(&["init"]);
        let out = fx.rvow(&["new", "--spec", "Two"]);
        let dir = PathBuf::from(out.lines().find_map(|l| l.strip_prefix("path: ")).unwrap());
        let pass = "echo 'test result: ok. 1 passed; 0 failed;'";
        std::fs::write(
            dir.join("tasks.md"),
            format!(
                "- TSK-new1 Code\n  covers: REQ-new1\n  verify: test -f code.txt && {pass}\n  files: src/**, code.txt\n\n- TSK-new2 Docs\n  covers: REQ-new1\n  verify: test -f docs.txt && {pass}\n  files: docs/**, docs.txt\n"
            ),
        )
        .unwrap();
        std::fs::write(
            dir.join("specs/out.md"),
            "## ADD REQ-new1 Two\nrvow SHALL do both.\n### Scenario: s\n- WHEN it runs\n- THEN both are done\n",
        )
        .unwrap();
        let id = out.lines().find_map(|l| l.strip_prefix("created: ")).unwrap().to_string();
        fx.rvow(&["accept", &id, "--yes"]);
        let located = fx.rvow(&["path", &id]);
        let tasks =
            std::fs::read_to_string(PathBuf::from(located.lines().find_map(|l| l.strip_prefix("path: ")).unwrap().trim()).join("tasks.md"))
                .unwrap();
        let ids: Vec<String> = tasks.lines().filter_map(|l| l.strip_prefix("- ")?.split_whitespace().next().map(String::from)).collect();
        (fx, ids[0].clone(), ids[1].clone())
    }

    /// The deny reason of one hook call, or `None` when it is allowed.
    fn hook(&self, tool: &str, input: serde_json::Value, agent: Option<&str>) -> Option<String> {
        use std::io::Write;
        use std::process::Stdio;
        let mut payload = serde_json::json!({"cwd": self.repo(), "tool_name": tool, "tool_input": input});
        if let Some(a) = agent {
            payload["agent_id"] = a.into();
        }
        let mut c = Command::new(RVOW);
        c.args(["hook", "claude", "pre-tool-use"])
            .current_dir(self.repo())
            .env("HOME", self.root.join("home"))
            .env("RVOW_HOME", self.home())
            .env("PATH", "/usr/bin:/bin:/usr/sbin:/sbin")
            .env_remove("GIT_DIR")
            .env_remove("GIT_WORK_TREE")
            .env_remove("GIT_INDEX_FILE")
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped());
        let mut child = c.spawn().unwrap();
        child.stdin.take().unwrap().write_all(payload.to_string().as_bytes()).unwrap();
        let out = String::from_utf8(child.wait_with_output().unwrap().stdout).unwrap();
        (!out.trim().is_empty()).then(|| {
            serde_json::from_str::<serde_json::Value>(&out).unwrap()["hookSpecificOutput"]["permissionDecisionReason"]
                .as_str()
                .unwrap()
                .to_string()
        })
    }
}

#[test]
fn scope_a_bound_subagent_edits_inside_its_lease_and_is_denied_outside() {
    let (fx, code, _docs) = Fx::two("scope");
    fx.rvow(&["delegate", &code]);
    assert!(lease::bind(&fx.project(), &code, "a1").unwrap());
    let src = fx.repo().join("src/a.txt");
    let doc = fx.repo().join("docs/a.md");

    assert_eq!(fx.hook("Edit", serde_json::json!({"file_path": src}), Some("a1")), None, "inside the lease");
    let reason = fx.hook("Edit", serde_json::json!({"file_path": doc}), Some("a1")).expect("outside the lease");
    assert!(
        reason.contains(&code) && reason.contains("docs/a.md") && reason.contains("src/**") && reason.contains("rvow block"),
        "{reason}"
    );

    assert_eq!(fx.hook("Edit", serde_json::json!({"file_path": doc}), None), None, "the main agent has no lease");
    assert_eq!(fx.hook("Edit", serde_json::json!({"file_path": doc}), Some("a2")), None, "another subagent has no lease");
    assert_eq!(fx.hook("Read", serde_json::json!({"file_path": doc}), Some("a1")), None, "a read is not an edit");
}

#[test]
fn scope_a_bound_subagent_proves_only_its_own_task() {
    let (fx, code, docs) = Fx::two("scope-prove");
    fx.rvow(&["delegate", &code]);
    assert!(lease::bind(&fx.project(), &code, "a1").unwrap());
    let bash = |cmd: &str, agent: Option<&str>| fx.hook("Bash", serde_json::json!({"command": cmd}), agent);

    assert_eq!(bash(&format!("rvow prove {code}"), Some("a1")), None, "its own task");
    assert_eq!(bash(&format!("rvow prove {code} 2>&1 | tail -5"), Some("a1")), None, "with a pipe");
    for cmd in [format!("rvow prove {docs}"), "rvow prove --stale".to_string(), format!("rvow prove {code}; rvow prove {docs}")] {
        let reason = bash(&cmd, Some("a1")).unwrap_or_else(|| panic!("allowed: {cmd}"));
        assert!(reason.contains(&code) && reason.contains("rvow prove"), "{cmd}: {reason}");
    }
    assert_eq!(bash(&format!("rvow prove {docs}"), None), None, "the main agent proves any task");
    assert_eq!(bash(&format!("rvow prove {docs}"), Some("a2")), None, "another subagent has no lease");
}

#[test]
fn scope_the_lease_stops_applying_when_the_task_is_proved() {
    let (fx, code, _docs) = Fx::two("scope-ends");
    fx.rvow(&["delegate", &code]);
    assert!(lease::bind(&fx.project(), &code, "a1").unwrap());
    let doc = fx.repo().join("docs/a.md");
    assert!(fx.hook("Edit", serde_json::json!({"file_path": doc}), Some("a1")).is_some());
    std::fs::write(fx.repo().join("code.txt"), "x").unwrap();
    fx.rvow(&["prove", &code]);
    assert_eq!(fx.hook("Edit", serde_json::json!({"file_path": doc}), Some("a1")), None);
}

#[test]
fn scope_the_lease_file_is_closed_to_writes_and_open_to_reads() {
    let (fx, code, _docs) = Fx::two("scope-file");
    fx.rvow(&["delegate", &code]);
    let file = fx.project().root.join(LEASE_FILE);
    for tool in ["Edit", "Write"] {
        let reason = fx.hook(tool, serde_json::json!({"file_path": file}), None).unwrap_or_else(|| panic!("{tool} allowed"));
        assert!(reason.contains(LEASE_FILE) && reason.contains("rvow delegate"), "{reason}");
    }
    for command in [format!("echo x >> {}", file.display()), format!("rm {}", file.display()), format!("sed -i s/a/b/ {}", file.display())]
    {
        let reason = fx.hook("Bash", serde_json::json!({"command": command}), None).unwrap_or_else(|| panic!("allowed: {command}"));
        assert!(reason.contains(LEASE_FILE), "{reason}");
    }
    // Reading shows who holds which lease and nothing secret.
    assert_eq!(fx.hook("Read", serde_json::json!({"file_path": file}), None), None);
    for command in [format!("cat {}", file.display()), format!("grep TSK {} | head -3", file.display())] {
        assert_eq!(fx.hook("Bash", serde_json::json!({"command": command}), None), None, "{command}");
    }
}

#[test]
fn lease_stale_after_prove_does_not_bring_a_bound_lease_back() {
    let (fx, code, _docs) = Fx::two("stale-bound");
    fx.rvow(&["delegate", &code]);
    assert!(lease::bind(&fx.project(), &code, "a1").unwrap());
    std::fs::write(fx.repo().join("code.txt"), "x").unwrap();
    fx.rvow(&["prove", &code]);
    std::fs::write(fx.repo().join("src/a.txt"), "changed").unwrap();
    let status = fx.rvow(&["status"]);
    assert!(status.contains("stale"), "{status}");

    let (p, home, repo) = (fx.project(), fx.home(), fx.repo());
    assert!(lease::holder(&p, &home, &repo, "a1", fx.now()).unwrap().is_none(), "the lease ended at prove");
    assert!(lease::leases(&p).unwrap().iter().all(|l| l.ended));
    let doc = repo.join("docs/a.md");
    assert_eq!(fx.hook("Edit", serde_json::json!({"file_path": doc}), Some("a1")), None, "the usual rules apply");
}

#[test]
fn lease_stale_after_prove_ends_an_unbound_lease_too() {
    let (fx, code, _docs) = Fx::two("stale-unbound");
    fx.rvow(&["delegate", &code]);
    std::fs::write(fx.repo().join("code.txt"), "x").unwrap();
    fx.rvow(&["prove", &code]);
    std::fs::write(fx.repo().join("src/a.txt"), "changed").unwrap();
    assert!(!lease::bind(&fx.project(), &code, "a1").unwrap(), "an ended lease cannot be bound");
    assert!(lease::leases(&fx.project()).unwrap().iter().all(|l| l.ended));
}

#[test]
fn lease_stale_after_block_ends_bound_and_unbound_leases() {
    let (fx, code, _docs) = Fx::two("stale-blocked");
    fx.rvow(&["delegate", &code]);
    assert!(lease::bind(&fx.project(), &code, "a1").unwrap());
    fx.rvow(&["delegate", &code]);
    fx.rvow(&["block", &code, "--reason", "cannot pass"]);
    let (p, home, repo) = (fx.project(), fx.home(), fx.repo());
    assert!(lease::leases(&p).unwrap().iter().all(|l| l.ended));
    assert!(!lease::bind(&p, &code, "a2").unwrap(), "an ended lease cannot be bound");
    assert!(lease::holder(&p, &home, &repo, "a1", fx.now()).unwrap().is_none());
}

// The mod binds a new subagent to the lease its prompt names.

impl Fx {
    /// The JSON that `rvow hook claude <event>` printed, or `None` when it printed nothing.
    fn event(&self, event: &str, payload: serde_json::Value) -> Option<serde_json::Value> {
        use std::io::Write;
        use std::process::Stdio;
        let mut c = Command::new(RVOW);
        c.args(["hook", "claude", event])
            .current_dir(self.repo())
            .env("HOME", self.root.join("home"))
            .env("RVOW_HOME", self.home())
            .env("PATH", "/usr/bin:/bin:/usr/sbin:/sbin")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped());
        let mut payload = payload;
        payload["cwd"] = self.repo().to_str().unwrap().into();
        let mut child = c.spawn().unwrap();
        child.stdin.take().unwrap().write_all(payload.to_string().as_bytes()).unwrap();
        let out = String::from_utf8(child.wait_with_output().unwrap().stdout).unwrap();
        (!out.trim().is_empty()).then(|| serde_json::from_str(&out).unwrap())
    }
}

#[test]
fn bind_a_prompt_with_a_lease_line_binds_the_agent_and_scopes_its_edits() {
    let (fx, code, _docs) = Fx::two("bind");
    let brief = fx.rvow(&["delegate", &code]);
    let out = fx.event("lease-bind", serde_json::json!({"agent_id": "a1", "prompt": format!("{brief}\nStart now.")})).unwrap();
    assert_eq!(out["bound"], true, "{out}");
    assert_eq!(out["task"], code.as_str());
    assert_eq!(lease::leases(&fx.project()).unwrap()[0].agent_id.as_deref(), Some("a1"));
    let doc = fx.repo().join("docs/a.md");
    assert!(fx.hook("Edit", serde_json::json!({"file_path": doc}), Some("a1")).is_some(), "the bound agent is scoped");
}

#[test]
fn bind_an_unknown_task_or_a_missing_marker_binds_nothing() {
    let (fx, code, _docs) = Fx::two("bind-none");
    fx.rvow(&["delegate", &code]);
    let unknown = fx.event("lease-bind", serde_json::json!({"agent_id": "a1", "prompt": "lease: TSK-zzzzzz\nwork"})).unwrap();
    assert_eq!(unknown["bound"], false, "{unknown}");
    assert_eq!(fx.event("lease-bind", serde_json::json!({"agent_id": "a1", "prompt": "Read the README."})), None);
    assert_eq!(fx.event("lease-bind", serde_json::json!({"prompt": format!("lease: {code}")})), None, "no agent id, nothing to bind");
    assert!(lease::leases(&fx.project()).unwrap().iter().all(|l| l.agent_id.is_none()));
    assert_eq!(lease::marker("text\n  lease: TSK-abc123  \nmore").as_deref(), Some("TSK-abc123"));
    assert_eq!(lease::marker("lease: TSK-ABC123"), None);
    assert_eq!(lease::marker("lease: TSK-abc"), None);
}

#[test]
fn bind_a_stopping_subagent_ends_its_lease() {
    let (fx, code, _docs) = Fx::two("bind-end");
    fx.rvow(&["delegate", &code]);
    fx.event("lease-bind", serde_json::json!({"agent_id": "a1", "prompt": format!("lease: {code}")}));
    let doc = fx.repo().join("docs/a.md");
    assert!(fx.hook("Edit", serde_json::json!({"file_path": doc}), Some("a1")).is_some());
    assert_eq!(fx.event("lease-end", serde_json::json!({"agent_id": "a1"})), None);
    assert_eq!(fx.hook("Edit", serde_json::json!({"file_path": doc}), Some("a1")), None, "the lease ended with the subagent");
}

#[test]
fn bind_next_lease_after_the_old_task_is_proved() {
    let (fx, code, docs) = Fx::two("bind-next");
    let first = fx.rvow(&["delegate", &code]);
    let out = fx.event("lease-bind", serde_json::json!({"agent_id": "a1", "prompt": first})).unwrap();
    assert_eq!(out["bound"], true, "{out}");
    std::fs::write(fx.repo().join("code.txt"), "x").unwrap();
    fx.rvow(&["prove", &code]);

    let second = fx.rvow(&["delegate", &docs]);
    let out = fx.event("lease-bind", serde_json::json!({"agent_id": "a1", "prompt": second})).unwrap();
    assert_eq!((out["bound"].clone(), out["task"].clone()), (true.into(), docs.as_str().into()), "{out}");
    assert!(out.get("reason").is_none(), "{out}");
    let (src, doc) = (fx.repo().join("src/a.txt"), fx.repo().join("docs/a.md"));
    assert_eq!(fx.hook("Edit", serde_json::json!({"file_path": doc}), Some("a1")), None, "inside the new lease");
    let reason = fx.hook("Edit", serde_json::json!({"file_path": src}), Some("a1")).expect("outside the new lease");
    assert!(reason.contains(&docs) && reason.contains("src/a.txt"), "{reason}");
}

#[test]
fn bind_next_lease_issued_earlier_is_the_one_held() {
    let (fx, code, docs) = Fx::two("bind-next-older");
    let first = fx.rvow(&["delegate", &code]);
    let second = fx.rvow(&["delegate", &docs]);
    let out = fx.event("lease-bind", serde_json::json!({"agent_id": "a1", "prompt": second})).unwrap();
    assert_eq!(out["bound"], true, "{out}");
    std::fs::write(fx.repo().join("docs.txt"), "x").unwrap();
    fx.rvow(&["prove", &docs]);

    let out = fx.event("lease-bind", serde_json::json!({"agent_id": "a1", "prompt": first})).unwrap();
    assert_eq!(out["bound"], true, "{out}");
    let (p, home, repo) = (fx.project(), fx.home(), fx.repo());
    let held = lease::holder(&p, &home, &repo, "a1", fx.now()).unwrap().expect("the lease bound last is held");
    assert_eq!(held.task, code);
    let doc = repo.join("docs/a.md");
    assert!(fx.hook("Edit", serde_json::json!({"file_path": doc}), Some("a1")).is_some(), "outside the held lease");
}

#[test]
fn bind_next_lease_is_refused_while_the_old_lease_is_live() {
    let (fx, code, docs) = Fx::two("bind-next-live");
    let first = fx.rvow(&["delegate", &code]);
    fx.event("lease-bind", serde_json::json!({"agent_id": "a1", "prompt": first})).unwrap();
    let second = fx.rvow(&["delegate", &docs]);
    let out = fx.event("lease-bind", serde_json::json!({"agent_id": "a1", "prompt": second})).unwrap();
    assert_eq!(out["bound"], false, "{out}");
    let want = format!("this session still holds the lease of {code}; prove or block {code}, or start a new session for {docs}");
    assert_eq!(out["reason"], want.as_str(), "{out}");
    assert!(!lease::bind(&fx.project(), &docs, "a1").unwrap());

    let (src, doc) = (fx.repo().join("src/a.txt"), fx.repo().join("docs/a.md"));
    assert_eq!(fx.hook("Edit", serde_json::json!({"file_path": src}), Some("a1")), None, "still on the first task");
    assert!(fx.hook("Edit", serde_json::json!({"file_path": doc}), Some("a1")).is_some(), "the second task is not held");
}

#[test]
fn bind_closed_task_without_end_task_does_not_hold_the_session() {
    let (fx, code, docs) = Fx::two("bind-closed");
    let first = fx.rvow(&["delegate", &code]);
    let out = fx.event("lease-bind", serde_json::json!({"agent_id": "a1", "prompt": first})).unwrap();
    assert_eq!(out["bound"], true, "{out}");
    std::fs::write(fx.repo().join("code.txt"), "x").unwrap();
    fx.rvow(&["prove", &code]);
    let path = fx.project().root.join(LEASE_FILE);
    let kept: String =
        std::fs::read_to_string(&path).unwrap().lines().filter(|l| !l.contains("end_task")).map(|l| format!("{l}\n")).collect();
    std::fs::write(&path, kept).unwrap();
    let held = lease::leases(&fx.project()).unwrap();
    assert!(held.iter().all(|l| !l.ended), "the proof left the lease open");

    let second = fx.rvow(&["delegate", &docs]);
    let out = fx.event("lease-bind", serde_json::json!({"agent_id": "a1", "prompt": second})).unwrap();
    assert_eq!((out["bound"].clone(), out["task"].clone()), (true.into(), docs.as_str().into()), "{out}");
    assert!(out.get("reason").is_none(), "{out}");
}

#[test]
fn bind_closed_open_task_still_holds_the_session() {
    let (fx, code, docs) = Fx::two("bind-closed-open");
    let first = fx.rvow(&["delegate", &code]);
    fx.event("lease-bind", serde_json::json!({"agent_id": "a1", "prompt": first})).unwrap();
    let second = fx.rvow(&["delegate", &docs]);
    let out = fx.event("lease-bind", serde_json::json!({"agent_id": "a1", "prompt": second})).unwrap();
    assert_eq!(out["bound"], false, "{out}");
    let want = format!("this session still holds the lease of {code}; prove or block {code}, or start a new session for {docs}");
    assert_eq!(out["reason"], want.as_str(), "{out}");
}
