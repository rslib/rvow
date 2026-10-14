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
        let dir = std::env::temp_dir().join(format!("rvow-cli-{}-{name}", std::process::id()));
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

    fn ok(&self, args: &[&str]) -> String {
        let (out, code) = self.rvow(args);
        assert_eq!(code, 0, "rvow {args:?}: {out}");
        out
    }
}

fn field<'a>(out: &'a str, key: &str) -> &'a str {
    out.lines().find_map(|l| l.strip_prefix(&format!("{key}: "))).unwrap_or_else(|| panic!("no {key} in {out}"))
}

/// An initialized project with an accepted change of one task.
fn accepted(name: &str) -> (Repo, PathBuf, String) {
    let r = Repo::new(name);
    let root = PathBuf::from(field(&r.ok(&["init"]), "created"));
    let out = r.ok(&["new", "--spec", "Out"]);
    let id = field(&out, "created").to_owned();
    let dir = Path::new(field(&out, "path")).to_owned();
    std::fs::write(
        dir.join("tasks.md"),
        "- TSK-new1 Write ok\n  covers: REQ-new1\n  verify: grep -q ok out.txt && echo 'test result: ok. 1 passed; 0 failed;'\n  files: out.txt\n",
    )
    .unwrap();
    std::fs::write(
        dir.join("specs/out.md"),
        "## ADD REQ-new1 Out\nrvow SHALL write ok.\n### Scenario: s\n- WHEN it runs\n- THEN out.txt holds ok\n",
    )
    .unwrap();
    let accept = r.cmd(env!("CARGO_BIN_EXE_rvow")).args(["accept", &id, "--yes"]).output().unwrap();
    let text = String::from_utf8(accept.stdout).unwrap();
    assert!(accept.status.success(), "{text}");
    assert!(text.contains(&format!("accepted {id} at ")), "{text}");
    assert!(!text.contains("help["), "{text}");
    (r, root, id)
}

#[test]
fn accept_yes_prints_no_checklist() {
    let r = Repo::new("yeschecklist");
    r.ok(&["init"]);
    let out = r.ok(&["new", "--spec", "Out"]);
    let id = field(&out, "created").to_owned();
    let dir = Path::new(field(&out, "path")).to_owned();
    std::fs::write(
        dir.join("tasks.md"),
        "- TSK-new1 Write ok\n  covers: REQ-new1\n  verify: grep -q ok out.txt && echo 'test result: ok. 1 passed; 0 failed;'\n  files: out.txt\n",
    )
    .unwrap();
    std::fs::write(
        dir.join("specs/out.md"),
        "## ADD REQ-new1 Out\nrvow SHALL write ok.\n### Scenario: s\n- WHEN it runs\n- THEN out.txt holds ok\n",
    )
    .unwrap();
    let accept = r.cmd(env!("CARGO_BIN_EXE_rvow")).args(["accept", &id, "--yes"]).output().unwrap();
    let out = String::from_utf8(accept.stdout).unwrap();
    assert!(accept.status.success(), "{out}");
    assert!(out.contains(&format!("accepted {id} at ")), "{out}");
    assert!(!out.contains("before you accept, check:"), "{out}");
}

#[test]
fn confirm_ids_list_only_those() {
    let (r, root, id) = accepted("confirmids");
    let dir =
        std::fs::read_dir(root.join("active")).unwrap().map(|e| e.unwrap().path()).find(|p| p.to_string_lossy().contains(&id)).unwrap();
    let tasks = std::fs::read_to_string(dir.join("tasks.md")).unwrap();
    for (i, title) in ["Write one", "Write two"].iter().enumerate() {
        std::fs::write(dir.join("tasks.md"), tasks.replace("Write ok", title)).unwrap();
        r.ok(&["amend", "--reason", &format!("edit {i}")]);
    }
    let all = r.ok(&["confirm"]);
    let ids: Vec<&str> = all.lines().filter_map(|l| l.split(" for ").next().filter(|s| s.starts_with("AMD-"))).collect();
    assert_eq!(ids.len(), 2, "{all}");
    let one = r.ok(&["confirm", ids[0]]);
    assert!(one.contains(ids[0]), "{one}");
    assert!(!one.contains(ids[1]), "{one}");
    let (out, code) = r.rvow(&["confirm", "AMD-nope"]);
    assert_eq!(code, 1, "{out}");
}

