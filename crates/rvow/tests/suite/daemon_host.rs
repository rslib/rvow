//! TSK-6ssn2x: the host pipe carries the questions and the answers.
//!
//! Each test starts `rvow daemon --host-pipe` as a child and is its host: the
//! questions come in on the child's stdout, and the answers go out on its
//! stdin. The socket only asks.

use std::thread;
use std::time::Duration;

use serde_json::{Value, json};

use crate::daemon::{Fx, answer_line, ask_request, engine_env, get_line, http, post};

/// The stage of the only change of the store, from `rvow ls`.
fn stage(fx: &Fx) -> String {
    let listed = fx.rvow(&["ls", "--human"]);
    listed.split_whitespace().nth(1).unwrap_or_default().to_owned()
}

/// One ask over the socket, which waits for the host: it runs in its own thread
/// so this test can answer it in time.
fn asked(sock: std::path::PathBuf, body: String) -> thread::JoinHandle<(String, String)> {
    thread::spawn(move || http(&sock, &post("/v1/ask", &body)))
}

/// The one reply of an ask that ended.
fn reply(body: &str, outcome: &str) -> Value {
    let reply: Value = serde_json::from_str(body).unwrap_or_else(|e| panic!("{e}: {body}"));
    assert_eq!(reply["outcome"], outcome, "{body}");
    reply
}

#[test]
fn daemon_host_accept_in_time() {
    let mut fx = Fx::with_ready_plan("host-accept");
    let sock = fx.host("30s");
    fx.as_harness();
    let env = engine_env(&sock);
    let change = fx.change();
    let ask = asked(sock.clone(), ask_request(&env, "accept", &change, &fx.repo(), Some("s1")));

    // The question the host shows the user: one line about the plan.
    let question = fx.next_line();
    assert_eq!(question["type"], "ask", "{question}");
    assert_eq!(question["kind"], "accept", "{question}");
    assert_eq!(question["change"], Value::from(change.clone()), "{question}");
    assert_eq!(question["session"], "s1", "{question}");
    let summary = question["summary"].as_str().unwrap();
    assert_eq!(summary, format!("{change}: 1 task, 1 spec"), "{summary}");
    assert!(!summary.contains("rvow SHALL"), "the summary is not the plan text: {summary}");
    let id = question["id"].as_u64().unwrap();
    let digest = question["digest"].as_str().unwrap().to_owned();
    assert_eq!(digest.len(), 64, "a sha256 hex digest: {digest}");

    // The user reads the dialog for a moment, then says yes.
    thread::sleep(Duration::from_millis(200));
    fx.tell(answer_line(id, "yes", &digest));

    assert_eq!(fx.next_line(), json!({"type": "resolved", "id": id, "outcome": "done"}));
    let (status, body) = ask.join().unwrap();
    assert_eq!(status, "HTTP/1.1 200 OK", "{body}");
    let answered = reply(&body, "done");
    assert!(answered["text"].as_str().unwrap().starts_with(&format!("accepted {change} at ")), "{body}");
    assert!(answered["text"].as_str().unwrap().contains("1 tasks, base "), "{body}");

    assert_eq!(stage(&fx), "active", "{}", fx.rvow(&["ls", "--human"]));
}

#[test]
fn daemon_host_a_changed_plan_is_asked_again() {
    let mut fx = Fx::with_ready_plan("host-changed");
    let sock = fx.host("30s");
    fx.as_harness();
    let env = engine_env(&sock);
    let change = fx.change();
    let ask = asked(sock.clone(), ask_request(&env, "accept", &change, &fx.repo(), None));

    let first = fx.next_line();
    let id = first["id"].as_u64().unwrap();
    let digest = first["digest"].as_str().unwrap().to_owned();

    // The agent writes the plan once more while the user reads it.
    let proposal = fx.change_dir(&change).join("proposal.md");
    let text = std::fs::read_to_string(&proposal).unwrap();
    std::fs::write(&proposal, format!("{text}A note written while the user read the plan.\n")).unwrap();

    // The answer names the plan the user read, which is not the plan any more.
    fx.tell(answer_line(id, "yes", &digest));
    assert_eq!(fx.next_line(), json!({"type": "resolved", "id": id, "outcome": "declined"}));
    let second = fx.next_line();
    assert_eq!(second["type"], "ask", "{second}");
    assert_ne!(second["id"], Value::from(id), "{second}");
    assert_ne!(second["digest"], Value::from(digest), "{second}");
    assert_eq!(second["change"], Value::from(change.clone()), "{second}");
    assert_eq!(stage(&fx), "backlog", "nothing ran for the plan that changed: {}", fx.rvow(&["ls", "--human"]));

    // The user reads the plan as it stands now and says yes.
    let next_id = second["id"].as_u64().unwrap();
    let next_digest = second["digest"].as_str().unwrap().to_owned();
    fx.tell(answer_line(next_id, "yes", &next_digest));
    assert_eq!(fx.next_line(), json!({"type": "resolved", "id": next_id, "outcome": "done"}));

    let (status, body) = ask.join().unwrap();
    assert_eq!(status, "HTTP/1.1 200 OK", "{body}");
    let answered = reply(&body, "done");
    assert!(answered["text"].as_str().unwrap().contains("the plan changed while the user read it"), "{body}");
    assert_eq!(stage(&fx), "active", "{}", fx.rvow(&["ls", "--human"]));
}

