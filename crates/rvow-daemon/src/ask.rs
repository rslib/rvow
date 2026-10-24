//! The asks: the questions the engine puts to the process that started it.
//!
//! rmaestro starts `rvow daemon --host-pipe` as its child and holds the child's
//! stdin, so that pipe is the only wire a human answer travels: the socket
//! never takes one (see `server`, `/v1/answer` refuses every peer). A harness
//! asks on the socket, the engine shows the question on its stdout, and the
//! user answers on the engine's stdin.
//!
//! An ask pauses the calling connection until the user answers or the ask
//! window passes. A window that passes leaves the ask open, so a late answer
//! still runs the action, and each answer is checked against the digest of the
//! plan as it stands now: an answer to a plan that changed runs nothing and
//! asks again.

use std::collections::HashMap;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{Receiver, RecvTimeoutError, Sender, channel};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use rvow_core::change;
use rvow_core::error::{Error, Result};
use rvow_core::proof;
use rvow_core::store::{self, Project};
use rvow_core::{amend, archive, show, vow};
use rvow_proto::{Answer, AskKind, AskOutcome, AskReply, AskRequest, FromHost, ToHost};

/// The most asks that may wait for the user at once; the next one is refused.
const MAX_OPEN: usize = 32;
/// The reply of an ask that no host pipe can take.
const NO_HOST: &str = "no host pipe; ask in the harness dialog";
/// The reply of an ask whose host went away before it answered.
const CANCELLED: &str = "the host ended before it answered";
/// The reply of an ask the user did not answer inside the ask window.
const WAITING: &str = "still waiting for the user; end your turn";
/// What a reply says when the plan changed while the user read it.
const CHANGED: &str = "the plan changed while the user read it";

/// The reply of a harness ask that this engine cannot put to a host.
pub fn no_host() -> AskReply {
    AskReply { outcome: AskOutcome::NoHost, text: NO_HOST.to_owned() }
}

/// A failed ask whose one line is `text`.
fn failed(text: &str) -> AskReply {
    AskReply { outcome: AskOutcome::Failed, text: text.to_owned() }
}

/// The host pipe: the writer of the engine's lines, the asks that wait for an
/// answer, and the folder each ask's action runs in.
pub struct Host {
    /// The rvow home every action runs against.
    home: PathBuf,
    /// How long one ask waits for an answer before it reports waiting.
    wait: Duration,
    /// The id of the next ask.
    next: AtomicU64,
    /// Set when the host pipe closed, so later asks answer `NoHost`.
    gone: AtomicBool,
    /// The asks that wait for an answer, by id.
    open: Mutex<HashMap<u64, Sender<HostReply>>>,
    /// The working folder of the last ask, for a `get` that names none.
    cwd: Mutex<PathBuf>,
    /// The one writer of the pipe.
    out: Mutex<Box<dyn Write + Send>>,
}

/// What the host did with one ask.
enum HostReply {
    /// The user answered, with the digest of the plan the dialog showed.
    Value { value: Answer, digest: String },
    /// The host pipe closed while this ask waited.
    Cancelled,
}

/// One question in flight: the project its action runs against, the change as
/// the store names it, and the digest and summary of what the user reads.
struct Question {
    project: Project,
    /// The change id as the store names it, not the prefix the harness sent.
    change: String,
    /// The digest of the plan this question shows.
    digest: String,
    /// One line about the plan, never its text.
    summary: String,
    /// The id of the question the host holds now.
    id: u64,
    /// Whether an earlier question was declined because the plan had changed.
    changed: bool,
}

/// What one wait came to.
enum Waited {
    /// The reply the socket sends back.
    Reply(AskReply),
    /// The ask window passed with the ask still open, on this channel.
    Open(Receiver<HostReply>),
}

impl Host {
    /// A host pipe that writes its lines to `out`, for a test that holds the
    /// other end.
    pub fn of(home: &Path, wait: Duration, cwd: &Path, out: Box<dyn Write + Send>) -> Host {
        Host {
            home: home.to_path_buf(),
            wait,
            next: AtomicU64::new(1),
            gone: AtomicBool::new(false),
            open: Mutex::new(HashMap::new()),
            cwd: Mutex::new(cwd.to_path_buf()),
            out: Mutex::new(out),
        }
    }

