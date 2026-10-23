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
        let dir = std::env::temp_dir().join(format!("rvow-friction-{}-{name}", std::process::id()));
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
    (r, root, id)
}

fn amend_one(r: &Repo, root: &Path, id: &str) -> String {
    let dir =
        std::fs::read_dir(root.join("active")).unwrap().map(|e| e.unwrap().path()).find(|p| p.to_string_lossy().contains(id)).unwrap();
    let tasks = std::fs::read_to_string(dir.join("tasks.md")).unwrap();
    std::fs::write(dir.join("tasks.md"), tasks.replace("Write ok", "Write one")).unwrap();
    let amended = r.ok(&["amend", "--reason", "edit one"]);
    amended.split_whitespace().find(|w| w.starts_with("AMD-")).unwrap().trim_matches(|c: char| !c.is_alphanumeric() && c != '-').to_owned()
}

#[test]
fn decided_accepted_names_the_decision() {
    let (r, root, id) = accepted("decided-accepted");
    let amd = amend_one(&r, &root, &id);
    r.ok(&["confirm", &amd, "--accept"]);
    let (out, code) = r.rvow(&["confirm", "--accept", &amd]);
    assert_eq!(code, 1, "{out}");
    assert!(out.contains("error[decided]"), "{out}");
    assert!(out.contains(&format!("{amd} is already accepted")), "{out}");
}

#[test]
fn decided_rejected_names_the_rejection() {
    let (r, root, id) = accepted("decided-rejected");
    let amd = amend_one(&r, &root, &id);
    r.ok(&["confirm", &amd, "--reject"]);
    let (out, code) = r.rvow(&["confirm", "--accept", &amd]);
    assert_eq!(code, 1, "{out}");
    assert!(out.contains("error[decided]"), "{out}");
    assert!(out.contains(&format!("{amd} is already rejected")), "{out}");
}

#[test]
fn decided_unknown_keeps_amendment_code() {
    let (r, _root, _id) = accepted("decided-unknown");
    let (out, code) = r.rvow(&["confirm", "--accept", "AMD-zzzzzz"]);
    assert_eq!(code, 1, "{out}");
    assert!(out.contains("error[amendment]"), "{out}");
}
