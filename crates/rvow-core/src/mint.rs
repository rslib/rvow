//! `rvow ids`: mints ids for the placeholders of an active change. It lives
//! here and not with the vow because it reads the active changes, which the
//! leases of the project decide.

use std::path::Path;

use crate::change::{self, Stage};
use crate::error::{Error, Result};
use crate::git;
use crate::id::Id;
use crate::proof;
use crate::store::Project;
use crate::vow::mint_placeholders;

/// `rvow ids`: mints ids for the placeholders of `target`, or of every active
/// change on the current branch, and returns the mapping in mint order.
pub fn mint_active(project: &Project, cwd: &Path, target: Option<&str>) -> Result<Vec<(String, Id)>> {
    let lock = project.lock()?;
    let (living, _) = change::load_living(project)?;
    let dirs = match target {
        Some(t) => {
            let c = change::resolve(project, t)?;
            if c.stage != Stage::Active {
                let fix = if c.stage == Stage::Backlog {
                    "placeholders stay valid in backlog; accept mints them"
                } else {
                    "only an active change has placeholders to mint"
                };
                return Err(Error::new("stage", format!("{} is in {}, not in active", c.id, c.stage.name()), fix));
            }
            vec![c]
        }
        None => {
            let branch = git::branch(cwd)?;
            proof::active(project, &living)?.into_iter().filter(|a| a.branch() == branch.as_deref()).map(|a| a.dir).collect()
        }
    };
    let mut out = Vec::new();
    for dir in dirs {
        let loaded = change::load(&dir.path, &living)?;
        out.extend(mint_placeholders(project, &lock, &dir, &loaded)?);
    }
    Ok(out)
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::config;
    use crate::git::testrepo::*;
    use crate::store::Mode;
    use crate::testutil::accepted;
    use crate::vow::{commit, prepare};

    /// A repository at `main` with one change in backlog whose task is what the
    /// caller writes, and a protected `tests/t.sh`.
    pub(crate) fn setup(tasks: &str) -> (TempDir, Project, std::path::PathBuf, change::ChangeDir) {
        let tmp = TempDir::new();
        let home = tmp.0.join("home");
        let repo = repo_with_commit(&tmp.0, "r");
        std::fs::create_dir_all(repo.join("tests")).unwrap();
        std::fs::write(repo.join("tests/t.sh"), "exit 0").unwrap();
        let p = crate::store::init(&repo, &home, Mode::Home).unwrap();
        let c = change::new(&p, "Output tail", "main", false).unwrap();
        std::fs::write(c.path.join("tasks.md"), tasks).unwrap();
        std::fs::write(
            c.path.join("specs/ledger.md"),
            "## ADD REQ-new1 Tail\nrvow SHALL keep the tail.\n### Scenario: s\n- WHEN a run ends\n- THEN the tail is kept\n",
        )
        .unwrap();
        (tmp, p, repo, c)
    }

    const TAIL: &str = "- TSK-new1 Tail\n  covers: REQ-new1\n  verify: echo 'test result: ok. 0 passed; 0 failed;'\n  files: src/**\n";

    #[test]
    fn a_deleted_protected_file_is_removed_drift() {
        let (tmp, p, repo, c) = setup(TAIL);
        git(&repo, &["add", "tests/t.sh"]);
        git(&repo, &["commit", "-q", "-m", "test"]);
        let home = tmp.0.join("home");
        let prepared = prepare(&p, &home, c, &repo, &mut std::io::sink()).unwrap();
        commit(&p, prepared).unwrap();
        std::fs::remove_file(repo.join("tests/t.sh")).unwrap();
        let (living, _) = change::load_living(&p).unwrap();
        let active = crate::proof::active(&p, &living).unwrap().remove(0);
        let cfg = config::load(&p, &home).unwrap();
        let items = crate::drift::drift(&p, &repo, &cfg, &active).unwrap();
        assert!(items.iter().any(|i| i.class == crate::drift::Class::Weaken && i.what == "protected file tests/t.sh removed"), "{items:?}");
        crate::proof::status(&p, &home, &repo, false).expect("status reads a change whose protected file is gone");
    }

    #[test]
    fn ids_mints_for_active_changes_on_the_branch() {
        let (_tmp, p, repo, _) = accepted();
        let dir = std::fs::read_dir(p.root.join("active")).unwrap().next().unwrap().unwrap().path();
        let tasks = dir.join("tasks.md");
        let mut text = std::fs::read_to_string(&tasks).unwrap();
        text.push_str("- TSK-new1 More\n  verify: true\n  files: a\n");
        std::fs::write(&tasks, text).unwrap();

        let out = mint_active(&p, &repo, None).unwrap();
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].0, "TSK-new1");
        let text = std::fs::read_to_string(&tasks).unwrap();
        assert!(text.contains(&out[0].1.to_string()) && !text.contains("TSK-new1"));
        assert!(mint_active(&p, &repo, None).unwrap().is_empty());

        git(&repo, &["checkout", "-q", "-b", "other"]);
        let backlog = change::new(&p, "Later", "other", true).unwrap();
        let err = mint_active(&p, &repo, Some(&backlog.id.to_string())).unwrap_err();
        assert_eq!(err.code, "stage");
        assert!(err.fix.contains("accept mints them"));
    }
}
