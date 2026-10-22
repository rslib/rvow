//! Read-only git queries. rvow never writes to a repository through this module.

use std::path::{Path, PathBuf};
use std::process::Command;

use crate::error::{Error, Result};

static BIN: std::sync::OnceLock<&'static str> = std::sync::OnceLock::new();

/// The git binary to run: `RVOW_GIT` when it names a file, else `git`.
/// A sandbox whose `git` on `PATH` cannot run sets `RVOW_GIT` to one that can.
/// `EVAL_RVOW_GIT` is the same setting through the eval harness, whose
/// sandbox passes only `EVAL_*` variables to the agent session.
/// Resolved once per process; the one leaked string is the whole cost.
pub fn bin_str() -> &'static str {
    BIN.get_or_init(|| Box::leak(resolve().into_boxed_str()))
}

/// The git binary as an owned string, for a caller that keeps it.
pub fn bin() -> String {
    bin_str().to_string()
}

fn resolve() -> String {
    for key in ["RVOW_GIT", "EVAL_RVOW_GIT"] {
        if let Some(v) = std::env::var(key).ok().filter(|v| !v.is_empty()) {
            return v;
        }
    }
    let path_env = std::env::var("PATH").unwrap_or_default();
    match cache_file() {
        Some(cache) => real_git(&path_env, cfg!(target_os = "macos"), &cache, find_real_git),
        None => "git".to_string(),
    }
}

/// The `git` that `PATH` names: the first executable file so called.
fn git_on_path(path_env: &str) -> Option<PathBuf> {
    path_env.split(':').filter(|d| !d.is_empty()).map(|d| Path::new(d).join("git")).find(|p| executable(p))
}

fn executable(path: &Path) -> bool {
    let Ok(meta) = std::fs::metadata(path) else {
        return false;
    };
    if !meta.is_file() {
        return false;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        meta.permissions().mode() & 0o111 != 0
    }
    #[cfg(not(unix))]
    {
        true
    }
}

/// The git to run for one `PATH`, macOS-ness, cache file and resolver. Split
/// off from the process environment so a test can pose the macOS shim case
/// anywhere.
fn real_git(path_env: &str, is_macos: bool, cache: &Path, resolver: impl Fn() -> Option<PathBuf>) -> String {
    real_git_in(path_env, Path::new("/usr/bin/git"), is_macos, cache, resolver, root_owned)
}

fn real_git_in(
    path_env: &str,
    shim: &Path,
    is_macos: bool,
    cache: &Path,
    resolver: impl Fn() -> Option<PathBuf>,
    trusted: impl Fn(&Path) -> bool,
) -> String {
    let Some(found) = git_on_path(path_env) else {
        return "git".to_string();
    };
    // Off macOS, and for any other git, the `git` on PATH is the one to run.
    if !is_macos || found != shim {
        return "git".to_string();
    }
    if let Some(cached) = cached_git(cache, trusted) {
        return cached;
    }
    match resolver().filter(|p| executable(p)) {
        Some(real) => {
            write_cache(cache, &real);
            path_string(&real)
        }
        // A failed lookup leaves the shim, which still runs git, only slower.
        None => "git".to_string(),
    }
}

fn path_string(path: &Path) -> String {
    path.to_string_lossy().into_owned()
}

/// The real git a previous rvow process recorded, while it is still there.
fn cached_git(cache: &Path, trusted: impl Fn(&Path) -> bool) -> Option<String> {
    let text = std::fs::read_to_string(cache).ok()?;
    let real = PathBuf::from(text.trim());
    (executable(&real) && trusted(&real)).then(|| path_string(&real))
}

/// The Xcode git belongs to root, and an agent cannot make a root-owned file,
/// so a path that an agent wrote into the cache is never run.
fn root_owned(path: &Path) -> bool {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        std::fs::metadata(path).is_ok_and(|m| m.uid() == 0)
    }
    #[cfg(not(unix))]
    {
        let _ = path;
        false
    }
}

/// Writes through a temp file in the same folder, so a reader never sees half
/// a path. Best effort: a failure only costs the next lookup.
fn write_cache(cache: &Path, real: &Path) {
    let Some(parent) = cache.parent() else { return };
    if std::fs::create_dir_all(parent).is_err() {
        return;
    }
    let tmp = parent.join(format!(".rvow-git-{}.tmp", std::process::id()));
    if std::fs::write(&tmp, path_string(real)).is_ok() && std::fs::rename(&tmp, cache).is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
}

