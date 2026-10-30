//! The allocation bounds of one plan read, built as their own test binary: it
//! counts every allocation of the process, which would reach every test of the
//! main binary under `cargo test`.

use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;
use std::path::{Path, PathBuf};
use std::process::Command;

use rvow_plan::git::testrepo::TempDir;
use rvow_plan::ledger::{self, Kind};
use rvow_plan::store::{Lock, Mode, Project, lock_file};
use serde_json::{Map, Value};

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

/// Runs `once` twice and returns the count of the second run: the first one
/// warms whatever the code allocates only once.
fn counted<F: FnMut()>(mut once: F) -> usize {
    once();
    reset();
    once();
    measured().0
}

/// A ledger of `n` note events, each naming the one before it.
fn write_ledger(path: &Path, n: usize) {
    let mut text = String::new();
    let mut parent = String::new();
    for i in 0..n {
        let parents = if parent.is_empty() { "[]".to_string() } else { format!("[\"{parent}\"]") };
        let line =
            format!("{{\"kind\":\"note\",\"time\":{},\"parents\":{parents},\"text\":\"note {i}\"}}", 1_700_000_000_000u64 + i as u64);
        parent = ledger::short_hash(line.as_bytes());
        text.push_str(&line);
        text.push('\n');
    }
    std::fs::write(path, text).unwrap();
}

/// The allocations of one append of a note on a ledger of `n` events.
fn append_allocs(n: usize) -> usize {
    let tmp = TempDir::new();
    let path = tmp.0.join("ledger.jsonl");
    write_ledger(&path, n);
    let lock: Lock = lock_file(&tmp.0.join(".lock")).unwrap();
    counted(|| {
        let mut data = Map::new();
        data.insert("text".to_string(), Value::from("counted"));
        ledger::append_sealed(&path, &lock, Kind::Note, data, None).unwrap();
    })
}

/// The allocations of one `read` of a ledger of `n` events.
fn read_allocs(n: usize) -> usize {
    let tmp = TempDir::new();
    let path = tmp.0.join("ledger.jsonl");
    write_ledger(&path, n);
    counted(|| {
        let (events, diags) = ledger::read(&path).unwrap();
        assert_eq!(events.len(), n);
        assert!(diags.is_empty(), "{diags:?}");
    })
}

/// One append reads the ledger, so its count must not follow the events already
/// in it: the same append cost 2492 allocations on 200 events and 4930 on 400
/// before, and 38 and 42 after.
const APPEND_RATIO_NUM: usize = 6;
const APPEND_RATIO_DEN: usize = 5;
/// The allocations a fixed part of one append may add between the two sizes.
const APPEND_SLACK: usize = 16;

#[test]
fn scale_ledger_append_allocs() {
    let small = append_allocs(200);
    let big = append_allocs(400);
    eprintln!("scale_ledger_append_allocs: 200 -> {small}, 400 -> {big}");
    let bound = small * APPEND_RATIO_NUM / APPEND_RATIO_DEN + APPEND_SLACK;
    assert!(big <= bound, "one append: 200 events cost {small} allocations, 400 cost {big}, over the bound {bound}");
}

/// The most allocations 200 more events may add to a `read`, which no longer
/// copies the hash of each one: 2437 before, 1837 after.
const READ_PER_EVENTS: usize = 2100;

#[test]
fn scale_ledger_read_allocs() {
    let n = 200;
    let small = read_allocs(n);
    let big = read_allocs(2 * n);
    let extra = big - small;
    eprintln!("scale_ledger_read_allocs: {n} events -> {small}, {} -> {big}, {extra} for {n} more", 2 * n);
    assert!(extra <= READ_PER_EVENTS, "read of {n} more events costs {extra} allocations, over the bound {READ_PER_EVENTS}");
}

/// The tasks of the fixture's change, every one of them covering every tracked
/// file, so a check matches each path against each task.
const TASKS: usize = 6;

fn git(repo: &Path, args: &[&str]) {
    let ok = Command::new("git")
        .args(args)
        .current_dir(repo)
        .env("GIT_AUTHOR_NAME", "t")
        .env("GIT_AUTHOR_EMAIL", "t@t")
        .env("GIT_COMMITTER_NAME", "t")
        .env("GIT_COMMITTER_EMAIL", "t@t")
        .status()
        .unwrap()
        .success();
    assert!(ok, "git {args:?}");
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

fn tasks() -> String {
    let mut text = String::new();
    for i in 0..TASKS {
        text.push_str(&format!(
            "- TSK-{} Task {i}\n  covers: REQ-new1\n  verify: test -f src/f00000.rs\n  files: src/**\n",
            id_body(900 + i)
        ));
    }
    text
}

/// A repository of `n` tracked files and a change whose `TASKS` tasks all cover
/// them, removed on drop: one `check` reads that listing.
struct Fx {
    _tmp: TempDir,
    repo: PathBuf,
    project: Project,
    change: String,
}

impl Fx {
    fn new(n: usize) -> Fx {
        let tmp = TempDir::new();
        let repo = tmp.0.join("repo");
        std::fs::create_dir_all(repo.join("src")).unwrap();
        git(&repo, &["init", "-q", "-b", "main"]);
        for i in 0..n {
            std::fs::write(repo.join(format!("src/f{i:05}.rs")), "x").unwrap();
        }
        git(&repo, &["add", "-A"]);
        git(&repo, &["commit", "-q", "-m", "init"]);
        let home = tmp.0.join("home");
        std::fs::create_dir_all(&home).unwrap();
        let project = rvow_repo::store::init(&repo, &home, Mode::Home).unwrap();
        let change = rvow_plan::change::new(&project, "T", "main", true).unwrap();
        std::fs::write(change.path.join(rvow_plan::change::TASKS), tasks()).unwrap();
        Fx { _tmp: tmp, repo, project, change: change.id.to_string() }
    }

    /// One `rvow check` of this change.
    fn check(&self) -> usize {
        let reports = rvow_plan::plan::check_over(&self.project, &self.repo, Some(&self.change), &[]).unwrap();
        assert_eq!(reports.len(), 1);
        reports[0].findings.len()
    }
}

/// The allocations of one `rvow check` over a repository of `n` tracked files.
fn check_allocs(n: usize) -> usize {
    let fx = Fx::new(n);
    counted(|| {
        let _ = fx.check();
    })
}

/// One `check` matches every tracked path against every task, so its count must
/// not follow the path count: the same check cost 4814 allocations on 1000
/// paths and 8833 on 2000 before, and 647 and 650 after.
const CHECK_RATIO_NUM: usize = 6;
const CHECK_RATIO_DEN: usize = 5;
/// The allocations a fixed part of one `check` may add between the two sizes.
const CHECK_SLACK: usize = 32;

#[test]
fn scale_plan_check_allocs() {
    // The fixture's tasks all cover the listing, so every pass matches every path.
    let found = Fx::new(20).check();
    assert!(found > TASKS + TASKS * (TASKS - 1) / 2, "the fixture must draw a wide, a shared and a hot warning: {found} findings");
    let small = check_allocs(1000);
    let big = check_allocs(2000);
    eprintln!("scale_plan_check_allocs: 1000 paths -> {small}, 2000 -> {big}");
    let bound = small * CHECK_RATIO_NUM / CHECK_RATIO_DEN + CHECK_SLACK;
    assert!(big <= bound, "one check: 1000 paths cost {small} allocations, 2000 cost {big}, over the bound {bound}");
}
