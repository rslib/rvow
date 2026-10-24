//! TSK-bazvm7: `rvow daemon` serves hook calls on a Unix socket, one owner per
//! rvow home.
//!
//! Every test starts the real binary and talks HTTP/1.1 to its socket by hand,
//! so the wire the engine answers is the wire a hook client sees. The child
//! gets its own `RVOW_HOME`, `HOME` and `XDG_CONFIG_HOME`, so no variable of
//! this test process changes and the tests can run side by side.

use std::io::{BufRead, BufReader, Read, Write};
use std::os::unix::fs::FileTypeExt;
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, Output, Stdio};
use std::sync::mpsc::{Receiver, channel};
use std::time::{Duration, Instant};

use serde_json::{Value, json};

const RVOW: &str = env!("CARGO_BIN_EXE_rvow");
const WAIT: Duration = Duration::from_secs(5);

/// A repository with an rvow home beside it, and every child it started.
pub(crate) struct Fx {
    root: PathBuf,
    children: Vec<Child>,
    /// The stdin of the `--host-pipe` child: the answers of this test, the host.
    input: Option<ChildStdin>,
    /// The child's stdout lines, read by a thread so a missing line is a
    /// failed test and not a test that hangs.
    lines: Option<Receiver<Value>>,
}

impl Drop for Fx {
    fn drop(&mut self) {
        for child in &mut self.children {
            // Only a child that still runs is killed: a reaped pid can be another process.
            if child.try_wait().is_ok_and(|status| status.is_none()) {
                let _ = child.kill();
                let _ = child.wait();
            }
        }
        rvow_core::git::testrepo::remove_tree(&self.root);
    }
}

