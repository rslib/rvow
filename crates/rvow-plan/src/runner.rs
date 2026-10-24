//! Runs a stored verify command: `sh -c` from the repository root, stdin from
//! `/dev/null`, in its own process group, with a timeout.

use std::borrow::Cow;
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
/// Node `--test` summaries (`\u{2139} tests N` or TAP `# tests N`, the last one
/// wins), vitest `Tests  ... (T)` totals and mocha `N passing` plus `N failing`
/// lines count the same way. Node reports a file whose tests a filter left out
/// as one passing test named by the file, so those results do not count.
/// A top-level node `--test` result named by a test file, which node prints
/// for a file in which no test ran: `\u{2139}`-style `\u{2714} a.test.ts (5ms)` or
/// TAP `ok 2 - a.test.ts`.
fn node_file_result(line: &str) -> bool {
    let name = if let Some(rest) = line.strip_prefix("\u{2714} ") {
        rest.rsplit_once(" (").map_or(rest, |(name, _)| name)
    } else if let Some(rest) = line.strip_prefix("ok ") {
        let Some((num, name)) = rest.split_once(" - ") else { return false };
        if !num.bytes().all(|b| b.is_ascii_digit()) {
            return false;
        }
        name
    } else {
        return false;
    };
    let ext = name.rsplit_once('.').map(|(_, e)| e);
    !name.contains(char::is_whitespace) && matches!(ext, Some("js" | "mjs" | "cjs" | "ts" | "mts" | "cts"))
}

#[derive(Debug, Default)]
struct TestCount {
    /// The libtest count of each cargo run: a `Finished` line starts the next run.
    libtest: Vec<u64>,
    cargo_finished: bool,
    pytest: Option<u64>,
    nextest: Option<u64>,
    jest: Option<u64>,
    node: Option<u64>,
    node_files: u64,
    vitest: Option<u64>,
    mocha_passing: Option<u64>,
    mocha_failing: Option<u64>,
    go_empty: u64,
    go_real: u64,
    ctest: Option<u64>,
    doctest: Option<u64>,
    gtest: Option<u64>,
    unity: Option<u64>,
}

fn count_before(line: &str, words: &[&str]) -> u64 {
    let tokens: Vec<&str> = line.split(|c: char| c.is_whitespace() || c == ',' || c == ';' || c == '.').filter(|t| !t.is_empty()).collect();
    tokens.windows(2).filter(|w| words.contains(&w[1])).filter_map(|w| w[0].parse::<u64>().ok()).sum()
}

/// The first whole number in `text`, after spaces: doctest pads its counts.
fn leading_number(text: &str) -> Option<u64> {
    let text = text.trim_start();
    let end = text.find(|c: char| !c.is_ascii_digit()).unwrap_or(text.len());
    text[..end].parse().ok()
}

