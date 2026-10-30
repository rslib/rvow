//! The engine socket: one process per rvow home serves the hook calls of every
//! harness in that home, so a call is a socket round trip and not a whole
//! process.
//!
//! One engine owns a home. The first process takes the lock beside the socket;
//! a later one reports the pid that holds it. The owner stops itself when no
//! call came for the idle window, so a home does not keep a process alive all
//! day.
//!
//! The wire is HTTP/1.1 over a Unix socket, one request per connection, so the
//! engine speaks what every language and every `curl` speaks. A request is read
//! into one buffer, its head is parsed as slices of that buffer, and its body
//! goes to the hook as the bytes the caller wrote.

use std::io::{ErrorKind, Read, Seek, SeekFrom, Write};
use std::os::fd::AsRawFd;
use std::os::unix::fs::{DirBuilderExt, MetadataExt, PermissionsExt};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use rvow_core::error::{Error, Result};
use rvow_proto::{AskRequest, Hello, HelloReply, HookReply, HookRequest, HookResponse, PROTO_MAJOR, PROTO_MINOR, socket_path};

use crate::ask::{self, Host};
use crate::context;
use crate::dispatch;
use crate::events;
use crate::trust::{self, Harnesses};

/// How many connections one engine serves at once; the next one gets a 503.
const MAX_CONNECTIONS: usize = 64;
/// Threads that stay up between calls, so their per-thread caches stay warm.
/// A call that finds none free gets a thread of its own, so a long ask never
/// holds up a hook.
const WARM_THREADS: usize = 4;
/// The biggest body the engine reads: a hook input is far smaller.
const MAX_BODY: usize = 8 * 1024 * 1024;
/// The biggest request head, headers included.
const MAX_HEAD: usize = 16 * 1024;
/// How long a connection has to send its request, and to take its answer.
const IO_TIMEOUT: Duration = Duration::from_secs(10);
/// The idle watcher's step, and how long the accept loop waits in `poll(2)`
/// before it looks at the stop flag again; a connection wakes it at once.
const POLL: Duration = Duration::from_millis(50);
/// How many bytes one read takes from the socket.
const CHUNK: usize = 8 * 1024;

/// The status lines the engine answers with.
const OK: &str = "200 OK";
const BAD: &str = "400 Bad Request";
const FORBIDDEN: &str = "403 Forbidden";
const NOT_FOUND: &str = "404 Not Found";
const METHOD: &str = "405 Method Not Allowed";
const MISMATCH: &str = "409 Conflict";
const LENGTH: &str = "411 Length Required";
const LARGE: &str = "413 Content Too Large";
const HEAD_TOO_LONG: &str = "431 Request Header Fields Too Large";
const SERVER: &str = "500 Internal Server Error";
const BUSY: &str = "503 Service Unavailable";

/// The error bodies, written whole so that a refusal allocates nothing.
const E_BAD: &[u8] = b"{\"error\":\"bad request\"}";
const E_METHOD: &[u8] = b"{\"error\":\"method not allowed\"}";
const E_PATH: &[u8] = b"{\"error\":\"not found\"}";
const E_LENGTH: &[u8] = b"{\"error\":\"content-length is required\"}";
const E_LARGE: &[u8] = b"{\"error\":\"body too large\"}";
const E_HEAD: &[u8] = b"{\"error\":\"head too large\"}";
const E_JSON: &[u8] = b"{\"error\":\"bad json\"}";
const E_PEER: &[u8] = b"{\"error\":\"peer may not call hooks\"}";
const E_HOOK: &[u8] = b"{\"error\":\"hook failed\"}";
const E_ASK: &[u8] = b"{\"error\":\"ask failed\"}";
const E_ANSWER: &[u8] = b"{\"error\":\"answers come only from the host pipe\"}";
const E_SERVER: &[u8] = b"{\"error\":\"server error\"}";
const E_BUSY: &[u8] = b"{\"error\":\"engine busy\"}";

/// How `serve` ended.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Served {
    /// Another engine already serves this home; `pid` is the process that does.
    Busy { pid: u32 },
    /// No call came for the idle window, so this engine stopped.
    Idle,
}

