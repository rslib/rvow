//! Where a project's rvow tree lives and how a repository finds it.

pub use rvow_store::store::*;

use std::collections::{BTreeMap, HashMap};
#[cfg(unix)]
use std::fs::File;
use std::path::{Path, PathBuf};

use rvow_base::error::{Error, Result};
use rvow_base::layout::{KEY_FILE, KEY_FOLDER, MARKETPLACE_DIR};
use rvow_git::git;

/// Refuses a repository whose identity git cannot report, and returns its roots.
fn identity(top: &Path) -> Result<Vec<String>> {
    if git::is_shallow(top)? {
        return Err(Error::new(
            "shallow",
            "a shallow clone hides the root commit that identifies the project",
            "run git fetch --unshallow",
        ));
    }
    let roots = git::root_commits(top)?;
    if roots.is_empty() {
        return Err(Error::new("no-commit", "the repository has no commit to identify it by", "make a first commit"));
    }
    Ok(roots)
}

/// Creates the rvow tree for the repository that contains `dir`.
pub fn init(dir: &Path, home: &Path, mode: Mode) -> Result<Project> {
    let top = git::toplevel(dir)?;
    let roots = identity(&top)?;
    let remote = git::origin_url(&top).and_then(|u| git::remote_path(&u));
    let root = match mode {
        Mode::Repo => top.join(REPO_DIR),
        Mode::Home => {
            let name = top.file_name().and_then(|n| n.to_str()).unwrap_or("repo").to_lowercase();
            home.join(remote.clone().unwrap_or_else(|| format!("local/{name}")))
        }
    };
    if let Some(found) = find(&top, home)? {
        return Err(Error::new(
            "exists",
            format!("this repository already has an rvow tree at {}", found.root.display()),
            "use the existing tree",
        ));
    }
    if root.join(PROJECT_FILE).exists() {
        return Err(Error::new(
            "name-taken",
            format!("{} belongs to another repository with the same name", root.display()),
            "rename one of the folders, or give this one a remote",
        ));
    }
    for d in DIRS {
        let p = root.join(d);
        std::fs::create_dir_all(&p).map_err(|e| Error::io(&p, e))?;
    }
    let meta = Meta { roots, remote, mode };
    let text = toml::to_string(&meta).map_err(|e| Error::internal(format!("project metadata does not serialize: {e}")))?;
    for (name, body) in [(PROJECT_FILE, text.as_str()), (CONFIG_FILE, ""), (IDS_FILE, "")] {
        let p = root.join(name);
        std::fs::write(&p, body).map_err(|e| Error::io(&p, e))?;
    }
    let project = Project { root, meta };
    match project.meta.mode {
        Mode::Repo => {
            add_union_merge(&top)?;
            let lines: Vec<String> = local_files().iter().map(|f| format!("{REPO_DIR}/{f}")).collect();
            add_lines(&top.join(".gitignore"), &lines.iter().map(String::as_str).collect::<Vec<_>>())?;
        }
        Mode::Home => {
            let key = format!("{KEY_FOLDER}/{KEY_FILE}");
            let mut lines = local_files().to_vec();
            lines.extend([
                format!("{MARKETPLACE_DIR}/"),
                key,
                INDEX_FILE.to_string(),
                GATE_REPOS_FILE.to_string(),
                format!("{WORKTREES_DIR}/"),
            ]);
            add_lines(&home.join(".gitignore"), &lines.iter().map(String::as_str).collect::<Vec<_>>())?;
        }
    }
    Ok(project)
}

/// What `rebind` did to the tree.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Rebind {
    Unchanged(PathBuf),
    Updated(PathBuf),
    Moved { from: PathBuf, to: PathBuf },
}

/// Makes the tree of the repository that contains `dir` match the repository again: it records the
/// current root commits and origin remote in `project.toml`, and moves a home tree to the path its
/// remote implies. Running it twice changes nothing.
pub fn rebind(dir: &Path, home: &Path) -> Result<Rebind> {
    rebind_impl(dir, home, None)
}

/// `rebind` with a filter-repo commit map, so a rewritten history whose root
/// commits no longer match is found again through the map.
pub fn rebind_with_map(dir: &Path, home: &Path, map_path: &Path) -> Result<Rebind> {
    let map = read_commit_map(map_path)?;
    rebind_impl(dir, home, Some(&map))
}