impl Fx {
    /// A repository with one commit, and an empty rvow home.
    pub(crate) fn new(name: &str) -> Fx {
        let root = std::env::temp_dir().join(format!("rvow-daemon-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("repo/src")).unwrap();
        std::fs::create_dir_all(root.join("home")).unwrap();
        let fx = Fx { root: root.canonicalize().unwrap(), children: Vec::new(), input: None, lines: None };
        std::fs::write(fx.repo().join("src/a.rs"), "x").unwrap();
        fx.git(&["init", "-q", "-b", "main"]);
        fx.git(&["add", "."]);
        fx.git(&["commit", "-q", "-m", "init"]);
        fx
    }

    /// Records this test process as a harness of the home, as the in-process
    /// session-start hook records the harness, so its hook calls are served.
    pub(crate) fn as_harness(&self) {
        rvow_daemon::trust::record(&self.home(), std::process::id()).unwrap();
    }

    /// `new`, plus one change of one task that waits in backlog/, not accepted.
    pub(crate) fn with_plan(name: &str) -> Fx {
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

    pub(crate) fn repo(&self) -> PathBuf {
        self.root.join("repo")
    }

    pub(crate) fn home(&self) -> PathBuf {
        self.root.join("rh")
    }

    /// The id of the only change of the store, from `rvow ls`.
    pub(crate) fn change(&self) -> String {
        let listed = self.rvow(&["ls", "--human"]);
        let id = listed.split_whitespace().next().unwrap_or_default();
        assert!(id.starts_with("CHG-"), "no change in {listed:?}");
        id.to_owned()
    }

    /// The folder of one change, from `rvow path`.
    pub(crate) fn change_dir(&self, change: &str) -> PathBuf {
        PathBuf::from(field(&self.rvow(&["path", change]), "path"))
    }

    /// Writes the plan of the only change: the tasks and the one spec delta.
    pub(crate) fn write_plan(&self, tasks: &str, delta: &str) {
        let dir = self.change_dir(&self.change());
        std::fs::write(dir.join("tasks.md"), tasks).unwrap();
        std::fs::write(dir.join("specs/out.md"), delta).unwrap();
    }

    /// `with_plan`, with the plan files that `rvow accept` takes: the task ids
    /// and the delta requirement are placeholders the vow mints.
    pub(crate) fn with_ready_plan(name: &str) -> Fx {
        let fx = Fx::with_plan(name);
        fx.write_plan(READY_TASKS, READY_DELTA);
        fx
    }

    /// Starts `rvow daemon --host-pipe` and holds both ends of its pipe: this
    /// test process is the host, so the child's stdout carries the questions
    /// and the child's stdin carries the answers.
    pub(crate) fn host(&mut self, ask_wait: &str) -> PathBuf {
        let mut c = Command::new(RVOW);
        self.env(&mut c);
        c.args(["daemon", "--host-pipe", "--ask-wait", ask_wait, "--idle", "30s"]);
        c.stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::inherit());
        let mut child = c.spawn().unwrap();
        self.input = Some(child.stdin.take().unwrap());
        let output = child.stdout.take().unwrap();
        self.children.push(child);
        let (sender, lines) = channel();
        std::thread::spawn(move || {
            let mut reader = BufReader::new(output);
            let mut line = String::new();
            while reader.read_line(&mut line).is_ok_and(|n| n > 0) {
                // A line that is not ours is not a question: skip it.
                if let Ok(value) = serde_json::from_str::<Value>(line.trim_end())
                    && sender.send(value).is_err()
                {
                    return;
                }
                line.clear();
            }
        });
        self.lines = Some(lines);
        let sock = self.sock();
        wait_for_socket(&sock);
        sock
    }

    /// Sends one line to the engine on the host pipe.
    pub(crate) fn tell(&mut self, line: Value) {
        let pipe = self.input.as_mut().expect("the host pipe is open");
        writeln!(pipe, "{line}").unwrap();
        pipe.flush().unwrap();
    }

    /// The next line the engine wrote to the host, as JSON.
    pub(crate) fn next_line(&mut self) -> Value {
        self.lines.as_ref().expect("the host pipe is open").recv_timeout(WAIT).expect("a line from the engine")
    }

    /// Closes this test's end of the pipe, as a host that quits does.
    pub(crate) fn close_pipe(&mut self) {
        self.input = None;
    }

    /// The socket the engine of this home listens on, as the wire types name it.
    pub(crate) fn sock(&self) -> PathBuf {
        rvow_proto::socket_path(&self.home())
    }

    fn env(&self, c: &mut Command) {
        c.current_dir(self.repo())
            .env("HOME", self.root.join("home"))
            .env("RVOW_HOME", self.home())
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

    /// Starts `rvow <args>` in the background and returns the index of the child.
    fn start(&mut self, args: &[&str]) -> usize {
        let mut c = Command::new(RVOW);
        self.env(&mut c);
        c.args(args).stdout(Stdio::piped()).stderr(Stdio::piped());
        self.children.push(c.spawn().unwrap());
        self.children.len() - 1
    }

    /// Runs `rvow <args>` to its end.
    fn run(&self, args: &[&str]) -> Output {
        let mut c = Command::new(RVOW);
        self.env(&mut c);
        c.args(args).output().unwrap()
    }

    pub(crate) fn rvow(&self, args: &[&str]) -> String {
        let out = self.run(args);
        assert!(out.status.success(), "rvow {args:?}: {}", String::from_utf8_lossy(&out.stdout));
        String::from_utf8(out.stdout).unwrap()
    }

    fn git(&self, args: &[&str]) {
        let mut c = Command::new("git");
        self.env(&mut c);
        assert!(c.args(args).status().unwrap().success(), "git {args:?}");
    }

    /// Whether child `at` exited within `within`.
    fn exited(&mut self, at: usize, within: Duration) -> bool {
        let deadline = Instant::now() + within;
        while Instant::now() < deadline {
            if self.children[at].try_wait().unwrap().is_some() {
                return true;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        false
    }

    /// Starts an engine and waits for its socket, so a call cannot race the bind.
    pub(crate) fn engine(&mut self) -> PathBuf {
        let sock = self.sock();
        self.start(&["daemon", "--idle", "30s", "--human"]);
        wait_for_socket(&sock);
        sock
    }

    /// One hook call through the CLI: the text `rvow hook` prints, no newline.
    fn hook(&self, call: &Value) -> String {
        let mut child = Command::new(RVOW);
        self.env(&mut child);
        let mut child = child
            .args(["hook", "claude", "pre-tool-use"])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        child.stdin.take().unwrap().write_all(call.to_string().as_bytes()).unwrap();
        let out = child.wait_with_output().unwrap();
        assert!(out.status.success(), "rvow hook: {}", String::from_utf8_lossy(&out.stderr));
        String::from_utf8(out.stdout).unwrap().trim_end().to_owned()
    }
}

fn field<'a>(out: &'a str, key: &str) -> &'a str {
    out.lines().find_map(|l| l.strip_prefix(&format!("{key}: "))).unwrap_or_else(|| panic!("no {key} in {out}"))
}

/// The tasks of a plan that `rvow accept` takes: one task that covers the
/// placeholder requirement of the delta below.
const READY_TASKS: &str = "- TSK-new1 Write a\n  covers: REQ-new1\n  verify: grep -q y src/a.rs && echo 'test result: ok. 1 passed; 0 failed;'\n  files: src/a.rs\n";

/// The one spec delta of that plan.
const READY_DELTA: &str = "## ADD REQ-new1 Out\nrvow SHALL write y.\n### Scenario: s\n- WHEN it runs\n- THEN a holds y\n";

/// Waits until the engine listens: the socket file exists and is a socket.
fn wait_for_socket(sock: &Path) {
    let deadline = Instant::now() + WAIT;
    while Instant::now() < deadline {
        if std::fs::metadata(sock).is_ok_and(|m| m.file_type().is_socket()) {
            return;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    panic!("no engine listening at {}", sock.display());
}

/// One hand-written request: the status line and the body.
pub(crate) fn http(sock: &Path, request: &[u8]) -> (String, String) {
    let mut stream = UnixStream::connect(sock).unwrap_or_else(|e| panic!("connect {}: {e}", sock.display()));
    stream.set_read_timeout(Some(WAIT)).unwrap();
    stream.write_all(request).unwrap();
    let mut text = String::new();
    stream.read_to_string(&mut text).unwrap();
    let (head, body) = text.split_once("\r\n\r\n").unwrap_or_else(|| panic!("no head in {text:?}"));
    (head.lines().next().unwrap().to_owned(), body.to_owned())
}

pub(crate) fn post(path: &str, body: &str) -> Vec<u8> {
    format!(
        "POST {path} HTTP/1.1\r\nhost: rvow\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
        body.len()
    )
    .into_bytes()
}

fn hello(env: &str) -> String {
    json!({"proto_major": 1, "proto_minor": 0, "build": "0.0.0", "env": env, "ui": "pane"}).to_string()
}

/// One call of `env` as the socket carries it, with `input` as the hook input.
pub(crate) fn request(env: &str, input: Value) -> String {
    json!({
        "hello": {"proto_major": 1, "proto_minor": 0, "build": "0.0.0", "env": env, "ui": "pane"},
        "call": {"harness": "claude", "event": "pre-tool-use", "input": input},
    })
    .to_string()
}

/// One ask of `env` as the socket carries it: what the harness wants the user
/// to decide, and where.
pub(crate) fn ask_request(env: &str, kind: &str, change: &str, cwd: &Path, session: Option<&str>) -> String {
    json!({
        "hello": {"proto_major": 1, "proto_minor": 0, "build": "0.0.0", "env": env, "ui": "pane"},
        "kind": kind,
        "change": change,
        "cwd": cwd.display().to_string(),
        "session": session,
    })
    .to_string()
}

/// One answer as the host pipe carries it.
pub(crate) fn answer_line(id: u64, value: &str, digest: &str) -> Value {
    json!({"type": "answer", "id": id, "value": value, "digest": digest})
}

/// One read-back of a plan part as the host pipe carries it.
pub(crate) fn get_line(change: &str, part: &str) -> Value {
    json!({"type": "get", "change": change, "part": part})
}

/// The hello reply of the engine at `sock`.
fn handshake(sock: &Path) -> Value {
    let (status, body) = http(sock, &post("/v1/hello", &hello("x")));
    assert_eq!(status, "HTTP/1.1 200 OK", "{body}");
    serde_json::from_str(&body).unwrap()
}

/// The environment hash of the engine at `sock`: the one its own hooks run in.
pub(crate) fn engine_env(sock: &Path) -> String {
    handshake(sock)["env"].as_str().unwrap().to_owned()
}

#[test]
fn daemon_second_start_names_the_first() {
    let mut fx = Fx::new("2nd");
    let sock = fx.engine();
    let pid = handshake(&sock)["pid"].as_u64().unwrap();

    let second = fx.run(&["daemon", "--idle", "30s", "--human"]);
    assert!(second.status.success(), "{}", String::from_utf8_lossy(&second.stderr));
    assert_eq!(String::from_utf8(second.stdout).unwrap().trim(), format!("an engine runs: pid {pid}"));

    // The first engine still answers.
    assert_eq!(handshake(&sock)["pid"].as_u64().unwrap(), pid);
}

#[test]
fn daemon_stale_socket_file_is_replaced() {
    let mut fx = Fx::new("stale");
    let sock = fx.sock();
    std::fs::create_dir_all(sock.parent().unwrap()).unwrap();
    std::fs::write(&sock, b"a socket of an engine that was killed").unwrap();

    fx.start(&["daemon", "--idle", "30s", "--human"]);
    wait_for_socket(&sock);
    assert_eq!(handshake(&sock)["proto_major"].as_u64().unwrap(), 1);
}

#[test]
fn daemon_the_65th_connection_is_refused() {
    let mut fx = Fx::new("busy");
    let sock = fx.engine();

    // These send nothing, so each holds one slot until its read timeout.
    let held: Vec<UnixStream> = (0..64).map(|_| UnixStream::connect(&sock).unwrap()).collect();
    // The engine takes them in a poll step or two.
    std::thread::sleep(Duration::from_millis(300));

    let (status, body) = http(&sock, &post("/v1/hello", &hello("x")));
    assert_eq!(status, "HTTP/1.1 503 Service Unavailable", "{body}");
    assert_eq!(body, "{\"error\":\"engine busy\"}");
    drop(held);
}

#[test]
fn daemon_a_request_after_the_connect_is_still_answered() {
    let mut fx = Fx::new("late");
    let sock = fx.engine();

    // The engine accepts long before the request arrives: a connection accepted
    // from a non-blocking listener must still wait for its request.
    let mut stream = UnixStream::connect(&sock).unwrap();
    stream.set_read_timeout(Some(WAIT)).unwrap();
    std::thread::sleep(Duration::from_millis(300));
    stream.write_all(&post("/v1/hello", &hello("x"))).unwrap();
    let mut text = String::new();
    stream.read_to_string(&mut text).unwrap();
    assert!(text.starts_with("HTTP/1.1 200 OK"), "{text}");
}

#[test]
fn daemon_hook_answers_like_rvow_hook() {
    let mut fx = Fx::with_plan("hook");
    let sock = fx.engine();
    fx.as_harness();
    let env = engine_env(&sock);
    let call = json!({
        "cwd": fx.repo(),
        "session_id": "s1",
        "tool_name": "Edit",
        "tool_input": {"file_path": fx.repo().join("src/a.rs"), "old_string": "x", "new_string": "y"},
    });

    let (status, body) = http(&sock, &post("/v1/hook", &request(&env, call.clone())));
    assert_eq!(status, "HTTP/1.1 200 OK", "{body}");
    let answer: Value = serde_json::from_str(&body).unwrap();
    assert!(answer["mismatch"].is_null(), "{body}");
    let answered = answer["reply"]["output"].as_str().unwrap();

    assert_eq!(answered, fx.hook(&call), "the engine answers as `rvow hook` does");
    let decision = serde_json::from_str::<Value>(answered).unwrap();
    assert_eq!(decision["hookSpecificOutput"]["permissionDecision"], "deny", "{answered}");
}

#[test]
fn daemon_mismatch_runs_nothing() {
    let mut fx = Fx::with_plan("mism");
    let sock = fx.engine();
    fx.as_harness();
    let call = json!({"cwd": fx.repo(), "tool_name": "Edit", "tool_input": {"file_path": fx.repo().join("src/a.rs")}});

    let (status, body) = http(&sock, &post("/v1/hook", &request("x", call)));
    assert_eq!(status, "HTTP/1.1 200 OK", "{body}");
    let answer: Value = serde_json::from_str(&body).unwrap();
    assert!(answer["reply"].is_null(), "another environment runs nothing: {body}");
    assert_eq!(answer["mismatch"]["env"].as_str().unwrap(), engine_env(&sock), "{body}");
}

#[test]
fn daemon_exits_when_idle() {
    let mut fx = Fx::new("idle");
    let sock = fx.sock();
    let at = fx.start(&["daemon", "--idle", "1s", "--human"]);
    wait_for_socket(&sock);

    assert!(fx.exited(at, WAIT), "an engine with no call for its idle window stops");
    assert!(!sock.exists(), "the engine removes its socket: {}", sock.display());
}

#[test]
fn daemon_bad_requests_get_errors() {
    let mut fx = Fx::new("bad");
    let sock = fx.engine();

    let (status, body) = http(&sock, b"GET /v1/hello HTTP/1.1\r\nhost: rvow\r\nconnection: close\r\n\r\n");
    assert_eq!(status, "HTTP/1.1 405 Method Not Allowed", "{body}");
    assert_eq!(body, "{\"error\":\"method not allowed\"}");

    let (status, body) = http(&sock, &post("/v1/nope", "{}"));
    assert_eq!(status, "HTTP/1.1 404 Not Found", "{body}");
    assert_eq!(body, "{\"error\":\"not found\"}");

    // A peer that may not call gets 403 before its length counts.
    let no_length = b"POST /v1/hook HTTP/1.1\r\nhost: rvow\r\nconnection: close\r\n\r\n";
    let (status, body) = http(&sock, no_length);
    assert_eq!(status, "HTTP/1.1 403 Forbidden", "{body}");
    fx.as_harness();
    let (status, body) = http(&sock, no_length);
    assert_eq!(status, "HTTP/1.1 411 Length Required", "{body}");
    assert_eq!(body, "{\"error\":\"content-length is required\"}");
}

#[test]
fn daemon_print_hello_names_the_socket_and_the_engine_env() {
    let mut fx = Fx::new("print-hello");
    let sock = fx.engine();
    let printed: Value = serde_json::from_str(fx.rvow(&["daemon", "--print-hello"]).trim()).unwrap();
    assert_eq!(printed["socket"].as_str().unwrap(), sock.to_str().unwrap(), "{printed}");
    assert_eq!(printed["hello"]["env"].as_str().unwrap(), engine_env(&sock), "the same environment hashes the same: {printed}");
    assert_eq!(printed["hello"]["proto_major"], 1, "{printed}");
}
