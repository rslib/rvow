//! What a hook call does about the engine once it answered.
//!
//! `rvow hook` never sends its own call to the engine: a hook process is a child
//! of the harness, like an agent's shell, so the engine would refuse it. The call
//! is answered in process, and only then does this module record the harness at a
//! session start and start an engine when none runs, so the next call of the home
//! finds one. The answer is flushed first, but the harness waits for the process
//! to exit, so this work is kept to a stat on most calls.

use std::os::unix::net::UnixStream;
use std::path::Path;
use std::time::Duration;

use rvow_core::store;
use rvow_proto::{RUN_DIR, socket_path};

use crate::HookEvent;

/// `off` stops this process and every process it starts from starting an engine.
const VAR: &str = "RVOW_ENGINE";
/// The file under the run folder whose age keeps two hooks of one home from
/// starting two engines.
const STAMP: &str = "engine.start";
/// How old the stamp has to be before a hook starts another engine.
const WINDOW: Duration = Duration::from_secs(60);

/// Records the harness process at a session start, and starts an engine of the
/// home when none runs. A hook never fails the harness, so an error here is
/// ignored: the worst case is a later call that finds no engine.
pub fn after_answer(event: HookEvent) {
    // The status line and the view run far more often than tool calls, and a
    // tool call or a session start finds the engine just as well.
    if matches!(event, HookEvent::Status | HookEvent::View) {
        return;
    }
    let off = std::env::var(VAR).is_ok_and(|value| value == "off");
    if off && !matches!(event, HookEvent::SessionStart) {
        return;
    }
    let Ok(home) = store::home_dir() else { return };
    if matches!(event, HookEvent::SessionStart) {
        // The harness is this process's parent: a hook process is its child, so
        // the pid recorded here is the pid the engine sees on its next call.
        let _ = rvow_daemon::trust::record(&home, std::os::unix::process::parent_id());
    }
    if off {
        return;
    }
    start_when_none(&home);
}

/// Starts an engine for `home` when nothing answers on its socket. The stamp
/// comes first: a hook probes the socket at most once per `WINDOW`, since each
/// probe wakes a running engine, and a hook runs on every tool call.
fn start_when_none(home: &Path) {
    let stamp = home.join(RUN_DIR).join(STAMP);
    if fresh(&stamp) || !touch(&stamp) {
        return;
    }
    // The connect is also how a socket file of a killed engine is found: a
    // connect to that file fails, and `serve` replaces it at bind.
    if UnixStream::connect(socket_path(home)).is_err() {
        spawn_engine();
    }
}

/// Whether the stamp was written less than `WINDOW` ago. A stamp that is
/// missing, unreadable or dated in the future is not fresh, so a hook starts an
/// engine rather than waiting for one that may never come.
fn fresh(stamp: &Path) -> bool {
    std::fs::metadata(stamp).and_then(|meta| meta.modified()).is_ok_and(|at| at.elapsed().is_ok_and(|age| age < WINDOW))
}

/// Writes the stamp, creating the run folder as this user's alone.
fn touch(stamp: &Path) -> bool {
    use std::os::unix::fs::DirBuilderExt;
    let Some(dir) = stamp.parent() else { return false };
    if std::fs::DirBuilder::new().recursive(true).mode(0o700).create(dir).is_err() {
        return false;
    }
    std::fs::File::create(stamp).is_ok()
}

/// Starts this binary as an engine, with no stdio of its own, in a process group
/// of its own so a signal to the harness's group does not reach it, and without
/// waiting for it.
///
/// The child inherits this process's environment, which is the harness's, so the
/// environment hash of the engine is the one of the calls it will serve.
fn spawn_engine() {
    let Ok(exe) = std::env::current_exe() else { return };
    let mut cmd = std::process::Command::new(exe);
    cmd.arg("daemon").stdin(std::process::Stdio::null()).stdout(std::process::Stdio::null()).stderr(std::process::Stdio::null());
    std::os::unix::process::CommandExt::process_group(&mut cmd, 0);
    let _ = cmd.spawn();
}