/// Reads a `git filter-repo` commit map: `<old> <new>` per line.
fn read_commit_map(path: &Path) -> Result<BTreeMap<String, String>> {
    let text = std::fs::read_to_string(path).map_err(|e| Error::io(path, e))?;
    let mut map = BTreeMap::new();
    for line in text.lines() {
        let mut parts = line.split_whitespace();
        if let (Some(old), Some(new)) = (parts.next(), parts.next()) {
            map.insert(old.to_string(), new.to_string());
        }
    }
    Ok(map)
}

/// Maps each root through the commit map, keeping an unmapped root as it is.
fn map_roots(roots: &[String], map: &BTreeMap<String, String>) -> Vec<String> {
    roots.iter().map(|r| map.get(r).cloned().unwrap_or_else(|| r.clone())).collect()
}

/// A project whose recorded roots map through `map` to `roots`, found the
/// same way `find` matches: one candidate, or one by remote, else ambiguous.
fn find_mapped(dir: &Path, home: &Path, roots: &[String], map: &BTreeMap<String, String>) -> Result<Option<Project>> {
    let remote = git::origin_url(dir).and_then(|u| git::remote_path(&u));
    let mut hits = Vec::new();
    walk(home, 0, &mut |root| {
        if let Ok(meta) = read_meta(root) {
            let mapped = map_roots(&meta.roots, map);
            if mapped.iter().any(|r| roots.contains(r)) {
                hits.push(Project { root: root.to_path_buf(), meta });
            }
        }
    });
    if hits.len() > 1 {
        hits.retain(|p| p.meta.remote.is_some() && p.meta.remote == remote);
    }
    match hits.len() {
        0 => Ok(None),
        1 => Ok(hits.pop()),
        _ => Err(Error::new(
            "ambiguous-project",
            format!(
                "several rvow projects match this repository through the map: {}",
                hits.iter().map(|p| p.root.display().to_string()).collect::<Vec<_>>().join(", ")
            ),
            "give each clone a distinct origin remote",
        )),
    }
}

fn rebind_impl(dir: &Path, home: &Path, map: Option<&BTreeMap<String, String>>) -> Result<Rebind> {
    let top = git::toplevel(dir)?;
    let roots = identity(&top)?;
    let remote = git::origin_url(&top).and_then(|u| git::remote_path(&u));
    let repo_root = top.join(REPO_DIR);
    if repo_root.join(CONFIG_FILE).exists() {
        let changed = write_meta(&repo_root, &Meta { roots, remote, mode: Mode::Repo })?;
        return Ok(if changed { Rebind::Updated(repo_root) } else { Rebind::Unchanged(repo_root) });
    }
    let found = match find(&top, home)? {
        Some(f) => Some(f),
        None => match map {
            Some(m) => find_mapped(&top, home, &roots, m)?,
            None => None,
        },
    };
    let Some(found) = found else {
        return Err(Error::new("no-project", "no rvow tree matches this repository", "run rvow init or give it a commit map"));
    };
    let _lock = found.lock()?;
    let meta = Meta { roots: roots.clone(), remote: remote.clone().or(found.meta.remote.clone()), ..found.meta.clone() };
    let want = remote.map(|r| home.join(r)).unwrap_or_else(|| found.root.clone());
    if want == found.root {
        let changed = write_meta(&found.root, &meta)?;
        return Ok(if changed { Rebind::Updated(found.root) } else { Rebind::Unchanged(found.root) });
    }
    if want.join(PROJECT_FILE).exists() {
        return Err(Error::new(
            "name-taken",
            format!("{} already holds another rvow tree", want.display()),
            format!("move or remove {}, then run rvow rebind again", want.display()),
        ));
    }
    if let Some(parent) = want.parent() {
        std::fs::create_dir_all(parent).map_err(|e| Error::io(parent, e))?;
    }
    std::fs::rename(&found.root, &want).map_err(|e| Error::io(&found.root, e))?;
    write_meta(&want, &meta)?;
    Ok(Rebind::Moved { from: found.root, to: want })
}

