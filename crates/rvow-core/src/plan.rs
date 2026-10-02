//! `rvow check`: a read-only report on a plan before the user accepts it.

use std::path::Path;

use crate::change::{self, ChangeDir, Loaded, Stage};
use crate::error::Result;
use crate::git;
use crate::lint::{Finding, Question, questions};
use crate::store::Project;
use crate::tasks::Task;

/// Shell builtins and keywords a verify command may start with.
const BUILTINS: &[&str] = &[
    "cd", "test", "[", "true", "false", "echo", "export", "set", "exit", ":", "printf", "read",
    "source", ".", "eval", "command", "type", "if", "for", "while", "!", "(", "{",
];

/// The program a shell command runs first, after `VAR=value` assignments and
/// `env`.
fn program(cmd: &str) -> Option<&str> {
    cmd.split_whitespace()
        .find(|w| !(w.contains('=') && !w.starts_with('=')) && *w != "env")
}

fn on_path(name: &str) -> bool {
    std::env::var_os("PATH").is_some_and(|paths| {
        std::env::split_paths(&paths).any(|dir| {
            let p = dir.join(name);
            p.is_file()
        })
    })
}

/// Tasks whose verify command starts with a program that neither exists on
/// PATH nor as a file in the repository. Such a command fails for the wrong
/// reason, so its failing baseline proves nothing.
pub fn missing_programs(top: &Path, tasks: &[Task]) -> Vec<(String, String)> {
    let mut out = Vec::new();
    for t in tasks {
        let Some(cmd) = t.verify.as_deref() else {
            continue;
        };
        let Some(prog) = program(cmd) else {
            continue;
        };
        let found = BUILTINS.contains(&prog)
            || if prog.contains('/') {
                let p = Path::new(prog);
                if p.is_absolute() {
                    p.exists()
                } else {
                    top.join(p).exists()
                }
            } else {
                on_path(prog)
            };
        if !found {
            out.push((t.id.to_string(), prog.to_string()));
        }
    }
    out
}

#[derive(Debug)]
pub struct Report {
    pub change: ChangeDir,
    pub findings: Vec<Finding>,
    pub missing: Vec<(String, String)>,
    pub questions: Vec<Question>,
}

impl Report {
    pub fn blocking(&self) -> bool {
        self.findings
            .iter()
            .any(|(_, d)| d.severity() == crate::diag::Severity::Error)
            || !self.missing.is_empty()
            || !self.questions.is_empty()
    }
}

fn report(change: ChangeDir, loaded: Loaded, top: &Path) -> Report {
    Report {
        missing: missing_programs(top, &loaded.tasks),
        questions: questions(&loaded.texts),
        findings: loaded.findings,
        change,
    }
}

/// Checks `target`, or every open change whose proposal names the current
/// branch.
pub fn check(project: &Project, cwd: &Path, target: Option<&str>) -> Result<Vec<Report>> {
    let top = git::toplevel(cwd)?;
    let (living, _) = change::load_living(project)?;
    let dirs = match target {
        Some(t) => vec![change::resolve(project, t)?],
        None => {
            let branch = git::branch(&top)?.unwrap_or_default();
            let (all, _) = change::list(project)?;
            let mut out = Vec::new();
            for c in all
                .into_iter()
                .filter(|c| matches!(c.stage, Stage::Backlog | Stage::Active))
            {
                if change::load(&c.path, &living)?.proposal.branch == branch {
                    out.push(c);
                }
            }
            out
        }
    };
    let mut out = Vec::new();
    for d in dirs {
        let loaded = change::load(&d.path, &living)?;
        out.push(report(d, loaded, &top));
    }
    Ok(out)
}

