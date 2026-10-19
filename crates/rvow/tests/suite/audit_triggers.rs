use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicUsize, Ordering};

fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn triggers_dir() -> PathBuf {
    root().join("evals/triggers")
}

fn trigger_cases() -> Vec<PathBuf> {
    let mut cases: Vec<PathBuf> =
        std::fs::read_dir(triggers_dir().join("cases")).unwrap().map(|e| e.unwrap().path()).filter(|p| p.is_dir()).collect();
    cases.sort();
    cases
}

fn trigger_prompt(case: &Path) -> (String, String) {
    let text = std::fs::read_to_string(case.join("prompt.md")).unwrap();
    let mut parts = text.splitn(3, "---");
    parts.next();
    (parts.next().unwrap_or("").to_string(), parts.next().unwrap_or("").to_string())
}

fn trigger_tags(case: &Path) -> String {
    trigger_prompt(case).0
}

#[test]
fn audit_triggers_cases_well_formed() {
    let cases = trigger_cases();
    assert!(!cases.is_empty(), "evals/triggers/cases holds no cases");
    let mut load = 0;
    let mut quiet = 0;
    for case in &cases {
        let name = case.file_name().unwrap().to_string_lossy().into_owned();
        let (front, body) = trigger_prompt(case);
        assert!(front.contains("tags:"), "{name} prompt.md needs tags");
        assert!(front.contains("max_turns:"), "{name} prompt.md needs max_turns");
        assert!(front.contains("timeout_seconds:"), "{name} prompt.md needs timeout_seconds");
        assert!(front.contains("allowed_tools: [Read, Glob, Grep, Skill]"), "{name} prompt.md must allow only Read, Glob, Grep and Skill");
        assert!(!body.trim().is_empty(), "{name} prompt.md needs a prompt body");
        let graders: Vec<PathBuf> = std::fs::read_dir(case.join("graders"))
            .unwrap()
            .map(|e| e.unwrap().path())
            .filter(|p| p.extension().is_some_and(|e| e == "md"))
            .collect();
        assert_eq!(graders.len(), 1, "{name} needs exactly one grader on the Skill tool");
        let text = std::fs::read_to_string(&graders[0]).unwrap();
        assert!(text.contains("type: tool_used"), "{name} grader must be a tool_used grader");
        assert!(text.contains("tool: Skill"), "{name} grader must watch the Skill tool");
        assert!(text.contains("rvow"), "{name} grader must match the rvow skill");
        if text.contains("min: 1") {
            load += 1;
        } else if text.contains("max: 0") {
            quiet += 1;
        } else {
            panic!("{name} grader needs min: 1 (load) or max: 0 (quiet)");
        }
    }
    assert!(load >= 12, "need at least 12 load cases, found {load}");
    assert!(quiet >= 6, "need at least 6 quiet cases, found {quiet}");
}

#[test]
fn audit_triggers_cases_cover_the_triggers() {
    let mut topics = std::collections::HashSet::new();
    for case in trigger_cases() {
        let tags = trigger_tags(&case);
        let topic = tags.split("topic:").nth(1).unwrap_or("").split([',', ']']).next().unwrap_or("").trim().to_string();
        assert!(!topic.is_empty(), "{} needs a topic: tag", case.display());
        topics.insert(topic);
    }
    for topic in ["chg-id", "task-id", "denied-call", "failed-gate", "stale-proof", "plan-first", "how-rvow-works"] {
        assert!(topics.contains(topic), "no load case covers {topic}");
    }
}

#[test]
fn audit_triggers_cases_val_share_and_names() {
    let cases = trigger_cases();
    let mut seen = std::collections::HashSet::new();
    let mut val = 0;
    for case in &cases {
        let name = case.file_name().unwrap().to_string_lossy().into_owned();
        assert!(seen.insert(name.clone()), "duplicate case name {name}");
        let tags = trigger_tags(case);
        assert!(tags.contains("train") != tags.contains("val"), "{name} needs exactly one of the train and val tags");
        val += usize::from(tags.contains("val"));
    }
    let share = val * 100 / cases.len();
    assert!((30..=50).contains(&share), "val share must be 30 to 50 percent, found {val}/{}", cases.len());
}

fn run_sh() -> String {
    std::fs::read_to_string(triggers_dir().join("run.sh")).unwrap()
}

#[test]
fn audit_triggers_runner_script_parses() {
    let out = Command::new("bash").args(["-n"]).arg(triggers_dir().join("run.sh")).output().unwrap();
    assert!(out.status.success(), "bash -n failed: {}", String::from_utf8_lossy(&out.stderr));
}

#[test]
fn audit_triggers_runner_install_isolated() {
    let text = run_sh();
    assert!(text.contains("mktemp"), "run.sh must build its home under mktemp");
    let install = text.find("\"$rvow\" install").expect("run.sh must run \"$rvow\" install");
    for key in ["HOME=$tmp", "RVOW_HOME=", "XDG_DATA_HOME=$tmp", "XDG_CONFIG_HOME=$tmp", "CLAUDE_CONFIG_DIR=$tmp"] {
        let pos = text.find(key).unwrap_or_else(|| panic!("run.sh must set {key}"));
        assert!(pos < install, "{key} must come before the install");
    }
}