/// The git dirs that this process already learned for a folder: the work
/// tree's git dir and its repository's common dir. A hook lives for one tool
/// call and moves no worktree, so the answer stays true for it, and the
/// snapshot of that call starts no git process of its own.
static GIT_DIRS: std::sync::OnceLock<std::sync::Mutex<BTreeMap<PathBuf, (PathBuf, PathBuf)>>> = std::sync::OnceLock::new();

/// The most folders whose git dirs this process keeps: a long-lived process
/// with many folders would else grow the map without end.
const DIR_LIMIT: usize = 256;

/// The absolute git dir and common dir of the work tree at `dir`, when a
/// `find` of this process already printed them.
pub fn known_dirs(dir: &Path) -> Option<(PathBuf, PathBuf)> {
    GIT_DIRS.get()?.lock().ok()?.get(dir).cloned()
}

/// Keeps the two dirs a `rev-parse` already printed for `dir`, under every
/// name this process may ask with: the folder the call came from and the work
/// tree root that git named for it.
fn remember_dirs(dir: &Path, top: &Path, git_dir: &str, common: &str) {
    let Ok(mut dirs) = GIT_DIRS.get_or_init(Default::default).lock() else { return };
    // Two keys per call, so a pair always lands in a map that has the room.
    if dirs.len() + 2 > DIR_LIMIT {
        dirs.clear();
    }
    let entry = (PathBuf::from(git_dir), PathBuf::from(common));
    dirs.insert(dir.to_path_buf(), entry.clone());
    dirs.insert(top.to_path_buf(), entry);
}

/// Finds the rvow tree of the repository that contains `dir`: repo mode when
/// `rvow/config.toml` exists, else the home project whose roots match.
pub fn find(dir: &Path, home: &Path) -> Result<Option<Project>> {
    // One git call answers for the work tree, its git dir and its repository,
    // since every hook runs this lookup and then snapshots the work tree.
    let out = git::run(dir, &["rev-parse", "--path-format=absolute", "--show-toplevel", "--absolute-git-dir", "--git-common-dir"])?;
    let mut lines = out.lines();
    let top = PathBuf::from(lines.next().unwrap_or_default());
    let git_dir = lines.next().unwrap_or_default();
    let common = lines.next().filter(|c| !c.is_empty());
    git::known_toplevel(dir, &top);
    if !git_dir.is_empty() {
        remember_dirs(dir, &top, git_dir, common.unwrap_or(git_dir));
        if let Ok(head) = std::fs::read_to_string(Path::new(git_dir).join("HEAD")) {
            git::known_branch(&top, &head);
        }
    }
    let common = common.map(String::from);
    let repo_root = top.join(REPO_DIR);
    if repo_root.join(CONFIG_FILE).exists() {
        let meta = read_meta(&repo_root)?;
        return Ok(Some(Project { root: repo_root, meta }));
    }
    if let Some(project) = common.as_deref().and_then(|c| index_hit(&top, home, c)) {
        return Ok(Some(project));
    }
    let roots = identity(&top)?;
    let remote = git::origin_url(&top).and_then(|u| git::remote_path(&u));
    let mut hits = Vec::new();
    walk(home, 0, &mut |root| {
        if let Ok(meta) = read_meta(root)
            && meta.roots.iter().any(|r| roots.contains(r))
        {
            hits.push(Project { root: root.to_path_buf(), meta });
        }
    });
    if hits.len() > 1 {
        hits.retain(|p| p.meta.remote.is_some() && p.meta.remote == remote);
    }
    match hits.len() {
        0 => Ok(None),
        1 => {
            let project = hits.pop();
            if let (Some(c), Some(p)) = (common.as_deref(), project.as_ref()) {
                index_store(home, c, p, &roots);
            }
            Ok(project)
        }
        _ => Err(Error::new(
            "ambiguous-project",
            format!(
                "several rvow projects match this repository: {}",
                hits.iter().map(|p| p.root.display().to_string()).collect::<Vec<_>>().join(", ")
            ),
            "give each clone a distinct origin remote",
        )),
    }
}