/// The test count of a C or C++ summary line that starts at column 0, or
/// `None` for any other line: ctest `N% tests passed, F tests failed out of
/// M` and `No tests were found!!!`, the doctest `[doctest] test cases:` line,
/// the GoogleTest `[==========] N tests from M test suites ran.` line and the
/// Unity `N Tests F Failures I Ignored` line. A line a test prints itself is
/// indented or has a prefix, so it never matches.
fn cxx_summary(line: &str) -> Option<(CxxRunner, u64)> {
    if line.starts_with("No tests were found!!!") {
        return Some((CxxRunner::Ctest, 0));
    }
    if let Some(rest) = line.strip_prefix("[doctest] test cases:") {
        let fields: Vec<&str> = rest.split('|').collect();
        let shaped = fields.iter().skip(1).any(|f| f.contains("passed")) && fields.iter().skip(1).any(|f| f.contains("failed"));
        return shaped.then(|| leading_number(fields[0])).flatten().map(|n| (CxxRunner::Doctest, n));
    }
    if let Some(rest) = line.strip_prefix("[==========] ") {
        let mut tokens = rest.split_whitespace();
        let n = tokens.next()?.parse::<u64>().ok()?;
        let shaped = matches!(tokens.next(), Some("test" | "tests")) && tokens.next() == Some("from") && rest.contains(" ran.");
        return shaped.then_some((CxxRunner::Gtest, n));
    }
    if let Some((percent, rest)) = line.split_once("% tests passed, ")
        && !percent.is_empty()
        && percent.bytes().all(|b| b.is_ascii_digit())
    {
        let total = rest.split_once(" tests failed out of ")?.1.trim();
        return total.parse::<u64>().ok().map(|n| (CxxRunner::Ctest, n));
    }
    let tokens: Vec<&str> = line.split(' ').collect();
    if let [n, "Tests", failures, "Failures", ignored, "Ignored"] = tokens[..]
        && failures.parse::<u64>().is_ok()
        && let Ok(ignored) = ignored.parse::<u64>()
    {
        // An ignored test did not run.
        return n.parse::<u64>().ok().map(|n| (CxxRunner::Unity, n.saturating_sub(ignored)));
    }
    None
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CxxRunner {
    Ctest,
    Doctest,
    Gtest,
    Unity,
}

fn indent(line: &str) -> usize {
    line.len() - line.trim_start().len()
}

/// One output line without its ANSI escape sequences, so a runner that prints
/// its summary in color is read the same as one that does not. A sequence is
/// `ESC`, then `[`, then any bytes, then one final byte in 0x40..=0x7e.
fn uncolored(line: &str) -> Cow<'_, str> {
    if !line.contains('\u{1b}') {
        return Cow::Borrowed(line);
    }
    let mut out = String::with_capacity(line.len());
    let mut chars = line.chars();
    while let Some(c) = chars.next() {
        if c != '\u{1b}' {
            out.push(c);
            continue;
        }
        if chars.next() == Some('[') {
            for c in chars.by_ref() {
                if ('\u{40}'..='\u{7e}').contains(&c) {
                    break;
                }
            }
        }
    }
    Cow::Owned(out)
}