#[test]
fn confirm_shows_changed_lines_only() {
    let (r, root, id) = accepted("confirmlines");
    let dir =
        std::fs::read_dir(root.join("active")).unwrap().map(|e| e.unwrap().path()).find(|p| p.to_string_lossy().contains(&id)).unwrap();
    let tasks = std::fs::read_to_string(dir.join("tasks.md")).unwrap();
    std::fs::write(dir.join("tasks.md"), tasks.replace("echo 'test", "echo 'tests")).unwrap();
    r.ok(&["amend", "--reason", "edit"]);
    let out = r.ok(&["confirm"]);
    assert!(out.contains("\n    - "), "{out}");
    assert!(out.contains("\n    + "), "{out}");
    assert!(!out.contains("before:") && !out.contains("after:"), "{out}");
    assert!(!out.contains("    - - TSK-new1 Write ok"), "{out}");
}

#[test]
fn accept_records_and_review_runs_nothing() {
    let r = Repo::new("reviewaccept");
    r.ok(&["init"]);
    let out = r.ok(&["new", "--spec", "Out"]);
    let id = field(&out, "created").to_owned();
    let dir = Path::new(field(&out, "path")).to_owned();
    std::fs::write(
        dir.join("tasks.md"),
        "- TSK-new1 Write ok\n  covers: REQ-new1\n  verify: touch ran.txt && grep -q ok out.txt && echo 'test result: ok. 1 passed; 0 failed;'\n  files: out.txt\n",
    )
    .unwrap();
    std::fs::write(
        dir.join("specs/out.md"),
        "## ADD REQ-new1 Out\nrvow SHALL write ok.\n### Scenario: s\n- WHEN it runs\n- THEN out.txt holds ok\n",
    )
    .unwrap();
    let review = r.ok(&["review", &id]);
    assert!(review.contains("before you accept, check:"), "{review}");
    assert!(review.contains("Write ok"), "{review}");
    assert!(!r.repo().join("ran.txt").exists(), "review ran a verify command");
    let out = r.cmd(env!("CARGO_BIN_EXE_rvow")).args(["accept", &id]).output().unwrap();
    let accept = String::from_utf8(out.stdout).unwrap();
    assert!(out.status.success(), "{accept}");
    assert!(accept.contains(&format!("accepted {id} at ")), "{accept}");
    assert!(!accept.contains("before you accept, check:"), "{accept}");
}

#[test]
fn accept_output_is_short() {
    let r = Repo::new("shortaccept");
    r.ok(&["init"]);
    let out = r.ok(&["new", "Small"]);
    let id2 = field(&out, "created").to_owned();
    let dir = Path::new(field(&out, "path")).to_owned();
    std::fs::write(
        dir.join("tasks.md"),
        "- TSK-new1 One\n  verify: grep -q ok out.txt && echo 'test result: ok. 1 passed; 0 failed;'\n  files: out.txt\n\n- TSK-new2 Two\n  verify: grep -q ok out.txt && echo 'test result: ok. 1 passed; 0 failed;'\n  files: out.txt\n",
    )
    .unwrap();
    let accept = r.cmd(env!("CARGO_BIN_EXE_rvow")).args(["accept", &id2]).output().unwrap();
    let text = String::from_utf8(accept.stdout).unwrap();
    assert!(accept.status.success(), "{text}");
    assert_eq!(text.lines().count(), 2, "{text}");
    assert!(text.contains("\nnext: TSK-"), "{text}");
    assert!(text.contains(&format!("accepted {id2} at ")), "{text}");
    assert!(text.contains(": 2 tasks, base "), "{text}");
}

