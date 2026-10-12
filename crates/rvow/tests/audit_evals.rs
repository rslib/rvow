use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicUsize, Ordering};

fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn agent_dir() -> PathBuf {
    root().join("evals/agent")
}

fn run_sh() -> String {
    std::fs::read_to_string(agent_dir().join("run.sh")).unwrap()
}

#[test]
fn audit_evals_runner_script_parses() {
    let out = Command::new("bash").args(["-n"]).arg(agent_dir().join("run.sh")).output().unwrap();
    assert!(out.status.success(), "bash -n failed: {}", String::from_utf8_lossy(&out.stderr));
}

#[test]
fn audit_evals_runner_install_isolated() {
    let text = run_sh();
    assert!(text.contains("mktemp"), "run.sh must build its home under mktemp");
    let install = text.find("\"$rvow\" install").expect("run.sh must run \"$rvow\" install");
    for key in ["HOME=$tmp", "RVOW_HOME="] {
        let pos = text.find(key).unwrap_or_else(|| panic!("run.sh must set {key}"));
        assert!(pos < install, "{key} must come before the install");
    }
    assert!(text.contains("CLAUDE_CONFIG_DIR=$tmp"), "the Claude registration must stay under the temp dir");
}

#[test]
fn audit_evals_runner_eval_flags() {
    let text = run_sh();
    assert!(text.contains("--no-publish"), "results stay local");
    assert!(text.contains("--max-cost-usd"), "a full run is cost-capped");
    assert!(text.contains("model=${1:-haiku}"), "haiku is the default model");
    assert!(text.contains("results/$model-"), "results go under evals/agent/results/<model>-<time>/");
}

#[test]
fn audit_evals_runner_gitignore() {
    let ignore = std::fs::read_to_string(agent_dir().join(".gitignore")).unwrap();
    assert!(ignore.lines().any(|l| l.trim() == "results/"), ".gitignore must ignore results/");
}

struct TempDir(PathBuf);

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn loop_cases() -> Vec<(&'static str, PathBuf)> {
    ["next-task", "outside-scope", "cannot-pass"].into_iter().map(|n| (n, agent_dir().join("cases").join(n))).collect()
}

fn graders(case: &Path) -> Vec<PathBuf> {
    let mut files: Vec<PathBuf> = std::fs::read_dir(case.join("graders"))
        .unwrap()
        .map(|e| e.unwrap().path())
        .filter(|p| p.extension().is_some_and(|e| e == "md"))
        .collect();
    files.sort();
    files
}

fn grader_text(path: &Path) -> String {
    std::fs::read_to_string(path).unwrap()
}

fn grader_type(path: &Path) -> String {
    grader_text(path).lines().find_map(|l| l.strip_prefix("type: ").map(|s| s.trim().to_string())).unwrap_or_default()
}

fn prompt_frontmatter(case: &Path) -> String {
    let text = std::fs::read_to_string(case.join("prompt.md")).unwrap();
    let mut parts = text.splitn(3, "---");
    parts.next();
    parts.next().unwrap_or("").to_string()
}

fn rvow_in(dir: &Path, args: &[&str]) -> (String, i32) {
    let out = Command::new(env!("CARGO_BIN_EXE_rvow"))
        .args(args)
        .current_dir(dir)
        .env("HOME", dir.join("home"))
        .env("RVOW_HOME", dir.join("home"))
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .output()
        .unwrap();
    (String::from_utf8(out.stdout).unwrap(), out.status.code().unwrap())
}

#[test]
fn audit_evals_loop_cases_have_files() {
    for (name, case) in loop_cases() {
        assert!(case.join("case.yaml").is_file(), "{name} needs case.yaml");
        assert!(case.join("prompt.md").is_file(), "{name} needs prompt.md");
        assert!(case.join("setup.sh").is_file(), "{name} needs setup.sh");
        let yaml = std::fs::read_to_string(case.join("case.yaml")).unwrap();
        assert!(yaml.contains("scaffold_script: setup.sh"), "{name} case.yaml must name its setup");
        let setup = std::fs::read_to_string(case.join("setup.sh")).unwrap();
        assert!(setup.contains("lib.sh"), "{name} setup.sh must source lib.sh");
        assert!(!graders(&case).is_empty(), "{name} needs at least one grader");
    }
}

