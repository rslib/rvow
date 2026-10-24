//! Parser for `tasks.md`. Every non-blank line must be a heading, a task line or
//! a field of the task above it, so a task written in another shape is an error
//! instead of a task that silently goes missing.

use crate::diag::{Diagnostic, Rule};
use crate::error::{Error, Result};
use serde::{Deserialize, Serialize};
use std::time::Duration;

use crate::id::{Kind, Ref};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Task {
    pub id: Ref,
    pub title: String,
    pub group: Option<String>,
    pub covers: Vec<Ref>,
    pub verify: Option<String>,
    pub mutants: Option<String>,
    pub files: Vec<String>,
    pub baseline_pass: bool,
    pub timeout: Option<Duration>,
    /// How strictly this task is held: `strict` unless its `mode:` line says
    /// `relaxed`, or the proposal sets a default. Part of the vow only when relaxed.
    pub mode: Mode,
    /// Whether the task block had its own `mode:` line; `mode` is the effective mode.
    pub mode_given: bool,
    /// How long a cached rerun pass of this task stays valid; not part of the vow.
    pub max_age: Option<Duration>,
    /// Tasks of the same change whose cache key is part of this task's key; not part of the vow.
    pub depends_on: Vec<Ref>,
    pub line: usize,
}

/// How strictly a task is held: the `mode:` field of a task or of the
/// proposal above it.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Mode {
    #[default]
    Strict,
    Relaxed,
}

impl Mode {
    pub fn name(self) -> &'static str {
        match self {
            Mode::Strict => "strict",
            Mode::Relaxed => "relaxed",
        }
    }
}

impl Task {
    pub fn verify_command(&self) -> Result<&str> {
        self.verify.as_deref().ok_or_else(|| {
            Error::new("no-verify", format!("{} has no verify command", self.id), "add a `verify:` line to the task in tasks.md")
        })
    }
}

