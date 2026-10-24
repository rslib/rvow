//! The run folder of the engine under the rvow home holds its socket, its lock
//! and the list of the harness processes. It belongs to rvow, so a tool of the
//! session neither reads nor writes it, and nothing talks to the socket but
//! rvow itself.

use std::path::Path;

use super::rule::denial;
use super::shell::run_reason;
use super::{Denial, Rule, at_or_under, key, real_path};

/// Why a tool call must not read or write the file at `file`: it is the run
/// folder of the engine, at `run`.
pub(crate) fn run_denial(run: &Path, cwd: &Path, file: &str) -> Option<Denial> {
    let target = real_path(Path::new(file), cwd);
    if !at_or_under(&key(&target), &key(&real_path(run, cwd))) {
        return None;
    }
    denial(Rule::Store, run_reason(&target))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    /// The store fixture of the store guard's tests, with the engine's run
    /// folder in it. Its path is resolved, so a comparison of keys holds.
    fn fixture() -> (crate::git::testrepo::TempDir, PathBuf, PathBuf) {
        let tmp = crate::git::testrepo::TempDir::new();
        let home = tmp.0.join("home");
        let repo = crate::git::testrepo::repo_with_commit(&tmp.0, "r");
        crate::store::init(&repo, &home, crate::store::Mode::Home).unwrap();
        let run = home.join(super::super::shell::RUN_DIR);
        std::fs::create_dir_all(&run).unwrap();
        std::fs::write(run.join("engine.sock"), "").unwrap();
        std::fs::write(run.join("harnesses"), "").unwrap();
        (tmp, home, repo)
    }

    #[test]
    fn shell_engine_socket_a_run_file_is_denied_and_a_neighbour_is_not() {
        let (_tmp, home, repo) = fixture();
        let run = home.join("run");
        let harnesses = run.join("harnesses");
        let d = run_denial(&run, &repo, harnesses.to_str().unwrap()).expect("an edit of the harness list");
        assert_eq!(d.rule, Rule::Store);
        assert!(d.reason.contains("belongs to rvow") && d.reason.contains("engine socket"), "{}", d.reason);
        for file in ["README.md", "runner/x.txt", "specs/x.md"] {
            assert_eq!(run_denial(&run, &repo, &home.join(file).display().to_string()), None, "{file}");
        }
    }

    /// The edit and the read go through the hook, with the home this process
    /// resolves, as the harness's call does. Nothing is written under it.
    #[test]
    fn shell_engine_socket_an_edit_and_a_read_through_the_hook_are_denied() {
        let Ok(home) = crate::store::home_dir() else { return };
        let tmp = crate::git::testrepo::TempDir::new();
        let repo = crate::git::testrepo::repo_with_commit(&tmp.0, "r");
        let project = crate::store::init(&repo, &tmp.0.join("home"), crate::store::Mode::Home).unwrap();
        let file = home.join("run/harnesses");
        for tool in ["Edit", "Write", "Read"] {
            let input = serde_json::json!({
                "cwd": repo.to_str().unwrap(),
                "tool_name": tool,
                "tool_input": {"file_path": file},
            });
            let d = crate::hook::pre_tool_use_denial(&project, &input).unwrap().unwrap_or_else(|| panic!("{tool}"));
            assert_eq!(d.rule, Rule::Store);
            assert!(d.reason.contains("engine socket"), "{tool}: {}", d.reason);
        }
    }

    /// The same, for a Bash command that names the socket.
    #[test]
    fn shell_engine_socket_a_bash_call_through_the_hook_is_denied() {
        let Ok(home) = crate::store::home_dir() else { return };
        let tmp = crate::git::testrepo::TempDir::new();
        let repo = crate::git::testrepo::repo_with_commit(&tmp.0, "r");
        let project = crate::store::init(&repo, &tmp.0.join("home"), crate::store::Mode::Home).unwrap();
        let input = serde_json::json!({
            "cwd": repo.to_str().unwrap(),
            "tool_name": "Bash",
            "tool_input": {"command": format!("nc -U {}/run/engine.sock", home.display())},
        });
        let d = crate::hook::pre_tool_use_denial(&project, &input).unwrap().expect("a call of the engine socket");
        assert_eq!(d.rule, Rule::Store);
        assert!(d.reason.contains("engine socket"), "{}", d.reason);
    }
}