    /// A host pipe whose lines go to this process's stdout.
    pub fn stdout(home: &Path, wait: Duration, cwd: &Path) -> Host {
        Host::of(home, wait, cwd, Box::new(std::io::stdout()))
    }

    /// Whether the host pipe is still open.
    pub fn live(&self) -> bool {
        !self.gone.load(Ordering::Acquire)
    }

    /// The host is gone: every ask that waits is cancelled, and a later ask
    /// answers `NoHost`.
    pub fn host_gone(&self) {
        self.gone.store(true, Ordering::Release);
        // The lock is dropped before the senders are woken: a waiting thread
        // must not wait on this map to take its own ask out of it.
        let waiting: Vec<(u64, Sender<HostReply>)> = self.lock_open().drain().collect();
        for (_id, sender) in waiting {
            let _ = sender.send(HostReply::Cancelled);
        }
    }

    /// One line the host sent.
    pub fn line(&self, from: FromHost) {
        match from {
            FromHost::Answer { id, value, digest } => self.answer(id, value, digest),
            FromHost::Get { change, part } => self.get(&change, &part),
        }
    }

    /// Serves one ask: it puts the question to the host and waits for the
    /// answer, or for the ask window to pass.
    pub fn ask(self: &Arc<Host>, request: &AskRequest) -> AskReply {
        if !self.live() {
            return no_host();
        }
        self.remember_cwd(request);
        let mut question = match self.preview(request) {
            Ok(question) => question,
            Err(e) => return failed(&e.message),
        };
        let (id, waiting) = match self.question(request, &question) {
            Ok(open) => open,
            Err(reply) => return reply,
        };
        question.id = id;
        match self.step(request, &mut question, waiting, Some(Instant::now() + self.wait)) {
            Waited::Reply(reply) => reply,
            Waited::Open(open) => {
                // The window passed: the ask stays open, so a later answer still
                // runs the action, and the hook gets its answer at once.
                let id = question.id;
                let host = Arc::clone(self);
                let request = request.clone();
                let kept = std::thread::Builder::new().name("rvow-ask".into()).spawn(move || {
                    // A late answer runs the action and tells the host; the
                    // connection that asked is gone by now.
                    let _ = host.step(&request, &mut question, open, None);
                });
                if kept.is_err() {
                    self.forget(id);
                    self.resolved(id, AskOutcome::Cancelled);
                    return failed("the engine cannot keep the question open");
                }
                AskReply { outcome: AskOutcome::Waiting, text: WAITING.to_owned() }
            }
        }
    }

    /// Waits for the host's answer to the open question until `window`, which
    /// is `None` for an ask that stays open as long as it takes.
    fn step(&self, request: &AskRequest, question: &mut Question, mut waiting: Receiver<HostReply>, window: Option<Instant>) -> Waited {
        loop {
            let answered = match window {
                // A window that has passed still takes one look at the channel.
                Some(deadline) => waiting.recv_timeout(deadline.saturating_duration_since(Instant::now())),
                None => waiting.recv().map_err(|_| RecvTimeoutError::Disconnected),
            };
            match answered {
                Err(RecvTimeoutError::Timeout) => return Waited::Open(waiting),
                // The ask lost its sender, so no host can answer it any more.
                Err(RecvTimeoutError::Disconnected) => return Waited::Reply(no_host()),
                Ok(HostReply::Cancelled) => {
                    self.forget(question.id);
                    self.resolved(question.id, AskOutcome::Cancelled);
                    return Waited::Reply(AskReply { outcome: AskOutcome::Cancelled, text: CANCELLED.to_owned() });
                }
                Ok(HostReply::Value { value, digest }) => {
                    // The plan is read again: the user may have read another one.
                    let now = match self.preview(request) {
                        Ok(now) => now,
                        Err(e) => {
                            self.forget(question.id);
                            self.resolved(question.id, AskOutcome::Failed);
                            return Waited::Reply(failed(&e.message));
                        }
                    };
                    if now.digest != digest {
                        // Run nothing, and put the question again as it stands.
                        self.forget(question.id);
                        self.resolved(question.id, AskOutcome::Declined);
                        question.digest = now.digest;
                        question.summary = now.summary;
                        question.changed = true;
                        match self.question(request, question) {
                            Ok((id, next)) => {
                                question.id = id;
                                waiting = next;
                                continue;
                            }
                            Err(reply) => return Waited::Reply(reply),
                        }
                    }
                    return Waited::Reply(match value {
                        Answer::No => {
                            self.forget(question.id);
                            self.resolved(question.id, AskOutcome::Declined);
                            self.reply(question, AskOutcome::Declined, "the user declined")
                        }
                        Answer::Yes => {
                            let (outcome, text) = self.run(request, question);
                            self.forget(question.id);
                            self.resolved(question.id, outcome);
                            self.reply(question, outcome, &text)
                        }
                    });
                }
            }
        }
    }