#[test]
fn audit_triggers_runner_eval_flags() {
    let text = run_sh();
    assert!(text.contains("--no-publish"), "results stay local");
    assert!(text.contains("--ablation none"), "one arm, no baseline");
    assert!(text.contains("--max-cost-usd"), "a full run is cost-capped");
    assert!(text.contains("model=${1:-haiku}"), "haiku is the default model");
    assert!(text.contains("results/$model-"), "results go under evals/triggers/results/<model>-<time>/");
    assert!(text.contains("--build-only"), "the offline test needs a build-only mode");
}

#[test]
fn audit_triggers_runner_gitignore() {
    let ignore = std::fs::read_to_string(triggers_dir().join(".gitignore")).unwrap();
    assert!(ignore.lines().any(|l| l.trim() == "results/"), ".gitignore must ignore results/");
}

struct TempDir(PathBuf);

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn scratch(name: &str) -> TempDir {
    static NEXT: AtomicUsize = AtomicUsize::new(0);
    let dir =
        std::env::temp_dir().join(format!("rvow-audit-triggers-{}-{name}-{}", std::process::id(), NEXT.fetch_add(1, Ordering::Relaxed)));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("home")).unwrap();
    TempDir(dir)
}

/// The env for an offline run: temp HOME and RVOW_HOME, never the real ones.
fn offline_env(dir: &Path) -> Vec<(String, String)> {
    let home = dir.join("home");
    vec![
        ("RVOW".to_string(), env!("CARGO_BIN_EXE_rvow").to_string()),
        ("HOME".to_string(), home.to_string_lossy().into_owned()),
        ("RVOW_HOME".to_string(), home.join("store").to_string_lossy().into_owned()),
        ("XDG_DATA_HOME".to_string(), home.join("data").to_string_lossy().into_owned()),
        ("XDG_CONFIG_HOME".to_string(), home.join("config").to_string_lossy().into_owned()),
        ("CLAUDE_CONFIG_DIR".to_string(), home.join("claude").to_string_lossy().into_owned()),
        ("PI_CODING_AGENT_DIR".to_string(), home.join("pi").to_string_lossy().into_owned()),
        ("GIT_CONFIG_GLOBAL".to_string(), "/dev/null".to_string()),
        ("GIT_CONFIG_NOSYSTEM".to_string(), "1".to_string()),
    ]
}

#[test]
fn audit_triggers_runner_build_only_plugin() {
    let dir = scratch("build");
    let dest = dir.0.join("plugin");
    let mut cmd = Command::new("bash");
    cmd.arg(triggers_dir().join("run.sh")).arg("--build-only").arg(&dest).current_dir(&dir.0);
    for (k, v) in offline_env(&dir.0) {
        cmd.env(k, v);
    }
    let out = cmd.output().unwrap();
    assert!(out.status.success(), "run.sh --build-only failed: {}", String::from_utf8_lossy(&out.stderr));
    assert!(dest.join(".claude-plugin/plugin.json").is_file(), "the plugin needs its manifest");
    let skill = dest.join("skills/rvow/SKILL.md");
    assert!(skill.is_file(), "the plugin needs the rendered skill");
    assert!(!dest.join("hooks").exists(), "the plugin must hold no hooks");
    assert_eq!(std::fs::read_to_string(&skill).unwrap(), installed_skill(&dir.0), "the plugin skill must match the installed Claude skill");
}

/// What `rvow install` writes as the Claude skill under a temp home (never
/// the real ones): the reference text the build-only plugin must match.
fn installed_skill(dir: &Path) -> String {
    let home = dir.join("refhome");
    std::fs::create_dir_all(&home).unwrap();
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_rvow"));
    cmd.arg("install").current_dir(dir);
    for (k, v) in [
        ("HOME", home.to_string_lossy().into_owned()),
        ("RVOW_HOME", home.join("store").to_string_lossy().into_owned()),
        ("XDG_DATA_HOME", home.join("data").to_string_lossy().into_owned()),
        ("XDG_CONFIG_HOME", home.join("config").to_string_lossy().into_owned()),
        ("CLAUDE_CONFIG_DIR", home.join("claude").to_string_lossy().into_owned()),
        ("PI_CODING_AGENT_DIR", home.join("pi").to_string_lossy().into_owned()),
        ("GIT_CONFIG_GLOBAL", "/dev/null".to_string()),
        ("GIT_CONFIG_NOSYSTEM", "1".to_string()),
    ] {
        cmd.env(k, v);
    }
    let out = cmd.output().unwrap();
    assert!(out.status.success(), "rvow install failed: {}", String::from_utf8_lossy(&out.stdout));
    std::fs::read_to_string(home.join("data/rvow/claude-plugin/plugins/rvow/skills/rvow/SKILL.md")).unwrap()
}