#[test]
fn audit_evals_loop_prompt_frontmatter() {
    for (name, case) in loop_cases() {
        let front = prompt_frontmatter(&case);
        for key in ["tags:", "max_turns:", "timeout_seconds:", "allowed_tools:"] {
            assert!(front.contains(key), "{name} prompt.md needs {key}");
        }
        assert!(front.contains("Skill"), "{name} prompt.md must allow Skill");
        assert!(front.contains("Bash"), "{name} prompt.md must allow Bash");
        let body = std::fs::read_to_string(case.join("prompt.md")).unwrap();
        assert!(!body.contains("rvow "), "{name} prompt must not name rvow commands; the mod teaches them");
    }
}

#[test]
fn audit_evals_loop_graders_not_llm_only() {
    for (name, case) in loop_cases() {
        assert!(graders(&case).iter().any(|g| grader_type(g.as_path()) != "llm"), "{name} needs at least one grader that is not an LLM");
    }
}

#[test]
fn audit_evals_loop_next_task_graders() {
    let case = agent_dir().join("cases").join("next-task");
    let texts: Vec<String> = graders(&case).iter().map(|g| grader_text(g.as_path())).collect();
    assert!(
        texts.iter().any(|t| t.contains("type: tool_used") && t.contains("rvow") && t.contains("prove") && t.contains("min: 1")),
        "next-task must grade that the agent runs rvow prove"
    );
    assert!(texts.iter().any(|t| t.contains("type: regex") && t.contains("app.py")), "next-task must grade the task file");
}

#[test]
fn audit_evals_loop_outside_scope_graders() {
    // The case passes when EITHER grader is green: an agent that leaves
    // README.md alone is correct, and so is one that records rvow amend
    // before editing. The eval report shows per-grader verdicts; the reader
    // applies the OR.
    let case = agent_dir().join("cases").join("outside-scope");
    let texts: Vec<String> = graders(&case).iter().map(|g| grader_text(g.as_path())).collect();
    assert!(
        texts.iter().any(|t| t.contains("README.md") && t.contains("not_contains")),
        "outside-scope must grade that README.md is unchanged"
    );
    assert!(
        texts.iter().any(|t| t.contains("type: tool_used") && t.contains("amend") && t.contains("min: 1")),
        "outside-scope must grade that the agent ran rvow amend"
    );
}

#[test]
fn audit_evals_loop_cannot_pass_graders() {
    let case = agent_dir().join("cases").join("cannot-pass");
    let texts: Vec<String> = graders(&case).iter().map(|g| grader_text(g.as_path())).collect();
    assert!(
        texts.iter().any(|t| t.contains("type: tool_used") && t.contains("block") && t.contains("min: 1")),
        "cannot-pass must grade that the agent runs rvow block"
    );
    assert!(
        texts.iter().any(|t| t.contains("type: regex") && t.contains("tasks.md") && t.contains("CHANGELOG")),
        "cannot-pass must grade that the verify line is unchanged"
    );
}

#[test]
fn audit_evals_loop_setups_build_state() {
    for (name, case) in loop_cases() {
        let dir = run_case_setup(&case.join("setup.sh"));
        let (status, code) = rvow_in(&dir.0, &["status"]);
        assert_eq!(code, 0, "{name}: rvow status failed: {status}");
        assert!(status.contains("1 changes"), "{name}: expected one active change: {status}");
        let (brief, code) = rvow_in(&dir.0, &["brief"]);
        assert_eq!(code, 0, "{name}: rvow brief failed: {brief}");
        if name == "cannot-pass" {
            let link = dir.0.join("change").join("tasks.md");
            assert!(link.is_file(), "cannot-pass setup must link the change under ./change");
            assert!(std::fs::read_to_string(&link).unwrap().contains("CHANGELOG"), "the linked tasks.md must hold the unpassable verify");
        }
    }
}
/// Runs a case setup.sh in a temp dir with the built binary on PATH and a
/// temp RVOW_HOME and HOME (never the real ones), and returns the dir.
fn planning_cases() -> Vec<(&'static str, PathBuf)> {
    ["plan", "owner-command", "question"].into_iter().map(|n| (n, agent_dir().join("cases").join(n))).collect()
}