/// Serves the hook calls of `home` on its socket until no call came for `idle`.
///
/// `ask` turns on the host pipe: `Some(wait)` reads the host's answers on this
/// process's stdin and writes the questions on its stdout, and `wait` is how
/// long one socket ask waits for an answer before it reports `Waiting`.
///
/// Only a recorded harness process may send hook calls and asks (see
/// `peer_may_call_hooks`).
pub fn serve(home: &Path, idle: Duration, ask: Option<Duration>) -> Result<Served> {
    let sock = socket_path(home);
    if let Some(dir) = sock.parent() {
        private_dir(&sock, dir)?;
    }
    let lock_path = lock_path(&sock);
    let lock = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(&lock_path)
        .map_err(|e| Error::io(&lock_path, e))?;
    if !lock_exclusive(&lock) {
        return Ok(Served::Busy { pid: lock_pid(&lock_path) });
    }
    write_pid(&lock, &lock_path)?;
    // One git answer cache for the whole engine, as `rvow hook` has one per run.
    rvow_core::git::remember_answers();

    // The host pipe is set up before the socket exists, so the first ask of a
    // client that saw the socket finds its reader running.
    let host = match ask {
        Some(wait) => Some(open_host(home, wait)?),
        None => None,
    };

    let (listener, dev, ino) = bind(&sock)?;
    let state = Arc::new(State {
        home: home.to_path_buf(),
        harnesses: Harnesses::new(home),
        hello: HelloReply {
            proto_major: PROTO_MAJOR,
            proto_minor: PROTO_MINOR,
            build: env!("CARGO_PKG_VERSION").to_owned(),
            // Computed once: every call of this engine is answered in this environment.
            env: context::env_hash(),
            pid: std::process::id(),
        },
        host,
        start: Instant::now(),
        last: AtomicU64::new(0),
        stop: AtomicBool::new(false),
        active: AtomicUsize::new(0),
    });

    // The idle watcher is the only writer of `stop` before the loop below ends.
    let watcher = {
        let state = Arc::clone(&state);
        std::thread::spawn(move || {
            loop {
                std::thread::sleep(POLL);
                if state.stop.load(Ordering::Acquire) {
                    return;
                }
                if idle_reached(&state, idle) {
                    state.stop.store(true, Ordering::Release);
                    return;
                }
            }
        })
    };

    let warm = {
        let state = Arc::clone(&state);
        Warm::new(WARM_THREADS, Arc::new(move |stream| connection(stream, &state)))
    };
    loop {
        // Read before the accept, so a call already queued is still served.
        let stopping = state.stop.load(Ordering::Acquire);
        match listener.accept() {
            Ok((stream, _peer)) => accept(&state, &warm, stream),
            Err(e) if e.kind() == ErrorKind::WouldBlock => {
                if stopping {
                    break;
                }
                wait_readable(&listener);
            }
            Err(e) if e.kind() == ErrorKind::Interrupted => {}
            // The socket is gone from under us: there is nothing more to serve.
            Err(_) => break,
        }
    }
    state.stop.store(true, Ordering::Release);
    // The watcher's step is short, so this returns at once, and the socket file
    // is removed by exactly one place: here.
    let _ = watcher.join();
    remove_own(&sock, dev, ino);
    Ok(Served::Idle)
}

/// Opens the host pipe: one reader thread on stdin, and the writer the asks
/// answer through.
fn open_host(home: &Path, wait: Duration) -> Result<Arc<Host>> {
    let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    let host = Arc::new(Host::stdout(home, wait, &cwd));
    let reader = Arc::clone(&host);
    // The reader holds stdin for the rest of this engine: the process that
    // started us is the only one that can answer an ask.
    std::thread::Builder::new()
        .name("rvow-host".into())
        .spawn(move || events::read(reader, std::io::stdin()))
        .map_err(|e| Error::new("daemon-host", format!("cannot read the host pipe: {e}"), "run rvow daemon without --host-pipe"))?;
    Ok(host)
}

/// The shared state of one engine: what it answers about itself, the host pipe
/// when one is open, and the idle and connection counters the watcher and the
/// accept loop pass to each other.
struct State {
    /// The rvow home every call is answered against.
    home: PathBuf,
    /// The harness processes that session-start hooks of this home recorded.
    harnesses: Harnesses,
    /// Our own hello: the answer of `/v1/hello` and the report of a mismatch.
    hello: HelloReply,
    /// The host pipe, when this engine was started with one.
    host: Option<Arc<Host>>,
    /// The clock the idle window is measured on.
    start: Instant,
    /// Millis since `start` at the last request; 0 is the engine's own start.
    last: AtomicU64,
    /// Set when the engine decides to stop.
    stop: AtomicBool,
    /// The connections being served now.
    active: AtomicUsize,
}

