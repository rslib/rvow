//! The allocation budget of each rvow hook on a fixed fixture store. The hooks
//! run in this process, so the counting allocator below sees them; the counts
//! are per thread, so the other modules of the suite do not disturb them.

use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use rvow_core::error::Result;
use rvow_core::store::Project;
use serde_json::{Value, json};

const RVOW: &str = env!("CARGO_BIN_EXE_rvow");

/// The living capabilities of the fixture, 5 requirements each.
const CAPS: usize = 40;
/// The tasks of the fixture's accepted change.
const TASKS: usize = 6;
/// The session id of every call, so each test measures one session's steady state.
const SESSION: &str = "s-alloc";

thread_local! {
    static COUNTS: Cell<(usize, usize)> = const { Cell::new((0, 0)) };
}

struct Counting;

unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        count(layout.size());
        unsafe { System.alloc(layout) }
    }

    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        count(layout.size());
        unsafe { System.alloc_zeroed(layout) }
    }

    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        count(new_size);
        unsafe { System.realloc(ptr, layout, new_size) }
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        unsafe { System.dealloc(ptr, layout) }
    }
}

#[global_allocator]
static ALLOCATOR: Counting = Counting;

/// Counts one allocation of `bytes` on this thread.
fn count(bytes: usize) {
    COUNTS.with(|counts| {
        let (allocations, total) = counts.get();
        counts.set((allocations + 1, total + bytes));
    });
}

fn reset() {
    COUNTS.with(|counts| counts.set((0, 0)));
}

fn measured() -> (usize, usize) {
    COUNTS.with(Cell::get)
}

/// A repository with 40 living capabilities and one accepted change of 6 tasks
/// over committed files, removed on drop. It is one agent session's store.
struct Fx {
    root: PathBuf,
    /// The rvow tree that `rvow init` wrote.
    project: PathBuf,
}