/// Asks the Xcode tools for the git behind `/usr/bin/git`, the xcrun shim that
/// costs about 6 ms on every call.
fn find_real_git() -> Option<PathBuf> {
    let out = Command::new("xcrun").args(["--find", "git"]).output().ok()?;
    if !out.status.success() {
        return None;
    }
    let path = String::from_utf8_lossy(&out.stdout).trim().to_string();
    (!path.is_empty()).then(|| PathBuf::from(path))
}

/// The cache sits in the rvow home. A missing home stays missing, since a CI
/// run must not make one.
fn cache_file() -> Option<PathBuf> {
    crate::store::home_dir().ok().filter(|home| home.is_dir()).map(|home| home.join("hook-state").join("git-bin"))
}

/// Removes the repository vars that a git hook sets for its own repository,
/// which would override `-C`. Only a set var is removed: any removal makes the
/// child's environment a copy of the whole parent environment.
fn without_hook_env(cmd: &mut Command) -> &mut Command {
    for var in ["GIT_DIR", "GIT_WORK_TREE", "GIT_INDEX_FILE"] {
        if std::env::var_os(var).is_some() {
            cmd.env_remove(var);
        }
    }
    cmd
}

pub fn run(dir: &Path, args: &[&str]) -> Result<String> {
    let out = without_hook_env(Command::new(bin_str()).arg("-C").arg(dir).args(args))
        .output()
        .map_err(|e| Error::new("git", format!("cannot run git: {e}"), "install git"))?;
    if !out.status.success() {
        return Err(Error::new(
            "git",
            format!("git {} failed: {}", args.join(" "), String::from_utf8_lossy(&out.stderr).trim()),
            "run the command in a git repository",
        ));
    }
    Ok(String::from_utf8_lossy(&out.stdout).trim_end().to_string())
}

/// The base branch ref to measure from: the local or the remote base branch,
/// whichever has the newer merge base with HEAD. When the two merge bases are
/// equal or neither contains the other, the remote one wins, as it did when it
/// always came first. `None` when neither side exists.
pub fn base_ref(top: &Path, base: &str) -> Option<String> {
    let local = format!("refs/heads/{base}");
    let remote = format!("refs/remotes/origin/{base}");
    // A side with no merge base counts as missing, so unrelated histories
    // fall back to the other side instead of failing here.
    let mb = |r: &str| run(top, &["merge-base", "HEAD", r]).ok();
    match (mb(&local), mb(&remote)) {
        (None, None) => None,
        (Some(_), None) => Some(local),
        (None, Some(_)) => Some(remote),
        (Some(l), Some(r)) if l != r && run(top, &["merge-base", "--is-ancestor", &r, &l]).is_ok() => Some(local),
        _ => Some(remote),
    }
}

/// The tip commit of `branch`: the local branch, else `origin/<branch>`.
/// `None` when neither exists.
pub fn branch_tip(top: &Path, branch: &str) -> Option<String> {
    [format!("refs/heads/{branch}"), format!("refs/remotes/origin/{branch}")]
        .iter()
        .find_map(|r| run(top, &["rev-parse", "--verify", "--quiet", &format!("{r}^{{commit}}")]).ok())
}

/// A question to git whose answer `remember_answers` keeps.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
enum Question {
    Toplevel,
    Branch,
}

type Answers = std::sync::Mutex<std::collections::HashMap<(PathBuf, Question), Known>>;

/// The most answers this process keeps: a long-lived process with many
/// folders would else grow the map without end.
const ANSWER_LIMIT: usize = 256;

/// One remembered answer, with the HEAD mtime it was stored under.
struct Known {
    answer: String,
    /// The mtime of the repository's HEAD file when the answer was stored.
    /// A change means HEAD moved, so the answer is stale. `None` for a
    /// question HEAD cannot date, which is never stale.
    head_mtime: Option<std::time::SystemTime>,
}

static ANSWERS: std::sync::OnceLock<Answers> = std::sync::OnceLock::new();