impl State {
    /// Millis since this engine started.
    fn now(&self) -> u64 {
        u64::try_from(self.start.elapsed().as_millis()).unwrap_or(u64::MAX)
    }

    /// Notes that a request just arrived, so the idle window starts over.
    fn touch(&self) {
        self.last.store(self.now(), Ordering::Relaxed);
    }
}

/// Whether no request came for `idle`. A host pipe that is still open keeps the
/// engine up: its questions are answered on this process's stdin, and only when
/// it closes does the idle window apply.
fn idle_reached(state: &State, idle: Duration) -> bool {
    if state.host.as_ref().is_some_and(|host| host.live()) {
        return false;
    }
    state.now().saturating_sub(state.last.load(Ordering::Relaxed)) >= u64::try_from(idle.as_millis()).unwrap_or(u64::MAX)
}

/// Serves one accepted connection, or refuses it when the engine is full.
fn accept(state: &Arc<State>, warm: &Warm<UnixStream>, stream: UnixStream) {
    if state.active.fetch_add(1, Ordering::Relaxed) >= MAX_CONNECTIONS {
        state.active.fetch_sub(1, Ordering::Relaxed);
        let mut stream = stream;
        respond(&mut stream, BUSY, E_BUSY);
        return;
    }
    if !warm.run(stream) {
        state.active.fetch_sub(1, Ordering::Relaxed);
    }
}

/// A few long-lived threads that run jobs, and a new thread per job when all
/// of them are busy.
struct Warm<T> {
    jobs: std::sync::mpsc::Sender<T>,
    idle: Arc<AtomicUsize>,
    work: Arc<dyn Fn(T) + Send + Sync>,
}

impl<T: Send + 'static> Warm<T> {
    fn new(threads: usize, work: Arc<dyn Fn(T) + Send + Sync>) -> Warm<T> {
        let (jobs, rx) = std::sync::mpsc::channel::<T>();
        let rx = Arc::new(std::sync::Mutex::new(rx));
        let idle = Arc::new(AtomicUsize::new(0));
        for n in 0..threads {
            let (rx, freed, work) = (Arc::clone(&rx), Arc::clone(&idle), Arc::clone(&work));
            let spawned = std::thread::Builder::new().name(format!("rvow-warm-{n}")).spawn(move || {
                loop {
                    let job = match rx.lock() {
                        Ok(rx) => rx.recv(),
                        Err(_) => return,
                    };
                    let Ok(job) = job else { return };
                    work(job);
                    freed.fetch_add(1, Ordering::AcqRel);
                }
            });
            if spawned.is_ok() {
                idle.fetch_add(1, Ordering::AcqRel);
            }
        }
        Warm { jobs, idle, work }
    }

    /// Runs `job` on a free warm thread, or else on a new one; `false` when no
    /// thread could take it.
    fn run(&self, job: T) -> bool {
        let claimed = self.idle.fetch_update(Ordering::AcqRel, Ordering::Acquire, |n| n.checked_sub(1)).is_ok();
        let job = if claimed {
            match self.jobs.send(job) {
                Ok(()) => return true,
                Err(back) => back.0,
            }
        } else {
            job
        };
        let work = Arc::clone(&self.work);
        std::thread::Builder::new().spawn(move || work(job)).is_ok()
    }
}

/// Waits until the listener has a connection to accept, or `POLL` passed.
fn wait_readable(listener: &UnixListener) {
    let mut fd = libc::pollfd { fd: listener.as_raw_fd(), events: libc::POLLIN, revents: 0 };
    let millis = libc::c_int::try_from(POLL.as_millis()).unwrap_or(libc::c_int::MAX);
    // SAFETY: one pollfd for a live descriptor; an error or a timeout only
    // sends the loop back to the accept, which answers WouldBlock again.
    unsafe { libc::poll(&mut fd, 1, millis) };
}

/// One live connection: it holds one slot of `MAX_CONNECTIONS` until it is done.
struct Live(Arc<State>);

impl Drop for Live {
    fn drop(&mut self) {
        self.0.active.fetch_sub(1, Ordering::Relaxed);
    }
}

