//! Fills a task's `verify: @runner key=value` line from the `[runners]` table
//! of the project config. The same fill backs `rvow check`, `rvow accept`,
//! `rvow prove`, the gate rerun and `rvow review`, so no caller runs the raw
//! `@runner` text.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::Duration;

use serde::{Deserialize, Serialize};

use crate::error::{Error, Result};
use crate::tasks::Task;
use crate::vow::sha256_hex;

/// The placeholder that rvow itself fills with the JUnit report path.
pub const JUNIT: &str = "junit";

/// One `[runners.<name>]` entry of the project config.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Runner {
    /// The command template; each `{param}` is filled from the task or `default`.
    pub run: String,
    /// The allowed values of each param, when the runner wants to bound them.
    pub params: BTreeMap<String, Vec<String>>,
    /// The value of each param that the task may leave out.
    pub default: BTreeMap<String, String>,
    /// Paths that must exist before the command runs, with params filled.
    pub needs: Vec<String>,
    /// Extra environment variables for the command.
    pub env: BTreeMap<String, String>,
    /// How long the command may run, when the task sets no `timeout:` of its own.
    #[serde(default, with = "opt_duration", skip_serializing_if = "Option::is_none")]
    pub timeout: Option<Duration>,
    /// An incremental build of what `run` executes, with params filled. Needs `artifacts`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub build: Option<String>,
    /// The built files `run` executes, relative to the work tree, with params filled. Needs `build`.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub artifacts: Vec<String>,
}

mod opt_duration {
    use std::time::Duration;

    use serde::{Deserialize, Deserializer, Serializer};

    pub fn serialize<S: Serializer>(v: &Option<Duration>, s: S) -> Result<S::Ok, S::Error> {
        match v {
            None => s.serialize_none(),
            Some(d) => s.serialize_some(&d.as_secs()),
        }
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<Option<Duration>, D::Error> {
        let text: Option<String> = Option::deserialize(d)?;
        text.map(|t| {
            crate::tasks::parse_duration(&t)
                .ok_or_else(|| serde::de::Error::custom(format!("timeout {t:?} is not a duration such as 90s, 20m or 1h")))
        })
        .transpose()
    }
}

impl Runner {
    /// The identity of the runner that a vow records for each task that uses
    /// it: every field that changes what the command runs.
    pub fn hash(&self) -> String {
        // `build` and `artifacts` only decide whether a proof may be kept, so they stay out of the stamp.
        let own = Runner { build: None, artifacts: Vec::new(), ..self.clone() };
        let text = serde_json::to_string(&own).map_err(|e| Error::internal(format!("runner does not serialize: {e}")));
        sha256_hex(text.unwrap_or_default().as_bytes())
    }
}

/// The first char of `value` that is not safe inside a shell command, if any.
fn bad_char(value: &str) -> Option<char> {
    value.chars().find(|c| !(c.is_ascii_alphanumeric() || "_./+=*^$-:".contains(*c)))
}

/// Whether `value` holds only chars that are safe inside a shell command.
/// Anything outside [A-Za-z0-9_.:/+=*^$-] is refused, and the char is named.
pub fn value_ok(value: &str) -> Result<()> {
    match bad_char(value) {
        None => Ok(()),
        Some(c) => Err(Error::new(
            "runner-value",
            format!("the runner value {value:?} holds the char {c:?}, which is not allowed"),
            "write the value with only A-Z a-z 0-9 _ . : / + = * ^ $ and -",
        )),
    }
}

/// Quotes `path` for the shell when it holds a char outside the safe set, so
/// a TMPDIR with a space works. A path without one passes through unchanged.
fn shell_quote(path: &str) -> String {
    if path.chars().all(|c| c.is_ascii_alphanumeric() || "_./:+-".contains(c)) {
        path.to_string()
    } else {
        format!("'{}'", path.replace('\'', "'\\''"))
    }
}

/// The `@name` reference of a verify line, with its `key=value` arguments, or
/// `None` when the verify is a plain command.
pub fn reference(verify: &str) -> Option<(&str, &str)> {
    let rest = verify.strip_prefix('@')?;
    Some(match rest.split_once(char::is_whitespace) {
        Some((name, args)) => (name, args.trim()),
        None => (rest, ""),
    })
}

/// The command a task runs: a filled runner, or its plain verify text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Plan {
    /// The command to run, every placeholder filled.
    pub command: String,
    /// Extra environment variables the command needs.
    pub env: Vec<(String, String)>,
    /// The runner's own timeout; a task `timeout:` still wins over it.
    pub timeout: Option<Duration>,
    /// The runner's name, when the verify names one.
    pub runner: Option<String>,
    /// The JUnit report path rvow filled `{junit}` with.
    pub junit: Option<PathBuf>,
    /// The runner's `needs` paths, with every param filled.
    pub needs: Vec<String>,
    /// The runner's `build` command, with every param filled.
    pub build: Option<String>,
    /// The runner's `artifacts` paths, with every param filled.
    pub artifacts: Vec<String>,
}

