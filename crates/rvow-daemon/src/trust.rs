//! Which processes may send hook calls to the engine.
//!
//! The same user id is no boundary: an agent's shell runs as the user and can
//! open the socket. Hook calls write state (session files, snapshots, observed
//! proofs, leases), so only the harness process may send them. The in-process
//! session-start hook records its parent, the harness, in the run folder, which
//! agents can neither run that hook into nor write (`rvow hook` and the run
//! folder are denied to them). The engine then serves a hook call only when the
//! peer process of the connection is a recorded harness, by pid and start time,
//! so a pid that a later process reuses is no harness.

use std::fmt::Write as _;
use std::io::{Read, Write};
use std::os::fd::AsRawFd;
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::SystemTime;

use rvow_core::error::{Error, Result};

use rvow_proto::RUN_DIR;
/// The file in that folder that lists the harness processes.
const HARNESSES: &str = "harnesses";
/// The most harness processes the list keeps; the oldest goes first.
const MAX_HARNESSES: usize = 64;

/// One harness process: its pid and the time it started, in the clock ticks
/// `start_time` reads.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Harness {
    pid: u32,
    start: u64,
}

/// The harness list of `home`.
pub fn harnesses_path(home: &Path) -> PathBuf {
    home.join(RUN_DIR).join(HARNESSES)
}

/// Records `pid` as a harness of `home`. It keeps only live processes, so the
/// list stays short, and it holds a lock so two session starts at once both land.
pub fn record(home: &Path, pid: u32) -> Result<()> {
    let Some(start) = start_time(pid) else {
        return Ok(());
    };
    let path = harnesses_path(home);
    if let Some(dir) = path.parent() {
        use std::os::unix::fs::DirBuilderExt;
        std::fs::DirBuilder::new().recursive(true).mode(0o700).create(dir).map_err(|e| Error::io(dir, e))?;
    }
    let mut file =
        std::fs::OpenOptions::new().read(true).write(true).create(true).truncate(false).open(&path).map_err(|e| Error::io(&path, e))?;
    // SAFETY: `flock` on a live descriptor; it waits for another session start,
    // which holds the lock only while it rewrites a few short lines.
    if unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX) } != 0 {
        return Err(Error::io(&path, std::io::Error::last_os_error()));
    }
    let mut text = String::new();
    file.read_to_string(&mut text).map_err(|e| Error::io(&path, e))?;
    let mut list: Vec<Harness> = parse(&text).filter(|h| h.pid != pid && start_time(h.pid) == Some(h.start)).collect();
    list.push(Harness { pid, start });
    let skip = list.len().saturating_sub(MAX_HARNESSES);
    let mut out = String::with_capacity(list.len() * 24);
    for h in &list[skip..] {
        let _ = writeln!(out, "{} {}", h.pid, h.start);
    }
    file.set_len(0).map_err(|e| Error::io(&path, e))?;
    std::io::Seek::rewind(&mut file).map_err(|e| Error::io(&path, e))?;
    file.write_all(out.as_bytes()).map_err(|e| Error::io(&path, e))
}

/// The lines of a harness list; a line that does not parse is skipped.
fn parse(text: &str) -> impl Iterator<Item = Harness> + '_ {
    text.lines().filter_map(|line| {
        let (pid, start) = line.split_once(' ')?;
        Some(Harness { pid: pid.parse().ok()?, start: start.parse().ok()? })
    })
}

/// The harness list of one home as the engine last read it, read again only
/// when the file changed, so a hook call costs one `stat`.
pub struct Harnesses {
    path: PathBuf,
    cache: Mutex<Option<(SystemTime, u64, Vec<Harness>)>>,
}

impl Harnesses {
    pub fn new(home: &Path) -> Harnesses {
        Harnesses { path: harnesses_path(home), cache: Mutex::new(None) }
    }

    /// Whether `pid` is a live recorded harness: the list names it with the
    /// start time the process has now.
    pub fn allows(&self, pid: u32) -> bool {
        let Ok(meta) = std::fs::metadata(&self.path) else { return false };
        let (Ok(mtime), len) = (meta.modified(), meta.len()) else { return false };
        let Ok(mut cache) = self.cache.lock() else { return false };
        let fresh = cache.as_ref().is_some_and(|(at, size, _)| *at == mtime && *size == len);
        if !fresh {
            let Ok(text) = std::fs::read_to_string(&self.path) else { return false };
            *cache = Some((mtime, len, parse(&text).collect()));
        }
        let Some((_, _, list)) = cache.as_ref() else { return false };
        let Some(recorded) = list.iter().find(|h| h.pid == pid) else { return false };
        start_time(pid) == Some(recorded.start)
    }
}

/// The pid of the process at the other end of `stream`, from the kernel.
pub fn peer_pid(stream: &UnixStream) -> Option<u32> {
    peer_pid_of(stream.as_raw_fd())
}