/// The project the index maps `common` to, when its folder is inside `home`
/// and still exists, its recorded roots still share a root commit with the
/// entry, and the repository at `top` still holds that commit, so a repository
/// cloned again at the same path never inherits the old project.
fn index_hit(top: &Path, home: &Path, common: &str) -> Option<Project> {
    let entry = read_index(home).remove(common)?;
    if !entry.project.starts_with(home) {
        return None;
    }
    let meta = read_meta(&entry.project).ok()?;
    let shared = meta.roots.iter().find(|r| entry.roots.contains(r))?;
    // Most hooks find the commit on disk; only a miss costs a git process.
    if !object_here(top, common, shared) {
        return None;
    }
    Some(Project { root: entry.project, meta })
}

/// The object ids this process has seen in the store of a repository, by the
/// common dir they were seen in, so a hit needs no read of the pack folder.
static OBJECTS: std::sync::OnceLock<std::sync::Mutex<HashMap<String, String>>> = std::sync::OnceLock::new();

/// The most objects this process keeps.
const OBJECT_LIMIT: usize = 256;

/// Whether the object store of the repository whose common dir is `common`
/// holds `hex`, from the ids this process already saw there.
fn object_here(top: &Path, common: &str, hex: &str) -> bool {
    if let Some(objects) = OBJECTS.get_or_init(Default::default).lock().ok()
        && objects.get(hex).is_some_and(|seen| seen == common)
    {
        return true;
    }
    let found =
        has_object(&Path::new(common).join("objects"), hex) || git::run(top, &["cat-file", "-e", &format!("{hex}^{{commit}}")]).is_ok();
    if found {
        keep_object(hex, common);
    }
    found
}

/// Remembers an object that a repository holds, clearing the map first when it
/// is full.
fn keep_object(hex: &str, common: &str) {
    let Some(mut objects) = OBJECTS.get_or_init(Default::default).lock().ok() else { return };
    if objects.len() >= OBJECT_LIMIT {
        objects.clear();
    }
    objects.insert(hex.to_string(), common.to_string());
}

/// Whether the object `hex` is in the object store at `objects`, as a loose
/// object or in a version 2 pack index, read without a git process. `false`
/// means "not seen here", not "missing": an alternate store, a multi-pack
/// index alone or an unknown format is left to git. Unlike `cat-file -e
/// <id>^{commit}` it does not check the type: the id hashes the content that
/// the project recorded as a root commit.
fn has_object(objects: &Path, hex: &str) -> bool {
    let Some(id) = hex_bytes(hex) else { return false };
    let (dir, file) = hex.split_at(2);
    if objects.join(dir).join(file).is_file() {
        return true;
    }
    let Ok(entries) = std::fs::read_dir(objects.join("pack")) else { return false };
    entries.flatten().map(|e| e.path()).filter(|p| p.extension().is_some_and(|x| x == "idx")).any(|p| pack_index_has(&p, &id))
}

/// The bytes of a lowercase hex object id of 40 or 64 digits.
fn hex_bytes(hex: &str) -> Option<Vec<u8>> {
    if !matches!(hex.len(), 40 | 64) {
        return None;
    }
    let digit = |c: u8| match c {
        b'0'..=b'9' => Some(c - b'0'),
        b'a'..=b'f' => Some(c - b'a' + 10),
        _ => None,
    };
    hex.as_bytes().chunks(2).map(|pair| Some(digit(*pair.first()?)? << 4 | digit(*pair.get(1)?)?)).collect()
}

/// Whether the version 2 pack index at `path` lists `id`: a binary search in
/// the fan-out range of its first byte, reading one id per step.
#[cfg(unix)]
fn pack_index_has(path: &Path, id: &[u8]) -> bool {
    use std::os::unix::fs::FileExt;
    const MAGIC: [u8; 8] = [0xff, b't', b'O', b'c', 0, 0, 0, 2];
    const IDS_AT: u64 = 8 + 256 * 4;
    let Ok(file) = File::open(path) else { return false };
    let mut head = [0u8; IDS_AT as usize];
    if file.read_exact_at(&mut head, 0).is_err() || head.get(..8) != Some(&MAGIC[..]) {
        return false;
    }
    let fan = |i: usize| head.get(8 + i * 4..12 + i * 4).and_then(|b| b.try_into().ok()).map(u32::from_be_bytes);
    let Some(&first) = id.first() else { return false };
    let first = usize::from(first);
    let lo = if first == 0 { Some(0) } else { fan(first - 1) };
    let (Some(mut lo), Some(mut hi)) = (lo, fan(first)) else { return false };
    let mut found = vec![0u8; id.len()];
    while lo < hi {
        let mid = lo + (hi - lo) / 2;
        if file.read_exact_at(&mut found, IDS_AT + u64::from(mid) * id.len() as u64).is_err() {
            return false;
        }
        match found.as_slice().cmp(id) {
            std::cmp::Ordering::Equal => return true,
            std::cmp::Ordering::Less => lo = mid + 1,
            std::cmp::Ordering::Greater => hi = mid,
        }
    }
    false
}