/// Keeps the answers of `toplevel` and `branch` for the rest of the process.
/// Only a hook turns it on: it runs for one tool call, and nothing moves HEAD
/// meanwhile. A longer process re-asks once HEAD moves, since a branch answer
/// carries the mtime of the HEAD file it came from.
pub fn remember_answers() {
    ANSWERS.get_or_init(Default::default);
}

/// Keeps one answer, clearing the map first when it is full.
fn keep(map: &mut std::collections::HashMap<(PathBuf, Question), Known>, key: (PathBuf, Question), known: Known) {
    if map.len() >= ANSWER_LIMIT {
        map.clear();
    }
    map.insert(key, known);
}

/// The HEAD file of the repository that holds `dir`, from `dir/.git` or from
/// the git dir a linked worktree's `.git` file names.
fn head_path(dir: &Path) -> Option<PathBuf> {
    let dot = dir.ancestors().map(|a| a.join(".git")).find(|p| p.exists())?;
    if dot.is_dir() {
        return Some(dot.join("HEAD"));
    }
    let text = std::fs::read_to_string(&dot).ok()?;
    let git_dir = PathBuf::from(text.trim().strip_prefix("gitdir: ")?);
    let git_dir = if git_dir.is_absolute() { git_dir } else { dot.parent()?.join(git_dir) };
    Some(git_dir.join("HEAD"))
}

/// The mtime of the HEAD file that a branch answer depends on. Only a branch
/// can change while the process lives, and only a moved HEAD changes it.
fn head_mtime(dir: &Path, question: Question) -> Option<std::time::SystemTime> {
    if question != Question::Branch {
        return None;
    }
    std::fs::metadata(head_path(dir)?).ok()?.modified().ok()
}

fn answer(dir: &Path, question: Question, ask: impl FnOnce() -> Result<String>) -> Result<String> {
    let Some(answers) = ANSWERS.get() else { return ask() };
    let key = (dir.to_path_buf(), question);
    let mtime = head_mtime(dir, question);
    let known = answers.lock().ok().and_then(|a| a.get(&key).filter(|k| k.head_mtime == mtime).map(|k| k.answer.clone()));
    if let Some(known) = known {
        return Ok(known);
    }
    let found = ask()?;
    if let Ok(mut a) = answers.lock() {
        keep(&mut a, key, Known { answer: found.clone(), head_mtime: mtime });
    }
    Ok(found)
}

/// Records the top folder of `dir` that another git call already printed.
pub(crate) fn known_toplevel(dir: &Path, top: &Path) {
    let Some(Ok(mut a)) = ANSWERS.get().map(|a| a.lock()) else { return };
    let key = (dir.to_path_buf(), Question::Toplevel);
    if !a.contains_key(&key) {
        keep(&mut a, key, Known { answer: top.display().to_string(), head_mtime: None });
    }
}

/// Records the branch of `dir` that `head_file`, the text of its git dir's
/// `HEAD`, names. A reftable repository writes a placeholder there, and a
/// detached or unknown HEAD records nothing, so `branch` still asks git.
pub(crate) fn known_branch(dir: &Path, head_file: &str) {
    let Some(name) = head_file.trim().strip_prefix("ref: refs/heads/") else { return };
    if name.is_empty() || name == ".invalid" {
        return;
    }
    let mtime = head_mtime(dir, Question::Branch);
    let Some(Ok(mut a)) = ANSWERS.get().map(|a| a.lock()) else { return };
    let key = (dir.to_path_buf(), Question::Branch);
    if !a.contains_key(&key) {
        keep(&mut a, key, Known { answer: name.to_string(), head_mtime: mtime });
    }
}

pub fn toplevel(dir: &Path) -> Result<PathBuf> {
    answer(dir, Question::Toplevel, || run(dir, &["rev-parse", "--show-toplevel"])).map(PathBuf::from)
}

pub fn is_shallow(dir: &Path) -> Result<bool> {
    Ok(run(dir, &["rev-parse", "--is-shallow-repository"])? == "true")
}

