//! `verify: @runner key=value`: the fill from the project config, the drift of
//! a used runner, the JUnit report rule and the run guards (CHG-vq0f01).

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicU32, Ordering};

use rvow_core::{change, config, drift, gate, ledger, plan, proof, review, store, vow};
use serde_json::Value;

static COUNTER: AtomicU32 = AtomicU32::new(0);

const SPEC: &str = "## ADD REQ-new1 Out\nrvow SHALL run the runner command.\n### Scenario: s\n- WHEN a task names a runner\n- THEN rvow runs the filled command\n";

/// The `[runners.cpp]` fixture: a grep on out.txt that fails until the work
/// writes the line, so the accept baseline fails as it must.
const RUNNER_CPP: &str = "\
[runners.cpp]
run = \"grep -q '{preset} {test}' out.txt && echo 'test result: ok. 1 passed; 0 failed;'\"
params = { preset = [\"tests\", \"asan\", \"tsan\"] }
default = { preset = \"tests\" }
";

struct Fx {
    _tmp: PathBuf,
    home: PathBuf,
    repo: PathBuf,
    project: store::Project,
}

fn git_cmd(repo: &Path, args: &[&str]) {
    let ok = Command::new("git")
        .arg("-C")
        .arg(repo)
        .args(args)
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_AUTHOR_NAME", "t")
        .env("GIT_AUTHOR_EMAIL", "t@t")
        .env("GIT_COMMITTER_NAME", "t")
        .env("GIT_COMMITTER_EMAIL", "t@t")
        .output()
        .unwrap();
    assert!(ok.status.success(), "git {args:?}: {ok:?}");
}

/// A project with one commit, and the given project config.toml.
fn fx(name: &str, config_text: &str) -> Fx {
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    let tmp = std::env::temp_dir().join(format!("rvow-runners-{}-{n}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&tmp);
    std::fs::create_dir_all(&tmp).unwrap();
    let tmp = tmp.canonicalize().unwrap();
    let home = tmp.join("home");
    let repo = tmp.join("r");
    std::fs::create_dir_all(&repo).unwrap();
    git_cmd(&repo, &["init", "-q", "-b", "main"]);
    std::fs::write(repo.join("a.txt"), "a").unwrap();
    git_cmd(&repo, &["add", "a.txt"]);
    git_cmd(&repo, &["commit", "-q", "-m", "init"]);
    let project = store::init(&repo, &home, store::Mode::Home).unwrap();
    if !config_text.is_empty() {
        std::fs::write(project.root.join("config.toml"), config_text).unwrap();
    }
    Fx { _tmp: tmp, home, repo, project }
}

/// A backlog change with one task that uses `verify` and covers REQ-new1.
fn backlog(f: &Fx, verify: &str, fields: &str) -> change::ChangeDir {
    let c = change::new(&f.project, "Run", "main", false).unwrap();
    std::fs::write(
        c.path.join("tasks.md"),
        format!("- TSK-new1 Run the runner\n  covers: REQ-new1\n  verify: {verify}\n  files: out.txt\n{fields}"),
    )
    .unwrap();
    std::fs::write(c.path.join("specs/out.md"), SPEC).unwrap();
    c
}

/// Accepts the change and returns the minted task id.
fn accept(f: &Fx, c: change::ChangeDir) -> String {
    let prepared = vow::prepare(&f.project, &f.home, c, &f.repo, &mut std::io::sink()).unwrap();
    accept_prepared(f, prepared)
}

fn accept_prepared(f: &Fx, prepared: vow::Prepared) -> String {
    let vowed = vow::commit(&f.project, prepared).unwrap();
    vowed.minted.iter().find(|(from, _)| from == "TSK-new1").unwrap().1.to_string()
}

fn active(f: &Fx) -> proof::Active {
    let (living, _) = change::load_living(&f.project).unwrap();
    proof::active(&f.project, &living).unwrap().remove(0)
}

fn task_dir(f: &Fx) -> PathBuf {
    std::fs::read_dir(f.project.root.join("active")).unwrap().next().unwrap().unwrap().path()
}

fn edit_tasks(f: &Fx, from: &str, to: &str) {
    let tasks = task_dir(f).join("tasks.md");
    let text = std::fs::read_to_string(&tasks).unwrap();
    assert!(text.contains(from), "{from} in {text}");
    std::fs::write(&tasks, text.replace(from, to)).unwrap();
}

fn prove(f: &Fx, task: &str) -> proof::Proved {
    proof::prove(&f.project, &f.home, &f.repo, task, &mut std::io::sink()).unwrap()
}

/// The code of each `runner` finding of the change's check report.
fn runner_findings(f: &Fx, c: &change::ChangeDir) -> Vec<String> {
    plan::check(&f.project, &f.repo, Some(&c.id.to_string()))
        .unwrap()
        .remove(0)
        .findings
        .into_iter()
        .filter(|(_, d)| d.rule.code() == "runner")
        .map(|(_, d)| format!("line {}: {}", d.line, d.message))
        .collect()
}

#[test]
fn runners_fill_template_default_env_and_review() {
    let f = fx("fill", RUNNER_CPP);
    let c = backlog(&f, "@cpp test=Seek", "");
    let mut echo = Vec::new();
    // The baseline fails as it must: out.txt holds nothing yet.
    let prepared = vow::prepare(&f.project, &f.home, c, &f.repo, &mut echo).unwrap();
    assert!(!prepared.baseline[0].outcome.passed());
    // The baseline ran the filled command: the default preset and the task's test.
    let echoed = String::from_utf8(echo).unwrap();
    assert!(echoed.contains("baseline TSK-new1: grep -q 'tests Seek' out.txt"), "{echoed}");

    let task = accept_prepared(&f, prepared);
    std::fs::write(f.repo.join("out.txt"), "tests Seek\n").unwrap();
    let proved = prove(&f, &task);
    assert!(proved.proved, "{:?}", proved.outcome);
    assert_eq!(proved.outcome.tests_run, Some(1));

    // rvow review shows the filled command, not the @runner text.
    let id = active(&f).dir.id.to_string();
    let packet = review::packet_for(&f.project, &f.home, &f.repo, &id).unwrap();
    assert!(packet.text.contains("verify: grep -q 'tests Seek' out.txt"), "packet shows the raw verify");
}

#[test]
fn runners_fill_explicit_params_env_and_runner_timeout() {
    // An env var and a runner timeout reach the run; the task timeout wins.
    let cfg = "\
[runners.cpp]
run = \"grep -q '{preset} {test}' out.txt && echo 'test result: ok. 1 passed; 0 failed;' && test \\\"$RVOW_TOOL\\\" = yes\"
default = { preset = \"tests\" }
env = { RVOW_TOOL = \"yes\" }
timeout = \"5m\"
";
    let f = fx("env", cfg);
    let c = backlog(&f, "@cpp preset=asan test=Seek", "");
    let task = accept(&f, c);
    std::fs::write(f.repo.join("out.txt"), "asan Seek\n").unwrap();
    assert!(prove(&f, &task).proved);
}

#[test]
fn runners_fill_check_errors_name_the_problem() {
    let f = fx("errors", RUNNER_CPP);
    let found = runner_findings(&f, &backlog(&f, "@nope test=Seek", ""));
    assert_eq!(found.len(), 1, "{found:?}");
    assert!(found[0].contains("nope") && found[0].contains("TSK-new1"), "{found:?}");

    let found = runner_findings(&f, &backlog(&f, "@cpp preset=dev test=Seek", ""));
    assert_eq!(found.len(), 1, "{found:?}");
    assert!(found[0].contains("dev") && found[0].contains("tests, asan, tsan"), "{found:?}");

    let found = runner_findings(&f, &backlog(&f, "@cpp test=Seek target=pool", ""));
    assert_eq!(found.len(), 1, "{found:?}");
    assert!(found[0].contains("target") && found[0].contains("does not use"), "{found:?}");

    // A {param} with no value anywhere: a runner with no default for preset.
    let f = fx("errors2", "[runners.cpp]\nrun = \"run {preset} {test}\"\n");
    let found = runner_findings(&f, &backlog(&f, "@cpp test=Seek", ""));
    assert_eq!(found.len(), 1, "{found:?}");
    assert!(found[0].contains("{preset}") && found[0].contains("no value"), "{found:?}");

    // A check with a runner error is blocking.
    let r = plan::check(&f.project, &f.repo, None).unwrap().remove(0);
    assert!(r.blocking(), "{r:?}");
}

#[test]
fn runners_fill_safety_refuses_a_bad_char_before_the_run() {
    // At check time the bad value is a finding that names the char.
    let f = fx("safety", RUNNER_CPP);
    let found = runner_findings(&f, &backlog(&f, "@cpp test='Seek", ""));
    assert_eq!(found.len(), 1, "{found:?}");
    assert!(found[0].contains('\'') && found[0].contains("TSK-new1"), "{found:?}");

    // And again before a run: prove runs nothing and refuses.
    let f = fx("safety2", RUNNER_CPP);
    let c = backlog(&f, "@cpp test=Seek", "");
    let task = accept(&f, c);
    edit_tasks(&f, "test=Seek", "test='Seek;rm");
    let err = match proof::prove(&f.project, &f.home, &f.repo, &task, &mut std::io::sink()) {
        Err(e) => e,
        Ok(_) => panic!("a bad value must refuse the run"),
    };
    assert_eq!(err.code, "runner-value", "{err}");
    assert!(err.message.contains(';'), "{err}");
    assert!(!f.repo.join("out.txt").exists(), "no command ran");
}

#[test]
fn runners_fill_plain_verify_still_works() {
    let f = fx("plain", "");
    let c = backlog(&f, "grep -q ok out.txt && echo 'test result: ok. 1 passed; 0 failed;'", "");
    let task = accept(&f, c);
    std::fs::write(f.repo.join("out.txt"), "ok\n").unwrap();
    assert!(prove(&f, &task).proved);
}

#[test]
fn runners_fill_plan_rules_look_at_the_filled_command() {
    // A runner that chains three commands and runs no counted test runner:
    // the slow and weak rules must judge the filled command, not the @text.
    let cfg = "[runners.slow]\nrun = \"true && true && true\"\n";
    let f = fx("plan", cfg);
    let c = backlog(&f, "@slow", "  baseline: pass\n");
    let r = plan::check(&f.project, &f.repo, Some(&c.id.to_string())).unwrap().remove(0);
    assert!(r.slow.iter().any(|s| s.message.contains("chains 3 commands")), "{:?}", r.slow);
    let weak: Vec<_> = r.findings.iter().filter(|(_, d)| d.rule.code() == "weak-verify").collect();
    assert!(weak.iter().any(|(_, d)| d.message.contains("true && true && true") && !d.message.contains("@slow")), "{weak:?}");
}

#[test]
fn runners_fill_gate_rerun_uses_the_filled_command() {
    // rerun_proven keeps a proven, committed task in the rerun, so the test
    // sees the gate run the filled command.
    let f = fx("gate", &format!("rerun_proven = true\n{RUNNER_CPP}"));
    let c = backlog(&f, "@cpp test=Seek", "");
    let task = accept(&f, c);
    std::fs::write(f.repo.join("out.txt"), "tests Seek\n").unwrap();
    git_cmd(&f.repo, &["add", "out.txt"]);
    git_cmd(&f.repo, &["commit", "-q", "-m", "out"]);
    prove(&f, &task);
    let r = gate::gate(&f.project, &f.home, &f.repo, gate::Options { strict: true, rerun: true, out: None }, &mut std::io::sink()).unwrap();
    assert!(r.passed(), "{r:?}");
    assert!(r.timings.iter().any(|(id, _)| id == &task), "the task reran: {:?}", r.timings);

    // The rerun runs the filled command too: a bad fill refuses the gate.
    edit_tasks(&f, "test=Seek", "test=Nope");
    let r = gate::gate(&f.project, &f.home, &f.repo, gate::Options { strict: true, rerun: true, out: None }, &mut std::io::sink()).unwrap();
    assert!(r.failures.iter().any(|f| f.contains("Nope")), "{r:?}");
}

#[test]
fn runners_fill_vow_event_records_the_filled_command() {
    let f = fx("event", RUNNER_CPP);
    let c = backlog(&f, "@cpp preset=asan test=Seek", "");
    let task = accept(&f, c);
    std::fs::write(f.repo.join("out.txt"), "asan Seek\n").unwrap();
    assert!(prove(&f, &task).proved);
    let dir = task_dir(&f);
    let (events, _) = ledger::read(&dir.join("ledger.jsonl")).unwrap();
    let verify = events.iter().find(|e| e.kind == ledger::Kind::Prove).and_then(|e| e.data.get("verify")).and_then(Value::as_str).unwrap();
    assert!(verify.contains("'asan Seek'") && !verify.contains('@'), "{verify}");
    let runners = config::runners(&f.project).unwrap();
    assert!(runners.contains_key("cpp"));
    let _ = drift::drift(&f.project, &f.repo, &config::load(&f.project, &f.home).unwrap(), &active(&f));
}

/// The drift items of the one active change, as (class, what).
fn drift_of(f: &Fx) -> Vec<(drift::Class, String)> {
    let top = rvow_core::git::toplevel(&f.repo).unwrap();
    let config = config::load(&f.project, &f.home).unwrap();
    drift::drift(&f.project, &top, &config, &active(f)).unwrap().into_iter().map(|i| (i.class, i.what)).collect()
}

fn edit_config(f: &Fx, from: &str, to: &str) {
    let path = f.project.root.join("config.toml");
    let text = std::fs::read_to_string(&path).unwrap();
    assert!(text.contains(from), "{from} in {text}");
    std::fs::write(&path, text.replace(from, to)).unwrap();
}

#[test]
fn runners_drift_changed_runner_is_weaken_per_task() {
    let f = fx("drift", RUNNER_CPP);
    let task = accept(&f, backlog(&f, "@cpp test=Seek", ""));
    assert_eq!(drift_of(&f), []);

    edit_config(&f, "grep -q '{preset} {test}' out.txt", "grep -q '{test}' out.txt");
    let items = drift_of(&f);
    assert_eq!(items, [(drift::Class::Weaken, format!("{task} runner cpp changed"))]);

    // The same edit is not a config change: runners stay out of the vow's
    // canonical config, so it is not reported twice.
    assert!(!items.iter().any(|(_, w)| w == "config changed"), "{items:?}");

    // rvow status and rvow gate report it.
    let status = proof::status(&f.project, &f.home, &f.repo, false).unwrap();
    assert!(status[0].drift.iter().any(|d| d.what.contains("runner cpp changed")), "{:?}", status[0].drift);
    let r = gate::gate(&f.project, &f.home, &f.repo, gate::Options::default(), &mut std::io::sink()).unwrap();
    assert!(r.failures.iter().any(|x| x.contains("weaken drift") && x.contains("runner cpp changed")), "{r:?}");
}

#[test]
fn runners_drift_runner_without_a_task_changes_freely() {
    let cfg = "\
[runners.idle]
run = \"true\"

[runners.cpp]
run = \"grep -q '{preset} {test}' out.txt && echo 'test result: ok. 1 passed; 0 failed;'\"
default = { preset = \"tests\" }
";
    let f = fx("unused", cfg);
    let _task = accept(&f, backlog(&f, "@cpp test=Seek", ""));
    edit_config(&f, "[runners.idle]\nrun = \"true\"", "[runners.idle]\nrun = \"false\"");
    assert_eq!(drift_of(&f), []);

    // A changed default of a used runner drifts too: the hash covers run,
    // env, timeout, params, default and needs.
    edit_config(&f, "default = { preset = \"tests\" }", "default = { preset = \"asan\" }");
    let items = drift_of(&f);
    assert!(items.iter().any(|(_, w)| w.ends_with("runner cpp changed")), "{items:?}");
}

/// The `[runners.t]` fixture: writes fail.xml while the work is missing,
/// pass.xml once out.txt holds "pass", and always fills `{junit}`.
const RUNNER_JUNIT: &str = "\
[runners.t]
run = 'if grep -q pass out.txt; then cp pass.xml {junit}; else cp fail.xml {junit}; exit 1; fi'
";
const XML_PASS: &str = "<testsuite name=\"t\"><testcase classname=\"c\" name=\"Seek\"></testcase><testcase classname=\"c\" name=\"Open\"></testcase></testsuite>\n";
const XML_FAIL: &str = "<testsuite name=\"t\"><testcase classname=\"c\" name=\"Seek\"><failure>boom</failure></testcase><testcase classname=\"c\" name=\"Open\"></testcase></testsuite>\n";

/// A repo with the two JUnit fixtures committed.
fn fx_junit(name: &str, cfg: &str) -> Fx {
    let f = fx(name, cfg);
    std::fs::write(f.repo.join("pass.xml"), XML_PASS).unwrap();
    std::fs::write(f.repo.join("fail.xml"), XML_FAIL).unwrap();
    git_cmd(&f.repo, &["add", "pass.xml", "fail.xml"]);
    git_cmd(&f.repo, &["commit", "-q", "-m", "fixtures"]);
    f
}

/// The vow data of the one active change.
fn vow_data(f: &Fx) -> serde_json::Map<String, Value> {
    let (events, _) = ledger::read(&task_dir(f).join("ledger.jsonl")).unwrap();
    events.iter().find(|e| e.kind == ledger::Kind::Vow).unwrap().data.clone()
}

#[test]
fn runners_junit_accept_records_the_failed_names() {
    let f = fx_junit("record", RUNNER_JUNIT);
    let task = accept(&f, backlog(&f, "@t", ""));
    let data = vow_data(&f);
    let junit = data.get("junit").unwrap();
    assert_eq!(junit[&task], serde_json::json!(["c.Seek"]), "{junit}");

    // The work makes the recorded test pass, and the proof needs it.
    std::fs::write(f.repo.join("out.txt"), "pass\n").unwrap();
    let proved = prove(&f, &task);
    assert!(proved.proved, "{:?}", proved.outcome);
    assert_eq!(proved.junit, Some(rvow_core::junit::Check::Passed));

    // A report whose test still fails fails the proof, with its name.
    std::fs::write(f.repo.join("out.txt"), "no\n").unwrap();
    let proved = prove(&f, &task);
    assert!(!proved.proved);
    assert_eq!(proved.junit, Some(rvow_core::junit::Check::Failed("the recorded test c.Seek failed".into())));
}

#[test]
fn runners_junit_accept_refuses_when_nothing_failed() {
    // The report the baseline writes names no failed test, so no test is the
    // proof target: the accept refuses and names the task.
    let f = fx_junit("nofail", RUNNER_JUNIT);
    std::fs::write(f.repo.join("fail.xml"), XML_PASS).unwrap();
    let c = backlog(&f, "@t", "");
    let err = match vow::prepare(&f.project, &f.home, c, &f.repo, &mut std::io::sink()) {
        Err(e) => e,
        Ok(_) => panic!("an accept with no failed test must refuse"),
    };
    assert_eq!(err.code, "baseline", "{err}");
    assert!(err.message.contains("TSK-new1") && err.message.contains("no failed test"), "{err}");
}

#[test]
fn runners_junit_prove_fails_when_a_recorded_test_is_missing() {
    let f = fx_junit("missing", RUNNER_JUNIT);
    let task = accept(&f, backlog(&f, "@t", ""));
    // The passing report no longer holds the recorded test.
    std::fs::write(f.repo.join("pass.xml"), "<testsuite><testcase classname=\"c\" name=\"Open\"></testcase></testsuite>\n").unwrap();
    std::fs::write(f.repo.join("out.txt"), "pass\n").unwrap();
    let proved = prove(&f, &task);
    assert!(!proved.proved);
    assert_eq!(proved.junit, Some(rvow_core::junit::Check::Failed("the recorded test c.Seek is missing from the report".into())));
}

#[test]
fn runners_junit_prove_fails_when_the_report_is_missing() {
    // Accepted with a report; then the runner is edited so the run holds
    // {junit} but writes no report: the proof fails and says so.
    let f = fx_junit("noreport", RUNNER_JUNIT);
    let task = accept(&f, backlog(&f, "@t", ""));
    edit_config(
        &f,
        "run = 'if grep -q pass out.txt; then cp pass.xml {junit}; else cp fail.xml {junit}; exit 1; fi'",
        "run = 'grep -q pass out.txt # {junit}'",
    );
    std::fs::write(f.repo.join("out.txt"), "pass\n").unwrap();
    let proved = prove(&f, &task);
    assert!(!proved.proved, "a missing report must fail the proof");
    assert!(matches!(&proved.junit, Some(rvow_core::junit::Check::Failed(m)) if m.contains("cannot be read")), "{:?}", proved.junit);
}

#[test]
fn runners_junit_gate_rerun_uses_the_same_rule() {
    let f = fx_junit("gate", &format!("rerun_proven = true\n{RUNNER_JUNIT}"));
    // pass.xml stays in scope, so a report edit reaches the rerun.
    let c = change::new(&f.project, "Run", "main", false).unwrap();
    std::fs::write(c.path.join("tasks.md"), "- TSK-new1 Run the runner\n  covers: REQ-new1\n  verify: @t\n  files: out.txt, pass.xml\n")
        .unwrap();
    std::fs::write(c.path.join("specs/out.md"), SPEC).unwrap();
    let task = accept(&f, c);
    std::fs::write(f.repo.join("out.txt"), "pass\n").unwrap();
    git_cmd(&f.repo, &["add", "out.txt"]);
    git_cmd(&f.repo, &["commit", "-q", "-m", "out"]);
    assert!(prove(&f, &task).proved);
    let opts = gate::Options { strict: true, rerun: true, out: None };
    let r = gate::gate(&f.project, &f.home, &f.repo, opts.clone(), &mut std::io::sink()).unwrap();
    assert!(r.passed(), "{r:?}");

    // A report that fails a recorded test fails the rerun, with its name.
    std::fs::write(f.repo.join("pass.xml"), XML_FAIL).unwrap();
    git_cmd(&f.repo, &["add", "pass.xml"]);
    git_cmd(&f.repo, &["commit", "-q", "-m", "report"]);
    let r = gate::gate(&f.project, &f.home, &f.repo, opts, &mut std::io::sink()).unwrap();
    assert!(r.failures.iter().any(|x| x.contains("junit") && x.contains("c.Seek")), "{r:?}");
}

/// A runner with a needs path: the build dir must exist before anything runs.
const RUNNER_NEEDS: &str = "\
[runners.cpp]
run = \"grep -q '{preset} {test}' out.txt && echo 'test result: ok. 1 passed; 0 failed;'\"
default = { preset = \"tests\" }
needs = [\"build-{preset}\"]
";

#[test]
fn runners_guard_needs_missing_refuses_before_any_run() {
    let f = fx("needs", RUNNER_NEEDS);
    // No build-tests dir: accept and prove run nothing and name the path.
    let c = backlog(&f, "@cpp test=Seek", "");
    let err = match vow::prepare(&f.project, &f.home, c, &f.repo, &mut std::io::sink()) {
        Err(e) => e,
        Ok(_) => panic!("a missing needs path must refuse the accept"),
    };
    assert_eq!(err.code, "needs", "{err}");
    assert!(err.message.contains("build-tests") && err.message.contains("TSK-new1"), "{err}");
    assert!(err.fix.contains("configure build-tests"), "{err}");
    assert!(!f.repo.join("out.txt").exists(), "no command ran");

    // With the dir the accept works, and prove needs it too.
    std::fs::create_dir_all(f.repo.join("build-tests")).unwrap();
    std::fs::write(f.repo.join("build-tests/keep"), "x").unwrap();
    git_cmd(&f.repo, &["add", "build-tests"]);
    git_cmd(&f.repo, &["commit", "-q", "-m", "build"]);
    let task = accept(&f, backlog(&f, "@cpp test=Seek", ""));
    std::fs::remove_dir_all(f.repo.join("build-tests")).unwrap();
    git_cmd(&f.repo, &["add", "-A"]);
    git_cmd(&f.repo, &["commit", "-q", "-m", "remove build"]);
    let err = match proof::prove(&f.project, &f.home, &f.repo, &task, &mut std::io::sink()) {
        Err(e) => e,
        Ok(_) => panic!("a missing needs path must refuse the prove"),
    };
    assert_eq!(err.code, "needs", "{err}");
    assert!(err.message.contains("build-tests"), "{err}");

    // The gate rerun refuses too: its clean worktree holds no build dir.
    std::fs::write(f.repo.join("out.txt"), "tests Seek\n").unwrap();
    git_cmd(&f.repo, &["add", "out.txt"]);
    git_cmd(&f.repo, &["commit", "-q", "-m", "out"]);
    std::fs::write(f.project.root.join("config.toml"), format!("rerun_proven = true\n{RUNNER_NEEDS}")).unwrap();
    let r = gate::gate(&f.project, &f.home, &f.repo, gate::Options { strict: true, rerun: true, out: None }, &mut std::io::sink()).unwrap();
    assert!(r.failures.iter().any(|x| x.contains("build-tests")), "{r:?}");
}

#[test]
fn runners_guard_configure_in_verify_warns() {
    let f = fx("configure", RUNNER_CPP);
    // A plain verify that configures warns and names the task.
    let c = backlog(&f, "cmake --preset tests && ctest --preset tests", "");
    let r = plan::check(&f.project, &f.repo, Some(&c.id.to_string())).unwrap().remove(0);
    let warns: Vec<_> = r.findings.iter().filter(|(_, d)| d.rule.code() == "configure-in-verify").collect();
    assert_eq!(warns.len(), 1, "{:?}", r.findings);
    assert!(warns[0].1.message.contains("TSK-new1"), "{}", warns[0].1.message);
    assert_eq!(warns[0].1.rule.severity(), rvow_core::diag::Severity::Warning);

    // A build command does not warn, by its words and not by substring.
    let f = fx("configure2", "");
    let c = backlog(&f, "cmake --build --preset tests", "");
    let r = plan::check(&f.project, &f.repo, Some(&c.id.to_string())).unwrap().remove(0);
    assert!(!r.findings.iter().any(|(_, d)| d.rule.code() == "configure-in-verify"), "{:?}", r.findings);

    // A runner whose run configures warns and names the runner.
    let cfg = "[runners.conf]\nrun = 'cmake -S . -B build'\n";
    let f = fx("configure3", cfg);
    let c = backlog(&f, "@conf", "");
    let r = plan::check(&f.project, &f.repo, Some(&c.id.to_string())).unwrap().remove(0);
    let warns: Vec<_> = r.findings.iter().filter(|(_, d)| d.rule.code() == "configure-in-verify").collect();
    assert_eq!(warns.len(), 1, "{:?}", r.findings);
    assert!(warns[0].1.message.contains("runner conf"), "{}", warns[0].1.message);

    // A runner that only builds does not warn.
    let cfg = "[runners.conf]\nrun = 'cmake --build --preset tests'\n";
    let f = fx("configure4", cfg);
    let c = backlog(&f, "@conf", "");
    let r = plan::check(&f.project, &f.repo, Some(&c.id.to_string())).unwrap().remove(0);
    assert!(!r.findings.iter().any(|(_, d)| d.rule.code() == "configure-in-verify"), "{:?}", r.findings);
}

#[test]
fn runners_guard_reconfigure_note_names_the_needs_dir() {
    let cfg = "\
[runners.c]
run = \"echo 'Re-running CMake...'; grep -q ok out.txt && echo 'test result: ok. 1 passed; 0 failed;'\"
needs = [\"build-tests\"]
";
    let f = fx("reconf", cfg);
    std::fs::create_dir_all(f.repo.join("build-tests")).unwrap();
    std::fs::write(f.repo.join("build-tests/keep"), "x").unwrap();
    git_cmd(&f.repo, &["add", "build-tests"]);
    git_cmd(&f.repo, &["commit", "-q", "-m", "build"]);
    let task = accept(&f, backlog(&f, "@c", ""));
    std::fs::write(f.repo.join("out.txt"), "ok\n").unwrap();
    let proved = prove(&f, &task);
    assert!(proved.proved, "{:?}", proved.outcome);
    assert_eq!(proved.note.as_deref(), Some("the build reconfigured; run `ninja -C build-tests -d explain` to see why"));

    // Without a needs path the note stays short.
    let cfg = "[runners.c]\nrun = \"echo 'Re-running CMake...'; grep -q ok out.txt && echo 'test result: ok. 1 passed; 0 failed;'\"\n";
    let f = fx("reconf2", cfg);
    let task = accept(&f, backlog(&f, "@c", ""));
    std::fs::write(f.repo.join("out.txt"), "ok\n").unwrap();
    let proved = prove(&f, &task);
    assert!(proved.proved);
    assert_eq!(proved.note.as_deref(), Some("the build reconfigured"));

    // A run without a reconfigure gets no note.
    let cfg = "[runners.c]\nrun = \"grep -q ok out.txt && echo 'test result: ok. 1 passed; 0 failed;'\"\n";
    let f = fx("reconf3", cfg);
    let task = accept(&f, backlog(&f, "@c", ""));
    std::fs::write(f.repo.join("out.txt"), "ok\n").unwrap();
    assert_eq!(prove(&f, &task).note, None);
}
