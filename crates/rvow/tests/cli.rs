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
        let out = self
            .cmd(env!("CARGO_BIN_EXE_rvow"))
            .args(args)
            .output()
            .unwrap();
        assert_eq!(String::from_utf8_lossy(&out.stderr), "", "rvow {args:?}");
        (
            String::from_utf8(out.stdout).unwrap(),
            out.status.code().unwrap(),
        )
    }

    fn ok(&self, args: &[&str]) -> String {
        let (out, code) = self.rvow(args);
        assert_eq!(code, 0, "rvow {args:?}: {out}");
        out
    }
}

fn field<'a>(out: &'a str, key: &str) -> &'a str {
    out.lines()
        .find_map(|l| l.strip_prefix(&format!("{key}: ")))
        .unwrap_or_else(|| panic!("no {key} in {out}"))
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
    let accept = r
        .cmd(env!("CARGO_BIN_EXE_rvow"))
        .args(["accept", &id, "--yes"])
        .output()
        .unwrap();
    let text = String::from_utf8(accept.stdout).unwrap();
    assert!(accept.status.success(), "{text}");
    assert!(text.contains("before you accept, check:"), "{text}");
    assert!(text.contains(&format!("accepted {id} at ")), "{text}");
    assert!(!text.contains("help["), "{text}");
    (r, root, id)
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
    assert!(
        r.ok(&["status"])
            .starts_with("changes: 0 active on branch main\n")
    );
    assert!(
        r.ok(&["spec", "list"])
            .starts_with("requirements: 0 in the living specs\n")
    );
    assert!(
        r.ok(&[])
            .contains("\nchanges: 0 active or waiting on branch main\n")
    );
    assert_eq!(r.ok(&["--human", "ls"]), "no changes\n");
}

#[test]
fn lists_and_views_print_toon() {
    let (r, root, id) = accepted("lists");
    let out = r.ok(&["ls"]);
    assert!(
        out.starts_with(&format!(
            "count: 1\nchanges[1]{{id,stage,branch,title}}:\n  {id},active,main,Out\nhelp[2]:\n"
        )),
        "{out}"
    );
    let out = r.ok(&["status"]);
    assert!(
        out.contains(&format!(
            "changes[1]:\n  - id: {id}\n    branch: main\n    tasks[1]{{id,state,title}}:\n"
        )),
        "{out}"
    );
    assert!(out.contains(",missing,Write ok\n"), "{out}");
    let out = r.ok(&[]);
    assert!(out.contains("\nnext:\n  task: TSK-"), "{out}");
    assert!(out.contains("\nneeds_you:\n  amendments: 0\n"), "{out}");
    assert!(
        out.contains(&format!("{id},Out,active,0/1 proved")),
        "{out}"
    );

    std::fs::create_dir_all(root.join("specs/ledger")).unwrap();
    std::fs::write(
        root.join("specs/ledger/spec.md"),
        "# Ledger\n\n## REQ-k3f9m2 Record a proof run\n\nWHEN a task runs, rvow SHALL record one event.\n\n### Scenario: one run\n- WHEN it runs\n- THEN one event exists\n",
    )
    .unwrap();
    let out = r.ok(&["spec", "list"]);
    assert!(
        out.starts_with("count: 1 requirements in 1 capabilities\nrequirements[1]{id,capability,title}:\n  REQ-k3f9m2,ledger,Record a proof run\n"),
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
        "error: this repository has no rvow tree\nhelp: run rvow init\n"
    );
    let (out, code) = r.rvow(&["--human", "ls"]);
    assert_eq!(code, 1);
    assert!(out.starts_with("error[no-project]: "), "{out}");

    let (out, code) = r.rvow(&["status", "--bogus"]);
    assert_eq!(code, 2);
    assert!(
        out.starts_with("error: unexpected argument '--bogus' found\nhelp[2]:\n"),
        "{out}"
    );
    assert!(
        out.contains("valid flags for `rvow status`: --all, --json;"),
        "{out}"
    );
    let (out, code) = r.rvow(&["--human", "accept"]);
    assert_eq!(code, 2);
    assert!(
        out.contains("valid flags for `rvow accept`: --yes;"),
        "{out}"
    );
    let (out, code) = r.rvow(&["spec"]);
    assert_eq!(code, 2);
    assert!(
        out.contains("valid subcommands for `rvow spec`: list\n"),
        "{out}"
    );
    assert!(r.ok(&["ls", "--help"]).contains("Usage: rvow ls"));
}