/// Every root commit reachable from any ref, sorted. Empty for a repository
/// with no commit.
pub fn root_commits(dir: &Path) -> Result<Vec<String>> {
    if run(dir, &["rev-parse", "--verify", "--quiet", "HEAD"]).is_err() {
        return Ok(Vec::new());
    }
    let out = run(dir, &["rev-list", "--max-parents=0", "--all"])?;
    let mut roots: Vec<String> = out.lines().map(String::from).collect();
    roots.sort();
    roots.dedup();
    Ok(roots)
}

pub fn origin_url(dir: &Path) -> Option<String> {
    run(dir, &["remote", "get-url", "origin"]).ok().filter(|u| !u.is_empty())
}

/// The current branch, or `None` on a detached HEAD.
pub fn branch(dir: &Path) -> Result<Option<String>> {
    let name = answer(dir, Question::Branch, || run(dir, &["rev-parse", "--abbrev-ref", "HEAD"]))?;
    Ok((name != "HEAD").then_some(name))
}

/// The path of the worktree of this repository that has `branch` checked
/// out, from `git worktree list --porcelain`. Prunable and bare entries are
/// skipped. `None` when no worktree has the branch.
pub fn worktree_for_branch(dir: &Path, branch: &str) -> Result<Option<PathBuf>> {
    let want = format!("refs/heads/{branch}");
    let out = run(dir, &["worktree", "list", "--porcelain"])?;
    for entry in out.split("\n\n") {
        let mut path = None;
        let mut on_branch = false;
        let mut skip = false;
        for line in entry.lines() {
            if let Some(p) = line.strip_prefix("worktree ") {
                path = Some(PathBuf::from(p));
            } else if let Some(b) = line.strip_prefix("branch ") {
                on_branch = b == want;
            } else if line == "bare" || line == "prunable" || line.starts_with("prunable ") {
                skip = true;
            }
        }
        if on_branch && !skip {
            return Ok(path);
        }
    }
    Ok(None)
}

/// The top of the worktree that holds `branch`: the worktree of `dir` when
/// it is on the branch, else another worktree of the same repository, else
/// `None`.
pub fn top_for_branch(dir: &Path, want: &str) -> Result<Option<PathBuf>> {
    let top = toplevel(dir)?;
    if branch(&top)?.as_deref() == Some(want) {
        return Ok(Some(top));
    }
    worktree_for_branch(&top, want)
}

/// The diff of one task's files against a base commit: the unified diff and
/// the changed paths.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TaskDiff {
    pub diff: String,
    pub files: Vec<String>,
}

/// The diff of every file that matches `globs` against `base`: committed
/// changes since the base, uncommitted edits and untracked files. An
/// untracked file shows as an added file. Files outside the globs stay out.
/// Empty when nothing in scope changed.
/// Splits NUL-terminated git output (`-z`) into paths. `-z` leaves paths
/// unquoted, so a name with non-ASCII bytes still matches the task's globs.
fn nul_paths(out: &str) -> Vec<String> {
    out.strip_suffix('\0').unwrap_or(out).split('\0').filter(|l| !l.is_empty()).map(String::from).collect()
}

pub fn task_diff(top: &Path, base: &str, globs: &[String]) -> Result<TaskDiff> {
    let set = crate::fingerprint::globset(globs)?;
    let mut untracked = std::collections::HashSet::new();
    for path in nul_paths(&run(top, &["ls-files", "--others", "--exclude-standard", "-z"])?) {
        untracked.insert(path);
    }
    let mut names = nul_paths(&run(top, &["diff", "--no-renames", "--name-only", "-z", base, "--"])?);
    names.extend(untracked.iter().cloned());
    names.sort();
    names.dedup();
    let files: Vec<String> = names.into_iter().filter(|f| set.is_match(f)).collect();
    let mut diff = String::new();
    for f in &files {
        // quotePath=false keeps non-ASCII names raw in the patch headers,
        // so an --in-diff consumer sees the same path as RVOW_CHANGED_FILES.
        let part = if untracked.contains(f) {
            diff_no_index(top, f)?
        } else {
            run(top, &["--literal-pathspecs", "-c", "core.quotePath=false", "diff", "--no-renames", base, "--", f])?
        };
        if !part.is_empty() {
            if !diff.is_empty() {
                diff.push('\n');
            }
            diff.push_str(&part);
        }
    }
    Ok(TaskDiff { diff, files })
}

