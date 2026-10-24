use rvow_core::error::{Error, Result};
use rvow_core::store;
use rvow_core::tasks::parse_duration;
use rvow_daemon::server::{Served, serve};
use rvow_proto::{Hello, PROTO_MAJOR, PROTO_MINOR, Ui, socket_path};

use crate::output::Reply;

/// `rvow daemon --print-hello`: the socket of this home and the hello that a
/// client running in this process's environment sends. The Claude mod and the
/// pi extension run it once per session, so their environment hash is the one
/// of the engine that the session's own hook started.
pub fn print_hello() -> Result<Reply> {
    let home = store::home_dir()?;
    let hello = Hello {
        proto_major: PROTO_MAJOR,
        proto_minor: PROTO_MINOR,
        build: env!("CARGO_PKG_VERSION").to_owned(),
        env: rvow_daemon::context::env_hash(),
        ui: Ui::Pane,
        harness_pid: None,
    };
    let socket = socket_path(&home);
    // A socket in a folder that another user can enter may be theirs: no engine then.
    if !rvow_proto::socket_dir_is_private(&socket) {
        return Err(Error::new(
            "daemon-dir",
            format!("{} is not a folder of this user alone", socket.parent().map_or_else(String::new, |d| d.display().to_string())),
            "check who owns that folder, remove it if it is not yours, and start a new session",
        ));
    }
    let line = serde_json::json!({ "socket": socket, "hello": hello });
    // The line is the whole answer in every output mode: a client parses it as JSON.
    println!("{line}");
    Ok(Reply::done(true))
}

/// `rvow daemon`: the engine of this rvow home. The first hook that needs one
/// starts it in the background; every later hook call finds it on its socket.
///
/// One engine owns a home, so a second start reports the pid that holds it and
/// leaves the serving to that process.
///
/// With `--host-pipe` the process that started the engine (rmaestro) holds its
/// stdin and stdout: the engine writes the questions it has for the user on
/// stdout and reads the answers on stdin, and no other channel carries one.
pub fn daemon(idle: &str, host_pipe: bool, ask_wait: &str) -> Result<Reply> {
    let home = store::home_dir()?;
    let idle = parse_duration(idle)
        .ok_or_else(|| Error::new("daemon-idle", format!("{idle:?} is not a length"), "pass --idle with a value such as 10m or 1h"))?;
    // `--ask-wait` is how long one ask waits for the host before it reports
    // waiting; without a host pipe there is nobody to wait for.
    let ask_wait = parse_duration(ask_wait).ok_or_else(|| {
        Error::new("daemon-ask-wait", format!("{ask_wait:?} is not a length"), "pass --ask-wait with a value such as 10m or 30s")
    })?;
    let ask = host_pipe.then_some(ask_wait);
    let mut r = Reply::new();
    match serve(&home, idle, ask)? {
        Served::Busy { pid } => {
            let line = format!("an engine runs: pid {pid}");
            if host_pipe {
                // With a host pipe stdout is the wire: a report on it would be
                // a line the host cannot read.
                eprintln!("rvow daemon: {line}");
            } else {
                r.line(line);
                r.set("engine_pid", u64::from(pid));
            }
        }
        // Idle: no call came for the whole window, so this engine printed nothing.
        Served::Idle => {}
    }
    Ok(r)
}