#[test]
fn daemon_host_no_answer_is_waiting() {
    let mut fx = Fx::with_ready_plan("host-waiting");
    let sock = fx.host("1s");
    fx.as_harness();
    let env = engine_env(&sock);
    let change = fx.change();
    let ask = asked(sock.clone(), ask_request(&env, "accept", &change, &fx.repo(), None));

    let question = fx.next_line();
    let id = question["id"].as_u64().unwrap();
    let digest = question["digest"].as_str().unwrap().to_owned();

    // The user says nothing, so the hook is told to end its turn.
    let (status, body) = ask.join().unwrap();
    assert_eq!(status, "HTTP/1.1 200 OK", "{body}");
    let answered = reply(&body, "waiting");
    assert_eq!(answered["text"], "still waiting for the user; end your turn", "{body}");
    assert_eq!(stage(&fx), "backlog", "nothing ran yet: {}", fx.rvow(&["ls", "--human"]));

    // The ask stays open: the user answers later, and the action still runs.
    fx.tell(answer_line(id, "yes", &digest));
    assert_eq!(fx.next_line(), json!({"type": "resolved", "id": id, "outcome": "done"}));
    assert_eq!(stage(&fx), "active", "{}", fx.rvow(&["ls", "--human"]));
}

#[test]
fn daemon_host_the_host_quits_during_an_ask() {
    let mut fx = Fx::with_ready_plan("host-quits");
    let sock = fx.host("30s");
    fx.as_harness();
    let env = engine_env(&sock);
    let change = fx.change();
    let ask = asked(sock.clone(), ask_request(&env, "accept", &change, &fx.repo(), None));

    let question = fx.next_line();
    let id = question["id"].as_u64().unwrap();

    // The host quits: the engine's stdin reaches its end.
    fx.close_pipe();
    let (status, body) = ask.join().unwrap();
    assert_eq!(status, "HTTP/1.1 200 OK", "{body}");
    reply(&body, "cancelled");
    assert_eq!(fx.next_line(), json!({"type": "resolved", "id": id, "outcome": "cancelled"}));
    assert_eq!(stage(&fx), "backlog", "the ask ran nothing: {}", fx.rvow(&["ls", "--human"]));

    // With the host gone, a later ask says so at once.
    let (status, body) = http(&sock, &post("/v1/ask", &ask_request(&env, "accept", &change, &fx.repo(), None)));
    assert_eq!(status, "HTTP/1.1 200 OK", "{body}");
    let answered = reply(&body, "no-host");
    assert!(answered["text"].as_str().unwrap().contains("no host pipe"), "{body}");
    assert_eq!(stage(&fx), "backlog", "{}", fx.rvow(&["ls", "--human"]));
}

#[test]
fn daemon_host_get_returns_one_part() {
    let mut fx = Fx::with_ready_plan("host-get");
    let sock = fx.host("30s");
    let change = fx.change();

    fx.tell(get_line(&change, "tasks"));
    let part = fx.next_line();
    assert_eq!(part["type"], "part", "{part}");
    assert_eq!(part["change"], Value::from(change.clone()), "{part}");
    assert_eq!(part["part"], "tasks", "{part}");
    assert!(part["text"].as_str().unwrap().contains("TSK-new1 Write a"), "{part}");

    // A line that does not parse, an answer to an ask nobody opened, and a line
    // over a megabyte are all ignored: the next part is still answered.
    let over = "x".repeat(1024 * 1024 + 100);
    fx.tell(json!("not a line of ours"));
    fx.tell(answer_line(u64::MAX, "yes", "x"));
    fx.tell(json!(over));
    fx.tell(get_line(&change, "proposal"));
    let proposal = fx.next_line();
    assert_eq!(proposal["type"], "part", "{proposal}");
    assert_eq!(proposal["part"], "proposal", "{proposal}");
    assert!(proposal["text"].as_str().unwrap().contains("# Out"), "{proposal}");

    // A part that the change does not have is answered with no text at all.
    fx.tell(get_line(&change, "nope"));
    let none = fx.next_line();
    assert_eq!(none["type"], "part", "{none}");
    assert_eq!(none["part"], "nope", "{none}");
    assert!(none["text"].is_null(), "{none}");

    // The engine is still the one serving that socket.
    assert!(!engine_env(&sock).is_empty(), "the engine still answers its handshake");
}

#[test]
fn daemon_host_archive_confirms_the_shown_amendments() {
    let mut fx = Fx::with_ready_plan("host-archive");
    let change = fx.change();
    fx.rvow(&["accept", &change, "--yes"]);
    std::fs::write(fx.repo().join("src/a.rs"), "y").unwrap();
    let task = fx.rvow(&["status", "--human"]).split_whitespace().find(|w| w.starts_with("TSK-")).unwrap().to_owned();
    fx.rvow(&["prove", &task]);
    // One pending amendment: the task's scope takes in another file.
    let tasks = fx.change_dir(&change).join("tasks.md");
    let text = std::fs::read_to_string(&tasks).unwrap();
    std::fs::write(&tasks, text.replace("  files: src/a.rs\n", "  files: src/a.rs, src/b.rs\n")).unwrap();
    fx.rvow(&["amend", "--reason", "take in b"]);

    let sock = fx.host("30s");
    fx.as_harness();
    let env = engine_env(&sock);
    let ask = asked(sock.clone(), ask_request(&env, "archive", &change, &fx.repo(), None));
    let question = fx.next_line();
    assert_eq!(question["kind"], "archive", "{question}");
    let digest = question["digest"].as_str().unwrap().to_owned();
    assert!(digest.contains('+'), "the plan digest and the amendment digest: {digest}");
    let id = question["id"].as_u64().unwrap();
    fx.tell(answer_line(id, "yes", &digest));

    let (status, body) = ask.join().unwrap();
    assert_eq!(status, "HTTP/1.1 200 OK", "{body}");
    reply(&body, "done");
    assert_eq!(stage(&fx), "done", "one yes accepts the amendment and archives: {}", fx.rvow(&["ls", "--human"]));
}
