//! TSK-brd6wx: a hook call answers in process, and once its answer is printed it
//! records the harness at a session start and starts the engine of its home when
//! none runs.
//!
//! Every child gets its own `RVOW_HOME`, `HOME` and `XDG_CONFIG_HOME`, and its
//! own `RVOW_ENGINE` (the suite sets `off` for everything it starts), so no
//! variable of this test process changes. An engine a hook starts is not a child
//! of this process: its pid comes from the hello reply and the fixture Drop
//! kills it.

use std::io::Write;
use std::os::unix::fs::FileTypeExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use serde_json::{Value, json};

use crate::daemon::{http, post};

const RVOW: &str = env!("CARGO_BIN_EXE_rvow");
/// How long a started engine has to answer on its socket.
const WAIT: Duration = Duration::from_secs(5);
/// The stamp that keeps two hooks of one home from starting two engines.
const STAMP: &str = "engine.start";
/// The harness list a session start writes.
const HARNESSES: &str = "harnesses";

/// A repository with an rvow home beside it, and the engines its hooks started.
struct Fx {
    root: PathBuf,
    /// The pids of the engines started here, killed when the test ends.
    engines: Vec<u32>,
}

impl Drop for Fx {
    fn drop(&mut self) {
        self.engines.sort_unstable();
        self.engines.dedup();
        for pid in self.engines.drain(..) {
            // SAFETY: one signal to one pid, and no memory of ours is touched.
            unsafe { libc::kill(pid as libc::pid_t, libc::SIGTERM) };
        }
        rvow_core::git::testrepo::remove_tree(&self.root);
    }
}

impl Fx {
    /// A repository with one commit, and an empty rvow home.
    fn new(name: &str) -> Fx {
        let root = std::env::temp_dir().join(format!("rvow-fb-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("repo/src")).unwrap();
        std::fs::create_dir_all(root.join("home")).unwrap();
        let fx = Fx { root: root.canonicalize().unwrap(), engines: Vec::new() };
        std::fs::write(fx.repo().join("src/a.rs"), "x").unwrap();
        fx.git(&["init", "-q", "-b", "main"]);
        fx.git(&["add", "."]);
        fx.git(&["commit", "-q", "-m", "init"]);
        fx
    }

    /// `new`, plus one change of one task that waits in backlog/, not accepted,
    /// so an edit of its file is denied and the answer is not empty.
    fn with_plan(name: &str) -> Fx {
        let fx = Fx::new(name);
        fx.rvow(&["init"]);
        let out = fx.rvow(&["new", "--spec", "Out"]);
        let dir = PathBuf::from(field(&out, "path"));
        std::fs::write(
            dir.join("tasks.md"),
            "- TSK-new1 Write a\n  covers: REQ-new1\n  verify: grep -q y src/a.rs && echo 'test result: ok. 1 passed; 0 failed;'\n  files: src/a.rs\n",
        )
        .unwrap();
        std::fs::write(
            dir.join("specs/out.md"),
            "## ADD req1 Out\nrvow SHALL write y.\n### Scenario: s\n- WHEN it runs\n- THEN a holds y\n",
        )
        .unwrap();
        fx
    }

    fn repo(&self) -> PathBuf {
        self.root.join("repo")
    }

    fn home(&self) -> PathBuf {
        self.root.join("rh")
    }

    /// The socket the engine of this home listens on, as the wire types name it.
    fn sock(&self) -> PathBuf {
        rvow_proto::socket_path(&self.home())
    }

    /// The stamp a hook writes before it starts an engine.
    fn stamp(&self) -> PathBuf {
        self.home().join(rvow_proto::RUN_DIR).join(STAMP)
    }

    /// The harness list a session start records.
    fn harnesses(&self) -> PathBuf {
        self.home().join(rvow_proto::RUN_DIR).join(HARNESSES)
    }

    /// The env every child of the fixture gets. `RVOW_ENGINE` is off for all of
    /// them, and only a hook call that asks for an engine turns it back on.
    fn env(&self, c: &mut Command) {
        c.current_dir(self.repo())
            .env("HOME", self.root.join("home"))
            .env("RVOW_HOME", self.home())
            .env("RVOW_ENGINE", "off")
            .env("XDG_CONFIG_HOME", self.root.join("config"))
            .env_remove("XDG_DATA_HOME")
            .env_remove("CLAUDE_CONFIG_DIR")
            .env_remove("PI_CODING_AGENT_DIR")
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_AUTHOR_NAME", "t")
            .env("GIT_AUTHOR_EMAIL", "t@t")
            .env("GIT_COMMITTER_NAME", "t")
            .env("GIT_COMMITTER_EMAIL", "t@t");
    }

    fn rvow(&self, args: &[&str]) -> String {
        let mut c = Command::new(RVOW);
        self.env(&mut c);
        let out = c.args(args).output().unwrap();
        assert!(out.status.success(), "rvow {args:?}: {}", String::from_utf8_lossy(&out.stdout));
        String::from_utf8(out.stdout).unwrap()
    }

    fn git(&self, args: &[&str]) {
        let mut c = Command::new("git");
        self.env(&mut c);
        assert!(c.args(args).status().unwrap().success(), "git {args:?}");
    }

    /// One `rvow hook claude <event>` call with `RVOW_ENGINE` set as given, and
    /// the text it printed without its newline.
    fn hook(&self, event: &str, input: &Value, engine: &str) -> String {
        let mut c = Command::new(RVOW);
        self.env(&mut c);
        c.env("RVOW_ENGINE", engine);
        let mut child =
            c.args(["hook", "claude", event]).stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped()).spawn().unwrap();
        child.stdin.take().unwrap().write_all(input.to_string().as_bytes()).unwrap();
        let out = child.wait_with_output().unwrap();
        assert!(out.status.success(), "rvow hook claude {event} with RVOW_ENGINE={engine}: {}", String::from_utf8_lossy(&out.stderr));
        String::from_utf8(out.stdout).unwrap().trim_end().to_owned()
    }

    /// The call of an Edit of src/a.rs by the session `s1`.
    fn edit(&self) -> Value {
        json!({
            "cwd": self.repo(),
            "session_id": "s1",
            "tool_name": "Edit",
            "tool_input": {"file_path": self.repo().join("src/a.rs"), "old_string": "x", "new_string": "y"},
        })
    }
}

fn field<'a>(out: &'a str, key: &str) -> &'a str {
    out.lines().find_map(|l| l.strip_prefix(&format!("{key}: "))).unwrap_or_else(|| panic!("no {key} in {out}"))
}