impl Drop for Plan {
    /// Removes the run's JUnit report, when the run wrote one. Prove,
    /// accept, the amend baseline and the gate read the report first, and
    /// the temp file must not pile up. A missing file is not an error.
    fn drop(&mut self) {
        if let Some(report) = self.junit.take() {
            let _ = std::fs::remove_file(report);
        }
    }
}

impl Plan {
    /// The first `needs` path that does not exist in `dir`, params filled.
    /// The caller refuses before it runs anything.
    pub fn missing_need(&self, dir: &Path) -> Option<String> {
        self.needs.iter().find(|p| !dir.join(p).exists()).cloned()
    }

    fn plain(verify: &str) -> Plan {
        Plan {
            command: verify.to_string(),
            env: Vec::new(),
            timeout: None,
            runner: None,
            junit: None,
            needs: Vec::new(),
            build: None,
            artifacts: Vec::new(),
        }
    }
}

/// Turns a task's verify line into the command to run. A plain verify passes
/// through unchanged; a `@runner` line is filled from the runner and checked:
/// the runner must exist, every value must be runner-safe and allowed by
/// `params`, every `{param}` of `run` must get a value, and every task key
/// must be one the `run` uses.
pub fn plan(runners: &BTreeMap<String, Runner>, task: &Task) -> Result<Plan> {
    plan_inner(runners, task, false)
}

/// Like `plan`, but a `{junit}` placeholder stays as `{junit}` for display:
/// `review`, `show`, `brief` and the delegate brief show the command without
/// a run's temp path. The returned plan holds no junit path.
pub fn plan_display(runners: &BTreeMap<String, Runner>, task: &Task) -> Result<Plan> {
    plan_inner(runners, task, true)
}