pub fn parse_tasks(src: &str) -> (Vec<Task>, Vec<Diagnostic>) {
    let mut tasks: Vec<Task> = Vec::new();
    let mut diags = Vec::new();
    let mut group = None;
    let mut seen_fields: Vec<&str> = Vec::new();

    for (i, raw) in src.lines().enumerate() {
        let n = i + 1;
        let line = raw.trim_end();
        if line.trim().is_empty() {
            continue;
        }
        if line.starts_with('#') {
            group = Some(line.trim_start_matches('#').trim().to_string());
            continue;
        }
        if let Some(rest) = line.strip_prefix("- ") {
            let (id, title) = rest.split_once(' ').unwrap_or((rest, ""));
            match id.parse::<Ref>() {
                Ok(id) if id.kind() == Kind::Tsk => {
                    if title.trim().is_empty() {
                        diags.push(Diagnostic::new(Rule::Structure, n, "a task needs a title after its id"));
                    }
                    tasks.push(Task {
                        id,
                        title: title.trim().to_string(),
                        group: group.clone(),
                        covers: Vec::new(),
                        verify: None,
                        mutants: None,
                        files: Vec::new(),
                        baseline_pass: false,
                        timeout: None,
                        mode: Mode::Strict,
                        mode_given: false,
                        max_age: None,
                        depends_on: Vec::new(),
                        line: n,
                    });
                    seen_fields.clear();
                }
                Ok(_) => diags.push(Diagnostic::new(Rule::IdForm, n, "a task id starts with TSK-")),
                Err(e) => diags.push(Diagnostic::new(Rule::IdForm, n, format!("{id:?}: {e}, a task line is `- TSK-xxxxxx <title>`"))),
            }
            continue;
        }
        let field = line.strip_prefix(char::is_whitespace).and_then(|l| l.trim().split_once(':'));
        let (Some(task), Some((key, value))) = (tasks.last_mut(), field) else {
            diags.push(Diagnostic::new(Rule::Structure, n, "not a heading, a `- TSK-...` task line or an indented `key: value` field"));
            continue;
        };
        let key = key.trim();
        let value = value.trim();
        if seen_fields.contains(&key) {
            diags.push(Diagnostic::new(Rule::TaskField, n, format!("{key}: appears twice")));
            continue;
        }
        match key {
            "covers" => {
                for item in split_list(value) {
                    match item.parse::<Ref>() {
                        Ok(r) if r.kind() == Kind::Req => task.covers.push(r),
                        _ => diags.push(Diagnostic::new(Rule::IdForm, n, format!("covers: {item:?} is not a requirement id"))),
                    }
                }
            }
            "verify" if !value.is_empty() => task.verify = Some(value.to_string()),
            "mutants" if !value.is_empty() => task.mutants = Some(value.to_string()),
            "files" => task.files = split_list(value).map(String::from).collect(),
            "baseline" if value == "pass" => task.baseline_pass = true,
            "timeout" => match parse_duration(value) {
                Some(d) => task.timeout = Some(d),
                None => {
                    diags.push(Diagnostic::new(Rule::TaskField, n, format!("timeout: {value:?} is not a duration such as 90s, 20m or 1h")))
                }
            },
            "depends_on" => {
                for item in split_list(value) {
                    match item.parse::<Ref>() {
                        Ok(r) if r.kind() == Kind::Tsk => task.depends_on.push(r),
                        _ => diags.push(Diagnostic::new(Rule::IdForm, n, format!("depends_on: {item:?} is not a task id"))),
                    }
                }
            }
            "max_age" => match parse_duration(value) {
                Some(d) => task.max_age = Some(d),
                None => {
                    diags.push(Diagnostic::new(Rule::TaskField, n, format!("max_age: {value:?} is not a duration such as 90s, 20m or 24h")))
                }
            },
            "mode" => {
                task.mode_given = true;
                match value {
                    "strict" => task.mode = Mode::Strict,
                    "relaxed" => task.mode = Mode::Relaxed,
                    _ => {
                        diags.push(Diagnostic::new(Rule::TaskField, n, format!("mode: {value:?} takes only the values strict or relaxed")))
                    }
                }
            }
            "baseline" => diags.push(Diagnostic::new(Rule::TaskField, n, "baseline: takes only the value pass")),
            _ => {
                diags.push(Diagnostic::new(
                    Rule::TaskField,
                    n,
                    format!(
                        "unknown or empty field {key}:, expected covers, verify, mutants, files, baseline, timeout, max_age, mode or depends_on"
                    ),
                ));
                continue;
            }
        }
        seen_fields.push(key);
    }

    for task in &tasks {
        if task.verify.is_none() {
            diags.push(Diagnostic::new(Rule::TaskField, task.line, format!("{} has no verify: command", task.id)));
        }
        if task.files.is_empty() {
            diags.push(Diagnostic::new(
                Rule::TaskField,
                task.line,
                format!("{} has no files: scope, write files: ** for the whole tree", task.id),
            ));
        }
    }
    (tasks, diags)
}

/// Gives every task with no `mode:` line of its own the change-level mode, so
/// the task holds what its proposal says.
pub fn apply_default(tasks: &mut [Task], default: Option<Mode>) {
    let Some(default) = default else {
        return;
    };
    for task in tasks.iter_mut().filter(|t| !t.mode_given) {
        task.mode = default;
    }
}

/// Parses `90s`, `20m` or `1h`.
pub fn parse_duration(s: &str) -> Option<Duration> {
    let unit = s.chars().last()?;
    let n: u64 = s.strip_suffix(unit)?.parse().ok()?;
    let secs = match unit {
        's' => n,
        'm' => n.checked_mul(60)?,
        'h' => n.checked_mul(3600)?,
        _ => return None,
    };
    (secs > 0).then(|| Duration::from_secs(secs))
}