/// `git diff --no-index /dev/null <file>`: the added-file diff of an
/// untracked file. It exits 1 on a difference, which is the expected case.
fn diff_no_index(top: &Path, rel: &str) -> Result<String> {
    let mut cmd = Command::new(bin_str());
    cmd.arg("-C").arg(top).args(["--literal-pathspecs", "-c", "core.quotePath=false", "diff", "--no-index", "--", "/dev/null", rel]);
    let out = without_hook_env(&mut cmd).output().map_err(|e| Error::new("git", format!("cannot run git: {e}"), "install git"))?;
    match out.status.code() {
        Some(0) | Some(1) => Ok(String::from_utf8_lossy(&out.stdout).trim_end().to_string()),
        _ => Err(Error::new(
            "git",
            format!("git diff --no-index failed: {}", String::from_utf8_lossy(&out.stderr).trim()),
            "run the command in a git repository",
        )),
    }
}

/// Turns a remote URL into a `host/owner/repo` path as ghq does, or `None` for a
/// local path or a URL with an unsafe part.
pub fn remote_path(url: &str) -> Option<String> {
    let url = url.trim().trim_end_matches('/');
    let url = url.strip_suffix(".git").unwrap_or(url);
    let rest = if let Some((_, rest)) = url.split_once("://") {
        let rest = rest.rsplit_once('@').map_or(rest, |(_, r)| r);
        let (host, path) = rest.split_once('/')?;
        let host = host.split(':').next()?;
        format!("{host}/{path}")
    } else {
        let (user_host, path) = url.split_once(':')?;
        let host = user_host.rsplit_once('@').map_or(user_host, |(_, h)| h);
        if host.is_empty() || host.contains('/') {
            return None;
        }
        format!("{host}/{path}")
    };
    let parts: Vec<&str> = rest.split('/').collect();
    let safe = parts.len() >= 3 && parts.iter().all(|p| !p.is_empty() && *p != "." && *p != ".." && !p.contains('\\'));
    safe.then(|| parts.join("/").to_lowercase())
}

#[cfg(test)]
pub(crate) mod testrepo {
    use std::path::{Path, PathBuf};
    use std::process::Command;
    use std::sync::atomic::{AtomicU32, Ordering};

    static COUNTER: AtomicU32 = AtomicU32::new(0);

    /// A fresh directory under the system temp dir, removed on drop.
    pub struct TempDir(pub PathBuf);

    impl TempDir {
        pub fn new() -> TempDir {
            let n = COUNTER.fetch_add(1, Ordering::Relaxed);
            let dir = std::env::temp_dir().join(format!("rvow-test-{}-{n}", std::process::id()));
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

    /// Runs git with the user's global and system config ignored, so fixture
    /// repositories never depend on or sign with the user's settings.
    pub fn git(dir: &Path, args: &[&str]) {
        let ok = Command::new(super::bin_str())
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
        assert!(ok.status.success(), "git {args:?}: {ok:?}");
    }

    pub fn repo_with_commit(parent: &Path, name: &str) -> PathBuf {
        let dir = parent.join(name);
        std::fs::create_dir_all(&dir).unwrap();
        git(&dir, &["init", "-q", "-b", "main"]);
        std::fs::write(dir.join("a.txt"), "a").unwrap();
        git(&dir, &["add", "a.txt"]);
        git(&dir, &["commit", "-q", "-m", "init"]);
        dir
    }
}

#[cfg(test)]
mod tests {
    use super::testrepo::*;
    use super::*;

    #[test]
    fn known_branch_reads_a_branch_and_skips_detached_and_reftable_heads() {
        remember_answers();
        let dir = Path::new("/known-branch-test");
        known_branch(&dir.join("a"), "ref: refs/heads/feat/x\n");
        known_branch(&dir.join("b"), "4b825dc642cb6eb9a060e54bf8d69288fbee4904\n");
        known_branch(&dir.join("c"), "ref: refs/heads/.invalid\n");
        let get = |d: &str| ANSWERS.get().unwrap().lock().unwrap().get(&(dir.join(d), Question::Branch)).map(|k| k.answer.clone());
        assert_eq!(get("a").as_deref(), Some("feat/x"));
        assert_eq!(get("b"), None);
        assert_eq!(get("c"), None);
    }