impl TestCount {
    fn feed(&mut self, line: &str) {
        // The color codes go before the parsers read the line; the tail that
        // is stored for display keeps them.
        let plain = uncolored(line);
        let line = plain.as_ref();
        let t = line.trim();
        if t.starts_with("Finished `") && t.contains(" target(s) in ") {
            self.cargo_finished = true;
            return;
        }
        if t.starts_with("test result:") {
            if self.cargo_finished || self.libtest.is_empty() {
                self.libtest.push(0);
                self.cargo_finished = false;
            }
            if let Some(run) = self.libtest.last_mut() {
                *run += count_before(t, &["passed", "failed"]);
            }
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
        if let Some((runner, n)) = cxx_summary(line) {
            let slot = match runner {
                CxxRunner::Ctest => &mut self.ctest,
                CxxRunner::Doctest => &mut self.doctest,
                CxxRunner::Gtest => &mut self.gtest,
                CxxRunner::Unity => &mut self.unity,
            };
            *slot.get_or_insert(0) += n;
            return;
        }
        if node_file_result(line) {
            self.node_files += 1;
            return;
        }
        if let Some(n) =
            line.strip_prefix("\u{2139} tests ").or_else(|| line.strip_prefix("# tests ")).and_then(|r| r.trim().parse::<u64>().ok())
        {
            self.node = Some(n);
            return;
        }
        if t.starts_with("Tests  ") && t.ends_with(')') {
            // vitest: "Tests  1 failed | 3 passed (4)".
            if let Some(n) = t.rfind('(').and_then(|i| t[i + 1..t.len() - 1].trim().parse::<u64>().ok()) {
                self.vitest = Some(n);
                return;
            }
        }
        if indent(line) <= 2 {
            // mocha: "  2 passing (5ms)" and "  1 failing".
            let mut tokens = t.splitn(3, ' ');
            let (n, word, rest) = (tokens.next().and_then(|n| n.parse::<u64>().ok()), tokens.next(), tokens.next());
            let tail_ok = rest.is_none_or(|r| r.starts_with('(') && r.ends_with(')'));
            if let (Some(n), Some(word), true) = (n, word, tail_ok) {
                match word {
                    "passing" => self.mocha_passing = Some(n),
                    "failing" => self.mocha_failing = Some(n),
                    _ => {}
                }
                return;
            }
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
        // Mocha always prints `N passing`, so a lone `N failing` line is not its summary.
        let mocha = self.mocha_passing.map(|p| p + self.mocha_failing.unwrap_or(0));
        // A cargo run in a chain that ran nothing proves nothing, like a runner that ran nothing.
        let libtest = match self.libtest.as_slice() {
            [] => None,
            runs if runs.len() > 1 && runs.contains(&0) => Some(0),
            runs => Some(runs.iter().sum()),
        };
        let counters: Vec<u64> = [
            libtest,
            self.pytest,
            self.nextest,
            self.jest,
            self.node.map(|n| n.saturating_sub(self.node_files)),
            self.vitest,
            mocha,
            go,
            self.ctest,
            self.doctest,
            self.gtest,
            self.unity,
        ]
        .into_iter()
        .flatten()
        .collect();
        match counters.as_slice() {
            [] => None,
            // A run of more than one runner in which one of them ran nothing
            // proves nothing, so the run records zero. One runner keeps its
            // own count: libtest sums its result lines into one counter.
            [_, ..] if counters.len() > 1 && counters.contains(&0) => Some(0),
            _ => Some(counters.iter().sum()),
        }
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

/// The tests `output` reports, counted line by line the same way `run` counts
/// them, for a command that exited 0: the summary of a run that rvow did not
/// start itself, such as the one the agent ran in its own shell. `None` when
/// the output holds no summary rvow can read.
pub fn tests_in(output: &str) -> Option<u64> {
    let mut count = TestCount::default();
    for line in output.lines() {
        count.feed(line);
    }
    count.finish("", Some(0))
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

pub fn names_secret(name: &str) -> bool {
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
    fn counts_node_spec_and_tap_summaries() {
        let t = Duration::from_secs(10);
        let spec = quiet("echo '\u{2139} tests 3'; echo '\u{2139} pass 3'", t);
        assert_eq!(spec.tests_run, Some(3), "node spec summary");
        let tap = quiet("echo '# tests 5'; echo '# pass 5'", t);
        assert_eq!(tap.tests_run, Some(5), "node tap summary");
        let zero = quiet("echo '\u{2139} tests 0'", t);
        assert_eq!(zero.tests_run, Some(0), "node spec zero");
        let last = quiet("echo '# tests 2'; echo '\u{2139} tests 7'", t);
        assert_eq!(last.tests_run, Some(7), "the last summary wins");
        let nested = quiet("echo '    \u{2139} tests 9'", t);
        assert_eq!(nested.tests_run, None, "an indented line is test output");
    }

    #[test]
    fn counts_node_filter_that_left_a_file_empty() {
        let t = Duration::from_secs(10);
        let spec = quiet("echo '\u{2714} extensions/pi/lease.test.ts (125.1ms)'; echo '\u{2139} tests 1'", t);
        assert_eq!(spec.tests_run, Some(0), "node names an empty file as a test");
        let tap = quiet("echo 'ok 1 - a.test.mjs'; echo '# tests 1'", t);
        assert_eq!(tap.tests_run, Some(0), "the same in TAP");
        let mixed = quiet("echo '\u{2714} one (0.3ms)'; echo '\u{2714} b.test.mjs (44ms)'; echo '\u{2139} tests 2'", t);
        assert_eq!(mixed.tests_run, Some(1), "only the real test counts");
        let named = quiet("echo '\u{2714} reads the config (1ms)'; echo '\u{2139} tests 1'", t);
        assert_eq!(named.tests_run, Some(1), "a test with a plain name counts");
    }

    #[test]
    fn counts_node_mocha_needs_the_summary_shape() {
        let t = Duration::from_secs(10);
        assert_eq!(quiet("echo '0 failing cases were found'", t).tests_run, None, "not a mocha summary");
        assert_eq!(quiet("echo '0 failing'", t).tests_run, None, "mocha always prints a passing line");
        assert_eq!(quiet("echo '  3 failing cases'; echo '  4 passing (2ms)'", t).tests_run, Some(4), "a suite title is not a count");
    }

    #[test]
    fn counts_node_vitest_total() {
        let t = Duration::from_secs(10);
        let ok = quiet("echo '      Tests  4 passed (4)'", t);
        assert_eq!(ok.tests_run, Some(4));
        let mixed = quiet("echo ' Tests  1 failed | 3 passed (4)'", t);
        assert_eq!(mixed.tests_run, Some(4), "vitest counts the total");
    }

    #[test]
    fn counts_cxx_ctest_summary() {
        let t = Duration::from_secs(10);
        let ok = quiet("echo '100% tests passed, 0 tests failed out of 12'", t);
        assert_eq!(ok.tests_run, Some(12));
        let mixed = quiet("echo '83% tests passed, 2 tests failed out of 12'", t);
        assert_eq!(mixed.tests_run, Some(12), "failed tests count too");
        let real = quiet(
            "echo '1/2 Test #1: dftracer_utils_core_hex64 ......   Passed    0.01 sec'; echo; echo '100% tests passed, 0 tests failed out of 2'; echo; echo 'Total Test time (real) =   0.03 sec'",
            t,
        );
        assert_eq!(real.tests_run, Some(2));
    }

    #[test]
    fn counts_cxx_ctest_no_tests_found() {
        let t = Duration::from_secs(10);
        let none = quiet("echo 'Test project /build'; echo 'No tests were found!!!'", t);
        assert_eq!(none.tests_run, Some(0), "ctest -R none prints this and exits 0");
        assert!(none.passed());
        let printed = quiet("echo '    No tests were found!!!'", t);
        assert_eq!(printed.tests_run, None, "an indented line is test output");
    }

    #[test]
    fn counts_cxx_doctest_test_cases_line() {
        let t = Duration::from_secs(10);
        let ok = quiet(
            "echo '[doctest] test cases:  5 |  5 passed | 0 failed | 0 skipped'; echo '[doctest] assertions: 21 | 21 passed | 0 failed |'; echo '[doctest] Status: SUCCESS!'",
            t,
        );
        assert_eq!(ok.tests_run, Some(5), "the test cases line counts, not the assertions line");
        let plain = quiet("echo '[doctest] test cases: 5 | 5 passed | 0 failed | 0 skipped'", t);
        assert_eq!(plain.tests_run, Some(5));
        let none = quiet("echo '[doctest] test cases: 0 | 0 passed | 0 failed | 0 skipped'", t);
        assert_eq!(none.tests_run, Some(0));
        let two = quiet(
            "echo '[doctest] test cases: 2 | 2 passed | 0 failed | 0 skipped'; echo '[doctest] test cases: 3 | 3 passed | 0 failed | 0 skipped'",
            t,
        );
        assert_eq!(two.tests_run, Some(5), "two binaries add up");
    }

    #[test]
    fn counts_cxx_gtest_ran_line() {
        let t = Duration::from_secs(10);
        let ok = quiet(
            "echo '[==========] Running 7 tests from 2 test suites.'; echo '[==========] 7 tests from 2 test suites ran. (3 ms total)'",
            t,
        );
        assert_eq!(ok.tests_run, Some(7), "the ran line counts, the running line does not");
        let one = quiet("echo '[==========] 1 test from 1 test suite ran. (0 ms total)'", t);
        assert_eq!(one.tests_run, Some(1));
        let none = quiet("echo '[==========] 0 tests from 0 test suites ran. (0 ms total)'", t);
        assert_eq!(none.tests_run, Some(0));
    }

    #[test]
    fn counts_cxx_unity_ignored_tests_did_not_run() {
        let t = Duration::from_secs(10);
        assert_eq!(quiet("echo '3 Tests 0 Failures 3 Ignored'", t).tests_run, Some(0));
        assert_eq!(quiet("echo '5 Tests 0 Failures 2 Ignored'", t).tests_run, Some(3));
    }

    #[test]
    fn counts_cxx_unity_summary() {
        let t = Duration::from_secs(10);
        let ok = quiet("echo '-----------------------'; echo '4 Tests 0 Failures 0 Ignored'; echo 'OK'", t);
        assert_eq!(ok.tests_run, Some(4));
        let failed = quiet("echo '13 Tests 2 Failures 1 Ignored'; echo 'FAIL'", t);
        assert_eq!(failed.tests_run, Some(12), "the ignored test did not run");
        let none = quiet("echo '0 Tests 0 Failures 0 Ignored'", t);
        assert_eq!(none.tests_run, Some(0));
    }

    #[test]
    fn counts_cxx_ignores_lines_that_tests_print_themselves() {
        let t = Duration::from_secs(10);
        let nested = quiet(
            "echo '    100% tests passed, 0 tests failed out of 9'; echo '  [doctest] test cases: 9 | 9 passed | 0 failed | 0 skipped'; echo '  [==========] 9 tests from 1 test suite ran.'; echo '  9 Tests 0 Failures 0 Ignored'; echo '1: 9 Tests 0 Failures 0 Ignored'",
            t,
        );
        assert_eq!(nested.tests_run, None, "indented or prefixed lines are test output");
        let shapes = quiet(
            "echo '9 Tests'; echo '9 Tests 0 Failures'; echo '[doctest] test cases: lots'; echo '[==========] 9 tests from 1 test suite'; echo '100% tests passed'",
            t,
        );
        assert_eq!(shapes.tests_run, None, "a line that is not the whole summary shape is no summary");
    }

    #[test]
    fn counts_node_mocha_passing() {
        let t = Duration::from_secs(10);
        let ok = quiet("echo '  2 passing (5ms)'", t);
        assert_eq!(ok.tests_run, Some(2));
        let mixed = quiet("echo '  2 passing (5ms)'; echo '  1 pending'; echo '  3 failing'", t);
        assert_eq!(mixed.tests_run, Some(5), "passing and failing count, pending does not");
        let zero = quiet("echo '  0 passing (1ms)'", t);
        assert_eq!(zero.tests_run, Some(0));
    }

    #[test]
    fn counts_mixed_a_colored_summary_is_read() {
        let t = Duration::from_secs(10);
        let vitest = quiet("printf '\\033[2m Tests \\033[22m \\033[1m\\033[32m4 passed\\033[39m\\033[22m\\033[90m (4)\\033[39m'", t);
        assert_eq!(vitest.tests_run, Some(4), "the color codes are removed before the count is read");
        assert!(vitest.tail[0].contains('\u{1b}'), "the tail keeps the codes: {:?}", vitest.tail);
        let libtest = quiet("printf '\\033[32mtest result: ok. 5 passed; 0 failed;\\033[0m'", t);
        assert_eq!(libtest.tests_run, Some(5));
    }

    #[test]
    fn counts_mixed_a_runner_that_ran_nothing_is_zero() {
        let t = Duration::from_secs(10);
        let empty = quiet("echo 'test result: ok. 5 passed; 0 failed;'; echo '\u{2139} tests 0'", t);
        assert_eq!(empty.tests_run, Some(0), "node ran nothing, so the run is no proof");
        let both = quiet("echo 'test result: ok. 5 passed; 0 failed;'; echo '\u{2139} tests 3'", t);
        assert_eq!(both.tests_run, Some(8), "two runners that ran add up");
        let c_and_cargo =
            quiet("echo '[doctest] test cases: 4 | 4 passed | 0 failed | 0 skipped'; echo 'test result: ok. 6 passed; 0 failed;'", t);
        assert_eq!(c_and_cargo.tests_run, Some(10), "a C suite and cargo add up");
        let one = quiet("echo 'test result: ok. 0 passed; 0 failed; 5 filtered out'", t);
        assert_eq!(one.tests_run, Some(0), "one runner that ran nothing is still zero");
    }

    #[test]
    fn counts_mixed_libtest_sums_its_own_result_lines() {
        let t = Duration::from_secs(10);
        let docs = quiet(
            "echo 'test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0s'; echo 'test result: ok. 5 passed; 0 failed;'",
            t,
        );
        assert_eq!(docs.tests_run, Some(5), "a crate with no doc tests and 5 unit tests is 5, not 0");
    }

    #[test]
    fn counts_chain_a_cargo_run_that_ran_nothing_is_zero() {
        let t = Duration::from_secs(10);
        let fin = "echo '    Finished `test` profile [unoptimized] target(s) in 0.1s'";
        let zero = "echo 'test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 9 filtered out; finished in 0s'";
        let five = "echo 'test result: ok. 5 passed; 0 failed;'";
        let chain = quiet(&format!("{fin}; {zero}; {zero}; {fin}; {five}"), t);
        assert_eq!(chain.tests_run, Some(0), "the first cargo run matched no test, so the chain proves nothing");
        let both = quiet(&format!("{fin}; {zero}; {five}; {fin}; {five}"), t);
        assert_eq!(both.tests_run, Some(10), "two cargo runs that each ran tests add up");
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
