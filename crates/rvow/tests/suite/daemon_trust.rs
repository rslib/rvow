//! TSK-r3e196: the engine serves hook calls only to a harness process that a
//! session-start hook recorded, by the peer process id of the connection.

use serde_json::{Value, json};

use crate::daemon::{Fx, engine_env, http, post, request};

/// A post-tool-use call whose Bash result would record an observed proof.
fn post_tool_use(fx: &Fx) -> Value {
    json!({
        "cwd": fx.repo(),
        "session_id": "s1",
        "tool_name": "Bash",
        "tool_input": {"command": "true"},
        "tool_response": {"stdout": "test result: ok. 1 passed; 0 failed;", "stderr": "", "exit_code": 0},
    })
}

#[test]
fn daemon_trust_an_unrecorded_peer_is_refused() {
    let mut fx = Fx::with_plan("untrusted");
    let sock = fx.engine();
    let env = engine_env(&sock);
    let (status, body) = http(&sock, &post("/v1/hook", &request(&env, post_tool_use(&fx))));
    assert_eq!(status, "HTTP/1.1 403 Forbidden", "{body}");
    assert_eq!(body, "{\"error\":\"peer may not call hooks\"}");
}

#[test]
fn daemon_trust_a_recorded_harness_is_served() {
    let mut fx = Fx::with_plan("trusted");
    let sock = fx.engine();
    fx.as_harness();
    let env = engine_env(&sock);
    let (status, body) = http(&sock, &post("/v1/hook", &request(&env, post_tool_use(&fx))));
    assert_eq!(status, "HTTP/1.1 200 OK", "{body}");
    let answer: Value = serde_json::from_str(&body).unwrap();
    assert!(answer["mismatch"].is_null(), "{body}");
}

#[test]
fn daemon_trust_hello_needs_no_harness() {
    let mut fx = Fx::new("hello");
    let sock = fx.engine();
    assert!(!engine_env(&sock).is_empty());
}