#[test]
fn decisions_end_with_next() {
    let (r, root, id) = accepted("nextline");
    let brief = r.ok(&["brief"]);
    assert!(brief.contains("next: TSK-"), "{brief}");
    let dir =
        std::fs::read_dir(root.join("active")).unwrap().map(|e| e.unwrap().path()).find(|p| p.to_string_lossy().contains(&id)).unwrap();
    let tasks = std::fs::read_to_string(dir.join("tasks.md")).unwrap();
    for (title, flag, want) in
        [("Write one", "--accept", "next: the plan edit stands"), ("Write two", "--reject", "next: undo the rejected plan edit")]
    {
        std::fs::write(dir.join("tasks.md"), tasks.replace("Write ok", title)).unwrap();
        let amended = r.ok(&["amend", "--reason", "edit"]);
        let amd = amended
            .split_whitespace()
            .find(|w| w.starts_with("AMD-"))
            .unwrap()
            .trim_matches(|c: char| !c.is_alphanumeric() && c != '-')
            .to_owned();
        let out = r.ok(&["confirm", &amd, flag]);
        assert!(out.contains(want), "{out}");
    }
    std::fs::write(dir.join("tasks.md"), tasks.replace("Write ok", "Write one")).unwrap();
    std::fs::write(r.repo().join("out.txt"), "ok").unwrap();
    let task =
        brief.lines().find_map(|l| l.trim_start().strip_prefix("next: ")).and_then(|l| l.split_whitespace().next()).unwrap().to_owned();
    let proved = r.cmd(env!("CARGO_BIN_EXE_rvow")).args(["prove", &task]).output().unwrap();
    assert!(proved.status.success());
    let out = r.ok(&["archive", &id]);
    assert!(out.contains("next: pick the next plan"), "{out}");
}

#[test]
fn stale_digest_prints_the_list() {
    let (r, root, id) = accepted("staledigest");
    let dir =
        std::fs::read_dir(root.join("active")).unwrap().map(|e| e.unwrap().path()).find(|p| p.to_string_lossy().contains(&id)).unwrap();
    let tasks = std::fs::read_to_string(dir.join("tasks.md")).unwrap();
    std::fs::write(dir.join("tasks.md"), tasks.replace("Write ok", "Write one")).unwrap();
    r.ok(&["amend", "--reason", "one"]);
    let listed = r.ok(&["confirm"]);
    let digest = listed.lines().find_map(|l| l.strip_prefix("digest ")).unwrap().to_owned();
    std::fs::write(dir.join("tasks.md"), tasks.replace("Write ok", "Write two")).unwrap();
    r.ok(&["amend", "--reason", "two"]);
    let (out, code) = r.rvow(&["confirm", "--accept", "--all", &digest]);
    assert_eq!(code, 1, "{out}");
    assert_eq!(out.matches(" for CHG-").count(), 2, "{out}");
    assert!(out.contains("digest "), "{out}");
    assert!(out.contains("changed since it was listed"), "{out}");
}

#[test]
fn archive_prints_next_last() {
    let (r, _root, id) = accepted("archivelast");
    let brief = r.ok(&["brief"]);
    let task =
        brief.lines().find_map(|l| l.trim_start().strip_prefix("next: ")).and_then(|l| l.split_whitespace().next()).unwrap().to_owned();
    std::fs::write(r.repo().join("out.txt"), "ok").unwrap();
    let proved = r.cmd(env!("CARGO_BIN_EXE_rvow")).args(["prove", &task]).output().unwrap();
    assert!(proved.status.success());
    let out = r.ok(&["archive", &id]);
    let at = |key: &str| out.find(key).unwrap_or_else(|| panic!("no {key} in {out}"));
    assert!(at("archived:") < at("merged"), "{out}");
    assert!(at("merged") < at("next:"), "{out}");
}