/// The folder the socket lives in: ours alone, since the socket is ours alone.
fn private_dir(sock: &Path, dir: &Path) -> Result<()> {
    std::fs::DirBuilder::new().recursive(true).mode(0o700).create(dir).map_err(|e| Error::io(dir, e))?;
    // A folder made by an older rvow, or by hand, is closed here; one of another
    // user refuses the change, and the check below names it.
    let _ = std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700));
    if !rvow_proto::socket_dir_is_private(sock) {
        return Err(Error::new(
            "daemon-dir",
            format!("{} is not a folder of this user alone, so another user could listen on the engine socket", dir.display()),
            "check who owns that folder, remove it if it is not yours, and run rvow daemon again",
        ));
    }
    Ok(())
}

/// The file whose lock says which process owns this home's engine.
fn lock_path(sock: &Path) -> PathBuf {
    let mut name = sock.as_os_str().to_owned();
    name.push(".lock");
    PathBuf::from(name)
}

/// Takes the lock beside the socket without waiting; `false` when another
/// process holds it. The lock lives as long as the descriptor does, which is
/// the rest of this process.
fn lock_exclusive(file: &std::fs::File) -> bool {
    // SAFETY: `flock` on a live descriptor, with LOCK_NB so it never blocks.
    unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) == 0 }
}

/// The pid the lock holder wrote, or 0 when the file says nothing yet.
fn lock_pid(path: &Path) -> u32 {
    std::fs::read_to_string(path).ok().and_then(|text| text.trim().parse().ok()).unwrap_or(0)
}

/// Writes our pid into the lock file, for the next process to read.
fn write_pid(lock: &std::fs::File, path: &Path) -> Result<()> {
    let pid = format!("{}\n", std::process::id());
    let mut file = lock;
    file.set_len(0).map_err(|e| Error::io(path, e))?;
    file.seek(SeekFrom::Start(0)).map_err(|e| Error::io(path, e))?;
    file.write_all(pid.as_bytes()).map_err(|e| Error::io(path, e))
}

/// Binds the socket, replacing a file that a killed engine left behind, and
/// returns the listener with the dev and inode of the file it bound.
fn bind(sock: &Path) -> Result<(UnixListener, u64, u64)> {
    match std::fs::remove_file(sock) {
        Ok(()) => {}
        Err(e) if e.kind() == ErrorKind::NotFound => {}
        Err(e) => return Err(Error::io(sock, e)),
    }
    let listener = UnixListener::bind(sock).map_err(|e| Error::io(sock, e))?;
    listener.set_nonblocking(true).map_err(|e| Error::io(sock, e))?;
    let meta = std::fs::metadata(sock).map_err(|e| Error::io(sock, e))?;
    Ok((listener, meta.dev(), meta.ino()))
}

/// Removes the socket, but only when it is still the one this engine bound: a
/// later engine has its own file, and the engine that bound this one is gone.
fn remove_own(sock: &Path, dev: u64, ino: u64) {
    if std::fs::metadata(sock).is_ok_and(|m| m.dev() == dev && m.ino() == ino) {
        let _ = std::fs::remove_file(sock);
    }
}

/// Serves one connection: one request, one answer, then close.
fn connection(mut stream: UnixStream, state: &Arc<State>) {
    let _live = Live(Arc::clone(state));
    // A socket accepted from a non-blocking listener inherits that flag on the
    // BSDs, where a read would then answer WouldBlock at once instead of
    // waiting: this connection waits, and the timeouts below bound the wait.
    let _ = stream.set_nonblocking(false);
    let _ = stream.set_read_timeout(Some(IO_TIMEOUT));
    let _ = stream.set_write_timeout(Some(IO_TIMEOUT));
    let due = Instant::now() + IO_TIMEOUT;
    let mut buf: Vec<u8> = Vec::with_capacity(CHUNK);
    let head_end = match read_head(&mut stream, &mut buf, due) {
        Ok(at) if at > 0 => at,
        Ok(_) => return respond(&mut stream, BAD, E_BAD),
        Err(r) => return refuse(&mut stream, r),
    };
    let head = match Head::parse(&buf[..head_end]) {
        Ok(head) => head,
        Err(r) => return refuse(&mut stream, r),
    };
    if head.route == Route::Answer {
        // The human answer reaches the engine on the host pipe alone, whatever
        // the peer claims: this route runs nothing, for anybody.
        return respond(&mut stream, FORBIDDEN, E_ANSWER);
    }
    // A peer that may not call is turned away before its body costs a read.
    if matches!(head.route, Route::Hook | Route::Ask) {
        if !peer_may_call_hooks(&stream, state) {
            return respond(&mut stream, FORBIDDEN, E_PEER);
        }
        // Only a call the engine serves keeps it from its idle exit.
        state.touch();
    }
    let Some(length) = head.length else {
        return refuse(&mut stream, Refusal { status: LENGTH, body: E_LENGTH });
    };
    if length > MAX_BODY {
        return refuse(&mut stream, Refusal { status: LARGE, body: E_LARGE });
    }
    let body = match read_body(&mut stream, &mut buf, head_end + 4, length, due) {
        Ok(body) => body,
        Err(r) => return refuse(&mut stream, r),
    };
    match head.route {
        Route::Hello => hello(body, state, &mut stream),
        Route::Hook => hook(body, state, &mut stream),
        Route::Ask => ask_route(body, state, &mut stream),
        // Answered above, before the body is read.
        Route::Answer => respond(&mut stream, FORBIDDEN, E_ANSWER),
    }
}

