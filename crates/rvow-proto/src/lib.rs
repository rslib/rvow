//! The wire types of the engine socket: the hello handshake and one hook call.

pub mod version;

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use serde_json::value::RawValue;

pub use version::{PROTO_MAJOR, PROTO_MINOR};

/// The folder under the rvow home that the engine keeps for itself: its socket,
/// its lock and the list of the harness processes.
pub const RUN_DIR: &str = "run";

/// The socket the engine of `home` listens on: `home/run/engine.sock` when that
/// path fits the platform's limit, else a short name in the temp folder.
///
/// A `sockaddr_un` holds about 104 bytes on macOS and 108 on Linux, so a long
/// home (a deep temp folder, a long user name) falls back to
/// `$TMPDIR/rvow-engine-<uid>/<16 hex>.sock`, named after the FNV-1a 64 hash of
/// the home path, in a folder of this user. FNV-1a is public domain
/// (http://www.isthe.com/chongo/tech/comp/fnv/).
///
/// On a shared `/tmp` another user can create that folder first, so a client
/// and the engine use the socket only when `socket_dir_is_private` holds.
pub fn socket_path(home: &Path) -> PathBuf {
    let direct = home.join(RUN_DIR).join("engine.sock");
    if direct.as_os_str().as_encoded_bytes().len() <= 100 {
        return direct;
    }
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in home.as_os_str().as_encoded_bytes() {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    std::env::temp_dir().join(format!("rvow-engine-{}", uid())).join(format!("{hash:016x}.sock"))
}

/// Whether the folder of `socket` is a real folder (not a link) of this user
/// that no other user can enter, so nobody else can listen on the socket.
pub fn socket_dir_is_private(socket: &Path) -> bool {
    use std::os::unix::fs::MetadataExt;
    let Some(dir) = socket.parent() else { return false };
    std::fs::symlink_metadata(dir).is_ok_and(|m| m.is_dir() && m.uid() == uid() && m.mode() & 0o077 == 0)
}

/// The user id of this process.
fn uid() -> u32 {
    // SAFETY: getuid has no failure case and touches no memory of ours.
    unsafe { libc::getuid() }
}

/// The harness a hook call comes from, which decides the shape of the answer.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Harness {
    Claude,
    Codex,
    Json,
}

/// The hook event a call names.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum HookEvent {
    PreToolUse,
    PostToolUse,
    SessionStart,
    Stop,
    Status,
    View,
    LeaseBind,
    LeaseEnd,
}

/// Where the daemon shows its pane, or that the harness host owns the view.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Ui {
    Pane,
    Host,
}

/// The first message a client sends: its protocol version, its build, the
/// environment it works in and the pid of the harness that started it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Hello {
    pub proto_major: u32,
    pub proto_minor: u32,
    pub build: String,
    pub env: String,
    pub ui: Ui,
    #[serde(default)]
    pub harness_pid: Option<u32>,
}

/// The daemon's answer to `Hello`: its own version, build, environment and pid.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct HelloReply {
    pub proto_major: u32,
    pub proto_minor: u32,
    pub build: String,
    pub env: String,
    pub pid: u32,
}

/// One hook call: the harness and event, with the input the hook reads.
///
/// `input` stays raw: the engine hands the caller's own bytes to the hook with
/// `input.get()`, so a call crosses the socket without a parse and a
/// reserialize. `RawValue` has no `PartialEq`, so this type has none either.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct HookCall {
    pub harness: Harness,
    pub event: HookEvent,
    pub input: Box<RawValue>,
}

/// The answer to a `HookCall`: the exact text `rvow hook` prints on stdout,
/// without its newline, or `None` when it prints nothing.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct HookReply {
    pub output: Option<String>,
}

/// One hook call as the socket carries it: the peer's `Hello` first, so the
/// engine can answer a call from another environment without running it.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct HookRequest {
    pub hello: Hello,
    pub call: HookCall,
}

/// The answer to a `HookRequest`: `reply` when the call ran, else `mismatch`
/// with the engine's own `HelloReply`.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct HookResponse {
    pub reply: Option<HookReply>,
    pub mismatch: Option<HelloReply>,
}