#[cfg(not(unix))]
fn pack_index_has(_path: &Path, _id: &[u8]) -> bool {
    false
}

fn walk(dir: &Path, depth: usize, f: &mut impl FnMut(&Path)) {
    if dir.join(PROJECT_FILE).is_file() {
        f(dir);
        return;
    }
    if depth >= MAX_DEPTH {
        return;
    }
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for e in entries.flatten() {
        if e.file_type().is_ok_and(|t| t.is_dir()) {
            walk(&e.path(), depth + 1, f);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rvow_git::git::testrepo::*;

    #[test]
    fn has_object_finds_a_loose_and_a_packed_commit_and_not_a_missing_one() {
        let tmp = TempDir::new();
        let repo = repo_with_commit(&tmp.0, "r");
        let head = git::run(&repo, &["rev-parse", "HEAD"]).unwrap();
        let objects = repo.join(".git/objects");
        assert!(has_object(&objects, &head), "a loose commit is found");
        git(&repo, &["gc", "-q"]);
        assert!(!objects.join(&head[..2]).join(&head[2..]).exists(), "gc packed the commit");
        assert!(has_object(&objects, &head), "a packed commit is found");
        assert!(!has_object(&objects, &"0".repeat(40)), "a missing id is not");
        assert!(!has_object(&objects, "not-hex"), "a malformed id is not");
    }

    fn ignored(repo: &Path, rel: &str) -> bool {
        std::process::Command::new(git::bin())
            .args(["-C", repo.to_str().unwrap(), "check-ignore", "-q", "--no-index", rel])
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .status()
            .unwrap()
            .success()
    }

    #[test]
    fn scale_store_find_sees_a_tree_made_after_a_miss() {
        let tmp = TempDir::new();
        let home = tmp.0.join("home");
        let first = repo_with_commit(&tmp.0, "one");
        assert_eq!(find(&first, &home).unwrap(), None);
        let p = init(&first, &home, Mode::Home).unwrap();
        assert_eq!(find(&first, &home).unwrap(), Some(p), "the tree made after the miss is found");
    }

    #[test]
    fn scale_store_find_sees_a_repo_tree_made_after_a_miss() {
        let tmp = TempDir::new();
        let home = tmp.0.join("home");
        let known = repo_with_commit(&tmp.0, "known");
        let projects = init(&known, &home, Mode::Home).unwrap();
        assert_eq!(find(&known, &home).unwrap(), Some(projects));

        let repo = repo_with_commit(&tmp.0, "r");
        git(&repo, &["commit", "-q", "--amend", "-m", "r"]);
        assert_eq!(find(&repo, &home).unwrap(), None);
        // The tree of a repository is what moves when `rvow init --in-repo`
        // writes one, so the miss goes stale with it.
        let p = init(&repo, &home, Mode::Repo).unwrap();
        assert_eq!(find(&repo, &home).unwrap(), Some(p), "the tree written into the repository is found");
    }

    #[test]
    fn scale_store_find_sees_a_home_tree_made_under_an_existing_folder_after_a_miss() {
        let tmp = TempDir::new();
        let home = tmp.0.join("home");
        let known = repo_with_commit(&tmp.0, "known");
        let projects = init(&known, &home, Mode::Home).unwrap();
        assert_eq!(find(&known, &home).unwrap(), Some(projects));

        // `local/` holds the known project already, so the new tree under it
        // moves neither the home folder nor the project index.
        let repo = repo_with_commit(&tmp.0, "r");
        git(&repo, &["commit", "-q", "--amend", "-m", "r"]);
        assert_eq!(find(&repo, &home).unwrap(), None);
        let p = init(&repo, &home, Mode::Home).unwrap();
        assert_eq!(find(&repo, &home).unwrap(), Some(p), "a hook must never miss a tree that exists");
    }

    #[test]
    fn scale_store_maps_stay_under_their_limits() {
        for i in 0..DIR_LIMIT + 8 {
            let dir = PathBuf::from(format!("/rvow-dir-{i}"));
            remember_dirs(&dir, &dir.join("top"), "/git", "/common");
        }
        let kept = GIT_DIRS.get().unwrap().lock().unwrap().len();
        assert!(kept <= DIR_LIMIT, "{kept} git dirs kept over a limit of {DIR_LIMIT}");

        for i in 0..OBJECT_LIMIT + 8 {
            keep_object(&format!("{i:040}"), "/common");
        }
        let kept = OBJECTS.get().unwrap().lock().unwrap().len();
        assert!(kept <= OBJECT_LIMIT, "{kept} objects kept over a limit of {OBJECT_LIMIT}");
    }

    #[test]
    fn init_ignores_local_state() {
        let tmp = TempDir::new();
        let local = [
            "hook-state/files-s1",
            "provenance/k/records.jsonl",
            "hook-stats.jsonl",
            "leases.jsonl",
            "yolo.json",
            "rerun-cache.jsonl",
            ".lock",
            "hook-state/reprove.pid",
            "hook-state/reprove.log",
        ];
        let shared = ["ids.jsonl", "config.toml", "project.toml", "spec-record.json", "backlog/x/proposal.md"];

        let home = tmp.0.join("home");
        git(&tmp.0, &["init", "-q", home.to_str().unwrap()]);
        let p = init(&repo_with_commit(&tmp.0, "r"), &home, Mode::Home).unwrap();
        let rel = p.root.strip_prefix(&home).unwrap().to_str().unwrap().to_string();
        for f in local {
            assert!(ignored(&home, &format!("{rel}/{f}")), "home mode must ignore {f}");
        }
        for f in shared {
            assert!(!ignored(&home, &format!("{rel}/{f}")), "home mode must track {f}");
        }
        assert!(ignored(&home, INDEX_FILE), "the project index is local to one machine");
        let key = rvow_base::paths::key_path(&home);
        assert!(ignored(&home, key.strip_prefix(&home).unwrap().to_str().unwrap()), "the seal key is a secret");

        let repo = repo_with_commit(&tmp.0, "o");
        init(&repo, &tmp.0.join("home2"), Mode::Repo).unwrap();
        for f in local {
            assert!(ignored(&repo, &format!("{REPO_DIR}/{f}")), "repo mode must ignore {f}");
        }
        for f in shared {
            assert!(!ignored(&repo, &format!("{REPO_DIR}/{f}")), "repo mode must track {f}");
        }
    }

    #[test]
    fn init_writes_the_ignore_lines() {
        let tmp = TempDir::new();
        let repo = repo_with_commit(&tmp.0, "r");
        let home = tmp.0.join("home");
        init(&repo, &home, Mode::Home).unwrap();
        let text = std::fs::read_to_string(home.join(".gitignore")).unwrap();
        let key = format!("{KEY_FOLDER}/{KEY_FILE}");
        assert_eq!(
            text,
            format!(
                ".lock\nhook-state/\nprovenance/\nhook-stats.jsonl\nleases.jsonl\nyolo.json\nrerun-cache.jsonl\nclaude-plugin/\n{key}\nproject-index.json\ngate-repos.json\nworktrees/\n"
            )
        );

        let other = repo_with_commit(&tmp.0, "o");
        std::fs::write(other.join(".gitignore"), "target/").unwrap();
        init(&other, &tmp.0.join("home2"), Mode::Repo).unwrap();
        let text = std::fs::read_to_string(other.join(".gitignore")).unwrap();
        assert_eq!(
            text,
            "target/\nrvow/.lock\nrvow/hook-state/\nrvow/provenance/\nrvow/hook-stats.jsonl\nrvow/leases.jsonl\nrvow/yolo.json\nrvow/rerun-cache.jsonl\n"
        );
        add_lines(&other.join(".gitignore"), &["rvow/.lock", "x"]).unwrap();
        let text = std::fs::read_to_string(other.join(".gitignore")).unwrap();
        assert!(text.ends_with("rvow/rerun-cache.jsonl\nx\n"), "{text}");
        assert_eq!(text.matches("rvow/.lock").count(), 1, "a line already there is not added again");
    }

    #[test]
    fn init_and_find_in_home_mode() {
        let tmp = TempDir::new();
        let home = tmp.0.join("home");
        let repo = repo_with_commit(&tmp.0, "MyRepo");
        assert_eq!(find(&repo, &home).unwrap(), None);

        let p = init(&repo, &home, Mode::Home).unwrap();
        assert_eq!(p.root, home.join("local/myrepo"));
        for d in DIRS {
            assert!(p.root.join(d).is_dir(), "{d}");
        }
        assert_eq!(find(&repo, &home).unwrap(), Some(p.clone()));
        assert_eq!(init(&repo, &home, Mode::Home).unwrap_err().code, "exists");

        git(&repo, &["worktree", "add", "-q", "-b", "wt", "../wt"]);
        assert_eq!(find(&tmp.0.join("wt"), &home).unwrap(), Some(p));
    }

    #[test]
    fn remote_decides_the_path_and_splits_forks() {
        let tmp = TempDir::new();
        let home = tmp.0.join("home");
        let up = repo_with_commit(&tmp.0, "up");
        git(&up, &["remote", "add", "origin", "git@github.com:a/rvow.git"]);
        let p = init(&up, &home, Mode::Home).unwrap();
        assert_eq!(p.root, home.join("github.com/a/rvow"));

        let fork = tmp.0.join("fork");
        git(&tmp.0, &["clone", "-q", up.to_str().unwrap(), fork.to_str().unwrap()]);
        git(&fork, &["remote", "set-url", "origin", "git@github.com:b/rvow.git"]);
        let f = init(&fork, &home, Mode::Home);
        assert_eq!(f.unwrap_err().code, "exists");
    }

    #[test]
    fn same_name_without_remote_is_refused() {
        let tmp = TempDir::new();
        let home = tmp.0.join("home");
        let a = repo_with_commit(&tmp.0.join("a"), "utils");
        let b = tmp.0.join("b/utils");
        std::fs::create_dir_all(&b).unwrap();
        git(&b, &["init", "-q"]);
        std::fs::write(b.join("other.txt"), "other").unwrap();
        git(&b, &["add", "other.txt"]);
        git(&b, &["commit", "-q", "-m", "other"]);
        init(&a, &home, Mode::Home).unwrap();
        assert_eq!(init(&b, &home, Mode::Home).unwrap_err().code, "name-taken");
    }

    #[test]
    fn repo_mode_wins_and_adds_union_merge() {
        let tmp = TempDir::new();
        let home = tmp.0.join("home");
        let repo = repo_with_commit(&tmp.0, "r");
        let p = init(&repo, &home, Mode::Repo).unwrap();
        assert_eq!(p.root, repo.join("rvow"));
        assert_eq!(find(&repo, &home).unwrap().unwrap().root, p.root);
        let attrs = std::fs::read_to_string(repo.join(".gitattributes")).unwrap();
        assert_eq!(attrs, "rvow/**/*.jsonl merge=union\n");
        add_union_merge(&repo).unwrap();
        let again = std::fs::read_to_string(repo.join(".gitattributes")).unwrap();
        assert_eq!(again, attrs);
    }

    #[test]
    fn refuses_empty_and_shallow_repositories() {
        let tmp = TempDir::new();
        let home = tmp.0.join("home");
        let empty = tmp.0.join("empty");
        std::fs::create_dir_all(&empty).unwrap();
        git(&empty, &["init", "-q"]);
        assert_eq!(init(&empty, &home, Mode::Home).unwrap_err().code, "no-commit");

        let src = repo_with_commit(&tmp.0, "src");
        std::fs::write(src.join("b.txt"), "b").unwrap();
        git(&src, &["add", "b.txt"]);
        git(&src, &["commit", "-q", "-m", "two"]);
        let url = format!("file://{}", src.display());
        let shallow = tmp.0.join("shallow");
        git(&tmp.0, &["clone", "-q", "--depth", "1", &url, shallow.to_str().unwrap()]);
        assert_eq!(init(&shallow, &home, Mode::Home).unwrap_err().code, "shallow");
    }

    #[test]
    fn rebind_moves_a_home_tree_to_its_remote_path() {
        let tmp = TempDir::new();
        let home = tmp.0.join("home");
        let repo = repo_with_commit(&tmp.0, "r");
        let p = init(&repo, &home, Mode::Home).unwrap();
        assert_eq!(rebind(&repo, &home).unwrap(), Rebind::Unchanged(p.root.clone()));

        git(&repo, &["remote", "add", "origin", "git@github.com:a/r.git"]);
        let to = home.join("github.com/a/r");
        assert_eq!(rebind(&repo, &home).unwrap(), Rebind::Moved { from: p.root.clone(), to: to.clone() });
        assert!(!p.root.exists());
        let found = find(&repo, &home).unwrap().unwrap();
        assert_eq!((found.root, found.meta.remote.as_deref()), (to.clone(), Some("github.com/a/r")));
        assert_eq!(rebind(&repo, &home).unwrap(), Rebind::Unchanged(to));
    }

    #[test]
    fn rebind_refuses_to_overwrite_another_tree() {
        let tmp = TempDir::new();
        let home = tmp.0.join("home");
        let a = repo_with_commit(&tmp.0, "a");
        init(&a, &home, Mode::Home).unwrap();
        git(&a, &["remote", "add", "origin", "git@github.com:x/a.git"]);
        let other = home.join("github.com/x/a");
        std::fs::create_dir_all(&other).unwrap();
        std::fs::write(other.join(PROJECT_FILE), "").unwrap();
        assert_eq!(rebind(&a, &home).unwrap_err().code, "name-taken");
    }

    #[test]
    fn rebind_rewrites_a_broken_repo_project_file() {
        let tmp = TempDir::new();
        let repo = repo_with_commit(&tmp.0, "r");
        let p = init(&repo, &tmp.0.join("home"), Mode::Repo).unwrap();
        std::fs::write(p.root.join(PROJECT_FILE), "not toml [").unwrap();
        assert_eq!(find(&repo, &tmp.0.join("home")).unwrap_err().code, "project-file");
        assert_eq!(rebind(&repo, &tmp.0.join("home")).unwrap(), Rebind::Updated(p.root.clone()));
        assert_eq!(find(&repo, &tmp.0.join("home")).unwrap(), Some(p));
    }

    #[test]
    fn rebind_without_a_tree_says_to_init() {
        let tmp = TempDir::new();
        let repo = repo_with_commit(&tmp.0, "r");
        assert_eq!(rebind(&repo, &tmp.0.join("home")).unwrap_err().code, "no-project");
    }

    #[test]
    fn rebind_with_map_finds_a_rewritten_repo() {
        let tmp = TempDir::new();
        let home = tmp.0.join("home");
        let old = repo_with_commit(&tmp.0, "old");
        let p = init(&old, &home, Mode::Home).unwrap();
        let a = git::run(&old, &["rev-parse", "HEAD"]).unwrap().trim().to_string();

        // A fresh repository with different content stands in for the same
        // project after a rewrite, so its root commits differ.
        let new = tmp.0.join("new");
        std::fs::create_dir_all(&new).unwrap();
        git(&new, &["init", "-q", "-b", "main"]);
        std::fs::write(new.join("a.txt"), "rewritten").unwrap();
        git(&new, &["add", "a.txt"]);
        git(&new, &["commit", "-q", "-m", "init"]);
        let b = git::run(&new, &["rev-parse", "HEAD"]).unwrap().trim().to_string();
        assert_ne!(a, b, "the two repositories have different roots after a rewrite");

        let map = tmp.0.join("commit-map");
        std::fs::write(&map, format!("{a} {b}\n")).unwrap();

        // Without the map the new repo matches no recorded root.
        assert_eq!(find(&new, &home).unwrap(), None);
        let changed = rebind_with_map(&new, &home, &map).unwrap();
        assert_eq!(changed, Rebind::Updated(p.root.clone()));

        // Now a plain find sees the project at its new root.
        let found = find(&new, &home).unwrap().unwrap();
        assert_eq!(found.root, p.root);
        assert_eq!(found.meta.roots, vec![b]);
    }
}
