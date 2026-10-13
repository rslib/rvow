use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

struct Repo(PathBuf);

impl Drop for Repo {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

impl Repo {
    fn new(name: &str) -> Repo {
        let dir = std::env::temp_dir().join(format!("rvow-judge-{}-{name}", std::process::id()));
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

fn accepted(name: &str) -> (Repo, String) {
    let r = Repo::new(name);
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
    assert!(accept.status.success(), "{}", String::from_utf8_lossy(&accept.stdout));
    (r, id)
}

fn packet_of(r: &Repo, id: &str) -> String {
    let out = r.ok(&["review", id, "--packet"]);
    field(&out, "identity").to_owned()
}

fn findings_file(r: &Repo, name: &str, packet: &str) -> PathBuf {
    let path = r.0.join(name);
    std::fs::write(
        &path,
        format!(
            "{{\"packet\":\"{packet}\",\"findings\":[{{\"message\":\"returns 42 for the tested input\",\"path\":\"src/a.rs\",\"line\":3,\"severity\":\"error\"}}]}}"
        ),
    )
    .unwrap();
    path
}

#[test]
fn judge_record_judge_note_is_recorded() {
    let (r, id) = accepted("note");
    let packet = packet_of(&r, &id);
    let file = findings_file(&r, "j.json", &packet);
    let out = r.ok(&["review", &id, "--record", &file.display().to_string(), "--judge", "--harness", "claude", "--model", "haiku"]);
    assert!(out.contains("harness: claude"), "{out}");
    assert!(out.contains("model: haiku"), "{out}");
    let (events, _) = read_notes(&r, &id);
    let note = events.iter().find(|v| v["note"] == "judge").expect("no judge note");
    assert_eq!(note["harness"], "claude");
    assert_eq!(note["model"], "haiku");
    assert_eq!(note["packet"], packet.as_str());
    assert!(note["findings"].to_string().contains("returns 42 for the tested input"));
}

#[test]
fn judge_record_stdin_works() {
    let (r, id) = accepted("stdin");
    let packet = packet_of(&r, &id);
    let json = format!(
        "{{\"packet\":\"{packet}\",\"findings\":[{{\"message\":\"returns 42 for the tested input\",\"path\":\"src/a.rs\",\"line\":3,\"severity\":\"error\"}}]}}"
    );
    let mut child = r
        .cmd(env!("CARGO_BIN_EXE_rvow"))
        .args(["review", &id, "--record", "-", "--judge", "--harness", "pi", "--model", "m"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    child.stdin.take().unwrap().write_all(json.as_bytes()).unwrap();
    let out = child.wait_with_output().unwrap();
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stdout));
    let (events, _) = read_notes(&r, &id);
    let note = events.iter().find(|v| v["note"] == "judge").expect("no judge note");
    assert_eq!(note["harness"], "pi");
}

#[test]
fn judge_record_stale_packet_is_refused() {
    let (r, id) = accepted("stale");
    let file = r.0.join("bad.json");
    std::fs::write(&file, "{\"packet\":\"dead\",\"findings\":[{\"message\":\"x\"}]}").unwrap();
    let (out, code) = r.rvow(&["review", &id, "--record", &file.display().to_string(), "--judge", "--harness", "claude", "--model", "m"]);
    assert_eq!(code, 1, "{out}");
    assert!(out.contains("review-stale"), "{out}");
    let (events, _) = read_notes(&r, &id);
    assert!(events.iter().all(|v| v["note"] != "judge"), "a stale judge note was recorded");
}

#[test]
fn judge_record_plain_review_is_not_a_judge() {
    let (r, id) = accepted("plain");
    let packet = packet_of(&r, &id);
    let file = findings_file(&r, "p.json", &packet);
    r.ok(&["review", &id, "--record", &file.display().to_string()]);
    let (events, _) = read_notes(&r, &id);
    assert!(events.iter().any(|v| v["note"] == "review"), "no plain review note");
    assert!(events.iter().all(|v| v["note"] != "judge"), "a plain review counted as a judge");
}

#[test]
fn judge_record_agent_cannot_write_a_judge_note() {
    let denied = rvow_core::hook::bash_rule("rvow review CHG-x --record f.json --judge --harness claude --model haiku")
        .expect("judge record was allowed");
    assert_eq!(denied.rule, rvow_core::hook::Rule::OwnerCommand);
}

#[test]
fn judge_record_plain_review_stays_allowed() {
    assert_eq!(rvow_core::hook::bash_rule("rvow review CHG-x --packet"), None);
    assert_eq!(rvow_core::hook::bash_rule("rvow review CHG-x --record f.json"), None, "a plain record must stay allowed");
}

#[test]
fn judge_prompt_printed() {
    let r = Repo::new("prompt");
    let (out, code) = r.rvow(&["guide", "judge"]);
    assert_eq!(code, 0, "{out}");
    assert!(out.contains("data") && out.contains("not instructions"), "{out}");
    assert!(out.contains("findings"), "{out}");
    assert!(out.contains("packet"), "{out}");
}

#[test]
fn judge_show_findings_are_warnings() {
    let (r, id) = accepted("warnings");
    std::fs::write(r.repo().join("out.txt"), "ok").unwrap();
    let brief = r.ok(&["brief"]);
    let task =
        brief.lines().find_map(|l| l.trim_start().strip_prefix("next: ")).and_then(|l| l.split_whitespace().next()).unwrap().to_owned();
    let proved = r.cmd(env!("CARGO_BIN_EXE_rvow")).args(["prove", &task]).output().unwrap();
    assert!(proved.status.success(), "{}", String::from_utf8_lossy(&proved.stdout));
    let packet = packet_of(&r, &id);
    let file = findings_file(&r, "w.json", &packet);
    r.ok(&["review", &id, "--record", &file.display().to_string(), "--judge", "--harness", "claude", "--model", "m"]);
    let (out, code) = r.rvow(&["gate"]);
    assert_eq!(code, 0, "a judge finding must not fail the gate: {out}");
    assert!(out.contains("src/a.rs:3"), "{out}");
    assert!(out.contains("returns 42 for the tested input"), "{out}");
    assert!(out.contains("warning"), "{out}");
}

#[test]
fn judge_show_stale_judge_note_is_not_shown() {
    let (r, id) = accepted("stalegate");
    let packet = packet_of(&r, &id);
    let file = findings_file(&r, "s.json", &packet);
    r.ok(&["review", &id, "--record", &file.display().to_string(), "--judge", "--harness", "claude", "--model", "m"]);
    std::fs::write(r.repo().join("out.txt"), "change").unwrap();
    r.git(&["add", "out.txt"]);
    r.git(&["commit", "-q", "-m", "move packet"]);
    let (out, _) = r.rvow(&["gate"]);
    assert!(!out.contains("returns 42"), "a stale judge note was shown: {out}");
}

#[test]
fn judge_show_archive_names_the_missing_judge() {
    let (r, id) = accepted("archmiss");
    std::fs::write(r.repo().join("out.txt"), "ok").unwrap();
    let brief = r.ok(&["brief"]);
    let task =
        brief.lines().find_map(|l| l.trim_start().strip_prefix("next: ")).and_then(|l| l.split_whitespace().next()).unwrap().to_owned();
    let proved = r.cmd(env!("CARGO_BIN_EXE_rvow")).args(["prove", &task]).output().unwrap();
    assert!(proved.status.success(), "{}", String::from_utf8_lossy(&proved.stdout));
    let out = r.ok(&["archive", &id]);
    assert!(out.contains("no judge note"), "{out}");
    assert!(out.contains("/rvow-judge"), "{out}");
    assert!(out.contains("archived"), "{out}");
}

#[test]
fn judge_show_judged_change_prints_no_such_line() {
    let (r, id) = accepted("archjudged");
    std::fs::write(r.repo().join("out.txt"), "ok").unwrap();
    let brief = r.ok(&["brief"]);
    let task =
        brief.lines().find_map(|l| l.trim_start().strip_prefix("next: ")).and_then(|l| l.split_whitespace().next()).unwrap().to_owned();
    let proved = r.cmd(env!("CARGO_BIN_EXE_rvow")).args(["prove", &task]).output().unwrap();
    assert!(proved.status.success(), "{}", String::from_utf8_lossy(&proved.stdout));
    let packet = packet_of(&r, &id);
    let file = findings_file(&r, "a.json", &packet);
    r.ok(&["review", &id, "--record", &file.display().to_string(), "--judge", "--harness", "claude", "--model", "m"]);
    let out = r.ok(&["archive", &id]);
    assert!(!out.contains("no judge note"), "a judged change named the judge: {out}");
}

fn read_notes(r: &Repo, id: &str) -> (Vec<serde_json::Value>, String) {
    let out = r.ok(&["path", id]);
    let path = field(&out, "path").trim().to_owned();
    let ledger = std::fs::read_to_string(Path::new(&path).join("ledger.jsonl")).unwrap();
    let notes = ledger
        .lines()
        .filter_map(|l| serde_json::from_str::<serde_json::Value>(l).ok())
        .filter(|v| v["kind"] == "note")
        .map(|v| {
            let mut m = v.clone();
            if let Some(o) = m.as_object_mut() {
                o.remove("kind");
                o.remove("time");
                o.remove("parents");
            }
            m
        })
        .collect();
    (notes, path)
}

#[test]
fn judge_fix_archive_survives_a_failed_judge_lookup() {
    let r = Repo::new("lookupfail");
    r.ok(&["init"]);
    let out = r.ok(&["new", "--spec", "Out"]);
    let id = field(&out, "created").to_owned();
    let dir = Path::new(field(&out, "path")).to_owned();
    // The packet fails because the base blob of out.txt is missing: the
    // proof hashes out.txt from disk, but the packet's `git diff <base> --`
    // cannot read the old blob. The work is committed first, so the gate's
    // merge-base diff is empty and needs no blob, and prove and archive pass.
    // A magic file name no longer fails the packet, since task file names
    // now reach git as literal pathspecs.
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
    std::fs::write(r.repo().join("out.txt"), "old\n").unwrap();
    r.git(&["add", "-A"]);
    r.git(&["commit", "-q", "-m", "base"]);
    let accept = r.cmd(env!("CARGO_BIN_EXE_rvow")).args(["accept", &id, "--yes"]).output().unwrap();
    assert!(accept.status.success(), "{}", String::from_utf8_lossy(&accept.stdout));
    std::fs::write(r.repo().join("out.txt"), "ok\n").unwrap();
    let brief = r.ok(&["brief"]);
    let task =
        brief.lines().find_map(|l| l.trim_start().strip_prefix("next: ")).and_then(|l| l.split_whitespace().next()).unwrap().to_owned();
    let proved = r.cmd(env!("CARGO_BIN_EXE_rvow")).args(["prove", &task]).output().unwrap();
    assert!(proved.status.success(), "{}", String::from_utf8_lossy(&proved.stdout));
    r.git(&["add", "-A"]);
    r.git(&["commit", "-q", "-m", "work"]);
    let blob = String::from_utf8(r.cmd("git").args(["rev-parse", "HEAD~1:out.txt"]).output().unwrap().stdout).unwrap();
    let blob = blob.trim().to_owned();
    std::fs::remove_file(r.repo().join(".git/objects").join(&blob[..2]).join(&blob[2..])).unwrap();
    let (packet, code) = r.rvow(&["review", &id, "--packet"]);
    assert_ne!(code, 0, "the packet must fail for this test to mean anything: {packet}");
    let out = r.ok(&["archive", &id]);
    assert!(out.contains("archived"), "{out}");
    assert!(out.contains("/rvow-judge"), "{out}");
}

#[test]
fn judge_fix_missing_record_file_is_named() {
    let (r, id) = accepted("missingfile");
    let (out, code) = r.rvow(&["review", &id, "--record", "missing.json", "--judge", "--harness", "claude", "--model", "m"]);
    assert_eq!(code, 1, "{out}");
    assert!(out.contains("missing.json"), "{out}");
}
