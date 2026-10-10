//! Runs a stored verify command: `sh -c` from the repository root, stdin from
//! `/dev/null`, in its own process group, with a timeout.

use std::collections::VecDeque;
use std::io::{BufRead, BufReader, Write};
use std::path::Path;
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::time::{Duration, Instant};

use serde::Serialize;

use crate::error::{Error, Result};

pub const TAIL_LINES: usize = 5;
const KILL_GRACE: Duration = Duration::from_secs(5);
const DRAIN: Duration = Duration::from_secs(1);

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Outcome {
    /// `None` when a signal ended the command.
    pub exit: Option<i32>,
    pub timed_out: bool,
    /// Tests the runner reported, or `None` for a runner rvow cannot read.
    pub tests_run: Option<u64>,
    pub duration_ms: u64,
    /// The last lines of output, masked.
    pub tail: Vec<String>,
}

impl Outcome {
    pub fn passed(&self) -> bool {
        self.exit == Some(0) && !self.timed_out
    }
}

/// Counts tests from runner summaries: libtest `test result:` lines (one per
/// test binary, summed), the final pytest `=== N passed ... ===` line, nextest
/// `Summary [` lines and jest `Tests:` lines (each run summed) and go test
/// package lines (`[no tests to run]` and `[no test files]` count as zero).
/// Nextest, jest and go lines count only where the tool prints them: a line
/// that a test prints itself is indented, or too far indented, and is ignored.
#[derive(Debug, Default)]
struct TestCount {
    libtest: Option<u64>,
    pytest: Option<u64>,
    nextest: Option<u64>,
    jest: Option<u64>,
    go_empty: u64,
    go_real: u64,
}

fn count_before(line: &str, words: &[&str]) -> u64 {
    let tokens: Vec<&str> = line.split(|c: char| c.is_whitespace() || c == ',' || c == ';' || c == '.').filter(|t| !t.is_empty()).collect();
    tokens.windows(2).filter(|w| words.contains(&w[1])).filter_map(|w| w[0].parse::<u64>().ok()).sum()
}

fn indent(line: &str) -> usize {
    line.len() - line.trim_start().len()
}

impl TestCount {
    fn feed(&mut self, line: &str) {
        let t = line.trim();
        if t.starts_with("test result:") {
            *self.libtest.get_or_insert(0) += count_before(t, &["passed", "failed"]);
            return;
        }
        if t.starts_with('=') && t.ends_with('=') && t.contains(" in ") {
            if t.contains("no tests ran") {
                self.pytest = Some(0);
                return;
            }
            let n = count_before(t, &["passed", "failed", "error", "errors", "xfailed", "xpassed"]);
            if n > 0 {
                self.pytest = Some(n);
            }
            return;
        }
        if t.starts_with("Summary [") && indent(line) <= 6 && t.contains(" run:") && t.contains("passed") {
            // nextest: "Summary [ 0.002s] 12 tests run: 12 passed, 0 skipped".
            *self.nextest.get_or_insert(0) += count_before(t, &["tests", "test"]);
            return;
        }
        if line.starts_with("Tests:") && t.contains("total") {
            // jest: "Tests:       1 failed, 12 passed, 13 total".
            *self.jest.get_or_insert(0) += count_before(t, &["total"]);
            return;
        }
        if line.starts_with("ok  \t") {
            // go test: "ok  \tpkg\t0.1s", with a marker when nothing ran.
            if t.contains("[no tests to run]") {
                self.go_empty += 1;
            } else {
                self.go_real += 1;
            }
        } else if line.starts_with("?   \t") && line.contains("[no test files]") {
            self.go_empty += 1;
        }
    }

    fn finish(&self, cmd: &str, exit: Option<i32>) -> Option<u64> {
        if cmd.contains("pytest") && exit == Some(5) {
            return Some(0);
        }
        let go = (self.go_empty > 0 && self.go_real == 0).then_some(0);
        self.libtest.or(self.pytest).or(self.nextest).or(self.jest).or(go)
    }
}

/// The surviving mutants in a cargo-mutants summary line such as
/// `12 mutants tested in 4s: 2 missed, 9 caught, 1 unviable`: the `N` in
/// `N missed` on the last matching line. `None` when the output holds no
/// summary.
pub fn survived(tail: &[String]) -> Option<u64> {
    let mut out = None;
    for line in tail {
        if !line.contains("mutant") || !line.contains("missed") {
            continue;
        }
        let mut rest = line.as_str();
        while let Some(i) = rest.find("missed") {
            let before = rest[..i].trim_end();
            if let Some(n) = before.split(|c: char| !c.is_ascii_digit()).rfind(|s| !s.is_empty())
                && let Ok(v) = n.parse::<u64>()
            {
                out = Some(v);
            }
            rest = &rest[i + "missed".len()..];
        }
    }
    out
}