    #[test]
    fn cache_limit_answers_map_never_holds_more_than_the_limit() {
        remember_answers();
        let base = Path::new("/answer-limit-test");
        for i in 0..ANSWER_LIMIT + 8 {
            known_toplevel(&base.join(i.to_string()), &base.join("top"));
        }
        let len = ANSWERS.get().unwrap().lock().unwrap().len();
        assert!(len <= ANSWER_LIMIT, "{len} answers kept over a limit of {ANSWER_LIMIT}");
    }

    #[test]
    fn cache_limit_keep_clears_a_full_map_and_keeps_the_new_answer() {
        let mut map = std::collections::HashMap::new();
        for i in 0..(ANSWER_LIMIT * 2 + 4) {
            let key = (PathBuf::from(i.to_string()), Question::Toplevel);
            keep(&mut map, key.clone(), Known { answer: "top".to_string(), head_mtime: None });
            assert!(map.len() <= ANSWER_LIMIT, "{} answers at {i}", map.len());
            assert_eq!(map.get(&key).map(|k| k.answer.as_str()), Some("top"));
        }
    }

    #[test]
    fn cache_limit_branch_answer_refreshes_after_head_moves() {
        remember_answers();
        let tmp = TempDir::new();
        let repo = repo_with_commit(&tmp.0, "r");
        git(&repo, &["branch", "other"]);
        assert_eq!(branch(&repo).unwrap().as_deref(), Some("main"));
        git(&repo, &["checkout", "-q", "other"]);
        // Both writes can land in one mtime tick, so the test moves HEAD on.
        let head = repo.join(".git/HEAD");
        let later = std::time::SystemTime::now() + std::time::Duration::from_secs(1);
        std::fs::OpenOptions::new().write(true).open(&head).unwrap().set_modified(later).unwrap();
        assert_eq!(branch(&repo).unwrap().as_deref(), Some("other"), "a moved HEAD is a miss");
    }

    #[cfg(unix)]
    fn make_exe(path: &Path) {
        use std::os::unix::fs::PermissionsExt;
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, "").unwrap();
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).unwrap();
    }

    /// A shim on `PATH`, a real git elsewhere, and a cache file under `tmp`.
    #[cfg(unix)]
    fn macos_fixture(tmp: &Path) -> (PathBuf, PathBuf, PathBuf, PathBuf) {
        let shim = tmp.join("shim/git");
        let real = tmp.join("real/git");
        make_exe(&shim);
        make_exe(&real);
        let cache = tmp.join("cache/rvow-git");
        std::fs::create_dir_all(cache.parent().unwrap()).unwrap();
        let path_env = tmp.join("shim").to_str().unwrap().to_string();
        (shim, real, cache, PathBuf::from(path_env))
    }

    #[cfg(unix)]
    #[test]
    fn real_git_shim_on_macos_resolves_once_and_writes_the_cache() {
        let tmp = TempDir::new();
        let (shim, real, cache, path_env) = macos_fixture(&tmp.0);
        let calls = std::cell::Cell::new(0);
        let resolver = || {
            calls.set(calls.get() + 1);
            Some(real.clone())
        };
        let got = real_git_in(path_env.to_str().unwrap(), &shim, true, &cache, resolver, executable);
        assert_eq!(got, real.to_str().unwrap());
        assert_eq!(calls.get(), 1);
        assert_eq!(std::fs::read_to_string(&cache).unwrap(), real.to_str().unwrap());
    }

    #[cfg(unix)]
    #[test]
    fn real_git_ignores_a_cached_path_it_does_not_trust() {
        let tmp = TempDir::new();
        let (shim, real, cache, path_env) = macos_fixture(&tmp.0);
        let planted = tmp.0.join("planted/git");
        make_exe(&planted);
        std::fs::write(&cache, planted.display().to_string()).unwrap();
        let trusted = |p: &Path| p == real.as_path();
        let got = real_git_in(path_env.to_str().unwrap(), &shim, true, &cache, || Some(real.clone()), trusted);
        assert_eq!(got, real.to_str().unwrap(), "a planted path is never run");
    }