fn split_list(value: &str) -> impl Iterator<Item = &str> {
    value.split(',').map(str::trim).filter(|s| !s.is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_task_without_verify_is_an_error() {
        let (tasks, _) = parse_tasks("- TSK-x7k2q4 Bare\n  files: **\n");
        assert_eq!(tasks[0].verify_command().unwrap_err().code, "no-verify");
    }

    #[test]
    fn parses_tasks_with_fields() {
        let src = "\
## Ledger

- TSK-x7k2q4 Record the output tail
  covers: REQ-new1, REQ-k3f9m2
  verify: cargo test -p rvow-core output_tail
  files: crates/rvow-core/**, Cargo.toml
  baseline: pass
- TSK-p3x8a1 Second
  verify: true
  files: **
  timeout: 20m
";
        let (tasks, diags) = parse_tasks(src);
        assert!(diags.is_empty(), "{diags:?}");
        assert_eq!(tasks.len(), 2);
        let t = &tasks[0];
        assert_eq!(t.group.as_deref(), Some("Ledger"));
        assert_eq!(t.covers.len(), 2);
        assert_eq!(t.verify.as_deref(), Some("cargo test -p rvow-core output_tail"));
        assert_eq!(t.files, ["crates/rvow-core/**", "Cargo.toml"]);
        assert!(t.baseline_pass);
        assert!(!tasks[1].baseline_pass);
        assert_eq!(tasks[1].timeout, Some(Duration::from_secs(1200)));
    }

    #[test]
    fn task_mode_relaxed_is_read() {
        let (tasks, diags) = parse_tasks("- TSK-x7k2q4 Tail\n  verify: true\n  files: a\n  mode: relaxed\n");
        assert!(diags.is_empty(), "{diags:?}");
        assert_eq!(tasks[0].mode, Mode::Relaxed);
        assert_eq!(tasks[0].mode.name(), "relaxed");
    }

    #[test]
    fn task_mode_absent_is_strict() {
        let (tasks, diags) = parse_tasks("- TSK-x7k2q4 Tail\n  verify: true\n  files: a\n");
        assert!(diags.is_empty(), "{diags:?}");
        assert_eq!(tasks[0].mode, Mode::Strict);
        assert_eq!(Mode::default(), Mode::Strict);
        assert_eq!(Mode::Strict.name(), "strict");
    }

    #[test]
    fn task_mode_unknown_value_is_an_error() {
        let (tasks, diags) = parse_tasks("- TSK-x7k2q4 Tail\n  verify: true\n  files: a\n  mode: loose\n");
        assert_eq!(tasks[0].mode, Mode::Strict, "an unreadable mode leaves the default");
        assert_eq!(diags.len(), 1, "{diags:?}");
        assert_eq!(diags[0].rule, Rule::TaskField);
        let message = &diags[0].message;
        for want in ["mode", "strict", "relaxed"] {
            assert!(message.contains(want), "{want:?} is missing from {message}");
        }
    }

    #[test]
    fn mode_default_change_mode_fills_tasks_without_a_line() {
        let (mut tasks, diags) =
            parse_tasks("- TSK-x7k2q4 Plain\n  verify: true\n  files: a\n- TSK-p3x8a1 Held\n  verify: true\n  files: a\n  mode: strict\n");
        assert!(diags.is_empty(), "{diags:?}");
        apply_default(&mut tasks, Some(Mode::Relaxed));
        assert_eq!(tasks[0].mode, Mode::Relaxed);
        assert!(!tasks[0].mode_given);
        assert_eq!(tasks[1].mode, Mode::Strict);
        assert!(tasks[1].mode_given);
        apply_default(&mut tasks, None);
        assert_eq!(tasks[0].mode, Mode::Relaxed, "no default keeps the mode");
    }

    #[test]
    fn durations() {
        assert_eq!(parse_duration("90s"), Some(Duration::from_secs(90)));
        assert_eq!(parse_duration("1h"), Some(Duration::from_secs(3600)));
        assert_eq!(parse_duration("0m"), None);
        assert_eq!(parse_duration("5"), None);
        assert_eq!(parse_duration("m"), None);
        assert_eq!(parse_duration("5d"), None);
        assert_eq!(parse_duration("5\u{e9}"), None);
    }

    #[test]
    fn odd_lines_and_missing_fields_are_errors() {
        let src = "\
- [ ] 1.1 Checkbox task
* TSK-x7k2q4 star bullet
- TSK-x7k2q4 No fields
  owner: me
  baseline: fail
  covers: TSK-p3x8a1
  verify: a
  verify: b
";
        let (tasks, diags) = parse_tasks(src);
        assert_eq!(tasks.len(), 1);
        let got: Vec<(Rule, usize)> = diags.iter().map(|d| (d.rule, d.line)).collect();
        assert_eq!(
            got,
            [
                (Rule::IdForm, 1),
                (Rule::Structure, 2),
                (Rule::TaskField, 4),
                (Rule::TaskField, 5),
                (Rule::IdForm, 6),
                (Rule::TaskField, 8),
                (Rule::TaskField, 3),
            ]
        );
    }
}