    /// The reply of one settled question, naming the plan change when the user
    /// answered a question the plan had outrun.
    fn reply(&self, question: &Question, outcome: AskOutcome, text: &str) -> AskReply {
        let text = if question.changed { format!("{text}; {CHANGED}") } else { text.to_owned() };
        AskReply { outcome, text }
    }

    /// Runs the action the user confirmed, with one line about the outcome.
    fn run(&self, request: &AskRequest, question: &Question) -> (AskOutcome, String) {
        let cwd = Path::new(&request.cwd);
        let acted = match request.kind {
            AskKind::Accept => accept(&question.project, &self.home, cwd, &question.change, &question.digest),
            AskKind::Confirm => confirm(&question.project, &question.change, &question.digest),
            AskKind::Archive => archived(&question.project, &self.home, cwd, &question.change, &question.digest),
        };
        match acted {
            Ok(text) => (AskOutcome::Done, text),
            Err(e) => (AskOutcome::Failed, e.message),
        }
    }

    /// What the user is about to decide: the project, the change id as the
    /// store names it, the digest of the plan, and one summary line.
    fn preview(&self, request: &AskRequest) -> Result<Question> {
        let cwd = Path::new(&request.cwd);
        let project = store::find(cwd, &self.home)?.ok_or_else(|| {
            Error::new(
                "ask",
                format!("no rvow project at {}", request.cwd),
                "run rvow init in this repository, or ask from the repository root",
            )
        })?;
        let dir = change::resolve(&project, &request.change)?;
        // The living specs without the lint: a hook path loads them on every call.
        let living = change::living(&project)?;
        let loaded = change::load(&dir.path, &living)?;
        // The pending set of the whole store, which is exactly the set that
        // `amend::confirm` compares its digest against: a question about this
        // branch alone could not be confirmed, since the decision it names
        // covers every pending amendment.
        let pending = amend::pending(&proof::active(&project, &living)?);
        let digest = match request.kind {
            AskKind::Accept => plan_digest(&loaded.texts),
            AskKind::Confirm => amend::digest(&pending),
            AskKind::Archive => format!("{}+{}", plan_digest(&loaded.texts), amend::digest(&pending)),
        };
        let summary = summary(&dir.id.to_string(), loaded.tasks.len(), loaded.deltas.len(), pending.len(), request.kind);
        Ok(Question { project, change: dir.id.to_string(), digest, summary, id: 0, changed: false })
    }

    /// Puts one question to the host, and returns the id it carries with the
    /// channel that waits for its answer.
    fn question(&self, request: &AskRequest, question: &Question) -> std::result::Result<(u64, Receiver<HostReply>), AskReply> {
        let (sender, waiting) = channel();
        let id = {
            let mut open = self.lock_open();
            if open.len() >= MAX_OPEN {
                return Err(failed("too many open questions"));
            }
            let id = self.next.fetch_add(1, Ordering::Relaxed);
            open.insert(id, sender);
            id
        };
        let asked = ToHost::Ask {
            id,
            kind: request.kind,
            change: question.change.clone(),
            digest: question.digest.clone(),
            summary: question.summary.clone(),
            session: request.session.clone(),
        };
        if !self.send(&asked) {
            // The pipe is broken: no host can take this question.
            self.forget(id);
            return Err(no_host());
        }
        Ok((id, waiting))
    }

