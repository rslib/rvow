//! Plans are read and written only through rvow: `rvow show` prints an outline
//! or one part, and `rvow put` replaces one part from stdin.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::Command;

struct Repo(PathBuf);

impl Drop for Repo {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

impl Repo {
    fn new(name: &str) -> Repo {
        let dir = std::env::temp_dir().join(format!("rvow-plan-access-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("repo")).unwrap();
        let r = Repo(dir.canonicalize().unwrap());
        r.git(&["init", "-q", "-b", "main"]);
        std::fs::write(r.repo().join("a.txt"), "a").unwrap();
        r.git(&["add", "a.txt"]);
        r.git(&["commit", "-q", "-m", "init"]);
        r
    }

    fn repo(&self) -> PathBuf {
        self.0.join("repo")
    }

    fn cmd(&self, program: &str) -> Command {
        let mut c = Command::new(program);
        c.current_dir(self.repo())
            .env("HOME", &self.0)
            .env("RVOW_HOME", self.0.join("home"))
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_AUTHOR_NAME", "t")
            .env("GIT_AUTHOR_EMAIL", "t@t")
            .env("GIT_COMMITTER_NAME", "t")
            .env("GIT_COMMITTER_EMAIL", "t@t");
        c
    }

    fn git(&self, args: &[&str]) {
        assert!(self.cmd("git").args(args).status().unwrap().success());
    }

    /// Runs rvow with stdout on a pipe, as an agent does: stdout and exit code.
    fn rvow(&self, args: &[&str]) -> (String, i32) {
        let out = self.cmd(env!("CARGO_BIN_EXE_rvow")).args(args).output().unwrap();
        assert_eq!(String::from_utf8_lossy(&out.stderr), "", "rvow {args:?}");
        (String::from_utf8(out.stdout).unwrap(), out.status.code().unwrap())
    }

    /// Runs rvow with stdin and `--human`, so the output is plain text.
    fn pipe(&self, args: &[&str], stdin: &str) -> (String, i32) {
        let mut child = self
            .cmd(env!("CARGO_BIN_EXE_rvow"))
            .args(args)
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .spawn()
            .unwrap();
        child.stdin.take().unwrap().write_all(stdin.as_bytes()).unwrap();
        let out = child.wait_with_output().unwrap();
        assert_eq!(String::from_utf8_lossy(&out.stderr), "", "rvow {args:?}");
        (String::from_utf8(out.stdout).unwrap(), out.status.code().unwrap())
    }

    fn ok(&self, args: &[&str]) -> String {
        let (out, code) = self.rvow(args);
        assert_eq!(code, 0, "rvow {args:?}: {out}");
        out
    }
}

fn field<'a>(out: &'a str, key: &str) -> &'a str {
    out.lines().find_map(|l| l.strip_prefix(&format!("{key}: "))).unwrap_or_else(|| panic!("no {key} in {out}"))
}

const PROPOSAL: &str = "\
branch: main

# Two tasks

## Why
Something is too slow.

## What changes
- Show one part.
- Replace one part.

## Out of scope
- Everything else.

## Assumptions
- Parts are small.
";

const TASKS: &str = "\
# Tasks

- TSK-new1 Do one
  covers: REQ-new1
  verify: cargo test --locked plan_access::plan_access_show_
  files: one.txt

- TSK-new2 Do two
  covers: REQ-new1
  verify: cargo test --locked plan_access::plan_access_show_
  files: two.txt
";

const SPEC: &str = "\
## ADD REQ-new1 Out

rvow SHALL keep the outline.

### Scenario: outline
- WHEN it runs
- THEN the outline is kept
";

/// An initialized project with a backlog change that holds a proposal, one spec
/// and two tasks. Returns the repo, the rvow home, the change id and its folder.
fn change(name: &str) -> (Repo, PathBuf, String, PathBuf) {
    let r = Repo::new(name);
    let root = PathBuf::from(field(&r.ok(&["init"]), "created"));
    let out = r.ok(&["new", "--spec", "Out"]);
    let id = field(&out, "created").to_owned();
    let dir = Path::new(field(&out, "path")).to_owned();
    std::fs::write(dir.join("proposal.md"), PROPOSAL).unwrap();
    std::fs::write(dir.join("tasks.md"), TASKS).unwrap();
    std::fs::write(dir.join("specs/out.md"), SPEC).unwrap();
    (r, root, id, dir)
}

#[test]
fn plan_access_show_outline_names_sections_requirements_and_tasks() {
    let (r, root, id, dir) = change("outline");
    let out = r.ok(&["show", &id]);
    assert!(out.contains("Why (4 words)"), "{out}");
    assert!(out.contains("What changes (8 words)"), "{out}");
    assert!(out.contains("Out of scope (3 words)"), "{out}");
    assert!(out.contains("Assumptions (4 words)"), "{out}");
    assert!(out.contains("REQ-new1 Out"), "{out}");
    assert!(out.contains("TSK-new1"), "{out}");
    assert!(out.contains("TSK-new2"), "{out}");
    assert!(out.contains("Do two"), "{out}");
    assert!(out.contains("missing"), "{out}");
    assert!(!out.contains(&dir.display().to_string()), "the outline holds no store path: {out}");
    assert!(!out.contains(&root.display().to_string()), "the outline holds no store path: {out}");
}

#[test]
fn plan_access_show_prints_one_part() {
    let (r, _root, id, _dir) = change("parts");
    let (proposal, code) = r.pipe(&["--human", "show", &id, "proposal"], "");
    assert_eq!((code, proposal.as_str()), (0, PROPOSAL));
    let (tasks, code) = r.pipe(&["--human", "show", &id, "tasks"], "");
    assert_eq!((code, tasks.as_str()), (0, TASKS));
    let (spec, code) = r.pipe(&["--human", "show", &id, "spec:out"], "");
    assert_eq!((code, spec.as_str()), (0, SPEC));
}

#[test]
fn plan_access_show_unknown_part_exits_two_and_lists_the_parts() {
    let (r, _root, id, _dir) = change("unknown");
    let (out, code) = r.rvow(&["show", &id, "notes"]);
    assert_eq!(code, 2, "{out}");
    assert!(out.contains("notes"), "{out}");
    assert!(out.contains("proposal"), "{out}");
    assert!(out.contains("tasks"), "{out}");
    assert!(out.contains("spec:out"), "{out}");
}

#[test]
fn plan_access_put_replaces_a_task_block() {
    let (r, _root, _id, dir) = change("put-task");
    let block = "- TSK-new1 Do one differently\n  covers: REQ-new1\n  verify: cargo test --locked plan_access::plan_access_show_\n  files: one.txt, more.txt\n";
    let (out, code) = r.pipe(&["--human", "put", "TSK-new1"], block);
    assert_eq!(code, 0, "{out}");
    assert_eq!(out.trim(), "ok", "a clean change prints ok");
    let tasks = std::fs::read_to_string(dir.join("tasks.md")).unwrap();
    assert!(tasks.contains("Do one differently"), "{tasks}");
    assert!(tasks.contains("files: one.txt, more.txt"), "{tasks}");
    assert!(tasks.contains("- TSK-new2 Do two"), "the other task is unchanged: {tasks}");
    assert!(tasks.contains("verify: cargo test --locked"), "the other task is unchanged: {tasks}");
    // A block with a chained verify prints the check findings instead of ok.
    let chained = "- TSK-new1 Do one differently\n  covers: REQ-new1\n  verify: test -f one.txt && echo 'test result: ok. 1 passed; 0 failed;'\n  files: one.txt\n";
    let (out, code) = r.pipe(&["--human", "put", "TSK-new1"], chained);
    assert_eq!(code, 0, "{out}");
    assert!(out.contains("warning[full-suite]"), "findings, not ok: {out}");
    // A block for another id is refused and writes nothing.
    let before = std::fs::read_to_string(dir.join("tasks.md")).unwrap();
    let (out, code) = r.pipe(&["--human", "put", "TSK-new2"], block);
    assert_eq!(code, 1, "{out}");
    assert_eq!(std::fs::read_to_string(dir.join("tasks.md")).unwrap(), before, "nothing was written");
}

#[test]
fn plan_access_put_replaces_a_part() {
    let (r, _root, id, dir) = change("put-part");
    let proposal = "branch: main\n\n# Other title\n\n## Why\nIt is better.\n\n## What changes\n- One thing.\n\n## Out of scope\n- The rest.\n\n## Assumptions\n- None.\n";
    let (out, code) = r.pipe(&["--human", "put", &id, "proposal"], proposal);
    assert_eq!(code, 0, "{out}");
    assert_eq!(std::fs::read_to_string(dir.join("proposal.md")).unwrap(), proposal);
    let (out, code) = r.pipe(&["--human", "put", &id, "spec:out"], "## ADD REQ-new1 Out\n\nrvow SHALL keep the outline whole.\n");
    assert_eq!(code, 0, "{out}");
    assert!(std::fs::read_to_string(dir.join("specs/out.md")).unwrap().contains("outline whole"));
    let (out, code) = r.rvow(&["put", &id, "notes"]);
    assert_eq!(code, 2, "{out}");
    assert!(out.contains("one of the parts"), "{out}");
}

#[test]
fn plan_access_put_replaces_a_requirement_block() {
    let (r, _root, _id, dir) = change("put-req");
    let block = "## ADD REQ-new1 Out in full\n\nrvow SHALL keep the whole outline.\n\n### Scenario: whole\n- WHEN it runs\n- THEN the whole outline is kept\n";
    let (out, code) = r.pipe(&["--human", "put", "REQ-new1"], block);
    assert_eq!(code, 0, "{out}");
    let spec = std::fs::read_to_string(dir.join("specs/out.md")).unwrap();
    assert!(spec.starts_with("## ADD REQ-new1 Out in full"), "{spec}");
    assert!(spec.contains("- THEN the whole outline is kept"), "{spec}");
}

/// A second backlog change in the same project, with the same tasks and spec as `change`.
fn second_change(r: &Repo) -> (String, PathBuf) {
    let out = r.ok(&["new", "--spec", "Out"]);
    let id = field(&out, "created").to_owned();
    let dir = Path::new(field(&out, "path")).to_owned();
    std::fs::write(dir.join("proposal.md"), PROPOSAL).unwrap();
    std::fs::write(dir.join("tasks.md"), TASKS).unwrap();
    std::fs::write(dir.join("specs/out.md"), SPEC).unwrap();
    (id, dir)
}

#[test]
fn plan_access_put_refuses_a_shared_task_id() {
    let (r, _root, id, dir) = change("put-shared-task");
    let (id2, dir2) = second_change(&r);
    let before = (std::fs::read(dir.join("tasks.md")).unwrap(), std::fs::read(dir2.join("tasks.md")).unwrap());
    let block = "- TSK-new1 Do one differently\n  covers: REQ-new1\n  verify: cargo test --locked plan_access::plan_access_show_\n  files: one.txt\n";
    let (out, code) = r.pipe(&["--human", "put", "TSK-new1"], block);
    assert_eq!(code, 1, "{out}");
    assert!(out.contains(&id) && out.contains(&id2), "the output names both changes: {out}");
    assert!(out.contains("ambiguous"), "{out}");
    assert_eq!(
        (std::fs::read(dir.join("tasks.md")).unwrap(), std::fs::read(dir2.join("tasks.md")).unwrap()),
        before,
        "nothing was written"
    );
}

#[test]
fn plan_access_put_refuses_a_shared_requirement_id() {
    let (r, _root, id, dir) = change("put-shared-req");
    let (id2, dir2) = second_change(&r);
    let before = (std::fs::read(dir.join("specs/out.md")).unwrap(), std::fs::read(dir2.join("specs/out.md")).unwrap());
    let block = "## ADD REQ-new1 Out in full\n\nrvow SHALL keep the whole outline.\n\n### Scenario: whole\n- WHEN it runs\n- THEN the whole outline is kept\n";
    let (out, code) = r.pipe(&["--human", "put", "REQ-new1"], block);
    assert_eq!(code, 1, "{out}");
    assert!(out.contains(&id) && out.contains(&id2), "the output names both changes: {out}");
    assert!(out.contains("ambiguous"), "{out}");
    assert_eq!(
        (std::fs::read(dir.join("specs/out.md")).unwrap(), std::fs::read(dir2.join("specs/out.md")).unwrap()),
        before,
        "nothing was written"
    );
}

fn hook_denial(r: &Repo, project: &rvow_core::store::Project, tool: &str, input: serde_json::Value) -> Option<rvow_core::hook::Denial> {
    let payload = serde_json::json!({"cwd": r.repo(), "tool_name": tool, "tool_input": input});
    rvow_core::hook::pre_tool_use_denial(project, &payload).unwrap()
}

#[test]
fn plan_access_hook_denies_find_from_the_root() {
    let (r, root, _id, _dir) = change("hook-find");
    let project = rvow_core::store::find(&r.repo(), &root).unwrap().unwrap();
    let found = hook_denial(&r, &project, "Bash", serde_json::json!({"command": "find / -name 'CHG-*'"}));
    let d = found.expect("the search is denied");
    assert!(d.reason.contains("rvow ls") && d.reason.contains("rvow show"), "{}", d.reason);
}

#[test]
fn plan_access_hook_denies_reading_a_proposal_by_path() {
    let (r, root, id, dir) = change("hook-read");
    let project = rvow_core::store::find(&r.repo(), &root).unwrap().unwrap();
    let found = hook_denial(&r, &project, "Read", serde_json::json!({"file_path": dir.join("proposal.md")}));
    let d = found.expect("a read of a plan file is denied");
    assert!(d.reason.contains("rvow show"), "{}", d.reason);
    assert!(d.reason.contains("proposal"), "{}", d.reason);
    assert!(d.reason.contains(&id), "{}", d.reason);
    // The same file through a Bash read, and the living specs, are denied too.
    let found = hook_denial(&r, &project, "Bash", serde_json::json!({"command": format!("cat {}", dir.join("proposal.md").display())}));
    assert!(found.is_some(), "a bash read of a plan file is denied");
    let found = hook_denial(&r, &project, "Read", serde_json::json!({"file_path": root.join("specs/out/spec.md")}));
    assert!(found.is_some(), "a read of a living spec is denied");
    // A read in the repository is still allowed.
    assert!(hook_denial(&r, &project, "Read", serde_json::json!({"file_path": r.repo().join("a.txt")})).is_none());
}

#[test]
fn plan_access_hook_denies_writing_a_plan_file_with_put_in_the_reason() {
    let (r, root, _id, dir) = change("hook-write");
    let project = rvow_core::store::find(&r.repo(), &root).unwrap().unwrap();
    let found = hook_denial(&r, &project, "Edit", serde_json::json!({"file_path": dir.join("tasks.md"), "new_string": "x"}));
    let d = found.expect("a write of a plan file is denied");
    assert!(d.reason.contains("rvow put"), "{}", d.reason);
    let found = hook_denial(&r, &project, "Bash", serde_json::json!({"command": format!("echo x > {}", dir.join("tasks.md").display())}));
    assert!(found.is_some(), "a bash write of a plan file is denied");
    // The operational files keep their own rules: the stats file stays open to reads.
    let found = hook_denial(&r, &project, "Read", serde_json::json!({"file_path": root.join("stats.jsonl")}));
    assert!(found.is_none(), "the stats file stays readable");
}

#[test]
fn plan_access_put_refuses_an_archived_change() {
    let (r, _root, id, _dir) = change("put-archived");
    r.ok(&["drop", &id, "--reason", "superseded by a better plan"]);
    let dir = PathBuf::from(r.ok(&["path", &id]).lines().find_map(|l| l.trim_start().strip_prefix("path: ")).unwrap());
    let before = std::fs::read_to_string(dir.join("proposal.md")).unwrap();
    let (out, code) = r.pipe(&["--human", "put", &id, "proposal"], "nope\n");
    assert_eq!(code, 1, "{out}");
    assert_eq!(std::fs::read_to_string(dir.join("proposal.md")).unwrap(), before, "nothing was written");
    let (out, code) = r.pipe(&["--human", "put", "TSK-new1"], "- TSK-new1 X\n  verify: true\n  files: a\n");
    assert_eq!(code, 1, "{out}");
}

#[test]
fn plan_access_put_creates_a_new_spec_file() {
    let (r, _root, id, dir) = change("put-new-spec");
    let block = "## ADD REQ-new9 Demo\n\nrvow SHALL keep the demo.\n### Scenario: kept\n- WHEN it runs\n- THEN the demo is kept\n";
    let (out, code) = r.pipe(&["--human", "put", &id, "spec:demo"], block);
    assert_eq!(code, 0, "{out}");
    assert!(out.trim() == "ok" || out.contains('['), "{out}");
    let text = std::fs::read_to_string(dir.join("specs/demo.md")).unwrap();
    assert!(text.contains("rvow SHALL keep the demo."), "{text}");
    let shown = r.ok(&["show", &id]);
    assert!(shown.contains("spec:demo"), "{shown}");
}

const DESIGN: &str = "# Design\n\nUse one task per capability and prove each with a test.\n";

/// `rvow put <change> design` writes a new design.md and `rvow show` reads it
/// back and lists the part; a change without one gives a clear error.
#[test]
fn plan_access_design_part() {
    let (r, _root, id, dir) = change("design");
    let shown = r.ok(&["show", &id]);
    assert!(!shown.contains("design"), "no design part before the file exists: {shown}");
    // A missing design errors with a fix that names the put command.
    let (out, code) = r.rvow(&["show", &id, "design"]);
    assert_eq!(code, 2, "{out}");
    assert!(out.contains("design"), "{out}");
    assert!(out.contains(&format!("rvow put {id} design")), "{out}");
    // Put creates design.md when it does not exist.
    let (out, code) = r.pipe(&["--human", "put", &id, "design"], DESIGN);
    assert_eq!(code, 0, "{out}");
    assert_eq!(std::fs::read_to_string(dir.join("design.md")).unwrap(), DESIGN);
    // Show reads it back and lists the part.
    let (out, code) = r.pipe(&["--human", "show", &id, "design"], "");
    assert_eq!((code, out.as_str()), (0, DESIGN));
    let shown = r.ok(&["show", &id]);
    assert!(shown.contains("parts: proposal, tasks, design"), "{shown}");
    // A second change without a design still errors.
    let (id2, _dir2) = second_change(&r);
    let (out, code) = r.rvow(&["show", &id2, "design"]);
    assert_eq!(code, 2, "{out}");
    assert!(out.contains(&format!("rvow put {id2} design")), "{out}");
}

#[test]
fn plan_access_put_refuses_a_bad_spec_name() {
    let (r, _root, id, dir) = change("put-bad-spec");
    let before = std::fs::read_to_string(dir.join("proposal.md")).unwrap();
    let (out, code) = r.rvow(&["put", &id, "spec:Bad/name"]);
    assert_eq!(code, 2, "{out}");
    assert!(out.contains("code: part"), "{out}");
    assert!(out.contains("[a-z0-9][a-z0-9-]*"), "{out}");
    let (out, code) = r.rvow(&["put", &id, "spec:Demo"]);
    assert_eq!(code, 2, "{out}");
    assert_eq!(std::fs::read_to_string(dir.join("proposal.md")).unwrap(), before, "nothing was written");
}

const GATED_VERIFY: &str = "grep -q ok out.txt && echo 'test result: ok. 1 passed; 0 failed;'";

/// An accepted change with one pending amendment that widens the files of a
/// task. Returns the repo, the change id and the amendment id.
fn amended(name: &str, reason: &str) -> (Repo, String, String) {
    let (r, root, id, dir) = change(name);
    let one = format!("- TSK-new1 Do one\n  covers: REQ-new1\n  verify: {GATED_VERIFY}\n  files: out.txt\n");
    std::fs::write(dir.join("tasks.md"), &one).unwrap();
    let accept = r.cmd(env!("CARGO_BIN_EXE_rvow")).args(["accept", &id, "--yes"]).output().unwrap();
    assert!(accept.status.success(), "{}", String::from_utf8_lossy(&accept.stdout));
    let active = std::fs::read_dir(root.join("active")).unwrap().next().unwrap().unwrap().path();
    let accepted = std::fs::read_to_string(active.join("tasks.md")).unwrap();
    std::fs::write(active.join("tasks.md"), accepted.replace("files: out.txt", "files: out.txt, extra.txt")).unwrap();
    let out = r.ok(&["amend", "--reason", reason]);
    let amd = out
        .split_whitespace()
        .map(|w| w.trim_matches(|c: char| !c.is_alphanumeric() && c != '-'))
        .find(|w| w.starts_with("AMD-"))
        .unwrap()
        .to_owned();
    (r, id, amd)
}

#[test]
fn plan_access_show_amendment_pending() {
    let (r, id, amd) = amended("amd-pending", "widen the files of one");
    let out = r.ok(&["show", &amd]);
    assert!(out.contains(&amd), "{out}");
    assert!(out.contains(&id), "{out}");
    assert!(out.contains("widen the files of one"), "{out}");
    assert!(out.contains("pending"), "{out}");
    assert!(out.contains("scope made wider"), "the items are named: {out}");
}

#[test]
fn plan_access_show_amendment_accepted() {
    let (r, id, amd) = amended("amd-accepted", "widen the files of one");
    r.ok(&["confirm", &amd, "--accept"]);
    let out = r.ok(&["show", &amd]);
    assert!(out.contains(&amd) && out.contains(&id), "{out}");
    assert!(out.contains("widen the files of one"), "{out}");
    assert!(out.contains("accepted"), "{out}");
    assert!(!out.contains("pending"), "{out}");
}

#[test]
fn plan_access_show_amendment_rejected() {
    let (r, _id, amd) = amended("amd-rejected", "widen the files of one");
    r.ok(&["confirm", &amd, "--reject"]);
    let out = r.ok(&["show", &amd]);
    assert!(out.contains("widen the files of one"), "{out}");
    assert!(out.contains("rejected"), "{out}");
}

#[test]
fn plan_access_show_amendment_unknown() {
    let (r, _id, _amd) = amended("amd-unknown", "widen the files of one");
    let (out, code) = r.rvow(&["show", "AMD-zzzzzz"]);
    assert_eq!(code, 1, "{out}");
    assert!(out.contains("no id matches"), "{out}");
}
