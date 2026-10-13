//! Reads the JUnit XML report a runner writes to `{junit}`: accept records
//! the test cases that failed, and prove needs those names to pass.

use std::path::Path;

use crate::error::{Error, Result};

/// One `<testcase>` of the report.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Case {
    /// The `name` attribute, or `classname.name` when both are there.
    pub name: String,
    /// The case holds a `<failure>` or an `<error>` child.
    pub failed: bool,
}

/// The test cases of the report at `path`, in document order. A missing or
/// unreadable report is an error that says so.
pub fn read(path: &Path) -> Result<Vec<Case>> {
    let text = std::fs::read_to_string(path).map_err(|e| {
        Error::new(
            "junit",
            format!("the JUnit report {} cannot be read: {e}", path.display()),
            "check that the runner's run writes its report to the {junit} path",
        )
    })?;
    parse(&text)
}

/// The cases of a report held as text.
pub fn parse(text: &str) -> Result<Vec<Case>> {
    let doc = roxmltree::Document::parse(text)
        .map_err(|e| Error::new("junit", format!("the JUnit report does not parse: {e}"), "write a well-formed JUnit XML report"))?;
    Ok(doc
        .descendants()
        .filter(|n| n.tag_name().name() == "testcase")
        .map(|n| {
            let name = match (n.attribute("classname"), n.attribute("name")) {
                (Some(class), Some(name)) => format!("{class}.{name}"),
                (None, Some(name)) => name.to_string(),
                (Some(class), None) => class.to_string(),
                (None, None) => String::new(),
            };
            let failed = n.children().any(|c| matches!(c.tag_name().name(), "failure" | "error"));
            Case { name, failed }
        })
        .collect())
}

/// Whether a report proves the named tests: each recorded name is there and
/// passed, and no test case failed. A failure names the first missing or
/// failed test; a report that cannot be read names the report.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Check {
    Passed,
    /// The first problem: a missing or failed test name, or the report.
    Failed(String),
}

/// Checks the report at `path` for a `baseline: pass` task: the report must
/// exist, hold at least one passed test case, and no failed one. Nothing is
/// recorded for such a task.
pub fn check_baseline_pass(path: &Path) -> Check {
    let cases = match read(path) {
        Ok(cases) => cases,
        Err(e) => return Check::Failed(e.message),
    };
    if let Some(c) = cases.iter().find(|c| c.failed) {
        return Check::Failed(format!("the test {} failed", c.name));
    }
    if cases.is_empty() {
        return Check::Failed("the JUnit report names no test cases".into());
    }
    Check::Passed
}

/// Checks the report at `path` against the failed names the vow recorded.
/// A missing or unreadable report fails, with a message that says so.
pub fn check(path: &Path, recorded: &[String]) -> Check {
    let cases = match read(path) {
        Ok(cases) => cases,
        Err(e) => return Check::Failed(e.message),
    };
    for name in recorded {
        match cases.iter().find(|c| &c.name == name) {
            None => return Check::Failed(format!("the recorded test {name} is missing from the report")),
            Some(c) if c.failed => return Check::Failed(format!("the recorded test {name} failed")),
            Some(_) => {}
        }
    }
    if let Some(c) = cases.iter().find(|c| c.failed) {
        return Check::Failed(format!("the test {} failed", c.name));
    }
    Check::Passed
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_names_classnames_and_failures() {
        let cases = parse(
            "<?xml version=\"1.0\"?><testsuite><testcase classname=\"pool\" name=\"Seek\"><failure>boom</failure></testcase>\
             <testcase classname=\"pool\" name=\"Open\"></testcase><testcase name=\"Bare\"><error/></testcase></testsuite>",
        )
        .unwrap();
        assert_eq!(
            cases,
            [
                Case { name: "pool.Seek".into(), failed: true },
                Case { name: "pool.Open".into(), failed: false },
                Case { name: "Bare".into(), failed: true }
            ]
        );
        assert!(parse("<oops").is_err());
    }

    #[test]
    fn check_passes_only_when_each_recorded_name_passed() {
        let dir = std::env::temp_dir();
        let report = dir.join("rvow-junit-unit.xml");
        let passing = "<testsuite><testcase classname=\"c\" name=\"Seek\"/></testsuite>";
        std::fs::write(&report, passing).unwrap();
        assert_eq!(check(&report, &["c.Seek".to_string()]), Check::Passed);
        assert_eq!(
            check(&report, &["c.Seek".to_string(), "c.Gone".to_string()]),
            Check::Failed("the recorded test c.Gone is missing from the report".into())
        );
        let failing = "<testsuite><testcase classname=\"c\" name=\"Seek\"><failure/></testcase></testsuite>";
        std::fs::write(&report, failing).unwrap();
        assert_eq!(check(&report, &["c.Seek".to_string()]), Check::Failed("the recorded test c.Seek failed".into()));
        assert_eq!(
            check(&report, &[]),
            Check::Failed("the test c.Seek failed".into()),
            "a failing case the vow did not record still fails the proof"
        );
        let missing = dir.join("rvow-junit-unit-gone.xml");
        let _ = std::fs::remove_file(&missing);
        assert!(matches!(check(&missing, &[]), Check::Failed(m) if m.contains("cannot be read")));
    }
}