/// Masks secrets and personal paths in one output line.
pub fn mask(line: &str, home: Option<&str>, repo: &str) -> String {
    let mut masker = TailMask::default();
    line.split('\n').map(|line| masker.line(line, home, repo)).collect::<Vec<_>>().join("\n")
}

#[derive(Default)]
struct TailMask {
    private_key: bool,
}

impl TailMask {
    fn line(&mut self, line: &str, home: Option<&str>, repo: &str) -> String {
        if line.contains("-----BEGIN ") && line.contains("PRIVATE KEY-----") {
            self.private_key = true;
            return "-----BEGIN PRIVATE KEY-----".into();
        }
        if self.private_key {
            if line.contains("-----END ") && line.contains("PRIVATE KEY-----") {
                self.private_key = false;
                return "-----END PRIVATE KEY-----".into();
            }
            return "<masked>".into();
        }
        mask_line(line, home, repo)
    }
}

fn mask_line(line: &str, home: Option<&str>, repo: &str) -> String {
    let mut line = mask_url_credentials(line);
    if !repo.is_empty() {
        line = line.replace(repo, "<repo>");
    }
    if let Some(h) = home.filter(|h| h.len() > 1) {
        line = line.replace(h, "~");
    }
    let is_tok = |c: char| c.is_ascii_alphanumeric() || "_-+/=.:".contains(c);
    let mut out = String::with_capacity(line.len());
    let mut tok = String::new();
    let mut hide_next = false;
    let mut prev_secret = false;
    for c in line.chars().chain(std::iter::once('\n')) {
        if is_tok(c) {
            tok.push(c);
            continue;
        }
        if !tok.is_empty() {
            if hide_next {
                out.push_str("<masked>");
                hide_next = false;
            } else {
                let t = tok.as_str();
                out.push_str(&mask_token(t));
                hide_next = (names_secret(t) && t.ends_with(['=', ':'])) || ((t == "=" || t == ":") && prev_secret);
                prev_secret = names_secret(t) && !t.contains(['=', ':']);
            }
            tok.clear();
        }
        if c != '\n' {
            out.push(c);
        }
    }
    out
}

pub(crate) fn names_secret(name: &str) -> bool {
    let upper = name.to_ascii_uppercase();
    SECRET_NAMES.iter().any(|s| upper.contains(s))
}

const PREFIXES: &[&str] = &["ghp_", "gho_", "ghu_", "ghs_", "github_pat_", "glpat-", "xoxb-", "xoxp-", "sk-", "AKIA"];
const SECRET_NAMES: &[&str] = &["TOKEN", "SECRET", "KEY", "PASSWORD", "PASSWD"];

fn mask_token(tok: &str) -> String {
    if let Some((name, value)) = tok.split_once(['=', ':'])
        && !value.is_empty()
        && names_secret(name)
    {
        let sep = &tok[name.len()..name.len() + 1];
        return format!("{name}{sep}<masked>");
    }
    let secret = PREFIXES.iter().any(|p| tok.starts_with(p) && tok.len() > p.len() + 8)
        || (tok.starts_with("eyJ") && tok.matches('.').count() == 2)
        || (tok.len() >= 32
            && !tok.contains('/')
            && !(matches!(tok.len(), 40 | 64) && tok.chars().all(|c| c.is_ascii_hexdigit()))
            && tok.chars().all(|c| c.is_ascii_alphanumeric() || "+/=_-".contains(c)));
    if secret { "<masked>".into() } else { tok.into() }
}

fn mask_url_credentials(line: &str) -> String {
    let mut out = String::new();
    let mut rest = line;
    while let Some(start) = rest.find("://") {
        let end = start + 3;
        out.push_str(&rest[..end]);
        rest = &rest[end..];
        let authority_end = rest.find(|c: char| c.is_whitespace() || "/?#\"'".contains(c)).unwrap_or(rest.len());
        let authority = &rest[..authority_end];
        if let Some(at) = authority.rfind('@') {
            out.push_str("<masked>@");
            out.push_str(&authority[at + 1..]);
        } else {
            out.push_str(authority);
        }
        rest = &rest[authority_end..];
    }
    out.push_str(rest);
    out
}