#[test]
fn audit_evals_planning_cases_have_files() {
    for (name, case) in planning_cases() {
        assert!(case.join("case.yaml").is_file(), "{name} needs case.yaml");
        assert!(case.join("prompt.md").is_file(), "{name} needs prompt.md");
        assert!(case.join("setup.sh").is_file(), "{name} needs setup.sh");
        let yaml = std::fs::read_to_string(case.join("case.yaml")).unwrap();
        assert!(yaml.contains("scaffold_script: setup.sh"), "{name} case.yaml must name its setup");
        let setup = std::fs::read_to_string(case.join("setup.sh")).unwrap();
        assert!(setup.contains("lib.sh"), "{name} setup.sh must source lib.sh");
        assert!(!graders(&case).is_empty(), "{name} needs at least one grader");
    }
}

#[test]
fn audit_evals_planning_prompt_frontmatter() {
    for (name, case) in planning_cases() {
        let front = prompt_frontmatter(&case);
        for key in ["tags:", "max_turns:", "timeout_seconds:", "allowed_tools:"] {
            assert!(front.contains(key), "{name} prompt.md needs {key}");
        }
        assert!(front.contains("Skill"), "{name} prompt.md must allow Skill");
        assert!(front.contains("Bash"), "{name} prompt.md must allow Bash");
        let body = std::fs::read_to_string(case.join("prompt.md")).unwrap();
        assert!(!body.contains("rvow "), "{name} prompt must not name rvow commands; the mod teaches them");
    }
}

#[test]
fn audit_evals_planning_graders_not_llm_only() {
    for (name, case) in planning_cases() {
        assert!(graders(&case).iter().any(|g| grader_type(g.as_path()) != "llm"), "{name} needs at least one grader that is not an LLM");
    }
}

#[test]
fn audit_evals_planning_plan_graders() {
    let case = agent_dir().join("cases").join("plan");
    let texts: Vec<String> = graders(&case).iter().map(|g| grader_text(g.as_path())).collect();
    assert!(
        texts.iter().any(|t| t.contains("type: tool_used") && t.contains("rvow") && t.contains("new") && t.contains("min: 1")),
        "plan must grade that the agent runs rvow new"
    );
    assert!(
        texts.iter().any(|t| t.contains("type: tool_used") && t.contains("check") && t.contains("min: 1")),
        "plan must grade that the agent runs rvow check"
    );
    assert!(
        texts.iter().any(|t| t.contains("type: regex") && t.contains("app.py") && t.contains("not_contains")),
        "plan must grade that the code file is unchanged"
    );
}

#[test]
fn audit_evals_planning_owner_graders() {
    // file_exists only sees files the run creates, never scaffolded ones,
    // so the backlog check is a regex over the live file behind the
    // ./plan link the setup leaves, not a file_exists grader.
    let case = agent_dir().join("cases").join("owner-command");
    let texts: Vec<String> = graders(&case).iter().map(|g| grader_text(g.as_path())).collect();
    assert!(!texts.iter().any(|t| t.contains("type: file_exists")), "owner-command must not use file_exists on scaffolded files");
    assert!(
        texts.iter().any(|t| t.contains("type: regex") && t.contains("plan/tasks.md")),
        "owner-command must grade that the plan is still in the backlog"
    );
    assert!(
        texts.iter().any(|t| t.contains("type: regex") && t.contains("last_message") && t.contains("/rvow-accept")),
        "owner-command must grade that the reply names /rvow-accept"
    );
}

#[test]
fn audit_evals_planning_question_graders() {
    let case = agent_dir().join("cases").join("question");
    let texts: Vec<String> = graders(&case).iter().map(|g| grader_text(g.as_path())).collect();
    assert!(
        texts.iter().any(|t| t.contains("type: tool_used")
            && t.contains("next")
            && t.contains("status")
            && t.contains("brief")
            && t.contains("min: 1")),
        "question must grade that the agent runs an rvow read command"
    );
    assert!(
        texts.iter().any(|t| t.contains("type: regex") && t.contains("last_message") && t.contains("grep")),
        "question must grade that the reply names the verify command"
    );
}