    #[test]
    #[cfg(unix)]
    fn real_git_uses_a_valid_cache_without_resolving() {
        let tmp = TempDir::new();
        let (shim, real, cache, path_env) = macos_fixture(&tmp.0);
        std::fs::write(&cache, format!("{}\n", real.display())).unwrap();
        let got = real_git_in(
            path_env.to_str().unwrap(),
            &shim,
            true,
            &cache,
            || {
                panic!("the cache must be used as it is");
            },
            executable,
        );
        assert_eq!(got, real.to_str().unwrap());
    }

    #[cfg(unix)]
    #[test]
    fn real_git_resolves_again_when_the_cached_path_is_gone() {
        let tmp = TempDir::new();
        let (shim, real, cache, path_env) = macos_fixture(&tmp.0);
        std::fs::write(&cache, tmp.0.join("gone/git").display().to_string()).unwrap();
        let got = real_git_in(path_env.to_str().unwrap(), &shim, true, &cache, || Some(real.clone()), executable);
        assert_eq!(got, real.to_str().unwrap());
        assert_eq!(std::fs::read_to_string(&cache).unwrap(), real.to_str().unwrap());
    }

    #[cfg(unix)]
    #[test]
    fn real_git_off_macos_keeps_git() {
        let tmp = TempDir::new();
        let (shim, _real, cache, path_env) = macos_fixture(&tmp.0);
        let got = real_git_in(
            path_env.to_str().unwrap(),
            &shim,
            false,
            &cache,
            || {
                panic!("a git that is not the macOS shim needs no lookup");
            },
            executable,
        );
        assert_eq!(got, "git");
        assert!(!cache.exists(), "nothing to cache when the shim is not in play");
    }

    #[cfg(unix)]
    #[test]
    fn real_git_keeps_git_for_a_non_shim_git_on_path() {
        let tmp = TempDir::new();
        let (_shim, _real, cache, path_env) = macos_fixture(&tmp.0);
        let elsewhere = tmp.0.join("other/git");
        let got = real_git_in(
            path_env.to_str().unwrap(),
            &elsewhere,
            true,
            &cache,
            || {
                panic!("a git that is not the macOS shim needs no lookup");
            },
            executable,
        );
        assert_eq!(got, "git");
        assert!(!cache.exists());
    }

    #[cfg(unix)]
    #[test]
    fn real_git_keeps_git_when_the_resolver_fails() {
        let tmp = TempDir::new();
        let (shim, _real, cache, path_env) = macos_fixture(&tmp.0);
        let got = real_git_in(path_env.to_str().unwrap(), &shim, true, &cache, || None, executable);
        assert_eq!(got, "git", "the shim is the fallback");
        assert!(!cache.exists(), "a failed lookup writes nothing");
    }

    #[test]
    fn remote_paths() {
        let cases = [
            ("git@github.com:Ray/rvow.git", Some("github.com/ray/rvow")),
            ("https://github.com/ray/rvow", Some("github.com/ray/rvow")),
            ("https://user@github.com/ray/rvow.git/", Some("github.com/ray/rvow")),
            ("ssh://git@host.x:2222/a/b/c.git", Some("host.x/a/b/c")),
            ("/srv/repos/rvow.git", None),
            ("../rvow", None),
            ("git@github.com:ray/../x.git", None),
            ("https://github.com/rvow", None),
        ];
        for (url, want) in cases {
            assert_eq!(remote_path(url).as_deref(), want, "{url}");
        }
    }

    #[test]
    fn queries_a_real_repository() {
        let tmp = TempDir::new();
        let empty = tmp.0.join("empty");
        std::fs::create_dir_all(&empty).unwrap();
        git(&empty, &["init", "-q"]);
        assert_eq!(root_commits(&empty).unwrap(), Vec::<String>::new());

        let repo = repo_with_commit(&tmp.0, "r");
        assert_eq!(root_commits(&repo).unwrap().len(), 1);
        assert!(!is_shallow(&repo).unwrap());
        assert_eq!(branch(&repo).unwrap().as_deref(), Some("main"));
        assert_eq!(origin_url(&repo), None);
        assert_eq!(toplevel(&repo.join(".")).unwrap(), repo);

        git(&repo, &["checkout", "-q", "--detach"]);
        assert_eq!(branch(&repo).unwrap(), None);
    }
}