/// The routes the engine answers.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Route {
    /// The handshake: what this engine is, and the environment it serves.
    Hello,
    /// One hook call.
    Hook,
    /// One question for the user, put on the host pipe.
    Ask,
    /// The human answer, which never comes from the socket.
    Answer,
}

/// How one request is refused: the status line and the body it answers with.
#[derive(Clone, Copy, Debug)]
struct Refusal {
    status: &'static str,
    body: &'static [u8],
}

const BAD_REQUEST: Refusal = Refusal { status: BAD, body: E_BAD };

/// The head of one request, parsed into values that borrow nothing, so the body
/// can be read straight after without copying the head.
#[derive(Debug)]
struct Head {
    route: Route,
    /// The `content-length` the request names, when it names one.
    length: Option<usize>,
}

impl Head {
    /// Parses one request head. A method that is not POST and a path that is
    /// not one of our routes are refused here, before any body is read.
    fn parse(head: &[u8]) -> std::result::Result<Head, Refusal> {
        let mut lines = head.split(|b| *b == b'\n').map(trim_cr);
        let Some(start) = lines.next().filter(|line| !line.is_empty()) else {
            return Err(BAD_REQUEST);
        };
        let mut words = start.split(|b| *b == b' ').filter(|word| !word.is_empty());
        let (Some(method), Some(path)) = (words.next(), words.next()) else {
            return Err(BAD_REQUEST);
        };
        if method != b"POST" {
            return Err(Refusal { status: METHOD, body: E_METHOD });
        }
        let route = if path == b"/v1/hello" {
            Route::Hello
        } else if path == b"/v1/hook" {
            Route::Hook
        } else if path == b"/v1/ask" {
            Route::Ask
        } else if path == b"/v1/answer" {
            Route::Answer
        } else {
            return Err(Refusal { status: NOT_FOUND, body: E_PATH });
        };
        let mut length = None;
        for line in lines {
            let Some(colon) = line.iter().position(|b| *b == b':') else { continue };
            let (name, rest) = line.split_at(colon);
            if name.eq_ignore_ascii_case(b"content-length") {
                // `rest` opens with the colon itself.
                length = Some(digits(trim(&rest[1..])).ok_or(BAD_REQUEST)?);
            }
        }
        Ok(Head { route, length })
    }
}

/// Reads the request head into `buf` and returns the index of the blank line
/// that ends it, so the body starts four bytes later.
fn read_head(stream: &mut UnixStream, buf: &mut Vec<u8>, due: Instant) -> std::result::Result<usize, Refusal> {
    loop {
        if let Some(at) = find(buf, b"\r\n\r\n") {
            return Ok(at);
        }
        if buf.len() > MAX_HEAD {
            return Err(Refusal { status: HEAD_TOO_LONG, body: E_HEAD });
        }
        if !read_more(stream, buf, due)? {
            return Err(BAD_REQUEST);
        }
    }
}

/// Reads the `length` bytes of a body that starts at `at`, and returns them.
fn read_body<'a>(
    stream: &mut UnixStream,
    buf: &'a mut Vec<u8>,
    at: usize,
    length: usize,
    due: Instant,
) -> std::result::Result<&'a [u8], Refusal> {
    let want = at + length;
    // One allocation for a body that did not arrive with the head.
    buf.reserve(length);
    while buf.len() < want {
        if !read_more(stream, buf, due)? {
            return Err(BAD_REQUEST);
        }
    }
    Ok(&buf[at..want])
}