/// What a harness asks the engine to put to the user, or to do for them.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum AskKind {
    /// Record the vow of a backlog plan.
    Accept,
    /// Accept the amendments that wait for the user's decision.
    Confirm,
    /// Merge an active change into the living specs.
    Archive,
}

/// The human answer to one ask.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Answer {
    /// Go ahead with the action.
    Yes,
    /// Do nothing.
    No,
}

/// One ask as the socket carries it: the peer's `Hello` first, then what to do
/// and where. The socket never carries the human answer, only the ask.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct AskRequest {
    pub hello: Hello,
    pub kind: AskKind,
    pub change: String,
    pub cwd: String,
    #[serde(default)]
    pub session: Option<String>,
}

/// How one ask ended.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum AskOutcome {
    /// The user answered yes and the action succeeded.
    Done,
    /// The user answered no, or read a plan that has changed.
    Declined,
    /// The action failed, or the engine cannot ask at all.
    Failed,
    /// The user has not answered yet; the ask stays open.
    Waiting,
    /// The host pipe closed while the ask waited.
    Cancelled,
    /// No host pipe holds this process, so nobody can answer.
    NoHost,
}

/// The answer to an `AskRequest`: how it ended and one line about it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct AskReply {
    pub outcome: AskOutcome,
    pub text: String,
}

/// One line the engine writes to the host on its stdout: a question for the
/// user, how an earlier question ended, or one part of a plan.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "kebab-case")]
pub enum ToHost {
    /// Put this question to the user. `digest` is what the question is about,
    /// so an answer to a plan that has changed is refused.
    Ask { id: u64, kind: AskKind, change: String, digest: String, summary: String, session: Option<String> },
    /// The ask `id` ended with this outcome, so the host can close its dialog.
    Resolved { id: u64, outcome: AskOutcome },
    /// The text of one part of a change, or none when there is no such part.
    Part { change: String, part: String, text: Option<String> },
}

/// One line the host writes to the engine on its stdin.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "kebab-case")]
pub enum FromHost {
    /// The user's answer to ask `id`, with the digest it was shown.
    Answer { id: u64, value: Answer, digest: String },
    /// Read back one part of a change for the dialog.
    Get { change: String, part: String },
}