#[test]
fn init_hints_git_init_for_an_untracked_home() {
    let r = Repo::new("inithint");
    let out = r.ok(&["init"]);
    let home = r.0.join("home");
    assert!(out.contains(&format!("git -C {} init", home.display())), "{out}");
    assert!(home.join(".gitignore").exists());

    let q = Repo::new("inithintrepo");
    let out = q.ok(&["init", "--in-repo"]);
    assert!(!out.contains("git -C"), "{out}");
    let ignore = std::fs::read_to_string(q.repo().join(".gitignore")).unwrap();
    assert!(ignore.lines().any(|l| l == "rvow/.lock"), "{ignore}");
}

#[test]
fn version_is_bare() {
    let r = Repo::new("version");
    for flag in ["--version", "-V", "-v"] {
        assert_eq!(r.ok(&[flag]), format!("{}\n", env!("CARGO_PKG_VERSION")));
    }
}

#[test]
fn home_view_without_a_project() {
    let r = Repo::new("home-none");
    let out = r.ok(&[]);
    assert!(out.starts_with("bin: "), "{out}");
    assert!(out.contains("\ndescription: Hold a coding agent to an accepted plan.\n"));
    assert!(out.contains("\nstate: no rvow project in this repository\n"));
    assert!(out.contains("Run `rvow init`"));
    assert!(r.ok(&["--human"]).starts_with("rvow: Hold a coding agent"));
}

#[test]
fn empty_states_name_the_zero() {
    let r = Repo::new("empty");
    r.ok(&["init"]);
    assert!(r.ok(&["ls"]).starts_with("changes: 0 in this project\n"));
    assert!(r.ok(&["status"]).starts_with("changes: 0 active on branch main\n"));
    assert!(r.ok(&["spec", "list"]).starts_with("requirements: 0 in the living specs\n"));
    assert!(r.ok(&[]).contains("\nchanges: 0 active or waiting on branch main\n"));
    assert_eq!(r.ok(&["--human", "ls"]), "no changes\n");
}

#[test]
fn lists_and_views_print_toon() {
    let (r, root, id) = accepted("lists");
    let out = r.ok(&["ls"]);
    assert!(out.starts_with(&format!("count: 1\nchanges[1]{{id,stage,branch,title}}:\n  {id},active,main,Out\nhelp[2]:\n")), "{out}");
    let out = r.ok(&["status"]);
    assert!(out.contains(&format!("changes[1]:\n  - id: {id}\n    branch: main\n    tasks[1]{{id,state,title}}:\n")), "{out}");
    assert!(out.contains(",missing,Write ok\n"), "{out}");
    let out = r.ok(&[]);
    assert!(out.contains("\nnext:\n  task: TSK-"), "{out}");
    assert!(out.contains("\nneeds_you:\n  amendments: 0\n"), "{out}");
    assert!(out.contains(&format!("{id},Out,active,0/1 proved")), "{out}");

    std::fs::create_dir_all(root.join("specs/ledger")).unwrap();
    std::fs::write(
        root.join("specs/ledger/spec.md"),
        "# Ledger\n\n## REQ-k3f9m2 Record a proof run\n\nWHEN a task runs, rvow SHALL record one event.\n\n### Scenario: one run\n- WHEN it runs\n- THEN one event exists\n",
    )
    .unwrap();
    let out = r.ok(&["spec", "list"]);
    assert!(
        out.starts_with(
            "count: 1 requirements in 1 capabilities\nrequirements[1]{id,capability,title}:\n  REQ-k3f9m2,ledger,Record a proof run\n"
        ),
        "{out}"
    );
    assert_eq!(r.ok(&["confirm"]), "no pending amendments\n");
}