/// Moves a backlog plan to the current branch by rewriting its `branch:` line,
/// so it can be accepted here. Returns the branch it came from.
pub fn claim(project: &Project, cwd: &Path, input: &str) -> Result<String> {
    let c = change::resolve(project, input)?;
    if c.stage != Stage::Backlog {
        return Err(crate::error::Error::new(
            "stage",
            format!(
                "{} is {}; only a backlog plan moves between branches",
                c.id,
                c.stage.name()
            ),
            "an accepted change keeps its branch; amend it instead",
        ));
    }
    let branch = git::branch(&git::toplevel(cwd)?)?.ok_or_else(|| {
        crate::error::Error::new(
            "detached",
            "HEAD is detached",
            "check out the branch to claim the plan for",
        )
    })?;
    let _lock = project.lock()?;
    let path = c.path.join(change::PROPOSAL);
    let text = std::fs::read_to_string(&path).map_err(|e| crate::error::Error::io(&path, e))?;
    let mut old = String::new();
    let mut out = String::with_capacity(text.len());
    for line in text.split_inclusive('\n') {
        match line.strip_prefix("branch:") {
            Some(rest) if old.is_empty() => {
                old = rest.trim().to_string();
                out.push_str(&format!("branch: {branch}\n"));
            }
            _ => out.push_str(line),
        }
    }
    if old.is_empty() {
        out = format!("branch: {branch}\n{text}");
    }
    std::fs::write(&path, out).map_err(|e| crate::error::Error::io(&path, e))?;
    Ok(old)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::git::testrepo::*;
    use crate::store::{Mode, init};

    #[test]
    fn claim_moves_a_backlog_plan_to_this_branch() {
        let tmp = TempDir::new();
        let repo = repo_with_commit(&tmp.0, "r");
        let p = init(&repo, &tmp.0.join("home"), Mode::Home).unwrap();
        let c = change::new(&p, "T", "elsewhere", true).unwrap();
        assert_eq!(claim(&p, &repo, &c.id.to_string()).unwrap(), "elsewhere");
        let text = std::fs::read_to_string(c.path.join(change::PROPOSAL)).unwrap();
        assert!(text.starts_with("branch: main\n"), "{text}");
        assert_eq!(check(&p, &repo, None).unwrap().len(), 1);
    }

    #[test]
    fn programs_skip_assignments_and_env() {
        assert_eq!(program("FOO=1 BAR=2 cargo test x"), Some("cargo"));
        assert_eq!(program("env RUST_LOG=x cargo test"), Some("cargo"));
        assert_eq!(program("./scripts/check.sh"), Some("./scripts/check.sh"));
        assert_eq!(program(""), None);
    }

    #[test]
    fn check_reports_missing_programs_and_questions() {
        let tmp = TempDir::new();
        let repo = repo_with_commit(&tmp.0, "r");
        std::fs::create_dir_all(repo.join("scripts")).unwrap();
        std::fs::write(repo.join("scripts/ok.sh"), "true").unwrap();
        let p = init(&repo, &tmp.0.join("home"), Mode::Home).unwrap();
        let c = change::new(&p, "T", "main", true).unwrap();
        std::fs::write(
            c.path.join("tasks.md"),
            "- TSK-new1 A\n  verify: sh -c true\n  files: a\n- TSK-new2 B\n  verify: no-such-tool-xyz run\n  files: a\n- TSK-new3 C\n  verify: ./scripts/ok.sh\n  files: a\n- TSK-new4 D\n  verify: ./scripts/gone.sh\n  files: a\n",
        )
        .unwrap();
        let prop = std::fs::read_to_string(c.path.join("proposal.md")).unwrap();
        std::fs::write(
            c.path.join("proposal.md"),
            prop + "[NEEDS CLARIFICATION: Which? | recommended: this]\n",
        )
        .unwrap();
        let reports = check(&p, &repo, None).unwrap();
        assert_eq!(reports.len(), 1);
        let r = &reports[0];
        assert_eq!(
            r.missing,
            [
                ("TSK-new2".to_string(), "no-such-tool-xyz".to_string()),
                ("TSK-new4".to_string(), "./scripts/gone.sh".to_string())
            ]
        );
        assert_eq!(r.questions.len(), 1);
        assert!(r.blocking());

        git(&repo, &["checkout", "-q", "-b", "other"]);
        assert!(check(&p, &repo, None).unwrap().is_empty());
        assert_eq!(check(&p, &repo, Some(&c.id.to_string())).unwrap().len(), 1);
    }
}