fn plan_inner(runners: &BTreeMap<String, Runner>, task: &Task, display: bool) -> Result<Plan> {
    let verify = task.verify_command()?;
    let Some((name, args)) = reference(verify) else {
        return Ok(Plan::plain(verify));
    };
    let runner = runners.get(name).ok_or_else(|| {
        Error::new(
            "runner",
            format!("{} names the runner {name:?}, which the project config does not hold", task.id),
            format!("add a [runners.{name}] table with a run command to config.toml, or drop @{name} from the verify"),
        )
    })?;
    // rvow quotes the {junit} path itself, so a quoted placeholder would
    // reach the shell wrapped twice.
    if runner.run.contains("'{junit}'") || runner.run.contains("\"{junit}\"") {
        return Err(Error::new(
            "runner",
            format!("runner {name:?} quotes {{junit}} in its run"),
            "write {junit} without quotes in the runner's run",
        ));
    }

    if runner.build.is_some() != !runner.artifacts.is_empty() {
        let (set, missing) = if runner.build.is_some() { ("build", "artifacts") } else { ("artifacts", "build") };
        return Err(Error::new(
            "runner",
            format!("runner {name:?} sets {set} but not {missing}"),
            format!("set both build and artifacts on runners.{name}, or neither"),
        ));
    }

    let mut given: BTreeMap<String, String> = BTreeMap::new();
    for token in args.split_whitespace() {
        let (key, value) = match token.split_once('=') {
            Some((k, v)) => (k, v),
            None => {
                return Err(Error::new(
                    "runner",
                    format!("{} passes {token:?} to runner {name:?} without a value", task.id),
                    "write the verify as @name key=value, one pair per word",
                ));
            }
        };
        if key == JUNIT {
            return Err(Error::new(
                "runner",
                format!("{} sets junit in its verify, but rvow fills the {JUNIT} path itself", task.id),
                format!("remove junit=... from the verify; the runner's run writes its report to {{{JUNIT}}}"),
            ));
        }
        value_ok(value)?;
        if given.insert(key.to_string(), value.to_string()).is_some() {
            return Err(Error::new(
                "runner",
                format!("{} passes the key {key:?} twice to runner {name:?}", task.id),
                "keep each key once in the verify",
            ));
        }
    }

    let used = placeholders(&runner.run);
    for (key, value) in &given {
        if !used.iter().any(|u| u == key) {
            return Err(Error::new(
                "runner",
                format!("{} passes {key}={value}, which the run of runner {name:?} does not use", task.id),
                format!("remove {key}=... from the verify, or add {{{key}}} to the runner's run"),
            ));
        }
        if let Some(allowed) = runner.params.get(key)
            && !allowed.contains(value)
        {
            return Err(Error::new(
                "runner",
                format!("{} passes {key}={value}, which runner {name:?} does not allow; its {key} allows {}", task.id, allowed.join(", ")),
                format!("use one of {} for {key}", allowed.join(", ")),
            ));
        }
    }

    let mut values: BTreeMap<String, String> = BTreeMap::new();
    for param in &used {
        if param == JUNIT {
            continue;
        }
        let from_task = given.get(param);
        let value = from_task.or_else(|| runner.default.get(param)).ok_or_else(|| {
            Error::new(
                "runner",
                format!("{}: the param {{{param}}} of runner {name:?} gets no value", task.id),
                format!("pass {param}=... in the verify, or set runners.{name}.default.{param}"),
            )
        })?;
        // A runner default gets the same checks as a task value: only the
        // shell-safe chars, and only a value the runner allows.
        if from_task.is_none() {
            if let Some(c) = bad_char(value) {
                return Err(Error::new(
                    "runner-value",
                    format!("the default {param}={value} of runner {name:?} holds the char {c:?}, which is not allowed"),
                    "write the default with only A-Z a-z 0-9 _ . : / + = * ^ $ and -",
                ));
            }
            if let Some(allowed) = runner.params.get(param)
                && !allowed.contains(value)
            {
                return Err(Error::new(
                    "runner",
                    format!("the default {param}={value} of runner {name:?} is not allowed; its {param} allows {}", allowed.join(", ")),
                    format!("use one of {} for the default {param}", allowed.join(", ")),
                ));
            }
        }
        values.insert(param.clone(), value.clone());
    }

    let junit = used.iter().any(|p| p == JUNIT).then(|| if display { PathBuf::from("{junit}") } else { junit_path(task, runner) });

    let mut filled = values.clone();
    if let Some(path) = &junit {
        let rendered = path.display().to_string();
        filled.insert(JUNIT.to_string(), if display { rendered } else { shell_quote(&rendered) });
    }
    let command = fill(&runner.run, &filled, &format!("the run of runner {name:?}"))?;
    let needs =
        runner.needs.iter().map(|need| fill(need, &values, &format!("a needs path of runner {name:?}"))).collect::<Result<Vec<_>>>()?;
    let env = runner.env.iter().map(|(k, v)| (k.clone(), v.clone())).collect();
    let build = runner.build.as_ref().map(|b| fill(b, &values, &format!("the build of runner {name:?}"))).transpose()?;
    let artifacts =
        runner.artifacts.iter().map(|a| fill(a, &values, &format!("an artifacts path of runner {name:?}"))).collect::<Result<Vec<_>>>()?;
    Ok(Plan {
        command,
        env,
        timeout: runner.timeout,
        runner: Some(name.to_string()),
        junit: if display { None } else { junit },
        needs,
        build,
        artifacts,
    })
}

/// The dir one run's JUnit report goes in: the runner's `RVOW_JUNIT_DIR`
/// when it sets one, else the system temp dir.
fn junit_dir(runner: &Runner) -> PathBuf {
    runner.env.get("RVOW_JUNIT_DIR").map(PathBuf::from).unwrap_or_else(std::env::temp_dir)
}

/// A fresh path in the report dir for one run's JUnit report.
fn junit_path(task: &Task, runner: &Runner) -> PathBuf {
    let hex = Error::random::<8>()
        .map(|b| b.iter().map(|x| format!("{x:02x}")).collect::<String>())
        .unwrap_or_else(|_| format!("pid-{}", std::process::id()));
    junit_dir(runner).join(format!("rvow-junit-{}-{hex}", task.id))
}