impl Drop for Fx {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

impl Fx {
    fn new(name: &str) -> Fx {
        let root = std::env::temp_dir().join(format!("rvow-alloc-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("repo")).unwrap();
        std::fs::create_dir_all(root.join("home")).unwrap();
        let mut fx = Fx { root: root.canonicalize().unwrap(), project: PathBuf::new() };
        fx.git(&["init", "-q", "-b", "main"]);
        std::fs::write(fx.repo().join("a.txt"), "a").unwrap();
        for i in 1..=TASKS {
            let path = fx.repo().join(format!("src/f{i}.rs"));
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(&path, block()).unwrap();
        }
        fx.git(&["add", "-A"]);
        fx.git(&["commit", "-q", "-m", "init"]);
        let out = fx.rvow(&["init"]);
        fx.project = PathBuf::from(field(&out, "created"));
        let out = fx.rvow(&["new", "--spec", "Out"]);
        let (id, dir) = (field(&out, "created"), PathBuf::from(field(&out, "path")));
        std::fs::write(dir.join("tasks.md"), tasks()).unwrap();
        std::fs::write(
            dir.join("specs/out.md"),
            "## ADD REQ-new1 Out\nrvow SHALL write ok.\n### Scenario: s\n- WHEN it runs\n- THEN the files hold ok\n",
        )
        .unwrap();
        fx.rvow(&["accept", &id, "--yes"]);
        for cap in 0..CAPS {
            let dir = fx.specs().join(format!("cap{cap:02}"));
            std::fs::create_dir_all(&dir).unwrap();
            std::fs::write(dir.join("spec.md"), spec_text(cap)).unwrap();
        }
        // The hooks read the store with this walk, so the fixture proves every
        // living spec loads clean before a test counts anything.
        let project = rvow_core::store::find(&fx.repo(), &fx.home()).unwrap().expect("the fixture has an rvow project");
        let (living, findings) = rvow_core::change::load_living(&project).unwrap();
        assert_eq!(living.len(), CAPS, "{:?}", living.iter().map(|(c, _)| c).collect::<Vec<_>>());
        assert!(findings.is_empty(), "{findings:?}");
        fx
    }

    fn repo(&self) -> PathBuf {
        self.root.join("repo")
    }

    /// The rvow home the fixture's subprocesses and the hooks both read.
    fn home(&self) -> PathBuf {
        self.root.join("rh")
    }

    fn specs(&self) -> PathBuf {
        self.project.join("specs")
    }

    fn command(&self) -> Command {
        let mut c = Command::new(RVOW);
        c.current_dir(self.repo())
            .env("HOME", self.root.join("home"))
            .env("RVOW_HOME", self.home())
            .env("PATH", "/usr/bin:/bin:/usr/sbin:/sbin")
            .env_remove("XDG_CONFIG_HOME")
            .env_remove("XDG_DATA_HOME")
            .env_remove("CLAUDE_CONFIG_DIR")
            .env_remove("PI_CODING_AGENT_DIR")
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_AUTHOR_NAME", "t")
            .env("GIT_AUTHOR_EMAIL", "t@t")
            .env("GIT_COMMITTER_NAME", "t")
            .env("GIT_COMMITTER_EMAIL", "t@t");
        c
    }

    fn git(&self, args: &[&str]) {
        let ok = Command::new("git")
            .args(args)
            .current_dir(self.repo())
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_AUTHOR_NAME", "t")
            .env("GIT_AUTHOR_EMAIL", "t@t")
            .env("GIT_COMMITTER_NAME", "t")
            .env("GIT_COMMITTER_EMAIL", "t@t")
            .status()
            .unwrap()
            .success();
        assert!(ok, "git {args:?}");
    }

    fn rvow(&self, args: &[&str]) -> String {
        let out = self.command().args(args).output().unwrap();
        assert!(out.status.success(), "rvow {args:?}: {}", String::from_utf8_lossy(&out.stdout));
        String::from_utf8(out.stdout).unwrap()
    }

    /// `rvow` with `text` on stdin, as `rvow put` reads the replacement text.
    fn put(&self, args: &[&str], text: &str) -> String {
        let mut child = self.command().args(args).stdin(Stdio::piped()).stdout(Stdio::piped()).spawn().unwrap();
        let mut stdin = child.stdin.take().unwrap();
        stdin.write_all(text.as_bytes()).unwrap();
        drop(stdin);
        let out = child.wait_with_output().unwrap();
        assert!(out.status.success(), "rvow {args:?}: {}", String::from_utf8_lossy(&out.stdout));
        String::from_utf8(out.stdout).unwrap()
    }

    /// Switches to a new branch whose plan waits for accept, with 6 tasks and no
    /// accepted change.
    fn waiting(&self) {
        self.git(&["checkout", "-q", "-b", "waiting"]);
        let out = self.rvow(&["new", "Waiting"]);
        let id = field(&out, "created");
        let proposal = "branch: waiting\n\n# Waiting\n\n## Why\nIt helps.\n\n## What changes\n- One thing.\n\n## Out of scope\nNone.\n";
        self.put(&["put", &id, "proposal"], proposal);
        self.put(&["put", &id, "tasks"], &tasks());
    }

    /// Points this process's env at the fixture, as the CLI that runs the hooks has it.
    fn set_env(&self) {
        unsafe {
            std::env::set_var("HOME", self.root.join("home"));
            std::env::set_var("RVOW_HOME", self.home());
            std::env::set_var("PATH", "/usr/bin:/bin:/usr/sbin:/sbin");
            std::env::set_var("GIT_CONFIG_GLOBAL", "/dev/null");
            std::env::set_var("GIT_CONFIG_NOSYSTEM", "1");
            std::env::remove_var("XDG_CONFIG_HOME");
            std::env::remove_var("XDG_DATA_HOME");
            std::env::remove_var("CLAUDE_CONFIG_DIR");
            std::env::remove_var("PI_CODING_AGENT_DIR");
        }
    }

    /// The call of a first Edit of the first task's file.
    fn edit(&self, event: &str) -> Value {
        json!({
            "session_id": SESSION,
            "hook_event_name": event,
            "cwd": self.repo(),
            "tool_name": "Edit",
            "tool_use_id": "t-alloc-1",
            "tool_input": {
                "file_path": self.repo().join("src/f1.rs"),
                "old_string": block(),
                "new_string": "b",
            },
        })
    }
}

fn field(out: &str, key: &str) -> String {
    out.lines().find_map(|l| l.strip_prefix(&format!("{key}: "))).unwrap_or_else(|| panic!("no {key} in {out}")).to_string()
}

/// The 3003-character block of code that src/f1.rs holds, so an edit of it is
/// as heavy as a real one.
fn block() -> String {
    "let a = 1;\n".repeat(273)
}

fn tasks() -> String {
    let mut text = String::new();
    for i in 1..=TASKS {
        let id = format!("TSK-{}", id_body(900 + i));
        text.push_str(&format!("- {id} Write file {i}\n  covers: REQ-new1\n  verify: test -f src/f{i}.rs\n  files: src/f{i}.rs\n"));
    }
    text
}

const ALPHABET: &[u8; 32] = b"0123456789abcdefghjkmnpqrstvwxyz";

/// A minted-looking id body of 6 Crockford base32 chars for `n`, the same on
/// every run. The digits of `n` are permuted, so the ids do not sort by `n`.
fn id_body(n: usize) -> String {
    const PERM: [usize; 6] = [3, 0, 5, 1, 4, 2];
    const SALT: [usize; 6] = [7, 19, 2, 11, 29, 5];
    let mut out = String::with_capacity(6);
    for k in 0..6 {
        let digit = (n >> (5 * PERM[k])) & 31;
        out.push(char::from(ALPHABET[(digit + SALT[k]) & 31]));
    }
    out
}

fn req_id(n: usize) -> String {
    format!("REQ-{}", id_body(n))
}

/// The text of one living capability: 5 requirements in EARS form, 2 scenarios each.
fn spec_text(cap: usize) -> String {
    let mut text = format!("# Capability {cap}\n\n");
    for req in 0..5 {
        text.push_str(&format!("## {} Rule {req} of cap {cap}\n\n", req_id(cap * 5 + req + 1)));
        text.push_str(&requirement_text(req, cap));
        for sc in 0..2 {
            text.push_str(&format!(
                "\n### Scenario: Case {sc}\n- GIVEN an accepted change\n- WHEN the call runs\n- THEN the state holds {req}\n"
            ));
        }
        text.push('\n');
    }
    text
}

fn requirement_text(req: usize, cap: usize) -> String {
    match req {
        0 => format!("WHEN a session starts, rvow SHALL print the brief of capability {cap}.\n"),
        1 => format!("IF a task fails, THEN rvow SHALL record the failure in ledger {cap}.\n"),
        2 => "WHILE a change is active, rvow SHALL record every call of the session.\n".to_string(),
        3 => format!("rvow SHALL list the proved tasks of capability {cap}.\n"),
        _ => "WHEN the user runs `rvow prove`, rvow SHALL append one event to the ledger.\n".to_string(),
    }
}

/// The allocations and bytes of a second, identical hook call with `store::find`
/// in the same count, after one warm-up call with the same session. That is the
/// steady state of an agent session.
fn counted<F>(fx: &Fx, mut hook: F) -> (usize, usize)
where
    F: FnMut(&Project, &Path, &Path) -> Result<Option<Value>>,
{
    let _guard = crate::env_lock::lock();
    fx.set_env();
    let mut once = || {
        let project = rvow_core::store::find(&fx.repo(), &fx.home()).unwrap().expect("the fixture has an rvow project");
        let _ = hook(&project, &fx.home(), &fx.repo()).unwrap();
    };
    once();
    reset();
    once();
    measured()
}

#[test]
fn alloc_harness_counts_a_known_allocation() {
    reset();
    let buf: Vec<u8> = Vec::with_capacity(100);
    let (count, bytes) = measured();
    assert!(count >= 1, "count={count}");
    assert!(bytes >= 100, "bytes={bytes}");
    assert_eq!(buf.capacity(), 100);
}

/// The steady-state count of one PreToolUse Edit call.
const PRE_TOOL_USE_EDIT: usize = 5400; // measured 2026-10-22 on macOS: 4902; budget +10% for other platforms; trunk d05d6ee: 86090

#[test]
fn alloc_pre_tool_use_edit() {
    let fx = Fx::new("pre-edit");
    let input = fx.edit("PreToolUse");
    let (count, bytes) = counted(&fx, |project, _home, _cwd| rvow_hook::hook::claude_pre_tool_use(project, &input));
    eprintln!("alloc pre_tool_use_edit: count={count} bytes={bytes}");
    assert!(count <= PRE_TOOL_USE_EDIT, "alloc pre_tool_use_edit: count={count} bytes={bytes} over {PRE_TOOL_USE_EDIT}");
}

/// The steady-state count of one PreToolUse Bash call.
const PRE_TOOL_USE_BASH: usize = 1510; // measured 2026-10-22 on macOS: 1365; budget +10% for other platforms; trunk d05d6ee: 7560

#[test]
fn alloc_pre_tool_use_bash() {
    let fx = Fx::new("pre-bash");
    let input = json!({
        "session_id": SESSION,
        "hook_event_name": "PreToolUse",
        "cwd": fx.repo(),
        "tool_name": "Bash",
        "tool_use_id": "t-alloc-2",
        "tool_input": {"command": "cargo test -p rvow-core"},
    });
    let (count, bytes) = counted(&fx, |project, _home, _cwd| rvow_hook::hook::claude_pre_tool_use(project, &input));
    eprintln!("alloc pre_tool_use_bash: count={count} bytes={bytes}");
    assert!(count <= PRE_TOOL_USE_BASH, "alloc pre_tool_use_bash: count={count} bytes={bytes} over {PRE_TOOL_USE_BASH}");
}

/// The steady-state count of one PostToolUse call.
const POST_TOOL_USE: usize = 120; // measured 2026-10-22 on macOS: 103; budget +10% for other platforms; trunk d05d6ee: 1288

#[test]
fn alloc_post_tool_use() {
    let fx = Fx::new("post");
    let input = fx.edit("PostToolUse");
    let (count, bytes) = counted(&fx, |project, _home, _cwd| rvow_hook::hook::claude_post_tool_use(project, &input));
    eprintln!("alloc post_tool_use: count={count} bytes={bytes}");
    assert!(count <= POST_TOOL_USE, "alloc post_tool_use: count={count} bytes={bytes} over {POST_TOOL_USE}");
}

/// The steady-state count of one Stop call.
const STOP: usize = 6280; // measured 2026-10-22 on macOS: 5704; budget +10% for other platforms; trunk d05d6ee: 66025

#[test]
fn alloc_stop() {
    let fx = Fx::new("stop");
    let input = json!({"session_id": SESSION, "hook_event_name": "Stop", "cwd": fx.repo()});
    let (count, bytes) = counted(&fx, |project, home, _cwd| rvow_hook::hook::claude_stop(project, home, &input));
    eprintln!("alloc stop: count={count} bytes={bytes}");
    assert!(count <= STOP, "alloc stop: count={count} bytes={bytes} over {STOP}");
}

/// The steady-state count of one background re-prove check.
const REPROVE_READY: usize = 4080; // measured 2026-10-22 on macOS: 3701; budget +10% for other platforms; before: 27192

#[test]
fn alloc_reprove_ready() {
    let fx = Fx::new("reprove-ready");
    let (count, bytes) = counted(&fx, |project, home, cwd| rvow_core::reprove::ready(project, home, cwd).map(|_| None));
    eprintln!("alloc reprove_ready: count={count} bytes={bytes}");
    assert!(count <= REPROVE_READY, "alloc reprove_ready: count={count} bytes={bytes} over {REPROVE_READY}");
}

/// The steady-state count of one load of the fixture's living specs.
const LOAD_LIVING: usize = 5090; // measured 2026-10-22 on macOS: 4620; budget +10% for other platforms; before: 26580

#[test]
fn alloc_load_living() {
    let fx = Fx::new("load-living");
    let (count, bytes) = counted(&fx, |project, _home, _cwd| rvow_core::change::load_living(project).map(|_| None));
    eprintln!("alloc load_living: count={count} bytes={bytes}");
    assert!(count <= LOAD_LIVING, "alloc load_living: count={count} bytes={bytes} over {LOAD_LIVING}");
}

/// The steady-state count of one SessionStart call.
const SESSION_START: usize = 4200; // measured 2026-10-22 on macOS: 3812; budget +10% for other platforms; before: 3812; trunk d05d6ee: 29184

#[test]
fn alloc_session_start() {
    let fx = Fx::new("start");
    // A session starts once, so each call is the first of a new session: the
    // warm-up and the counted call have their own session ids.
    let mut n = 0;
    let (count, bytes) = counted(&fx, |project, home, _cwd| {
        n += 1;
        let input =
            json!({"session_id": format!("{SESSION}-{n}"), "hook_event_name": "SessionStart", "source": "startup", "cwd": fx.repo()});
        rvow_hook::hook::claude_session_start(Some(project), home, &input, &[])
    });
    eprintln!("alloc session_start: count={count} bytes={bytes}");
    assert!(count <= SESSION_START, "alloc session_start: count={count} bytes={bytes} over {SESSION_START}");
}

/// The steady-state count of one SessionStart call while a plan waits for accept.
const SESSION_START_WAITING: usize = 4720; // measured 2026-10-22 on macOS: 4283; budget +10% for other platforms; before: 7300; was: 30971

#[test]
fn alloc_session_start_with_a_waiting_plan() {
    let fx = Fx::new("start-waiting");
    fx.waiting();
    let mut n = 0;
    let (count, bytes) = counted(&fx, |project, home, _cwd| {
        n += 1;
        let input =
            json!({ "session_id": format!("{SESSION}-w{n}"), "hook_event_name": "SessionStart", "source": "startup", "cwd": fx.repo() });
        rvow_hook::hook::claude_session_start(Some(project), home, &input, &[])
    });
    eprintln!("alloc session_start_waiting: count={count} bytes={bytes}");
    assert!(count <= SESSION_START_WAITING, "alloc session_start_waiting: count={count} bytes={bytes} over {SESSION_START_WAITING}");
}

/// The steady-state count of one Stop call followed by its background re-prove check.
const STOP_WITH_REPROVE: usize = 6750; // measured 2026-10-22 on macOS: 6136; budget +10% for other platforms; before: 9153

#[test]
fn alloc_stop_with_reprove() {
    let fx = Fx::new("stop-reprove");
    let input = json!({"session_id": SESSION, "hook_event_name": "Stop", "cwd": fx.repo()});
    let (count, bytes) = counted(&fx, |project, home, cwd| {
        let living = rvow_core::change::living(project)?;
        let out = rvow_hook::hook::claude_stop_over(project, home, &input, &living)?;
        rvow_core::reprove::ready_over(project, home, cwd, &living)?;
        Ok(out)
    });
    eprintln!("alloc stop_with_reprove: count={count} bytes={bytes}");
    assert!(count <= STOP_WITH_REPROVE, "alloc stop_with_reprove: count={count} bytes={bytes} over {STOP_WITH_REPROVE}");
}

/// The steady-state count of one view call, the status line's source.
const VIEW: usize = 10450; // measured 2026-10-22 on macOS: 9498; budget +10% for other platforms; trunk d05d6ee: 68850

#[test]
fn alloc_view() {
    let fx = Fx::new("view");
    let (count, bytes) = counted(&fx, rvow_core::view::view);
    eprintln!("alloc view: count={count} bytes={bytes}");
    assert!(count <= VIEW, "alloc view: count={count} bytes={bytes} over {VIEW}");
}
