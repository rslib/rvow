//! A repository finds its known project through a local index instead of
//! walking the rvow home (REQ-0rx146).

use std::path::{Path, PathBuf};
use std::process::Command;

use rvow_core::store::{self, INDEX_FILE, Mode};
use serde_json::{Value, json};

struct TempDir(PathBuf);

impl TempDir {
    fn new(name: &str) -> TempDir {
        let dir = std::env::temp_dir().join(format!("rvow-project-index-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        TempDir(dir.canonicalize().unwrap())
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn git(dir: &Path, args: &[&str]) {
    let out = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(args)
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_AUTHOR_NAME", "t")
        .env("GIT_AUTHOR_EMAIL", "t@t")
        .env("GIT_COMMITTER_NAME", "t")
        .env("GIT_COMMITTER_EMAIL", "t@t")
        .output()
        .unwrap();
    assert!(out.status.success(), "git {args:?}: {out:?}");
}

/// A repository whose one commit differs from every other fixture's.
fn repo_with_commit(parent: &Path, name: &str) -> PathBuf {
    let dir = parent.join(name);
    std::fs::create_dir_all(&dir).unwrap();
    git(&dir, &["init", "-q", "-b", "main"]);
    std::fs::write(dir.join("a.txt"), name).unwrap();
    git(&dir, &["add", "a.txt"]);
    git(&dir, &["commit", "-q", "-m", name]);
    dir
}

fn index(home: &Path) -> serde_json::Map<String, Value> {
    let text = std::fs::read_to_string(home.join(INDEX_FILE)).unwrap();
    serde_json::from_str::<Value>(&text).unwrap().as_object().unwrap().clone()
}

/// A second project in `home` that names the same root commits as `repo`, so a
/// walk that reaches it finds two matches and refuses.
fn decoy(home: &Path, project: &Path) {
    let dest = home.join("decoy/copy");
    std::fs::create_dir_all(&dest).unwrap();
    std::fs::copy(project.join(store::PROJECT_FILE), dest.join(store::PROJECT_FILE)).unwrap();
}

fn fixture(name: &str) -> (TempDir, PathBuf, PathBuf, store::Project) {
    let tmp = TempDir::new(name);
    let home = tmp.0.join("home");
    let repo = repo_with_commit(&tmp.0, name);
    let project = store::init(&repo, &home, Mode::Home).unwrap();
    (tmp, home, repo, project)
}

#[test]
fn project_index_second_lookup_walks_no_folder() {
    let (_tmp, home, repo, project) = fixture("alpha");
    assert_eq!(store::find(&repo, &home).unwrap(), Some(project.clone()));
    decoy(&home, &project.root);
    assert_eq!(store::find(&repo, &home).unwrap(), Some(project), "a walk would reach the decoy and report two projects");
}

#[test]
fn project_index_moved_project_is_walked_for_and_updated() {
    let (_tmp, home, repo, project) = fixture("beta");
    store::find(&repo, &home).unwrap();
    let moved = home.join("elsewhere/beta");
    std::fs::create_dir_all(moved.parent().unwrap()).unwrap();
    std::fs::rename(&project.root, &moved).unwrap();

    let found = store::find(&repo, &home).unwrap().unwrap();
    assert_eq!(found.root, moved);
    let entries = index(&home);
    assert_eq!(entries.len(), 1);
    let entry = entries.values().next().unwrap();
    assert_eq!(entry["project"], json!(moved.to_str().unwrap()));
}

#[test]
fn project_index_entry_for_another_repository_is_not_trusted() {
    let (tmp, home, repo, project) = fixture("gamma");
    let other_repo = repo_with_commit(&tmp.0, "delta");
    let other = store::init(&other_repo, &home, Mode::Home).unwrap();
    store::find(&repo, &home).unwrap();

    let mut entries = index(&home);
    let key = entries.keys().next().unwrap().clone();
    entries[&key]["project"] = json!(other.root.to_str().unwrap());
    std::fs::write(home.join(INDEX_FILE), Value::Object(entries).to_string()).unwrap();

    assert_eq!(store::find(&repo, &home).unwrap(), Some(project.clone()));
    assert_eq!(index(&home)[&key]["project"], json!(project.root.to_str().unwrap()));
}

#[test]
fn project_index_missing_file_walks_once_and_writes_one_entry() {
    let (_tmp, home, repo, project) = fixture("epsilon");
    assert!(!home.join(INDEX_FILE).exists());
    assert_eq!(store::find(&repo, &home).unwrap(), Some(project));
    assert_eq!(index(&home).len(), 1);
}

#[test]
fn project_index_corrupt_file_counts_as_empty() {
    let (_tmp, home, repo, project) = fixture("zeta");
    std::fs::write(home.join(INDEX_FILE), "{ not json").unwrap();
    assert_eq!(store::find(&repo, &home).unwrap(), Some(project));
    assert_eq!(index(&home).len(), 1);
}

#[test]
fn project_index_a_new_repository_at_the_same_path_is_not_trusted() {
    // The repository at a path is replaced by another one: the index entry
    // still names the old project, whose root commit the new repository lacks.
    let (tmp, home, repo, project) = fixture("replaced");
    assert_eq!(store::find(&repo, &home).unwrap(), Some(project.clone()));
    std::fs::remove_dir_all(&repo).unwrap();
    let repo = repo_with_commit(&tmp.0, "replaced-other");
    std::fs::rename(&repo, tmp.0.join("replaced")).unwrap();
    let repo = tmp.0.join("replaced");
    assert_eq!(store::find(&repo, &home).unwrap(), None, "the old project must not answer for a new repository");
}

#[test]
fn project_index_write_adds_the_ignore_line_to_an_older_home() {
    // A home made before the index existed lacks its ignore line.
    let (_tmp, home, repo, _project) = fixture("older");
    let ignore = home.join(".gitignore");
    let text = std::fs::read_to_string(&ignore).unwrap().replace(&format!("{INDEX_FILE}\n"), "");
    std::fs::write(&ignore, &text).unwrap();
    store::find(&repo, &home).unwrap();
    let text = std::fs::read_to_string(&ignore).unwrap();
    assert_eq!(text.lines().filter(|l| *l == INDEX_FILE).count(), 1, "{text}");
}

#[test]
fn project_index_entry_outside_the_home_is_not_trusted() {
    // An entry that points at a project folder outside the rvow home, such as
    // one an agent made, is ignored and the walk finds the real project.
    let (tmp, home, repo, project) = fixture("outside");
    assert_eq!(store::find(&repo, &home).unwrap(), Some(project.clone()));
    let fake = tmp.0.join("fake");
    std::fs::create_dir_all(&fake).unwrap();
    std::fs::copy(project.root.join(store::PROJECT_FILE), fake.join(store::PROJECT_FILE)).unwrap();
    let mut entries = index(&home);
    let key = entries.keys().next().unwrap().clone();
    entries[&key]["project"] = json!(fake);
    std::fs::write(home.join(INDEX_FILE), Value::Object(entries).to_string()).unwrap();
    assert_eq!(store::find(&repo, &home).unwrap(), Some(project));
}