/// Each distinct `{name}` of a template, in order of first use.
pub(crate) fn placeholders(text: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let mut rest = text;
    while let Some(i) = rest.find('{') {
        let after = &rest[i + 1..];
        match after.find('}') {
            Some(j) => {
                let name = after[..j].to_string();
                if !out.contains(&name) {
                    out.push(name);
                }
                rest = &after[j + 1..];
            }
            None => break,
        }
    }
    out
}

/// Replaces each `{name}` of `text` with its value. A name with no value is an
/// error that names it, and so is a `{` with no closing `}`.
fn fill(text: &str, values: &BTreeMap<String, String>, what: &str) -> Result<String> {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(i) = rest.find('{') {
        out.push_str(&rest[..i]);
        let after = &rest[i + 1..];
        let j = after
            .find('}')
            .ok_or_else(|| Error::new("runner", format!("{what} holds a {{ with no closing brace"), "write each placeholder as {name}"))?;
        let name = &after[..j];
        let value = values.get(name).ok_or_else(|| {
            Error::new(
                "runner",
                format!("{what} holds the param {{{name}}} with no value"),
                format!("pass {name}=... in the verify, or set it as a runner default"),
            )
        })?;
        out.push_str(value);
        rest = &after[j + 1..];
    }
    out.push_str(rest);
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tasks::parse_tasks;

    fn task(verify: &str) -> Task {
        parse_tasks(&format!("- TSK-x7k2q4 T\n  verify: {verify}\n  files: a\n")).0.remove(0)
    }

    fn runners(text: &str) -> BTreeMap<String, Runner> {
        #[derive(serde::Deserialize)]
        struct Table {
            runners: BTreeMap<String, Runner>,
        }
        toml::from_str::<Table>(text).unwrap().runners
    }

    #[test]
    fn fills_defaults_params_env_and_timeout() {
        let rs = runners(
            "[runners.cpp]\nrun = \"cmake --build --preset {preset} --target {target} && ctest --preset {preset} -R '{test}' --no-tests=error --output-junit {junit}\"\nparams = { preset = [\"tests\", \"asan\", \"tsan\"] }\ndefault = { preset = \"tests\" }\nneeds = [\"build-{preset}\"]\nenv = { CPM_SOURCE_CACHE = \"/tmp/cpm\" }\ntimeout = \"5m\"\n",
        );
        let t = task("@cpp preset=tsan target=pool_test test=PoolShutdown");
        let p = plan(&rs, &t).unwrap();
        assert_eq!(
            p.command,
            format!(
                "cmake --build --preset tsan --target pool_test && ctest --preset tsan -R 'PoolShutdown' --no-tests=error --output-junit {}",
                p.junit.as_ref().unwrap().display()
            )
        );
        assert_eq!(p.env, [("CPM_SOURCE_CACHE".to_string(), "/tmp/cpm".to_string())]);
        assert_eq!(p.timeout, Some(Duration::from_secs(300)));
        assert_eq!(p.runner.as_deref(), Some("cpp"));
        assert_eq!(p.needs, ["build-tsan".to_string()]);
        assert!(p.junit.as_ref().is_some_and(|j| j.to_string_lossy().contains("TSK")));

        // A param with no value anywhere is an error that names it.
        let t = task("@cpp test=Pool");
        let e = plan(&rs, &t).unwrap_err();
        assert_eq!(e.code, "runner");
        assert!(e.message.contains("target") && e.message.contains("TSK"), "{e}");
    }

    const ARTIFACT_RUNNER: &str = "[runners.cpp]\nrun = \"cmake --build --preset {preset} --target {target} && ctest --preset {preset} -R '{target}'\"\nbuild = \"cmake --build --preset {preset} --target {target}\"\nartifacts = [\"build/{preset}/{target}\"]\ndefault = { preset = \"tests\" }\n";

    #[test]
    fn runner_artifacts_fill_build_and_artifacts_with_the_params() {
        let rs = runners(ARTIFACT_RUNNER);
        let p = plan(&rs, &task("@cpp target=pool_test")).unwrap();
        assert_eq!(p.build.as_deref(), Some("cmake --build --preset tests --target pool_test"));
        assert_eq!(p.artifacts, ["build/tests/pool_test".to_string()]);
        let plain = runners("[runners.cpp]\nrun = \"true\"\n");
        let p = plan(&plain, &task("@cpp")).unwrap();
        assert_eq!((p.build.as_deref(), p.artifacts.is_empty()), (None, true));
        let p = plan(&rs, &task("cargo test")).unwrap();
        assert_eq!((p.build.as_deref(), p.artifacts.is_empty()), (None, true));
    }

    #[test]
    fn runner_artifacts_need_a_build_and_the_build_needs_artifacts() {
        let only_artifacts = runners("[runners.cpp]\nrun = \"t {target}\"\nartifacts = [\"build/{target}\"]\n");
        let e = plan(&only_artifacts, &task("@cpp target=a")).unwrap_err();
        assert_eq!(e.code, "runner");
        assert!(e.message.contains("artifacts") && e.message.contains("build"), "{e}");
        let only_build = runners("[runners.cpp]\nrun = \"t {target}\"\nbuild = \"b {target}\"\n");
        let e = plan(&only_build, &task("@cpp target=a")).unwrap_err();
        assert_eq!(e.code, "runner");
        assert!(e.message.contains("build") && e.message.contains("artifacts"), "{e}");
    }

    #[test]
    fn runner_artifacts_refuse_a_param_the_run_does_not_use_and_junit() {
        let stray = runners("[runners.cpp]\nrun = \"t {target}\"\nbuild = \"b {other}\"\nartifacts = [\"build/{target}\"]\n");
        let e = plan(&stray, &task("@cpp target=a")).unwrap_err();
        assert!(e.message.contains("other"), "{e}");
        let junit = runners("[runners.cpp]\nrun = \"t {target} {junit}\"\nbuild = \"b {target}\"\nartifacts = [\"{junit}\"]\n");
        assert!(plan(&junit, &task("@cpp target=a")).is_err());
    }

    #[test]
    fn runner_artifacts_leave_the_runner_stamp_alone() {
        let with = runners(ARTIFACT_RUNNER).remove("cpp").unwrap();
        let without = Runner { build: None, artifacts: Vec::new(), ..with.clone() };
        assert_eq!(with.hash(), without.hash(), "adding build and artifacts must not make old proofs runner-changed");
    }

    #[test]
    fn plain_verify_passes_through() {
        let rs = runners("[runners.cpp]\nrun = \"true\"\n");
        let p = plan(&rs, &task("make check")).unwrap();
        assert_eq!(p.command, "make check");
        assert_eq!(p.runner, None);
        assert_eq!(p.junit, None);
    }

    #[test]
    fn refuses_unknown_runner_unused_key_and_bad_value() {
        let rs = runners(
            "[runners.cpp]\nrun = \"ctest --preset {preset} -R {test}\"\nparams = { preset = [\"tests\", \"tsan\"] }\ndefault = { preset = \"tests\" }\n",
        );
        let e = plan(&rs, &task("@nope test=Seek")).unwrap_err();
        assert!(e.message.contains("nope") && e.fix.contains("[runners.nope]"), "{e}");
        let e = plan(&rs, &task("@cpp preset=dev test=Seek")).unwrap_err();
        assert!(e.message.contains("dev") && e.fix.contains("tests, tsan"), "{e}");
        let e = plan(&rs, &task("@cpp test=Seek extra=1")).unwrap_err();
        assert!(e.message.contains("extra"), "{e}");
        let e = plan(&rs, &task("@cpp test='Seek")).unwrap_err();
        assert_eq!(e.code, "runner-value");
        assert!(e.message.contains('\''), "{e}");
        let e = plan(&rs, &task("@cpp test=Seek junit=x")).unwrap_err();
        assert!(e.message.contains("junit"), "{e}");
        let e = plan(&rs, &task("@cpp test=Seek preset")).unwrap_err();
        assert!(e.message.contains("without a value"), "{e}");
    }

    #[test]
    fn a_bad_value_char_is_named() {
        let rs = runners("[runners.cpp]\nrun = \"run {test}\"\n");
        for (value, ch) in [("Seek;rm", ';'), ("x`y", '`'), ("x'y", '\'')] {
            let e = plan(&rs, &task(&format!("@cpp test={value}"))).unwrap_err();
            assert_eq!(e.code, "runner-value", "{value}");
            assert!(e.message.contains(ch), "{value}: {e}");
        }
        assert!(plan(&rs, &task("@cpp test=Pool.Shutdown:x/*1+2^3-$a")).is_ok());
    }
}