    /// Takes the ask `id` out of the open set, so no later line reaches it.
    fn forget(&self, id: u64) {
        self.lock_open().remove(&id);
    }

    /// The host's answer to ask `id`. An answer to an ask that is not open is
    /// ignored: only the question the host holds now is decided.
    fn answer(&self, id: u64, value: Answer, digest: String) {
        let sender = self.lock_open().get(&id).cloned();
        if let Some(sender) = sender {
            let _ = sender.send(HostReply::Value { value, digest });
        }
    }

    /// The text of one part of a change, as `rvow show <id> <part>` prints it,
    /// in the folder of the last ask.
    fn get(&self, change: &str, part: &str) {
        let text = self.part(change, part);
        self.send(&ToHost::Part { change: change.to_owned(), part: part.to_owned(), text });
    }

    /// The text of one part, or `None` when there is no such change or part.
    fn part(&self, change: &str, part: &str) -> Option<String> {
        let cwd = self.cwd.lock().ok()?.clone();
        let project = store::find(&cwd, &self.home).ok().flatten()?;
        let dir = change::resolve(&project, change).ok()?;
        show::change_part(&project, &dir, part).ok()
    }

    /// Remembers the folder of an ask, which a `get` reads a plan in.
    fn remember_cwd(&self, request: &AskRequest) {
        if let Ok(mut cwd) = self.cwd.lock() {
            *cwd = PathBuf::from(&request.cwd);
        }
    }

    /// Tells the host how one ask ended.
    fn resolved(&self, id: u64, outcome: AskOutcome) {
        self.send(&ToHost::Resolved { id, outcome });
    }

    /// Writes one line to the host: one `write_all` of one buffer, and a flush.
    /// `false` when the pipe took nothing.
    fn send(&self, line: &ToHost) -> bool {
        let Ok(mut body) = serde_json::to_vec(line) else { return false };
        body.push(b'\n');
        // The lock covers the one write and the flush, and nothing else: an ask
        // never holds it while it waits for the host.
        let mut out = self.out.lock().unwrap_or_else(|e| e.into_inner());
        out.write_all(&body).is_ok() && out.flush().is_ok()
    }

    /// The open asks, under one lock of the map alone.
    fn lock_open(&self) -> std::sync::MutexGuard<'_, HashMap<u64, Sender<HostReply>>> {
        self.open.lock().unwrap_or_else(|e| e.into_inner())
    }
}

/// The digest of a plan: the sha256 of every text of the change, each named and
/// separated, so a changed byte of the plan changes the digest.
fn plan_digest(texts: &[(String, String)]) -> String {
    let mut bytes = Vec::new();
    for (name, text) in texts {
        bytes.extend_from_slice(name.as_bytes());
        bytes.push(0);
        bytes.extend_from_slice(text.as_bytes());
        bytes.push(0);
    }
    rvow_base::hash::sha256_hex(&bytes)
}

/// One line about the plan, never its text: what the action will change, and
/// how much of it there is.
fn summary(change: &str, tasks: usize, specs: usize, pending: usize, kind: AskKind) -> String {
    let mut items: Vec<String> = Vec::new();
    if matches!(kind, AskKind::Accept | AskKind::Archive) {
        items.push(count(tasks, "task"));
        items.push(count(specs, "spec"));
    }
    if pending > 0 {
        items.push(count(pending, "pending amendment"));
    }
    if items.is_empty() {
        items.push("no pending amendment".to_owned());
    }
    format!("{change}: {}", items.join(", "))
}

/// `1 task` or `3 tasks`.
fn count(n: usize, what: &str) -> String {
    if n == 1 { format!("1 {what}") } else { format!("{n} {what}s") }
}