#[cfg(test)]
mod tests {
    #[test]
    fn engine_context_socket_folder_must_be_private() {
        use std::os::unix::fs::PermissionsExt;
        let dir = std::env::temp_dir().join(format!("rvow-proto-private-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let sock = dir.join("engine.sock");
        std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o755)).unwrap();
        assert!(!socket_dir_is_private(&sock), "other users can enter a 0755 folder");
        std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700)).unwrap();
        assert!(socket_dir_is_private(&sock));
        std::fs::remove_dir_all(&dir).unwrap();
        let long = PathBuf::from("/").join("x".repeat(120));
        let fallback = socket_path(&long);
        assert!(fallback.parent().unwrap().ends_with(format!("rvow-engine-{}", uid())), "{}", fallback.display());
    }

    use super::*;
    use serde_json::json;

    #[test]
    fn engine_context_proto_round_trips() {
        assert_eq!((PROTO_MAJOR, PROTO_MINOR), (1, 0));

        assert_eq!(serde_json::to_value(Harness::Claude).unwrap(), json!("claude"));
        assert_eq!(serde_json::to_value(Harness::Codex).unwrap(), json!("codex"));
        assert_eq!(serde_json::to_value(Harness::Json).unwrap(), json!("json"));
        assert_eq!(serde_json::to_value(HookEvent::PreToolUse).unwrap(), json!("pre-tool-use"));
        assert_eq!(serde_json::to_value(HookEvent::LeaseEnd).unwrap(), json!("lease-end"));
        assert_eq!(serde_json::to_value(Ui::Pane).unwrap(), json!("pane"));
        assert_eq!(serde_json::to_value(Ui::Host).unwrap(), json!("host"));

        let hello = Hello {
            proto_major: PROTO_MAJOR,
            proto_minor: PROTO_MINOR,
            build: "0.0.0".into(),
            env: "abc".into(),
            ui: Ui::Pane,
            harness_pid: Some(7),
        };
        let text = serde_json::to_string(&hello).unwrap();
        assert!(text.contains("\"ui\":\"pane\""), "{text}");
        assert!(text.contains("\"proto_major\":1"), "{text}");
        assert_eq!(serde_json::from_str::<Hello>(&text).unwrap(), hello);
        // A peer that names no harness pid leaves the field out.
        let mut without: serde_json::Value = serde_json::from_str(&text).unwrap();
        without.as_object_mut().unwrap().remove("harness_pid");
        let bare: Hello = serde_json::from_value(without).unwrap();
        assert_eq!(bare.harness_pid, None);

        let reply = HelloReply { proto_major: PROTO_MAJOR, proto_minor: PROTO_MINOR, build: "0.0.0".into(), env: "abc".into(), pid: 9 };
        let text = serde_json::to_string(&reply).unwrap();
        assert_eq!(serde_json::from_str::<HelloReply>(&text).unwrap(), reply);

        let call = HookCall {
            harness: Harness::Claude,
            event: HookEvent::PreToolUse,
            input: RawValue::from_string("{\"cwd\": \"/r\"}".to_owned()).unwrap(),
        };
        let text = serde_json::to_string(&call).unwrap();
        assert!(text.contains("\"harness\":\"claude\""), "{text}");
        assert!(text.contains("\"event\":\"pre-tool-use\""), "{text}");
        // The input crosses as the caller wrote it, not as a reserialized value.
        assert!(text.contains("\"input\":{\"cwd\": \"/r\"}"), "{text}");
        let back: HookCall = serde_json::from_str(&text).unwrap();
        assert_eq!(back.harness, call.harness);
        assert_eq!(back.event, call.event);
        assert_eq!(back.input.get(), call.input.get());

        let request = HookRequest { hello: hello.clone(), call };
        let text = serde_json::to_string(&request).unwrap();
        assert!(text.contains("\"call\":{\"harness\":\"claude\""), "{text}");
        let back: HookRequest = serde_json::from_str(&text).unwrap();
        assert_eq!(back.hello, request.hello);
        assert_eq!(back.call.input.get(), request.call.input.get());

        let answer = HookReply { output: Some("{\"ok\":true}".into()) };
        let text = serde_json::to_string(&answer).unwrap();
        assert_eq!(serde_json::from_str::<HookReply>(&text).unwrap(), answer);
        let nothing = HookReply { output: None };
        assert_eq!(serde_json::to_string(&nothing).unwrap(), "{\"output\":null}");
        assert_eq!(serde_json::from_str::<HookReply>("{\"output\":null}").unwrap(), nothing);

        let ran = HookResponse { reply: Some(answer.clone()), mismatch: None };
        let text = serde_json::to_string(&ran).unwrap();
        let back: HookResponse = serde_json::from_str(&text).unwrap();
        assert_eq!(back.reply, ran.reply);
        assert!(back.mismatch.is_none(), "{text}");

        let refused = HookResponse { reply: None, mismatch: Some(reply) };
        let text = serde_json::to_string(&refused).unwrap();
        assert!(text.contains("\"reply\":null"), "{text}");
        let back: HookResponse = serde_json::from_str(&text).unwrap();
        assert!(back.reply.is_none(), "{text}");
        assert_eq!(back.mismatch, refused.mismatch);
    }

    #[test]
    fn engine_context_ask_wire_round_trips() {
        assert_eq!(serde_json::to_value(AskKind::Accept).unwrap(), json!("accept"));
        assert_eq!(serde_json::to_value(AskKind::Archive).unwrap(), json!("archive"));
        assert_eq!(serde_json::to_value(Answer::Yes).unwrap(), json!("yes"));
        assert_eq!(serde_json::to_value(AskOutcome::NoHost).unwrap(), json!("no-host"));
        assert_eq!(serde_json::to_value(AskOutcome::Done).unwrap(), json!("done"));

        let ask = AskRequest {
            hello: Hello {
                proto_major: PROTO_MAJOR,
                proto_minor: PROTO_MINOR,
                build: "0.0.0".into(),
                env: "abc".into(),
                ui: Ui::Host,
                harness_pid: Some(7),
            },
            kind: AskKind::Accept,
            change: "CHG-abc123".into(),
            cwd: "/r".into(),
            session: None,
        };
        let text = serde_json::to_string(&ask).unwrap();
        assert!(text.contains("\"kind\":\"accept\""), "{text}");
        let back: AskRequest = serde_json::from_str(&text).unwrap();
        assert_eq!(back.kind, ask.kind);
        assert_eq!(back.change, ask.change);
        assert_eq!(back.session, None);
        // A caller that names no session leaves the field out.
        let mut bare: serde_json::Value = serde_json::from_str(&text).unwrap();
        bare.as_object_mut().unwrap().remove("session");
        assert_eq!(serde_json::from_value::<AskRequest>(bare).unwrap().session, None);

        let reply = AskReply { outcome: AskOutcome::Waiting, text: "still waiting for the user; end your turn".into() };
        let text = serde_json::to_string(&reply).unwrap();
        assert!(text.contains("\"outcome\":\"waiting\""), "{text}");
        assert_eq!(serde_json::from_str::<AskReply>(&text).unwrap(), reply);

        // The tagged lines carry the type in the line itself.
        let question = ToHost::Ask {
            id: 3,
            kind: AskKind::Confirm,
            change: "CHG-abc123".into(),
            digest: "d".into(),
            summary: "CHG-abc123: 1 pending amendment".into(),
            session: Some("s1".into()),
        };
        let text = serde_json::to_string(&question).unwrap();
        assert!(text.starts_with("{\"type\":\"ask\","), "{text}");
        assert_eq!(serde_json::from_str::<ToHost>(&text).unwrap(), question);

        let done = ToHost::Resolved { id: 3, outcome: AskOutcome::Done };
        let text = serde_json::to_string(&done).unwrap();
        assert_eq!(text, "{\"type\":\"resolved\",\"id\":3,\"outcome\":\"done\"}");
        assert_eq!(serde_json::from_str::<ToHost>(&text).unwrap(), done);

        let part = ToHost::Part { change: "CHG-abc123".into(), part: "tasks".into(), text: None };
        let text = serde_json::to_string(&part).unwrap();
        assert!(text.starts_with("{\"type\":\"part\","), "{text}");
        assert_eq!(serde_json::from_str::<ToHost>(&text).unwrap(), part);

        let answer = FromHost::Answer { id: 3, value: Answer::No, digest: "d".into() };
        let text = serde_json::to_string(&answer).unwrap();
        assert_eq!(text, "{\"type\":\"answer\",\"id\":3,\"value\":\"no\",\"digest\":\"d\"}");
        assert_eq!(serde_json::from_str::<FromHost>(&text).unwrap(), answer);

        let get = FromHost::Get { change: "CHG-abc123".into(), part: "proposal".into() };
        let text = serde_json::to_string(&get).unwrap();
        assert_eq!(text, "{\"type\":\"get\",\"change\":\"CHG-abc123\",\"part\":\"proposal\"}");
        assert_eq!(serde_json::from_str::<FromHost>(&text).unwrap(), get);
        // A line that names no type is not one of ours.
        assert!(serde_json::from_str::<FromHost>("{\"id\":1}").is_err());
    }

    #[test]
    fn engine_context_socket_path_stays_short_and_steady() {
        assert_eq!(socket_path(Path::new("/tmp/h")), Path::new("/tmp/h/run/engine.sock"));

        let long = "x".repeat(120);
        let path = socket_path(Path::new(&long));
        assert!(path.starts_with(std::env::temp_dir()), "{}", path.display());
        assert!(path.as_os_str().as_encoded_bytes().len() < 100, "{}", path.display());
        assert_eq!(path.file_name().unwrap().to_str().unwrap().len(), 16 + ".sock".len());
        assert_eq!(path, socket_path(Path::new(&long)), "one home keeps one socket");
        assert_ne!(path, socket_path(Path::new(&"y".repeat(120))), "another home gets another socket");

        // Just under the limit still fits the home, one byte over does not.
        let home = "h".repeat(100 - "/run/engine.sock".len());
        assert_eq!(socket_path(Path::new(&home)), Path::new(&home).join("run/engine.sock"));
        assert!(socket_path(Path::new(&(home + "h"))).starts_with(std::env::temp_dir()));
    }
}