/// Reads one chunk of the request into `buf`; `false` when the peer sent
/// nothing more.
/// The whole request is due by `due`: each read waits only for what is left,
/// so a peer that sends one byte at a time cannot hold a slot.
fn read_more(stream: &mut UnixStream, buf: &mut Vec<u8>, due: Instant) -> std::result::Result<bool, Refusal> {
    let left = due.saturating_duration_since(Instant::now());
    if left.is_zero() || stream.set_read_timeout(Some(left)).is_err() {
        return Err(BAD_REQUEST);
    }
    // The socket writes straight into the buffer's tail, so a byte is copied once.
    let len = buf.len();
    buf.resize(len + CHUNK, 0);
    loop {
        match stream.read(&mut buf[len..]) {
            Ok(0) => {
                buf.truncate(len);
                return Ok(false);
            }
            Ok(n) => {
                buf.truncate(len + n);
                return Ok(true);
            }
            Err(e) if e.kind() == ErrorKind::Interrupted => {}
            // A timeout or a reset: the request never arrived whole.
            Err(_) => {
                buf.truncate(len);
                return Err(BAD_REQUEST);
            }
        }
    }
}

/// The offset of `needle` in `hay`.
fn find(hay: &[u8], needle: &[u8]) -> Option<usize> {
    hay.windows(needle.len()).position(|window| window == needle)
}

/// A header line without the carriage return the wire puts before its newline.
fn trim_cr(line: &[u8]) -> &[u8] {
    line.strip_suffix(b"\r").unwrap_or(line)
}

/// A header value without the spaces around it.
fn trim(value: &[u8]) -> &[u8] {
    let space = |b: &u8| matches!(*b, b' ' | b'\t');
    let start = value.iter().position(|b| !space(b)).unwrap_or(value.len());
    let end = value.iter().rposition(|b| !space(b)).map_or(start, |at| at + 1);
    &value[start..end]
}

/// The number a header value holds, or `None` when it holds anything else.
fn digits(value: &[u8]) -> Option<usize> {
    if value.is_empty() {
        return None;
    }
    let mut n: usize = 0;
    for byte in value {
        if !byte.is_ascii_digit() {
            return None;
        }
        n = n.checked_mul(10)?.checked_add(usize::from(byte - b'0'))?;
    }
    Some(n)
}

/// The handshake: what this engine is, its pid and its environment.
fn hello(body: &[u8], state: &State, stream: &mut UnixStream) {
    if serde_json::from_slice::<Hello>(body).is_ok() {
        respond_json(stream, &state.hello);
    } else {
        respond(stream, BAD, E_JSON);
    }
}

/// One hook call: the peer's environment decides whether it runs here, then the
/// hook answers, and a panic inside the hook is only a 500.
fn hook(body: &[u8], state: &State, stream: &mut UnixStream) {
    let Ok(request) = serde_json::from_slice::<HookRequest>(body) else {
        return respond(stream, BAD, E_JSON);
    };
    // A call from another environment would be answered with this home's store.
    if request.hello.proto_major != PROTO_MAJOR || request.hello.env != state.hello.env {
        return respond_json(stream, &HookResponse { reply: None, mismatch: Some(state.hello.clone()) });
    }
    let answered = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        dispatch::answer_hook_at(Some(&state.home), request.call.harness, request.call.event, request.call.input.get())
    }));
    match answered {
        Ok(output) => respond_json(stream, &HookResponse { reply: Some(HookReply { output }), mismatch: None }),
        Err(_) => respond(stream, SERVER, E_HOOK),
    }
}