/// Records the vow of a change, with the line `rvow accept` prints. The vow is
/// of the plan with `digest`, checked under the project lock, so an edit after
/// the user answered records nothing.
fn accept(project: &Project, home: &Path, cwd: &Path, change: &str, digest: &str) -> Result<String> {
    let dir = change::resolve(project, change)?;
    let prepared = vow::prepare(project, home, dir, cwd, &mut std::io::sink())?;
    let tasks = prepared.loaded.tasks.len();
    let base = prepared.base_commit[..12.min(prepared.base_commit.len())].to_owned();
    let vowed = vow::commit_seen(project, prepared, &|now| plan_digest(&now.texts) == digest)?;
    Ok(format!("accepted {} at {} (vow {}): {tasks} tasks, base {base}", vowed.id, vowed.path.display(), vowed.event))
}

/// Records the user's decision about every pending amendment, as `rvow confirm
/// --all <digest>` does. The digest is the one the dialog showed, so a set that
/// changed since then records nothing, and an empty set records nothing either.
fn confirm(project: &Project, change: &str, digest: &str) -> Result<String> {
    let decided = amend::confirm(project, &[], amend::Decision::Accept, Some(digest))?;
    let mut ids = String::new();
    for id in &decided {
        if !ids.is_empty() {
            ids.push_str(", ");
        }
        ids.push_str(&id.to_string());
    }
    Ok(if ids.is_empty() { format!("{change}: no pending amendment") } else { format!("{ids}: accepted") })
}

/// Merges an active change into the living specs, with the line `rvow archive`
/// prints. As in the archive dialog, one yes first accepts the pending
/// amendments the question showed: `digest` is the plan digest, `+`, and the
/// amendment digest, so a set that changed since records nothing.
fn archived(project: &Project, home: &Path, cwd: &Path, change: &str, digest: &str) -> Result<String> {
    let (plan, amendments) = digest.rsplit_once('+').unwrap_or((digest, ""));
    let living = change::living(project)?;
    // The archive gate refuses any later edit as drift that no amendment records,
    // so a plan read again here is the one the user answered for.
    let dir = change::resolve(project, change)?;
    if plan_digest(&change::load(&dir.path, &living)?.texts) != plan {
        return Err(Error::new(
            "plan-changed",
            format!("{change}: the plan changed after the user read it, so nothing was archived"),
            "run the archive again so the user reads the plan as it is now",
        ));
    }
    if !amend::pending(&rvow_core::proof::active(project, &living)?).is_empty() {
        amend::confirm(project, &[], amend::Decision::Accept, Some(amendments))?;
    }
    let archived = archive::archive(project, home, cwd, change)?;
    Ok(format!("archived {} to {}", archived.id, archived.path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn daemon_ask_the_plan_digest_covers_every_text() {
        let one = plan_digest(&[("proposal.md".into(), "a".into())]);
        assert_eq!(one.len(), 64, "a sha256 hex digest");
        assert_eq!(one, plan_digest(&[("proposal.md".into(), "a".into())]));
        assert_ne!(one, plan_digest(&[("proposal.md".into(), "b".into())]), "another text");
        assert_ne!(one, plan_digest(&[("tasks.md".into(), "a".into())]), "another name");
        // The separator keeps two files from reading as one.
        assert_ne!(plan_digest(&[("a".into(), "b".into())]), plan_digest(&[("ab".into(), String::new())]));
    }

    #[test]
    fn daemon_ask_the_summary_counts_the_plan_and_never_shows_it() {
        assert_eq!(summary("CHG-x", 3, 2, 1, AskKind::Accept), "CHG-x: 3 tasks, 2 specs, 1 pending amendment");
        assert_eq!(summary("CHG-x", 1, 1, 0, AskKind::Accept), "CHG-x: 1 task, 1 spec");
        assert_eq!(summary("CHG-x", 0, 0, 2, AskKind::Confirm), "CHG-x: 2 pending amendments");
        assert_eq!(summary("CHG-x", 0, 0, 0, AskKind::Confirm), "CHG-x: no pending amendment");
        assert_eq!(summary("CHG-x", 1, 0, 1, AskKind::Archive), "CHG-x: 1 task, 0 specs, 1 pending amendment");
    }
}