#[test]
fn errors_are_records_on_stdout() {
    let r = Repo::new("errors");
    let (out, code) = r.rvow(&["ls"]);
    assert_eq!(code, 1);
    assert_eq!(
        out,
        "error: this repository has no rvow tree\ncode: no-project\nhelp: \"run rvow init; it writes to the rvow home, not the repository\"\n"
    );
    let (out, code) = r.rvow(&["--human", "ls"]);
    assert_eq!(code, 1);
    assert!(out.starts_with("error[no-project]: "), "{out}");

    let (out, code) = r.rvow(&["status", "--bogus"]);
    assert_eq!(code, 2);
    assert!(out.starts_with("error: unexpected argument '--bogus' found\ncode: usage\nhelp[2]:\n"), "{out}");
    assert!(out.contains("valid flags for `rvow status`: --all, --json;"), "{out}");
    let (out, code) = r.rvow(&["--human", "accept"]);
    assert_eq!(code, 2);
    assert!(out.contains("valid flags for `rvow accept`: --yes;"), "{out}");
    let (out, code) = r.rvow(&["spec"]);
    assert_eq!(code, 2);
    assert!(out.contains("valid subcommands for `rvow spec`: list\n"), "{out}");
    assert!(r.ok(&["ls", "--help"]).contains("Usage: rvow ls"));
}

#[test]
fn ingest_round_trip_keeps_the_proofs() {
    let (r, _root, id) = accepted("ingestrt");
    std::fs::write(r.repo().join("out.txt"), "ok").unwrap();
    r.git(&["add", "out.txt"]);
    r.git(&["commit", "-q", "-m", "ok"]);

    // gate streams the rerun to stderr, so drive it directly and check the exit
    // code; the observable result is the run file it writes.
    let gate = r.cmd(env!("CARGO_BIN_EXE_rvow")).args(["gate", "--strict", "--rerun", "--out", "run.jsonl"]).output().unwrap();
    assert!(gate.status.success(), "gate: {gate:?}");

    let file = std::fs::read_to_string(r.repo().join("run.jsonl")).unwrap();
    let line = file.lines().next().expect("gate --out wrote no runs");
    let run: serde_json::Value = serde_json::from_str(line).unwrap();
    assert_eq!(run["proved"], serde_json::json!(true));
    assert_eq!(run["tests_run"], serde_json::json!(1));
    assert_eq!(run["change"], serde_json::json!(id));
    let task = run["task"].as_str().unwrap().to_string();

    let ingested = r.ok(&["ingest", "run.jsonl"]);
    assert!(ingested.contains("count: 1") && ingested.contains("skipped: 0"), "{ingested}");

    let shown = r.ok(&["show", &task]);
    assert!(shown.contains("proved, 1 tests"), "{shown}");
}

#[test]
fn review_shows_each_check() {
    let r = Repo::new("reviewcheck");
    r.ok(&["init"]);
    let out = r.ok(&["new", "--spec", "Out"]);
    let id = field(&out, "created").to_owned();
    let dir = Path::new(field(&out, "path")).to_owned();
    std::fs::write(
        dir.join("tasks.md"),
        "- TSK-new1 Write tail\n  covers: REQ-new1\n  verify: cargo test tail\n  files: src/tail.rs\n  baseline: pass\n  mutants: cargo mutants --in-diff d.diff\n",
    )
    .unwrap();
    std::fs::write(
        dir.join("specs/out.md"),
        "## ADD REQ-new1 Out\n\nWHEN a run ends, rvow SHALL keep the tail.\n\n### Scenario: tail\n- WHEN a run ends\n- THEN the tail is kept\n",
    )
    .unwrap();
    let review = r.ok(&["review", &id]);
    assert!(review.contains("verify: cargo test tail"), "{review}");
    assert!(review.contains("files: src/tail.rs"), "{review}");
    assert!(review.contains("baseline: pass"), "{review}");
    assert!(review.contains("mutants: cargo mutants --in-diff d.diff"), "{review}");

    std::fs::write(dir.join("tasks.md"), "- TSK-new1 Write tail\n  covers: REQ-new1\n  verify: cargo test tail\n  files: src/tail.rs\n")
        .unwrap();
    let review = r.ok(&["review", &id]);
    assert!(!review.contains("baseline:"), "{review}");
    assert!(!review.contains("mutants:"), "{review}");
}