/// One ask: a recorded harness of this environment asks the engine to put a
/// question to the user. Only the host pipe can answer it, so an engine
/// without one answers `NoHost` at once and the harness asks in its own dialog.
fn ask_route(body: &[u8], state: &State, stream: &mut UnixStream) {
    let Ok(request) = serde_json::from_slice::<AskRequest>(body) else {
        return respond(stream, BAD, E_JSON);
    };
    // A question from another environment would be decided with this home's
    // store and this process's stdin: answer with our own hello instead.
    if request.hello.proto_major != PROTO_MAJOR || request.hello.env != state.hello.env {
        return respond_json_at(stream, MISMATCH, &state.hello);
    }
    // The ask can wait for the user for minutes, so nothing of the engine's is
    // held here: the state is borrowed, and the waits happen on a channel.
    let answered = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| match &state.host {
        Some(host) => host.ask(&request),
        None => ask::no_host(),
    }));
    match answered {
        Ok(reply) => respond_json(stream, &reply),
        Err(_) => respond(stream, SERVER, E_ASK),
    }
}

/// Whether this peer may call the hooks of this home: it is a harness process
/// that a session-start hook recorded, by pid and start time (see `trust`).
fn peer_may_call_hooks(stream: &UnixStream, state: &State) -> bool {
    trust::peer_pid(stream).is_some_and(|pid| state.harnesses.allows(pid))
}

/// Writes one JSON response with `status`, or a 500 when it cannot be written.
fn respond_json_at<T: serde::Serialize>(stream: &mut UnixStream, status: &str, value: &T) {
    match serde_json::to_vec(value) {
        Ok(body) => respond(stream, status, &body),
        Err(_) => respond(stream, SERVER, E_SERVER),
    }
}

/// Writes one JSON response.
fn respond_json<T: serde::Serialize>(stream: &mut UnixStream, value: &T) {
    respond_json_at(stream, OK, value);
}

/// Writes one response, into one pre-sized buffer, with one write.
fn respond(stream: &mut UnixStream, status: &str, body: &[u8]) {
    const REST: &[u8] = b"\r\ncontent-type: application/json\r\ncontent-length: ";
    const TAIL: &[u8] = b"\r\nconnection: close\r\n\r\n";
    let mut out = Vec::with_capacity("HTTP/1.1 ".len() + status.len() + REST.len() + 20 + TAIL.len() + body.len());
    out.extend_from_slice(b"HTTP/1.1 ");
    out.extend_from_slice(status.as_bytes());
    out.extend_from_slice(REST);
    push_number(&mut out, body.len() as u64);
    out.extend_from_slice(TAIL);
    out.extend_from_slice(body);
    let _ = stream.write_all(&out);
    drain(stream);
}

/// How long and how far the engine reads past its answer before it closes.
const DRAIN_WAIT: Duration = Duration::from_millis(50);
const DRAIN_MAX: usize = 64 * 1024;

/// Ends the answer and reads what the peer still sends, so the close is clean.
/// Linux resets a Unix socket that is closed with unread data in it, and the
/// peer of an early refusal (a 403 before the body, a full engine) then reads
/// a reset instead of the answer.
fn drain(stream: &mut UnixStream) {
    let _ = stream.shutdown(std::net::Shutdown::Write);
    let _ = stream.set_read_timeout(Some(DRAIN_WAIT));
    let mut chunk = [0u8; 4096];
    let mut read = 0;
    while read < DRAIN_MAX {
        match stream.read(&mut chunk) {
            Ok(0) | Err(_) => break,
            Ok(n) => read += n,
        }
    }
}

/// Writes one refusal, which is one of the fixed error bodies.
fn refuse(stream: &mut UnixStream, refusal: Refusal) {
    respond(stream, refusal.status, refusal.body);
}