#[cfg(unix)]
fn signal_group(pid: u32, sig: i32) {
    // SAFETY: kill with a negative pid signals the process group that
    // `process_group(0)` created for the child; it touches no memory.
    unsafe {
        libc::kill(-(pid as i32), sig);
    }
}

/// Runs `cmd` with the extra variables `env` and streams its output to `echo`
/// while it runs.
pub fn run(cmd: &str, cwd: &Path, env: &[(String, String)], timeout: Duration, echo: &mut dyn Write) -> Result<Outcome> {
    let (reader, writer) = std::io::pipe().map_err(|e| Error::io(cwd, e))?;
    let mut command = Command::new("sh");
    command
        .arg("-c")
        .arg(cmd)
        .current_dir(cwd)
        .envs(env.iter().map(|(k, v)| (k, v)))
        .env_remove("GIT_DIR")
        .env_remove("GIT_WORK_TREE")
        .env_remove("GIT_INDEX_FILE")
        .stdin(Stdio::null())
        .stdout(writer.try_clone().map_err(|e| Error::io(cwd, e))?)
        .stderr(writer);
    #[cfg(unix)]
    std::os::unix::process::CommandExt::process_group(&mut command, 0);
    let start = Instant::now();
    let mut child = command.spawn().map_err(|e| Error::new("spawn", format!("cannot run sh: {e}"), "check that sh is on PATH"))?;
    drop(command);

    let (tx, rx) = mpsc::channel::<String>();
    std::thread::spawn(move || {
        for line in BufReader::new(reader).split(b'\n').map_while(|l| l.ok()) {
            if tx.send(String::from_utf8_lossy(&line).into_owned()).is_err() {
                break;
            }
        }
    });

    let home = std::env::var("HOME").ok();
    let repo = cwd.display().to_string();
    let mut count = TestCount::default();
    let mut tail = VecDeque::with_capacity(TAIL_LINES + 1);
    let mut masker = TailMask::default();
    let mut take = |line: String, echo: &mut dyn Write| {
        let _ = writeln!(echo, "{line}");
        count.feed(&line);
        tail.push_back(masker.line(&line, home.as_deref(), &repo));
        if tail.len() > TAIL_LINES {
            tail.pop_front();
        }
    };

    let deadline = start + timeout;
    let mut timed_out = false;
    let status = loop {
        while let Ok(line) = rx.try_recv() {
            take(line, echo);
        }
        if let Some(status) = child.try_wait().map_err(|e| Error::io(cwd, e))? {
            break status;
        }
        if !timed_out && Instant::now() >= deadline {
            timed_out = true;
            #[cfg(unix)]
            signal_group(child.id(), libc::SIGTERM);
            #[cfg(not(unix))]
            let _ = child.kill();
            let grace = Instant::now() + KILL_GRACE;
            while Instant::now() < grace {
                if child.try_wait().map_err(|e| Error::io(cwd, e))?.is_some() {
                    break;
                }
                std::thread::sleep(Duration::from_millis(50));
            }
            #[cfg(unix)]
            signal_group(child.id(), libc::SIGKILL);
            let _ = child.kill();
        }
        if let Ok(line) = rx.recv_timeout(Duration::from_millis(50)) {
            take(line, echo);
        }
    };
    // Children that the command left in the background would hold the pipe
    // open for ever, so the group is ended and the drain is bounded.
    #[cfg(unix)]
    signal_group(child.id(), libc::SIGKILL);
    let drain_end = Instant::now() + DRAIN;
    while let Ok(line) = rx.recv_timeout(drain_end.saturating_duration_since(Instant::now())) {
        take(line, echo);
    }
    let exit = status.code();
    Ok(Outcome { exit, timed_out, tests_run: count.finish(cmd, exit), duration_ms: start.elapsed().as_millis() as u64, tail: tail.into() })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn counts_surviving_mutants() {
        let summary = |s: &str| survived(&[s.to_string()]);
        assert_eq!(summary("12 mutants tested in 4s: 2 missed, 9 caught, 1 unviable"), Some(2));
        assert_eq!(summary("1 mutant tested: 0 missed, 1 caught"), Some(0));
        assert_eq!(summary("ok"), None);
        assert_eq!(summary("missed the bus"), None, "no mutant word, no count");
        let tail = vec!["12 mutants tested in 4s: 2 missed, 9 caught".to_string(), "done".to_string()];
        assert_eq!(survived(&tail), Some(2));
    }

    fn quiet(cmd: &str, timeout: Duration) -> Outcome {
        let dir = std::env::temp_dir();
        run(cmd, &dir, &[], timeout, &mut std::io::sink()).unwrap()
    }

    #[test]
    fn exit_status_and_tail() {
        let o = quiet("for i in 1 2 3 4 5 6 7; do echo line$i; done; exit 3", Duration::from_secs(10));
        assert_eq!(o.exit, Some(3));
        assert!(!o.passed());
        assert_eq!(o.tail, ["line3", "line4", "line5", "line6", "line7"]);
        assert_eq!(o.tests_run, None);
    }

    #[test]
    fn stdin_is_empty_and_stderr_is_kept() {
        let o = quiet("read x || echo no-input >&2", Duration::from_secs(10));
        assert!(o.passed());
        assert_eq!(o.tail, ["no-input"]);
    }

    #[test]
    fn timeout_kills_the_group() {
        let o = quiet("sleep 30 & sleep 30", Duration::from_millis(300));
        assert!(o.timed_out);
        assert!(!o.passed());
        assert!(o.duration_ms < 10_000, "{}", o.duration_ms);
    }

    #[test]
    fn counts_libtest_and_pytest() {
        let o = quiet(
            "echo 'test result: ok. 3 passed; 1 failed; 0 ignored'; echo 'test result: ok. 0 passed; 0 failed;'",
            Duration::from_secs(10),
        );
        assert_eq!(o.tests_run, Some(4));
        let o = quiet("echo '========== 2 passed, 1 failed in 0.12s =========='", Duration::from_secs(10));
        assert_eq!(o.tests_run, Some(3));
        let o = quiet("echo pytest; exit 5", Duration::from_secs(10));
        assert_eq!(o.tests_run, Some(0));
        let o = quiet("echo 'test result: ok. 0 passed; 0 failed; 5 filtered out'", Duration::from_secs(10));
        assert_eq!(o.tests_run, Some(0));
    }

    #[test]
    fn counts_nextest_jest_and_go() {
        let nextest = quiet("echo '     Summary [   0.002s] 12 tests run: 12 passed, 0 skipped'", Duration::from_secs(10));
        assert_eq!(nextest.tests_run, Some(12), "nextest counts its tests");
        let jest = quiet("echo 'Tests:       1 failed, 12 passed, 13 total'", Duration::from_secs(10));
        assert_eq!(jest.tests_run, Some(13), "jest counts its total");
        let go = quiet("echo 'ok  \texample \t0.006s [no tests to run]'", Duration::from_secs(10));
        assert_eq!(go.tests_run, Some(0), "go with no tests to run is zero");
    }

    #[test]
    fn masks_secrets_and_paths() {
        let m = |s: &str| mask(s, Some("/Users/me"), "/Users/me/code/r");
        assert_eq!(m("GITHUB_TOKEN=abc123 ok"), "GITHUB_TOKEN=<masked> ok");
        assert_eq!(m("api_key: hunter2 next"), "api_key: <masked> next");
        assert_eq!(m("PASSWORD = x y"), "PASSWORD = <masked> y");
        assert_eq!(m("a = b"), "a = b");
        assert_eq!(m("api_key:hunter2"), "api_key:<masked>");
        assert_eq!(m("token ghp_ABCDEFGHIJKLMNOP1234"), "token <masked>");
        assert_eq!(m("AKIAABCDEFGHIJKLMNOP"), "<masked>");
        assert_eq!(m("eyJhbGc.eyJzdWI.sig"), "<masked>");
        assert_eq!(m("blob 0123456789abcdef0123456789abcdef0123456z"), "blob <masked>");
        assert_eq!(m("at /Users/me/code/r/src/a.rs:3"), "at <repo>/src/a.rs:3");
        assert_eq!(m("cache /Users/me/.cargo"), "cache ~/.cargo");
        assert_eq!(m("test result: ok. 3 passed"), "test result: ok. 3 passed");
        // A multi-line PEM private key block: the header stays, the key bodies are masked.
        let pem = "-----BEGIN PRIVATE KEY-----\nMIIEvgIBADANBgkqhkiG9w0BAQEFAASCBKgwggSkAgEAAoIBAQC7VJ\n-----END PRIVATE KEY-----";
        let out = m(pem);
        assert!(out.contains("BEGIN PRIVATE KEY") && out.contains("END PRIVATE KEY"), "{out}");
        assert!(!out.contains("MIIEvgIBADANBgkqhkiG9w0BAQEFAASCBKgwggSkAgEAAoIBAQC7VJ"), "key body leaked: {out}");
    }
}
