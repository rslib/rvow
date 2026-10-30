//! Hook measurement: every denial names a rule, and the counts say which rule fires most.

use std::path::PathBuf;
use std::process::Command;

use rvow_core::store::{self, Project};
use rvow_hook::hook::{self, Denial, Rule};
use serde_json::json;

const RVOW: &str = env!("CARGO_BIN_EXE_rvow");

struct Fx {
    root: PathBuf,
}

impl Drop for Fx {
    fn drop(&mut self) {
        rvow_core::git::testrepo::remove_tree(&self.root);
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
    /// A repository with a protected test file. `accepted` decides whether the one plan is accepted.
    fn new(name: &str, accepted: bool) -> Fx {
        let root = std::env::temp_dir().join(format!("rvow-hook-stats-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("repo/src")).unwrap();
        std::fs::create_dir_all(root.join("repo/tests")).unwrap();
        std::fs::create_dir_all(root.join("repo/docs")).unwrap();
        let fx = Fx { root: root.canonicalize().unwrap() };
        let repo = fx.repo();
        std::fs::write(repo.join("src/a.txt"), "a").unwrap();
        std::fs::write(repo.join("tests/t.sh"), "exit 0").unwrap();
        std::fs::write(repo.join("docs/a.md"), "a").unwrap();
        for args in [vec!["init", "-q", "-b", "main"], vec!["add", "."], vec!["commit", "-q", "-m", "init"]] {
            assert!(git_command(&repo).args(&args).status().unwrap().success(), "git {args:?}");
        }
        fx.rvow(&["init"]);
        let out = fx.rvow(&["new", "--spec", "Code"]);
        let dir = PathBuf::from(out.lines().find_map(|l| l.strip_prefix("path: ")).unwrap());
        std::fs::write(
            dir.join("tasks.md"),
            "- TSK-new1 Code\n  covers: REQ-new1\n  verify: test -f code.txt && echo 'test result: ok. 1 passed; 0 failed;'\n  files: src/**, code.txt\n",
        )
        .unwrap();
        std::fs::write(
            dir.join("specs/out.md"),
            "## ADD REQ-new1 Code\nrvow SHALL code.\n### Scenario: s\n- WHEN it runs\n- THEN it is done\n",
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

    /// The denial of one tool call, through the library entry that the hook uses.
    fn denial(&self, tool: &str, input: serde_json::Value) -> Option<Denial> {
        let payload = json!({"cwd": self.repo(), "tool_name": tool, "tool_input": input});
        hook::pre_tool_use_denial(&self.project(), &payload).unwrap()
    }

    fn edit(&self, file: PathBuf) -> Option<Denial> {
        self.denial("Edit", json!({"file_path": file}))
    }
}

fn rule_of(d: Option<Denial>, what: &str) -> Rule {
    d.unwrap_or_else(|| panic!("{what} was allowed")).rule
}

#[test]
fn rules_the_list_is_short_and_its_names_are_unique_kebab_case() {
    assert!(Rule::ALL.len() <= 20);
    let names: Vec<&str> = Rule::ALL.iter().map(|r| r.name()).collect();
    let mut sorted = names.clone();
    sorted.sort();
    sorted.dedup();
    assert_eq!(sorted.len(), names.len(), "{names:?}");
    assert!(names.iter().all(|n| !n.is_empty() && n.chars().all(|c| c.is_ascii_lowercase() || c == '-')), "{names:?}");
}

#[test]
fn rules_shell_denials_name_their_family() {
    for (command, rule) in [
        ("rvow accept CHG-x", Rule::OwnerCommand),
        ("rvow confirm --accept", Rule::OwnerCommand),
        ("rvow ingest run.jsonl", Rule::OwnerCommand),
        ("sh -c 'rvow accept x'", Rule::OwnerCommand),
        ("echo cnZvdyBhY2NlcHQgeA== | base64 -d | sh", Rule::OwnerCommand),
        ("git commit -n -m x", Rule::GitHooks),
        ("git push --no-verify", Rule::GitHooks),
        ("git config core.hooksPath /tmp/x", Rule::GitHooks),
        ("rm .git/hooks/pre-push", Rule::GitHooks),
        ("echo x >> ledger.jsonl", Rule::Ledger),
        ("echo x >> rerun-cache.jsonl", Rule::CacheFile),
        ("echo x >> leases.jsonl", Rule::LeaseFile),
    ] {
        let d = hook::bash_rule(command).unwrap_or_else(|| panic!("allowed: {command}"));
        assert_eq!(d.rule, rule, "{command}: {}", d.reason);
        assert_eq!(hook::bash_denial(command), Some(d.reason), "the text-only entry agrees: {command}");
    }
    assert_eq!(hook::bash_rule("ls -la"), None);
}

#[test]
fn rules_edit_denials_name_their_family() {
    let fx = Fx::new("edits", true);
    let repo = fx.repo();
    let root = fx.project().root;
    assert_eq!(rule_of(fx.edit(repo.join("tests/t.sh")), "a protected test"), Rule::ProtectedTest);
    assert_eq!(rule_of(fx.edit(repo.join("docs/a.md")), "a file outside every task"), Rule::OutsideScope);
    assert_eq!(rule_of(fx.edit(repo.join(".claude/settings.json")), "the repository's Claude settings"), Rule::EnforcementFile);
    assert_eq!(rule_of(fx.edit(repo.join(".git/config")), "a file of .git"), Rule::GitHooks);
    assert_eq!(rule_of(fx.edit(root.join("active/ledger.jsonl")), "the ledger"), Rule::Ledger);
    assert_eq!(rule_of(fx.edit(root.join("archive/done/x.md")), "an archived change"), Rule::History);
    assert_eq!(rule_of(fx.edit(root.join("specs/x.md")), "a living spec"), Rule::History);
    assert_eq!(rule_of(fx.edit(root.join("rerun-cache.jsonl")), "the cache file"), Rule::CacheFile);
    assert_eq!(rule_of(fx.denial("Write", json!({"file_path": root.join("leases.jsonl")})), "the lease file"), Rule::LeaseFile);

    assert_eq!(fx.edit(repo.join("src/a.txt")), None, "a file of the task is allowed");
    assert_eq!(fx.denial("Read", json!({"file_path": repo.join("docs/a.md")})), None, "a read is allowed");
}

#[test]
fn rules_a_plan_that_waits_for_accept_names_its_rule() {
    let fx = Fx::new("unaccepted", false);
    assert_eq!(rule_of(fx.edit(fx.repo().join("src/a.txt")), "a code edit before accept"), Rule::UnacceptedPlan);
}

#[test]
fn rules_a_script_that_holds_accept_keeps_the_owner_rule() {
    let fx = Fx::new("script", true);
    std::fs::write(fx.repo().join("go.sh"), "#!/bin/sh\nrvow accept CHG-x\n").unwrap();
    let d = hook::script_rule("sh go.sh", &fx.repo()).expect("the script is denied");
    assert_eq!(d.rule, Rule::OwnerCommand, "{}", d.reason);
    assert_eq!(hook::script_denial("sh go.sh", &fx.repo()), Some(d.reason));
}

// The hook counts its outcomes.

impl Fx {
    /// The JSON that `rvow hook claude <event>` printed, or `None` when it printed nothing.
    fn event(&self, event: &str, mut payload: serde_json::Value) -> Option<serde_json::Value> {
        use std::io::Write;
        use std::process::Stdio;
        payload["cwd"] = self.repo().to_str().unwrap().into();
        let mut c = Command::new(RVOW);
        c.args(["hook", "claude", event])
            .current_dir(self.repo())
            .env("HOME", self.root.join("home"))
            .env("RVOW_HOME", self.home())
            .env("PATH", "/usr/bin:/bin:/usr/sbin:/sbin")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped());
        let mut child = c.spawn().unwrap();
        child.stdin.take().unwrap().write_all(payload.to_string().as_bytes()).unwrap();
        let out = String::from_utf8(child.wait_with_output().unwrap().stdout).unwrap();
        (!out.trim().is_empty()).then(|| serde_json::from_str(&out).unwrap())
    }

    fn bash_event(&self, command: &str, session: &str) -> Option<serde_json::Value> {
        self.event("pre-tool-use", json!({"tool_name": "Bash", "tool_input": {"command": command}, "session_id": session}))
    }

    /// Every parsed line of the stats file, the duration lines too.
    fn all_lines(&self) -> Vec<serde_json::Value> {
        std::fs::read_to_string(self.project().root.join(rvow_core::stats::STATS_FILE))
            .map(|t| t.lines().map(|l| serde_json::from_str(l).unwrap()).collect())
            .unwrap_or_default()
    }

    /// The outcome lines of the stats file, with no duration line.
    fn stats(&self) -> Vec<serde_json::Value> {
        self.all_lines().into_iter().filter(|l| l.get("duration_ms").is_none()).collect()
    }

    /// The duration lines of the stats file.
    fn durations(&self) -> Vec<serde_json::Value> {
        self.all_lines().into_iter().filter(|l| l.get("duration_ms").is_some()).collect()
    }
}

#[test]
fn stats_record_a_denial_with_its_rule_and_session() {
    let fx = Fx::new("rec-deny", true);
    assert!(fx.bash_event("rvow accept CHG-x", "s1").is_some());
    let lines = fx.stats();
    assert_eq!(lines.len(), 1, "{lines:?}");
    assert_eq!(lines[0]["hook"], "pre-tool-use");
    assert_eq!(lines[0]["outcome"], "deny");
    assert_eq!(lines[0]["rule"], "owner-command");
    assert_eq!(lines[0]["session"], "s1");
    assert!(lines[0]["time"].as_u64().unwrap() > 0);

    assert!(fx.bash_event("ls -la", "s1").is_none());
    assert_eq!(fx.stats().len(), 1, "an allowed call is not counted");
}

#[test]
fn stats_record_a_stop_block_and_the_pass_that_follows() {
    let fx = Fx::new("rec-stop", true);
    std::fs::write(fx.repo().join("src/b.txt"), "touched").unwrap();
    let edit = json!({"file_path": fx.repo().join("src/b.txt")});
    let write = json!({"tool_name": "Write", "tool_input": edit, "session_id": "s1"});
    assert!(fx.event("pre-tool-use", write).is_none());
    let session = json!({"session_id": "s1"});
    assert!(fx.event("stop", session.clone()).is_some(), "the first stop is held back");
    assert!(fx.event("stop", session).is_none(), "the same reason passes the second time");
    let lines: Vec<_> = fx.stats().into_iter().filter(|l| l["hook"] == "stop").collect();
    let outcomes: Vec<&str> = lines.iter().map(|l| l["outcome"].as_str().unwrap()).collect();
    assert_eq!(outcomes, ["block", "pass"], "{lines:?}");
    assert!(lines.iter().all(|l| l["hook"] == "stop" && l["rule"].is_null() && l["session"] == "s1"), "{lines:?}");

    assert!(fx.event("stop", json!({"agent_id": "a1", "session_id": "s1"})).is_none());
    assert_eq!(fx.stats().last().unwrap()["outcome"], "pass", "a subagent stop is counted as a pass");
}

#[test]
fn stats_record_no_command_text_and_no_path() {
    let fx = Fx::new("rec-text", true);
    let secret = "CHG-zq7secretname";
    fx.bash_event(&format!("rvow accept {secret} /Users/someone/private/dir"), "s1");
    let text = std::fs::read_to_string(fx.project().root.join(rvow_core::stats::STATS_FILE)).unwrap();
    assert!(!text.contains(secret) && !text.contains("/Users/someone") && !text.contains("accept"), "{text}");
}

#[test]
fn stats_record_the_stats_file_is_closed_to_writes() {
    let fx = Fx::new("rec-closed", true);
    let file = fx.project().root.join(rvow_core::stats::STATS_FILE);
    for tool in ["Edit", "Write"] {
        let d = fx.denial(tool, json!({"file_path": file})).unwrap_or_else(|| panic!("{tool} allowed"));
        assert_eq!(d.rule, Rule::StatsFile, "{}", d.reason);
        assert!(d.reason.contains("rvow stats"), "{}", d.reason);
    }
    assert!(fx.denial("Read", json!({"file_path": file})).is_none(), "reading the counts is harmless");
    for command in [format!("echo x >> {}", file.display()), format!("rm {}", file.display())] {
        let d = hook::bash_rule(&command).unwrap_or_else(|| panic!("allowed: {command}"));
        assert_eq!(d.rule, Rule::StatsFile, "{command}: {}", d.reason);
    }
}

// `rvow stats` shows the counts.

impl Fx {
    /// Writes stats lines as the hooks would, `age_days` old.
    fn put(&self, age_days: u64, hook: &str, outcome: &str, rule: Option<&str>, n: usize) {
        let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_millis() as u64;
        let time = now - age_days * 86_400_000;
        let file = self.project().root.join(rvow_core::stats::STATS_FILE);
        let mut text = std::fs::read_to_string(&file).unwrap_or_default();
        for i in 0..n {
            let line = json!({"time": time, "hook": hook, "rule": rule, "outcome": outcome, "session": format!("s{}", i % 2)});
            text.push_str(&format!("{line}\n"));
        }
        std::fs::write(file, text).unwrap();
    }
}

#[test]
fn stats_show_counts_each_rule_and_the_stop_blocks_per_100() {
    let fx = Fx::new("show", true);
    fx.put(0, "pre-tool-use", "deny", Some("owner-command"), 3);
    fx.put(0, "pre-tool-use", "deny", Some("outside-scope"), 1);
    fx.put(0, "stop", "block", None, 2);
    fx.put(0, "stop", "pass", None, 8);
    let out = fx.rvow(&["stats"]);
    assert!(out.contains("owner-command,3") && out.contains("outside-scope,1"), "{out}");
    assert!(out.contains("stop_blocks: 2") && out.contains("stop_passes: 8"), "{out}");
    assert!(out.contains("stop_blocks_per_100: 20"), "{out}");
    assert!(out.contains("events: 14") && out.contains("sessions: 2"), "{out}");
    let human = fx.rvow(&["stats", "--human"]);
    assert!(human.contains("denied owner-command: 3") && human.contains("20 stop blocks per 100 stops"), "{human}");
    assert!(human.find("owner-command").unwrap() < human.find("outside-scope").unwrap(), "the loudest rule comes first: {human}");
}

#[test]
fn stats_show_an_empty_file_says_how_counts_get_recorded() {
    let fx = Fx::new("show-empty", true);
    let out = fx.rvow(&["stats"]);
    assert!(out.contains("events: 0") && out.contains("rvow install"), "{out}");
    assert!(!out.contains("stop_blocks_per_100"), "{out}");
}

#[test]
fn stats_show_leaves_out_lines_older_than_the_window() {
    let fx = Fx::new("show-old", true);
    fx.put(9, "pre-tool-use", "deny", Some("history"), 2);
    fx.put(1, "pre-tool-use", "deny", Some("ledger"), 1);
    let week = fx.rvow(&["stats"]);
    assert!(week.contains("ledger,1") && !week.contains("history"), "{week}");
    let more = fx.rvow(&["stats", "--days", "10"]);
    assert!(more.contains("history,2") && more.contains("ledger,1"), "{more}");
}

#[test]
fn stats_show_skips_a_line_that_does_not_parse() {
    let fx = Fx::new("show-bad", true);
    fx.put(0, "stop", "pass", None, 1);
    let file = fx.project().root.join(rvow_core::stats::STATS_FILE);
    let mut text = std::fs::read_to_string(&file).unwrap();
    text.push_str("not json\n");
    std::fs::write(file, text).unwrap();
    assert!(fx.rvow(&["stats"]).contains("events: 1"));
}

// Every hook call has a duration.

impl Fx {
    /// Writes duration lines as the hooks would, `n` of them with `ms` each.
    fn put_ms(&self, hook: &str, ms: u64, n: usize) {
        let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_millis() as u64;
        let file = self.project().root.join(rvow_core::stats::STATS_FILE);
        let mut text = std::fs::read_to_string(&file).unwrap_or_default();
        for _ in 0..n {
            text.push_str(&format!("{}\n", json!({"time": now, "hook": hook, "session": "s1", "duration_ms": ms})));
        }
        std::fs::write(file, text).unwrap();
    }
}

#[test]
fn hook_stats_duration_an_allowed_read_adds_one_line() {
    let fx = Fx::new("dur-read", true);
    let read = json!({"file_path": fx.repo().join("docs/a.md")});
    assert!(fx.event("pre-tool-use", json!({"tool_name": "Read", "tool_input": read, "session_id": "s1"})).is_none());
    let lines = fx.all_lines();
    assert_eq!(lines.len(), 1, "{lines:?}");
    assert_eq!(lines[0]["hook"], "pre-tool-use");
    assert_eq!(lines[0]["session"], "s1");
    assert!(lines[0]["duration_ms"].is_u64(), "{lines:?}");
    assert!(lines[0]["time"].as_u64().unwrap() > 0);
}

#[test]
fn hook_stats_duration_a_line_holds_no_command_text_and_no_path() {
    let fx = Fx::new("dur-text", true);
    let command = "cat /Users/someone/private/zq7secret.txt";
    assert!(fx.bash_event(command, "s1").is_none());
    let lines = fx.durations();
    assert_eq!(lines.len(), 1, "{lines:?}");
    let mut keys: Vec<&str> = lines[0].as_object().unwrap().keys().map(String::as_str).collect();
    keys.sort();
    assert_eq!(keys, ["duration_ms", "hook", "session", "steps", "time"], "{lines:?}");
    let text = std::fs::read_to_string(fx.project().root.join(rvow_core::stats::STATS_FILE)).unwrap();
    assert!(!text.contains("zq7secret") && !text.contains("/Users/someone") && !text.contains("cat"), "{text}");
}

#[test]
fn hook_stats_duration_a_denied_call_still_adds_its_duration() {
    let fx = Fx::new("dur-deny", true);
    assert!(fx.bash_event("rvow accept CHG-x", "s1").is_some());
    assert_eq!(fx.stats().len(), 1);
    assert_eq!(fx.durations().len(), 1);
}

#[test]
fn hook_stats_duration_shows_the_median_and_the_95th_percentile_of_each_hook() {
    let fx = Fx::new("dur-show", true);
    fx.put_ms("pre-tool-use", 10, 20);
    fx.put_ms("pre-tool-use", 200, 1);
    fx.put_ms("stop", 5, 1);
    let out = fx.rvow(&["stats"]);
    assert!(out.contains("pre-tool-use,21,10,10"), "{out}");
    assert!(out.contains("stop,1,5,5"), "{out}");
    assert!(out.contains("events: 0"), "durations are not events: {out}");
    let human = fx.rvow(&["stats", "--human"]);
    assert!(human.contains("pre-tool-use: median 10 ms, 95th percentile 10 ms"), "{human}");
}

#[test]
fn hook_stats_duration_rotates_the_file_when_it_grows() {
    // Above the limit the file moves aside whole, so no line written by another
    // hook at the same time is lost, and recent lines still count.
    let fx = Fx::new("dur-rotate", true);
    fx.put(1, "pre-tool-use", "deny", Some("ledger"), 1);
    fx.put_ms("stop", 5, 3);
    let p = fx.project();
    rvow_core::stats::record_duration_bounded(&p, rvow_core::stats::Hook::Stop, Some("s1"), 7, 1 << 30);
    assert_eq!(fx.all_lines().len(), 5, "a small file stays in place");
    rvow_core::stats::record_duration_bounded(&p, rvow_core::stats::Hook::Stop, Some("s1"), 8, 10);
    let lines = fx.all_lines();
    assert_eq!(lines.len(), 1, "the file starts again with the new line: {lines:?}");
    assert_eq!(lines[0]["duration_ms"], 8, "{lines:?}");
    let old = p.root.join(rvow_core::stats::ROTATED_FILE);
    assert_eq!(std::fs::read_to_string(&old).unwrap().lines().count(), 5, "the old lines moved aside whole");
    let out = fx.rvow(&["stats"]);
    assert!(out.contains("ledger"), "a moved denial still counts: {out}");
    assert!(out.contains("stop,5,"), "the 5 durations of both files count: {out}");
}

#[test]
fn stats_record_the_project_index_is_closed_to_writes() {
    let fx = Fx::new("index-closed", true);
    let file = fx.root.join("home").join(rvow_core::store::INDEX_FILE);
    for tool in ["Edit", "Write"] {
        let d = fx.denial(tool, json!({"file_path": file})).unwrap_or_else(|| panic!("{tool} allowed"));
        assert!(d.reason.contains(rvow_core::store::INDEX_FILE), "{}", d.reason);
    }
    assert!(hook::bash_rule(&format!("echo x > {}", file.display())).is_some(), "a shell write is denied");
}

// The brief names missing work of the newest change before stale work.

impl Fx {
    /// The first task id that `rvow status` lists.
    fn first_task(&self) -> String {
        let out = self.rvow(&["status"]);
        out.split(|c: char| !(c.is_ascii_alphanumeric() || c == '-'))
            .find(|w| w.len() == 10 && w.starts_with("TSK-"))
            .expect("a task id in the status")
            .to_string()
    }

    /// Accepts a second change with one task that wants `next.txt`, and returns its task id.
    fn second_change(&self) -> String {
        let out = self.rvow(&["new", "--spec", "Next"]);
        let dir = PathBuf::from(out.lines().find_map(|l| l.strip_prefix("path: ")).unwrap());
        std::fs::write(
            dir.join("tasks.md"),
            "- TSK-new1 Next\n  covers: REQ-new1\n  verify: test -f next.txt && echo 'test result: ok. 1 passed; 0 failed;'\n  files: next.txt\n",
        )
        .unwrap();
        std::fs::write(
            dir.join("specs/next.md"),
            "## ADD REQ-new1 Next\nrvow SHALL do next.\n### Scenario: s\n- WHEN it runs\n- THEN it is done\n",
        )
        .unwrap();
        let id = out.lines().find_map(|l| l.strip_prefix("created: ")).unwrap().to_string();
        self.rvow(&["accept", &id, "--yes"]);
        let located = self.rvow(&["path", &id]);
        let tasks =
            std::fs::read_to_string(PathBuf::from(located.lines().find_map(|l| l.strip_prefix("path: ")).unwrap().trim()).join("tasks.md"))
                .unwrap();
        tasks.lines().find_map(|l| l.strip_prefix("- ")?.split_whitespace().next().map(String::from)).unwrap()
    }

    /// Proves the first change's task and then edits a file in its scope, so the proof is stale.
    fn make_stale(&self, task: &str) {
        std::fs::write(self.repo().join("code.txt"), "one").unwrap();
        self.rvow(&["prove", task]);
        std::fs::write(self.repo().join("code.txt"), "two").unwrap();
    }
}

#[test]
fn brief_a_missing_task_of_the_newest_change_comes_before_an_old_stale_task() {
    let fx = Fx::new("brief-missing", true);
    let old = fx.first_task();
    fx.make_stale(&old);
    std::thread::sleep(std::time::Duration::from_millis(20));
    let new = fx.second_change();
    let brief = fx.rvow(&["brief"]);
    assert!(brief.contains(&format!("next: {new} Next")), "{brief}");
    assert!(!brief.contains(&format!("next: {old}")), "{brief}");
}

#[test]
fn brief_only_stale_work_left_names_the_stale_task() {
    let fx = Fx::new("brief-stale", true);
    let task = fx.first_task();
    fx.make_stale(&task);
    let brief = fx.rvow(&["brief"]);
    assert!(brief.contains(&format!("next: {task} Code")), "{brief}");
}