/// Appends the decimal digits of `n`, without a formatting allocation.
fn push_number(out: &mut Vec<u8>, mut n: u64) {
    let mut digits = [0u8; 20];
    let mut at = digits.len();
    loop {
        at -= 1;
        digits[at] = b'0' + (n % 10) as u8;
        n /= 10;
        if n == 0 {
            break;
        }
    }
    out.extend_from_slice(&digits[at..]);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scale_daemon_calls_reuse_warm_threads() {
        let (done, seen) = std::sync::mpsc::channel();
        let warm = Warm::new(
            2,
            Arc::new(move |n: usize| {
                let _ = done.send((n, std::thread::current().id()));
                // The thread is free only after its job returns; widen that window.
                std::thread::sleep(Duration::from_millis(2));
            }),
        );
        let mut threads = std::collections::BTreeSet::new();
        for n in 0..20 {
            assert!(warm.run(n));
            let (got, id) = seen.recv().unwrap();
            assert_eq!(got, n);
            threads.insert(format!("{id:?}"));
            // A hook call comes after the last one ended, when its thread is free again.
            let due = Instant::now() + Duration::from_secs(5);
            while warm.idle.load(Ordering::Acquire) < 2 && Instant::now() < due {
                std::thread::yield_now();
            }
        }
        assert!(threads.len() <= 2, "{threads:?}");
    }

    #[test]
    fn scale_daemon_busy_warm_threads_do_not_block_a_call() {
        let (hold, wait) = std::sync::mpsc::channel::<()>();
        let wait = Arc::new(std::sync::Mutex::new(wait));
        let (done, seen) = std::sync::mpsc::channel();
        let warm = Warm::new(
            1,
            Arc::new(move |n: usize| {
                if n == 0 {
                    let _ = wait.lock().unwrap().recv();
                }
                let _ = done.send(n);
            }),
        );
        assert!(warm.run(0));
        assert!(warm.run(1));
        assert_eq!(seen.recv_timeout(Duration::from_secs(5)), Ok(1));
        hold.send(()).unwrap();
        assert_eq!(seen.recv_timeout(Duration::from_secs(5)), Ok(0));
    }

    fn head(text: &str) -> std::result::Result<Head, Refusal> {
        Head::parse(text.as_bytes())
    }

    #[test]
    fn server_head_reads_the_route_the_method_and_the_length() {
        let parsed = head("POST /v1/hook HTTP/1.1\r\nhost: x\r\nContent-Length: 12\r\n\r\n").unwrap();
        assert_eq!(parsed.route, Route::Hook);
        assert_eq!(parsed.length, Some(12));
        // Header names are case-insensitive; methods are not.
        assert_eq!(head("POST /v1/hello HTTP/1.1\r\ncOnTeNt-LeNgTh:0\r\n\r\n").unwrap().route, Route::Hello);
        // The ask route is the harness asking the user; the answer route exists
        // so that every peer is refused it, whatever it sends.
        assert_eq!(head("POST /v1/ask HTTP/1.1\r\ncontent-length: 0\r\n\r\n").unwrap().route, Route::Ask);
        assert_eq!(head("POST /v1/answer HTTP/1.1\r\ncontent-length: 0\r\n\r\n").unwrap().route, Route::Answer);
        assert_eq!(head("post /v1/hello HTTP/1.1\r\ncontent-length: 0\r\n\r\n").unwrap_err().status, METHOD);

        assert_eq!(head("GET /v1/hello HTTP/1.1\r\n\r\n").unwrap_err().status, METHOD);
        assert_eq!(head("POST /v1/nope HTTP/1.1\r\n\r\n").unwrap_err().status, NOT_FOUND);
        assert_eq!(head("POST /v1/hook HTTP/1.1\r\n\r\n").unwrap().length, None, "no length is the caller's 411");
        assert_eq!(head("POST /v1/hook HTTP/1.1\r\ncontent-length: x\r\n\r\n").unwrap_err().status, BAD);
        assert_eq!(head("").unwrap_err().status, BAD);
    }

    #[test]
    fn server_reads_one_request_that_arrives_in_parts() {
        let (mut from, mut to) = UnixStream::pair().unwrap();
        to.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
        let writer = std::thread::spawn(move || {
            for part in [&b"POST /v1/hook HTTP/1.1\r\ncontent-length: 7\r\n\r\n"[..], b"{\"a\":", b"1}"] {
                from.write_all(part).unwrap();
                std::thread::sleep(Duration::from_millis(5));
            }
        });
        let mut buf: Vec<u8> = Vec::new();
        let due = Instant::now() + Duration::from_secs(5);
        let head_end = read_head(&mut to, &mut buf, due).unwrap();
        let parsed = Head::parse(&buf[..head_end]).unwrap();
        assert_eq!(parsed.length, Some(7));
        assert_eq!(read_body(&mut to, &mut buf, head_end + 4, 7, due).unwrap(), b"{\"a\":1}");
        writer.join().unwrap();
    }

    #[test]
    fn server_writes_a_whole_response_in_one_buffer() {
        let (mut from, mut to) = UnixStream::pair().unwrap();
        respond(&mut from, OK, b"{\"a\":1}");
        // The reader needs the end of the stream, which is the drop of the writer.
        drop(from);
        let mut text = String::new();
        to.read_to_string(&mut text).unwrap();
        assert_eq!(text, "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: 7\r\nconnection: close\r\n\r\n{\"a\":1}");
    }
}
