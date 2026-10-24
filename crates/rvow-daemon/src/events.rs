//! The host pipe's reader: one line in, one answer to the ask it names.
//!
//! The engine's stdin belongs to the process that started it, so a line that is
//! too long or does not parse is dropped: a broken host never takes the engine
//! down with it. When the pipe closes, the engine knows the host is gone, which
//! cancels every ask that still waits.

use std::io::{BufRead, BufReader, Read};
use std::sync::Arc;

use rvow_proto::FromHost;

use crate::ask::Host;

/// The longest line the host may send, which is far more than any line of ours.
const MAX_LINE: u64 = 1024 * 1024;

/// Reads `FromHost` lines from `input` until it closes, then marks the host
/// gone.
pub fn read(host: Arc<Host>, input: impl Read + Send + 'static) {
    let mut lines = BufReader::new(input);
    let mut line: Vec<u8> = Vec::new();
    while next_line(&mut lines, &mut line) {
        if let Ok(from) = serde_json::from_slice::<FromHost>(&line) {
            host.line(from);
        }
    }
    host.host_gone();
}

/// Reads one line without its newline; `false` when the host sent nothing more.
/// A line over `MAX_LINE` is dropped whole, so no part of it is ever answered.
fn next_line(input: &mut impl BufRead, line: &mut Vec<u8>) -> bool {
    line.clear();
    let mut over = false;
    loop {
        // The cap bounds the buffer, so a host that never sends a newline can
        // not grow this process without end.
        match input.by_ref().take(MAX_LINE + 1).read_until(b'\n', line) {
            // The pipe closed: the host is gone.
            Ok(0) => return false,
            Ok(_) if line.ends_with(b"\n") => break,
            // Too long for one line of the wire: drop it and read on.
            Ok(_) => {
                over = true;
                line.clear();
            }
            Err(_) => return false,
        }
    }
    line.pop();
    if line.last() == Some(&b'\r') {
        line.pop();
    }
    if over {
        line.clear();
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::path::Path;
    use std::sync::Mutex;
    use std::time::Duration;

    /// A pipe end a test can read back.
    #[derive(Clone)]
    struct Recorder(Arc<Mutex<Vec<u8>>>);

    impl std::io::Write for Recorder {
        fn write(&mut self, body: &[u8]) -> std::io::Result<usize> {
            self.0.lock().unwrap().extend_from_slice(body);
            Ok(body.len())
        }

        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    #[test]
    fn daemon_events_reads_the_lines_of_the_host_and_ignores_the_rest() {
        let out = Arc::new(Mutex::new(Vec::new()));
        let host = Arc::new(Host::of(
            Path::new("/no/such/home"),
            Duration::from_secs(1),
            Path::new("/no/such/folder"),
            Box::new(Recorder(Arc::clone(&out))),
        ));
        assert!(host.live(), "the pipe is open until its end");
        let lines = concat!(
            "not a line of ours\n",
            "{\"type\": \"answer\", \"id\": 9, \"value\": \"yes\", \"digest\": \"d\"}\n",
            "{\"type\": \"get\", \"change\": \"CHG-x\", \"part\": \"tasks\"}\n",
        );
        read(Arc::clone(&host), lines.as_bytes());
        assert!(!host.live(), "the end of the pipe is the host gone");

        // An answer to an ask nobody opened writes nothing, and the part of a
        // change that no project has is answered with no text at all.
        let written = String::from_utf8(out.lock().unwrap().clone()).unwrap();
        assert_eq!(written.lines().count(), 1, "{written}");
        assert_eq!(written, "{\"type\":\"part\",\"change\":\"CHG-x\",\"part\":\"tasks\",\"text\":null}\n");
    }

    #[test]
    fn daemon_events_drops_a_line_that_is_too_long_for_the_wire() {
        let mut over = vec![b'x'; (MAX_LINE as usize) + 10];
        over.push(b'\n');
        over.extend_from_slice(b"{\"type\": \"get\", \"change\": \"C\", \"part\": \"p\"}\n");
        let mut lines: Vec<u8> = Vec::new();
        let mut input = BufReader::new(&over[..]);
        assert!(next_line(&mut input, &mut lines), "the line ends at its newline");
        assert!(lines.is_empty(), "the head of a line over the cap is dropped whole");
        assert!(next_line(&mut input, &mut lines), "the line after it reads as itself");
        assert_eq!(lines, b"{\"type\": \"get\", \"change\": \"C\", \"part\": \"p\"}");
        assert!(!next_line(&mut input, &mut lines), "the pipe reached its end");
    }

    #[test]
    fn daemon_events_reads_one_line_without_its_endings() {
        let mut lines: Vec<u8> = Vec::new();
        let mut input = BufReader::new(&b"{\"a\":1}\r\n{broken\n"[..]);
        assert!(next_line(&mut input, &mut lines));
        assert_eq!(lines, b"{\"a\":1}", "the newline and the carriage return are not the line");
        assert!(next_line(&mut input, &mut lines));
        assert_eq!(lines, b"{broken", "a line that does not parse is still one line");
        assert!(!next_line(&mut input, &mut lines));
    }
}