#[test]
fn check_warns_weak_verify() {
    let r = Repo::new("weakverify");
    r.ok(&["init"]);
    let out = r.ok(&["new", "--spec", "Out"]);
    let id = field(&out, "created").to_owned();
    let dir = Path::new(field(&out, "path")).to_owned();
    std::fs::write(dir.join("proposal.md"), "branch: main\n\n# Out\n\n## Why\nw\n\n## What changes\nw\n\n## Out of scope\nNone.\n")
        .unwrap();

    std::fs::write(
        dir.join("tasks.md"),
        "- TSK-new1 Trivial\n  verify: true\n  files: a\n- TSK-new2 Faked\n  verify: echo 'test result: ok. 5 passed'\n  files: a\n- TSK-new3 Combined\n  verify: test -f out.txt && cargo test out\n  files: a\n",
    )
    .unwrap();
    let checked = r.ok(&["check", &id]);
    assert!(checked.contains("weak-verify") && checked.contains("TSK-new1"), "{checked}");
    assert!(checked.contains("TSK-new2"), "{checked}");
    assert!(!checked.contains("TSK-new3"), "{checked}");

    std::fs::write(
        dir.join("tasks.md"),
        "- TSK-new4 Script\n  verify: bash check.sh\n  files: a\n  baseline: pass\n- TSK-new5 Runner\n  verify: cargo test --locked guard\n  files: a\n  baseline: pass\n- TSK-new6 NoBaseline\n  verify: bash check.sh\n  files: a\n",
    )
    .unwrap();
    let checked = r.ok(&["check", &id]);
    assert!(checked.contains("weak-verify") && checked.contains("TSK-new4"), "{checked}");
    assert!(!checked.contains("TSK-new5"), "{checked}");
    assert!(!checked.contains("TSK-new6"), "{checked}");

    std::fs::write(dir.join("tasks.md"), "- TSK-new7 Trivial\n  verify: true\n  files: a\n").unwrap();
    let checked = r.ok(&["--human", "check", &id]);
    assert!(checked.contains("weak-verify") && checked.contains("TSK-new7"), "{checked}");
    assert!(checked.contains("ready to accept"), "{checked}");
}

#[test]
fn review_lists_weak_checks() {
    let r = Repo::new("reviewweak");
    r.ok(&["init"]);
    let out = r.ok(&["new", "--spec", "Out"]);
    let id = field(&out, "created").to_owned();
    let dir = Path::new(field(&out, "path")).to_owned();

    std::fs::write(dir.join("tasks.md"), "- TSK-new1 Trivial\n  verify: true\n  files: a\n").unwrap();
    let review = r.ok(&["review", &id]);
    let warning = review.find("warning[weak-verify]").expect("no weak-check warning in review");
    assert!(review[warning..].contains("TSK-new1"), "{review}");
    let checklist = review.find("before you accept, check:").expect("no checklist in review");
    assert!(warning < checklist, "the weak-check warning must come before the checklist:\n{review}");

    std::fs::write(dir.join("tasks.md"), "- TSK-new1 Real\n  verify: cargo test tail\n  files: a\n").unwrap();
    let review = r.ok(&["review", &id]);
    assert!(!review.contains("weak-verify"), "{review}");
}

#[test]
fn show_change_keeps_task_rows() {
    let r = Repo::new("showrows");
    r.ok(&["init"]);
    let out = r.ok(&["new", "--spec", "Out"]);
    let id = field(&out, "created").to_owned();
    let dir = Path::new(field(&out, "path")).to_owned();
    std::fs::write(
        dir.join("tasks.md"),
        "- TSK-new1 Write tail\n  covers: REQ-new1\n  verify: cargo test tail\n  files: src/tail.rs\n  baseline: pass\n  mutants: cargo mutants --in-diff d.diff\n- TSK-new2 Second\n  verify: cargo test second\n  files: src/second.rs\n",
    )
    .unwrap();
    let shown = r.ok(&["--human", "show", &id]);
    assert!(shown.lines().any(|l| l == "  TSK-new1 Write tail"), "{shown}");
    assert!(shown.lines().any(|l| l == "  TSK-new2 Second"), "{shown}");
    assert!(!shown.contains("verify:"), "{shown}");
}
