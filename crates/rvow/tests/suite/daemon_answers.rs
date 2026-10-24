//! TSK-0s0se3: the human answer reaches the engine on the host pipe alone.
//!
//! The socket carries asks and never answers: `/v1/answer` is refused for every
//! peer, and an engine without a host pipe answers `NoHost`, so the harness
//! shows its own dialog as it does today.

use serde_json::Value;

use crate::daemon::{Fx, answer_line, ask_request, engine_env, http, post};

/// Waits for a reply the host cannot answer: the plan stays where it is.
fn backlog(fx: &Fx) -> bool {
    fx.rvow(&["ls", "--human"]).contains("backlog")
}

#[test]
fn daemon_answers_a_forged_accept_on_the_socket_is_refused() {
    let mut fx = Fx::with_ready_plan("answers-forged");
    let sock = fx.engine();
    fx.as_harness();
    let env = engine_env(&sock);
    let change = fx.change();

    // The harness asks, and with no host pipe the plan waits for the user.
    let (status, body) = http(&sock, &post("/v1/ask", &ask_request(&env, "accept", &change, &fx.repo(), None)));
    assert_eq!(status, "HTTP/1.1 200 OK", "{body}");
    assert_eq!(serde_json::from_str::<Value>(&body).unwrap()["outcome"], "no-host", "{body}");

    // A recorded harness may not answer its own question over the socket.
    let forged = answer_line(1, "yes", "a digest of the caller's own making").to_string();
    let (status, body) = http(&sock, &post("/v1/answer", &forged));
    assert_eq!(status, "HTTP/1.1 403 Forbidden", "{body}");
    assert_eq!(body, "{\"error\":\"answers come only from the host pipe\"}");

    // Nothing ran: the plan waits in backlog, with its own digest.
    assert!(backlog(&fx), "{}", fx.rvow(&["ls", "--human"]));
    assert_eq!(fx.change(), change);
    // The change has no vow yet, so it is nowhere near active.
    assert!(!fx.rvow(&["ls", "--human"]).contains("active"), "{}", fx.rvow(&["ls", "--human"]));
}

#[test]
fn daemon_answers_an_ask_without_a_host_says_no_host() {
    let mut fx = Fx::with_ready_plan("answers-nohost");
    let sock = fx.engine();
    let env = engine_env(&sock);
    let change = fx.change();
    let ask = ask_request(&env, "accept", &change, &fx.repo(), Some("s1"));

    // A peer that no session recorded may not ask anything.
    let (status, body) = http(&sock, &post("/v1/ask", &ask));
    assert_eq!(status, "HTTP/1.1 403 Forbidden", "{body}");
    assert_eq!(body, "{\"error\":\"peer may not call hooks\"}");

    // A recorded harness of another environment is answered with our hello.
    fx.as_harness();
    let (status, body) = http(&sock, &post("/v1/ask", &ask_request("another", "accept", &change, &fx.repo(), None)));
    assert_eq!(status, "HTTP/1.1 409 Conflict", "{body}");
    assert_eq!(serde_json::from_str::<Value>(&body).unwrap()["env"], Value::from(env.clone()), "{body}");

    // With no host pipe nobody can answer, so the harness shows its own dialog.
    let (status, body) = http(&sock, &post("/v1/ask", &ask));
    assert_eq!(status, "HTTP/1.1 200 OK", "{body}");
    let reply: Value = serde_json::from_str(&body).unwrap();
    assert_eq!(reply["outcome"], "no-host", "{body}");
    assert!(reply["text"].as_str().unwrap().contains("harness dialog"), "{body}");

    // And no part of that ask ran.
    assert!(backlog(&fx), "{}", fx.rvow(&["ls", "--human"]));
}