/// The handshake body: what a client posts to ask an engine what it is.
fn hello() -> String {
    json!({"proto_major": 1, "proto_minor": 0, "build": "0.0.0", "env": "x", "ui": "pane"}).to_string()
}

/// The pid of the engine that answers on `fx`'s socket, recorded so the drop
/// kills it.
fn engine_pid(fx: &mut Fx) -> u32 {
    let sock = fx.sock();
    let (status, body) = http(&sock, &post("/v1/hello", &hello()));
    assert_eq!(status, "HTTP/1.1 200 OK", "{body}");
    let pid = u32::try_from(serde_json::from_str::<Value>(&body).unwrap()["pid"].as_u64().unwrap()).unwrap();
    fx.engines.push(pid);
    pid
}

/// Waits until an engine answers on `fx`'s socket within `WAIT`, and returns its
/// pid.
fn started(fx: &mut Fx) -> u32 {
    let sock = fx.sock();
    let deadline = Instant::now() + WAIT;
    while Instant::now() < deadline {
        if std::fs::metadata(&sock).is_ok_and(|m| m.file_type().is_socket()) {
            return engine_pid(fx);
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    panic!("no engine answered at {}", sock.display());
}

/// When the stamp was last written.
fn stamped(path: &Path) -> std::time::SystemTime {
    std::fs::metadata(path).and_then(|meta| meta.modified()).unwrap_or_else(|e| panic!("no stamp at {}: {e}", path.display()))
}

#[test]
fn daemon_fallback_no_engine_answers_and_starts_one() {
    let mut fx = Fx::with_plan("start");
    let call = fx.edit();

    // With no engine, the call is still answered, in process.
    let off = fx.hook("pre-tool-use", &call, "off");
    assert!(off.contains("\"permissionDecision\":\"deny\""), "the call is answered: {off}");
    assert!(!fx.stamp().exists() && !fx.sock().exists(), "off starts nothing");

    let on = fx.hook("pre-tool-use", &call, "on");
    assert_eq!(on, off, "starting an engine never changes the answer");
    assert!(fx.stamp().exists(), "on writes the stamp before it starts an engine");
    let pid = started(&mut fx);
    assert!(pid > 0, "an engine answers on the socket of the home");
}

#[test]
fn daemon_fallback_two_calls_start_one_engine() {
    let mut fx = Fx::with_plan("twice");
    let call = fx.edit();

    fx.hook("pre-tool-use", &call, "on");
    let at = stamped(&fx.stamp());
    std::thread::sleep(Duration::from_secs(1));
    fx.hook("pre-tool-use", &call, "on");

    let pid = started(&mut fx);
    assert_eq!(stamped(&fx.stamp()), at, "a second call does not write the stamp again");
    std::thread::sleep(Duration::from_millis(200));
    assert_eq!(engine_pid(&mut fx), pid, "one engine answers for the home");
}

#[test]
fn daemon_fallback_off_starts_nothing() {
    let fx = Fx::with_plan("off");
    let call = fx.edit();

    let answer = fx.hook("pre-tool-use", &call, "off");
    assert!(!answer.is_empty(), "the call is answered in process");
    std::thread::sleep(Duration::from_secs(1));
    assert!(!fx.sock().exists(), "off writes no socket: {}", fx.sock().display());
    assert!(!fx.stamp().exists(), "off writes no stamp: {}", fx.stamp().display());
}

#[test]
fn daemon_fallback_session_start_records_the_harness() {
    let fx = Fx::with_plan("sess");
    fx.hook("session-start", &json!({"cwd": fx.repo(), "session_id": "s1"}), "off");

    let text = std::fs::read_to_string(fx.harnesses()).expect("a session start records the harness");
    let mine = std::process::id().to_string();
    assert!(text.lines().any(|l| l.split_once(' ').is_some_and(|(pid, _)| pid == mine)), "the list must name this process: {text}");
    assert!(!fx.sock().exists(), "off starts no engine");
}

#[test]
fn daemon_fallback_a_stale_socket_file_starts_an_engine() {
    let mut fx = Fx::with_plan("stale");
    let sock = fx.sock();
    std::fs::create_dir_all(sock.parent().unwrap()).unwrap();
    std::fs::write(&sock, b"a socket of an engine that was killed").unwrap();

    fx.hook("pre-tool-use", &fx.edit(), "on");
    let pid = started(&mut fx);
    assert!(pid > 0, "the engine that replaced the file answers");
    assert!(std::fs::metadata(&sock).is_ok_and(|m| m.file_type().is_socket()), "the file is a socket again");
}