#[cfg(target_os = "macos")]
fn peer_pid_of(fd: std::os::fd::RawFd) -> Option<u32> {
    let mut pid: libc::pid_t = 0;
    let mut len = std::mem::size_of::<libc::pid_t>() as libc::socklen_t;
    // SAFETY: LOCAL_PEERPID writes one pid_t into `pid`, whose size `len` gives.
    let ok = unsafe { libc::getsockopt(fd, libc::SOL_LOCAL, libc::LOCAL_PEERPID, (&raw mut pid).cast(), &mut len) } == 0;
    (ok && pid > 0).then(|| pid.unsigned_abs())
}

#[cfg(target_os = "linux")]
fn peer_pid_of(fd: std::os::fd::RawFd) -> Option<u32> {
    let mut cred = libc::ucred { pid: 0, uid: 0, gid: 0 };
    let mut len = std::mem::size_of::<libc::ucred>() as libc::socklen_t;
    // SAFETY: SO_PEERCRED writes one ucred into `cred`, whose size `len` gives.
    let ok = unsafe { libc::getsockopt(fd, libc::SOL_SOCKET, libc::SO_PEERCRED, (&raw mut cred).cast(), &mut len) } == 0;
    (ok && cred.pid > 0).then(|| cred.pid.unsigned_abs())
}

#[cfg(not(any(target_os = "macos", target_os = "linux")))]
fn peer_pid_of(_fd: std::os::fd::RawFd) -> Option<u32> {
    None
}

/// When process `pid` started, or `None` when it does not run. The unit only
/// has to be the same across calls on one machine.
#[cfg(target_os = "macos")]
fn start_time(pid: u32) -> Option<u64> {
    let pid = libc::c_int::try_from(pid).ok()?;
    let mut info = std::mem::MaybeUninit::<libc::proc_bsdinfo>::zeroed();
    let size = libc::c_int::try_from(std::mem::size_of::<libc::proc_bsdinfo>()).ok()?;
    // SAFETY: PROC_PIDTBSDINFO fills at most `size` bytes of `info`.
    let n = unsafe { libc::proc_pidinfo(pid, libc::PROC_PIDTBSDINFO, 0, info.as_mut_ptr().cast(), size) };
    if n != size {
        return None;
    }
    // SAFETY: the call filled the whole struct, as its return value says.
    let info = unsafe { info.assume_init() };
    Some(info.pbi_start_tvsec * 1_000_000 + info.pbi_start_tvusec)
}

/// Field 22 of `/proc/<pid>/stat`, the start in clock ticks after boot; the
/// command name in field 2 may hold spaces, so fields count from its `)`.
#[cfg(target_os = "linux")]
fn start_time(pid: u32) -> Option<u64> {
    let text = std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
    let rest = &text[text.rfind(')')? + 1..];
    rest.split_whitespace().nth(19)?.parse().ok()
}

#[cfg(not(any(target_os = "macos", target_os = "linux")))]
fn start_time(_pid: u32) -> Option<u64> {
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn home() -> rvow_core::git::testrepo::TempDir {
        rvow_core::git::testrepo::TempDir::new()
    }

    #[test]
    fn daemon_trust_our_own_process_has_a_start_time() {
        let me = std::process::id();
        assert!(start_time(me).is_some());
        assert_eq!(start_time(me), start_time(me));
        assert_eq!(start_time(u32::MAX - 1), None, "no such process");
    }

    #[test]
    fn daemon_trust_a_recorded_live_harness_is_allowed_and_another_pid_is_not() {
        let tmp = home();
        let me = std::process::id();
        let harnesses = Harnesses::new(&tmp.0);
        assert!(!harnesses.allows(me), "nothing recorded yet");
        record(&tmp.0, me).unwrap();
        assert!(harnesses.allows(me));
        assert!(!harnesses.allows(1), "pid 1 was never recorded");
    }

    #[test]
    fn daemon_trust_a_reused_pid_with_another_start_time_is_refused() {
        let tmp = home();
        let me = std::process::id();
        let path = harnesses_path(&tmp.0);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        let real = start_time(me).unwrap();
        std::fs::write(&path, format!("{me} {}\n", real + 1)).unwrap();
        assert!(!Harnesses::new(&tmp.0).allows(me), "the recorded process was another one");
    }

    #[test]
    fn daemon_trust_record_drops_dead_entries_and_keeps_one_line_per_pid() {
        let tmp = home();
        let me = std::process::id();
        let path = harnesses_path(&tmp.0);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, format!("{} 5\nnot a line\n{me} 1\n", u32::MAX - 1)).unwrap();
        record(&tmp.0, me).unwrap();
        record(&tmp.0, me).unwrap();
        let text = std::fs::read_to_string(&path).unwrap();
        assert_eq!(text, format!("{me} {}\n", start_time(me).unwrap()), "{text}");
    }

    #[test]
    fn daemon_trust_a_socket_names_its_peer() {
        let (a, b) = UnixStream::pair().unwrap();
        assert_eq!(peer_pid(&a), Some(std::process::id()));
        drop(b);
    }
}