#[test]
fn audit_evals_planning_setups_build_state() {
    for (name, case) in planning_cases() {
        let dir = run_case_setup(&case.join("setup.sh"));
        let (brief, code) = rvow_in(&dir.0, &["brief"]);
        assert_eq!(code, 0, "{name}: rvow brief failed: {brief}");
        let (ls, code) = rvow_in(&dir.0, &["ls"]);
        assert_eq!(code, 0, "{name}: rvow ls failed: {ls}");
        if name == "plan" {
            assert!(!ls.contains("backlog"), "plan: expected no change on the branch: {ls}");
        } else if name == "owner-command" {
            assert!(ls.contains(",backlog,"), "owner-command: expected a ready backlog plan: {ls}");
            let link = dir.0.join("plan").join("tasks.md");
            assert!(link.is_file(), "owner-command setup must link the plan under ./plan");
            assert!(std::fs::read_to_string(&link).unwrap().contains("covers:"), "the linked tasks.md must hold the plan");
        } else {
            let (status, code) = rvow_in(&dir.0, &["status"]);
            assert_eq!(code, 0, "{name}: rvow status failed: {status}");
            assert!(status.contains("1 changes"), "{name}: expected one active change: {status}");
        }
    }
}

#[test]
fn audit_evals_planning_case_list_exact() {
    let mut names: Vec<String> =
        std::fs::read_dir(agent_dir().join("cases")).unwrap().map(|e| e.unwrap().file_name().to_string_lossy().into_owned()).collect();
    names.sort();
    assert_eq!(names, ["cannot-pass", "next-task", "outside-scope", "owner-command", "plan", "question"]);
}
fn run_case_setup(setup: &Path) -> TempDir {
    static NEXT: AtomicUsize = AtomicUsize::new(0);
    let dir = std::env::temp_dir().join(format!("rvow-audit-evals-{}-{}", std::process::id(), NEXT.fetch_add(1, Ordering::Relaxed)));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let home = dir.join("home");
    std::fs::create_dir_all(&home).unwrap();
    let bindir = Path::new(env!("CARGO_BIN_EXE_rvow")).parent().unwrap().to_path_buf();
    let path = format!("{}:{}", bindir.display(), std::env::var("PATH").unwrap());
    let status = Command::new("bash")
        .arg(setup)
        .current_dir(&dir)
        .env("HOME", &home)
        .env("RVOW_HOME", &home)
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_AUTHOR_NAME", "t")
        .env("GIT_AUTHOR_EMAIL", "t@t")
        .env("GIT_COMMITTER_NAME", "t")
        .env("GIT_COMMITTER_EMAIL", "t@t")
        .env("PATH", path)
        .status()
        .unwrap();
    assert!(status.success(), "setup {} failed in {}", setup.display(), dir.display());
    TempDir(dir)
}

#[test]
fn audit_evals_runner_case_setups_build_offline() {
    let cases = agent_dir().join("cases");
    let mut setups: Vec<PathBuf> = Vec::new();
    if cases.is_dir() {
        let mut entries: Vec<PathBuf> = std::fs::read_dir(&cases).unwrap().map(|e| e.unwrap().path()).collect();
        entries.sort();
        for entry in entries {
            let setup = entry.join("setup.sh");
            if setup.is_file() {
                setups.push(setup);
            }
        }
    }
    for setup in &setups {
        let dir = run_case_setup(setup);
        assert!(dir.0.join(".git").is_dir(), "{} left no git repository", setup.display());
        assert!(dir.0.join("rvow").is_dir(), "{} left no in-repo rvow tree", setup.display());
        let out = Command::new(env!("CARGO_BIN_EXE_rvow"))
            .args(["brief"])
            .current_dir(&dir.0)
            .env("HOME", dir.0.join("home"))
            .env("RVOW_HOME", dir.0.join("home"))
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .output()
            .unwrap();
        assert!(out.status.success(), "rvow brief failed in {}: {}", dir.0.display(), String::from_utf8_lossy(&out.stdout));
    }
}
