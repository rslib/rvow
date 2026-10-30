//! Reads a Bash command the way a shell would, well enough to spot the calls
//! that the hooks deny: quotes and backslashes that split a word are removed,
//! a plain variable assignment is expanded, and the code inside `sh -c`,
//! `eval` and `$(...)` is read as commands too. Anything a shell computes at
//! run time (a generated name, a downloaded script) stays a known gap that the
//! final gate covers.

use std::borrow::Cow;
use std::collections::HashMap;
use std::path::{Path, PathBuf};

use rvow_base::layout::{CACHE_FILE, IDS_FILE, INDEX_FILE, LEASE_FILE, LEDGER, ROTATED_FILE, STATS_FILE, WITNESS_FILE, YOLO_FILE};
use rvow_base::paths::{at_or_under, key, real_path};

use crate::rule::{Denial, Rule, denial};
use crate::witness;

const SEPARATORS: &[char] = &[';', '&', '|', '(', ')', '`', '\n'];
pub const SHELLS: &[&str] = &["sh", "bash", "zsh", "dash", "ksh", "ash", "fish"];
const INTERPRETERS: &[&str] = &["python", "python3", "node", "perl", "ruby", "php", "deno", "bun"];
/// Programs whose first argument is a program text, such as awk's `BEGIN{system(...)}`.
const AWKS: &[&str] = &["awk", "gawk", "mawk", "nawk"];
/// The names of git: `rgit` takes git's commands and flags.
const GITS: &[&str] = &["git", "rgit"];
const WRAPPERS: &[&str] = &[
    "env",
    "xargs",
    "sudo",
    "doas",
    "command",
    "builtin",
    "exec",
    "nohup",
    "nice",
    "time",
    "timeout",
    "setsid",
    "stdbuf",
    "watch",
    "ionice",
    "caffeinate",
    "if",
    "then",
    "do",
    "else",
    "elif",
    "while",
    "until",
    "!",
    "{",
];
const DECLARATIONS: &[&str] = &["export", "declare", "local", "readonly", "typeset"];
/// Shell builtins, whose `PATH` entry a shell does not use.
const BUILTINS: &[&str] = &[
    ":", ".", "[", "alias", "bg", "break", "builtin", "cd", "command", "continue", "declare", "dirs", "disown", "echo", "enable", "eval",
    "exec", "exit", "export", "false", "fc", "fg", "getopts", "hash", "help", "history", "jobs", "kill", "let", "local", "logout", "popd",
    "printf", "pushd", "pwd", "read", "readonly", "return", "set", "shift", "source", "suspend", "test", "times", "trap", "true", "type",
    "typeset", "ulimit", "umask", "unalias", "unset", "wait",
];
/// `git commit` options whose value is the next word.
const COMMIT_VALUE_OPTIONS: &[&str] =
    &["--message", "--file", "--author", "--date", "--reuse-message", "--reedit-message", "--fixup", "--squash", "--template", "--cleanup"];
/// `git commit` short options that take a value, so the rest of the cluster is that value.
const COMMIT_VALUE_LETTERS: &str = "mFCctuS";
const MAX_DEPTH: u8 = 5;

/// The commands of a shell line: one word list per simple command, plus every
/// word seen (redirection targets included).
struct Shell {
    commands: Vec<Vec<String>>,
    words: Vec<String>,
}

fn base_name(word: &str) -> &str {
    word.rsplit('/').next().unwrap_or(word)
}

fn is_assignment(word: &str) -> bool {
    word.split_once('=').is_some_and(|(name, _)| !name.is_empty() && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_'))
        && !word.starts_with(|c: char| c.is_ascii_digit())
}

/// `cmd` without heredoc bodies, which a shell reads as data. A body fed to a
/// shell (`bash <<EOF`) stays, since it runs.
fn without_heredocs(cmd: &str) -> String {
    let mut kept = String::new();
    let mut end: Option<String> = None;
    for line in cmd.lines() {
        if let Some(e) = &end {
            if line.trim() == e {
                end = None;
            }
            continue;
        }
        kept.push_str(line);
        kept.push('\n');
        let Some(i) = line.find("<<").filter(|i| !line[*i..].starts_with("<<<")) else {
            continue;
        };
        let feeds_shell = line.split_whitespace().any(|w| SHELLS.contains(&base_name(w.trim_matches(['\'', '"']))));
        let tag = line[i + 2..].trim_start_matches('-').trim_start();
        let tag: String = tag.trim_start_matches(['\'', '"']).chars().take_while(|c| c.is_alphanumeric() || *c == '_').collect();
        // A body fed to a shell runs, so it stays in the text.
        if !tag.is_empty() && !feeds_shell {
            end = Some(tag);
        }
    }
    kept
}

/// The code of every `$(...)` and backtick span in `text`.
fn substitutions(text: &str) -> Vec<String> {
    let chars: Vec<char> = text.chars().collect();
    let mut out = Vec::new();
    let mut i = 0;
    while i < chars.len() {
        if chars[i] == '$' && chars.get(i + 1) == Some(&'(') {
            let mut depth = 1;
            let mut j = i + 2;
            while j < chars.len() && depth > 0 {
                match chars[j] {
                    '(' => depth += 1,
                    ')' => depth -= 1,
                    _ => {}
                }
                j += 1;
            }
            let end = if depth == 0 { j - 1 } else { j };
            out.push(chars[i + 2..end].iter().collect());
            i = j;
        } else if chars[i] == '`' {
            let end = chars[i + 1..].iter().position(|c| *c == '`').map_or(chars.len(), |p| i + 1 + p);
            out.push(chars[i + 1..end].iter().collect());
            i = end + 1;
        } else {
            i += 1;
        }
    }
    out
}

struct Tokens {
    commands: Vec<Vec<String>>,
    targets: Vec<String>,
    spans: Vec<String>,
}

struct Lexer {
    tokens: Tokens,
    word: String,
    has_word: bool,
    command: Vec<String>,
    redirect: bool,
}

impl Lexer {
    fn end_word(&mut self) {
        if !self.has_word {
            return;
        }
        let word = std::mem::take(&mut self.word);
        if std::mem::take(&mut self.redirect) {
            self.tokens.targets.push(word);
        } else {
            self.command.push(word);
        }
        self.has_word = false;
    }

    fn end_command(&mut self) {
        self.end_word();
        self.redirect = false;
        if !self.command.is_empty() {
            self.tokens.commands.push(std::mem::take(&mut self.command));
        }
    }
}

/// Splits `text` into simple commands. Quotes and backslashes only join the
/// characters they hold into the word, as in a shell.
fn tokenize(text: &str) -> Tokens {
    let mut lx = Lexer {
        tokens: Tokens { commands: Vec::new(), targets: Vec::new(), spans: Vec::new() },
        word: String::new(),
        has_word: false,
        command: Vec::new(),
        redirect: false,
    };
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '\\' => match chars.next() {
                Some('\n') | None => {}
                Some(n) => {
                    lx.word.push(n);
                    lx.has_word = true;
                }
            },
            '\'' => {
                lx.has_word = true;
                for n in chars.by_ref() {
                    if n == '\'' {
                        break;
                    }
                    lx.word.push(n);
                }
            }
            '"' => {
                lx.has_word = true;
                let mut raw = String::new();
                while let Some(n) = chars.next() {
                    match n {
                        '"' => break,
                        '\\' => {
                            raw.push(n);
                            raw.extend(chars.next());
                        }
                        _ => raw.push(n),
                    }
                }
                lx.tokens.spans.extend(substitutions(&raw));
                let mut it = raw.chars();
                while let Some(n) = it.next() {
                    if n == '\\' {
                        lx.word.extend(it.next());
                    } else {
                        lx.word.push(n);
                    }
                }
            }
            '<' | '>' => {
                // A file descriptor in front (`2>`) is not a word.
                if lx.has_word && lx.word.chars().all(|d| d.is_ascii_digit()) {
                    lx.word.clear();
                    lx.has_word = false;
                }
                lx.end_word();
                while chars.next_if(|n| matches!(n, '<' | '>' | '&' | '|')).is_some() {}
                lx.redirect = true;
            }
            c if c.is_whitespace() && c != '\n' => lx.end_word(),
            c if SEPARATORS.contains(&c) => lx.end_command(),
            c => {
                lx.word.push(c);
                lx.has_word = true;
            }
        }
    }
    lx.end_command();
    lx.tokens
}

/// The program word that starts each simple command of `text`, with any
/// assignment or declaration before it skipped. Only the commands the text
/// starts directly are read; the code inside `sh -c` is not.
pub fn programs(text: &str) -> Vec<String> {
    tokenize(&without_heredocs(text))
        .commands
        .iter()
        .filter_map(|seg| {
            let start = seg.iter().position(|w| !is_assignment(w) && !DECLARATIONS.contains(&base_name(w)))?;
            seg.get(start).cloned()
        })
        .collect()
}

/// Whether a shell runs `word` as a builtin, so its `PATH` entry does not matter.
pub fn is_builtin(word: &str) -> bool {
    BUILTINS.contains(&base_name(word))
}

/// The variables a command line sees: the ones it sets, then, with `env`, the
/// process environment, read one name at a time instead of copied whole.
#[derive(Clone, Default)]
struct Vars {
    own: HashMap<String, String>,
    env: bool,
}

impl Vars {
    fn with_env() -> Vars {
        Vars { own: HashMap::new(), env: true }
    }

    fn get(&self, name: &str) -> Option<Cow<'_, str>> {
        match self.own.get(name) {
            Some(v) => Some(Cow::Borrowed(v)),
            None if self.env => std::env::var(name).ok().map(Cow::Owned),
            None => None,
        }
    }

    fn insert(&mut self, name: &str, value: &str) {
        self.own.insert(name.to_string(), value.to_string());
    }
}

fn expand_vars(word: &str, vars: &Vars) -> String {
    if !word.contains('$') {
        return word.to_string();
    }
    let mut out = String::new();
    let chars: Vec<char> = word.chars().collect();
    let mut i = 0;
    while i < chars.len() {
        if chars[i] != '$' {
            out.push(chars[i]);
            i += 1;
            continue;
        }
        let braced = chars.get(i + 1) == Some(&'{');
        let start = i + 1 + usize::from(braced);
        let len = chars[start.min(chars.len())..].iter().take_while(|c| c.is_ascii_alphanumeric() || **c == '_').count();
        let name: String = chars[start.min(chars.len())..start.min(chars.len()) + len].iter().collect();
        let end = start + len;
        let closed = !braced || chars.get(end) == Some(&'}');
        match vars.get(&name).filter(|_| !name.is_empty() && closed) {
            Some(v) => {
                out.push_str(&v);
                i = end + usize::from(braced);
            }
            None => {
                out.push('$');
                i += 1;
            }
        }
    }
    out
}

/// The words of `seg` that start a command: the first one, or after a wrapper
/// such as `env` or `xargs` any of them, since the wrapper's own options and
/// values are not worth parsing.
fn invocations(seg: &[String]) -> Vec<&[String]> {
    let start = seg.iter().position(|w| !is_assignment(w) && !DECLARATIONS.contains(&base_name(w))).unwrap_or(seg.len());
    let Some(first) = seg.get(start) else {
        return Vec::new();
    };
    let name = base_name(first);
    let mut out: Vec<&[String]> =
        if WRAPPERS.contains(&name) { (start..seg.len()).map(|i| &seg[i..]).collect() } else { vec![&seg[start..]] };
    if name == "find" {
        out.extend(seg.iter().enumerate().filter(|(_, w)| matches!(w.as_str(), "-exec" | "-execdir" | "-ok")).map(|(i, _)| &seg[i + 1..]));
    }
    out
}

fn is_shell(name: &str) -> bool {
    SHELLS.contains(&name)
}

fn is_interpreter(name: &str) -> bool {
    INTERPRETERS.iter().any(|i| name == *i || (name.starts_with(i) && name[i.len()..].chars().all(|c| c.is_ascii_digit() || c == '.')))
}

/// The code a shell invocation runs from its argument: `sh -c CODE`, `eval ...`.
fn inline_code(inv: &[String]) -> Option<String> {
    let name = base_name(&inv[0]);
    if name == "eval" {
        return Some(inv[1..].join(" "));
    }
    if !is_shell(name) {
        return None;
    }
    if let Some(code) = inv.iter().find_map(|w| w.strip_prefix("--command=")) {
        return Some(code.to_string());
    }
    let at = inv.iter().position(|w| (w.starts_with('-') && !w.starts_with("--") && w.contains('c')) || w == "--command")?;
    inv.get(at + 1).cloned()
}

#[cfg(test)]
thread_local! {
    static PARSES: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

fn parse(cmd: &str, depth: u8, outer: &Vars) -> Shell {
    #[cfg(test)]
    if depth == 0 {
        PARSES.with(|n| n.set(n.get() + 1));
    }
    let tokens = tokenize(&without_heredocs(cmd));
    let mut vars = outer.clone();
    for seg in &tokens.commands {
        for w in seg {
            if let Some((name, value)) = w.split_once('=').filter(|_| is_assignment(w)) {
                vars.insert(name, value);
            } else if !DECLARATIONS.contains(&base_name(w)) {
                break;
            }
        }
    }
    let mut commands: Vec<Vec<String>> = tokens.commands.iter().map(|seg| seg.iter().map(|w| expand_vars(w, &vars)).collect()).collect();
    let mut words: Vec<String> = commands.iter().flatten().cloned().collect();
    words.extend(tokens.targets.iter().map(|w| expand_vars(w, &vars)));
    if depth < MAX_DEPTH {
        let mut nested: Vec<String> = tokens.spans;
        for seg in &commands {
            nested.extend(invocations(seg).into_iter().filter_map(inline_code));
        }
        for code in nested {
            let inner = parse(&code, depth + 1, &vars);
            commands.extend(inner.commands);
            words.extend(inner.words);
        }
    }
    Shell { commands, words }
}

/// The top-level parse of `cmd`, with the process environment when `env`. The
/// last parse of each kind is kept: one hook call runs a dozen rules over one
/// command. A command with no `$` reads no variable, so either kind serves it.
fn parsed(cmd: &str, env: bool) -> std::rc::Rc<Shell> {
    use std::rc::Rc;
    type Slot = std::cell::RefCell<Option<(String, Rc<Shell>)>>;
    thread_local! {
        static LAST: [Slot; 2] = const { [std::cell::RefCell::new(None), std::cell::RefCell::new(None)] };
    }
    let env = env && cmd.contains('$');
    let hit = |slot: &Slot| slot.borrow().as_ref().filter(|(c, _)| c == cmd).map(|(_, sh)| Rc::clone(sh));
    if let Some(sh) = LAST.with(|last| hit(&last[usize::from(env)])) {
        return sh;
    }
    let sh = Rc::new(parse(cmd, 0, &if env { Vars::with_env() } else { Vars::default() }));
    LAST.with(|last| *last[usize::from(env)].borrow_mut() = Some((cmd.to_string(), Rc::clone(&sh))));
    sh
}

/// The owned subcommand that the text of a script names along with `rvow`, or
/// `None`. The words may be far apart, since code can keep `rvow` in a variable.
fn owned_in_code(code: &str) -> Option<&'static str> {
    // A slash command name such as `/rvow-accept` cannot run from a script,
    // so every `/rvow-` word is dropped before the token check.
    let mut kept = String::with_capacity(code.len());
    let mut rest = code;
    while let Some(at) = rest.find('/') {
        let after = &rest[at + 1..];
        if after.starts_with("rvow-") {
            kept.push_str(&rest[..at]);
            let end = after.find(|c: char| !(c.is_alphanumeric() || c == '-')).unwrap_or(after.len());
            rest = &after[end..];
        } else {
            kept.push_str(&rest[..at + 1]);
            rest = after;
        }
    }
    kept.push_str(rest);
    let mut found: Option<&'static str> = None;
    let mut rvow = false;
    for token in kept.split(|c: char| !c.is_alphanumeric()) {
        match token {
            "rvow" => rvow = true,
            _ if found.is_some() => {}
            "accept" => found = Some("accept"),
            "confirm" => found = Some("confirm"),
            "decide" => found = Some("decide"),
            "hook" => found = Some("hook"),
            "daemon" => found = Some("daemon"),
            _ => {}
        }
    }
    found.filter(|_| rvow)
}

fn owner_command(sub: &str) -> String {
    format!(
        "rvow {sub} belongs to the user; the hooks deny it for agents. Call the rvow_{sub} tool (tool search loads it when it is not listed); without it, ask the user to run /rvow-{sub} <id>, or to type ! rvow {sub}"
    )
}

/// The denial for one `rvow` subcommand the user owns, with the words after it.
fn sub_denial(sub: &str, args: &[String]) -> Option<Denial> {
    match sub {
        "accept" | "confirm" => denial(Rule::OwnerCommand, owner_command(sub)),
        // The user decides a stale relaxed task; `--list` only reads the queue and stays allowed.
        "decide" if !args.iter().any(|w| matches!(w.as_str(), "--list" | "--help" | "-h")) => denial(
            Rule::OwnerCommand,
            "rvow decide records the user's choice about a stale task and belongs to the user; the hooks deny it for agents. Ask the user with the rvow_ask tool, then run rvow decide <task> now|archive|keep with their answer".into(),
        ),
        // Install re-pins the pre-push gate to the binary on PATH, which an agent may have rebuilt.
        "install" if !args.iter().any(|w| w == "--help" || w == "-h") => denial(
            Rule::OwnerCommand,
            "rvow install re-pins the pre-push gate and belongs to the user; the hooks deny it for agents. Ask the user to type ! rvow install, or ! rvow install --all".into(),
        ),
        "review" if args.iter().any(|w| w == "--judge" || w.starts_with("--judge=")) => denial(
            Rule::OwnerCommand,
            "rvow review --judge records the judge note and belongs to the user; ask the user to run /rvow-judge".into(),
        ),
        "yolo" => denial(
            Rule::YoloFile,
            "rvow yolo relaxes the hooks and belongs to the user; ask the user to run `! rvow yolo <tier>`".into(),
        ),
        "hook" => denial(
            Rule::WitnessFile,
            "rvow hook answers the harness and records what the user made; an agent does not run it. Run `rvow status` to see the plan".into(),
        ),
        // The engine answers hooks and, with --host-pipe, takes the user's answers:
        // an agent that ran it could serve the harness's calls and answer itself.
        "daemon" => denial(
            Rule::OwnerCommand,
            "rvow daemon is the engine that answers the hooks and carries the user's answers; the hooks start it and agents do not run it. Run `rvow status` to see the plan".into(),
        ),
        "ingest" => denial(
            Rule::OwnerCommand,
            "rvow ingest records proofs from a CI file and belongs to the user; ask the user to run `! rvow ingest <file>`, or run `rvow prove <task>` to prove it here"
                .into(),
        ),
        _ => None,
    }
}

/// The subcommand of an `rvow` or `cargo run` call that the user owns.
fn rvow_denial(inv: &[String]) -> Option<Denial> {
    let name = base_name(&inv[0]);
    let args: &[String] = if name == "rvow" {
        &inv[1..]
    } else if name == "cargo" {
        let run = inv.iter().position(|w| w == "run")?;
        &inv[run + 1..]
    } else if inv[0].starts_with('$') {
        // A command named by a variable that is not known here.
        let sub = inv[1..].iter().find(|w| matches!(w.as_str(), "accept" | "confirm" | "decide" | "hook" | "daemon"))?;
        return sub_denial(sub, &[]);
    } else if is_interpreter(name) || AWKS.contains(&name) {
        let sub = inv[1..].iter().find_map(|w| owned_in_code(w))?;
        return sub_denial(sub, &[]);
    } else {
        return None;
    };
    let sub = args.iter().find(|x| !x.starts_with('-') && *x != "rvow")?;
    sub_denial(sub, args)
}

/// `-n` of a `git commit`, also inside a cluster such as `-qn` or `-anm`.
fn commit_skips_hooks(args: &[String]) -> bool {
    let mut skip_value = false;
    for w in args {
        if skip_value {
            skip_value = false;
            continue;
        }
        if w == "--no-verify" || w == "-n" {
            return true;
        }
        if COMMIT_VALUE_OPTIONS.contains(&w.as_str()) {
            skip_value = true;
        } else if w.starts_with('-') && !w.starts_with("--") {
            let letters: Vec<char> = w[1..].chars().collect();
            for (i, l) in letters.iter().enumerate() {
                if *l == 'n' {
                    return true;
                }
                if COMMIT_VALUE_LETTERS.contains(*l) {
                    skip_value = i + 1 == letters.len();
                    break;
                }
            }
        }
    }
    false
}

/// The subcommand of a `git` call and the words after it.
fn git_subcommand(args: &[String]) -> Option<(&str, &[String])> {
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "-c" | "-C" | "--git-dir" | "--work-tree" | "--namespace" => i += 2,
            w if w.starts_with('-') => i += 1,
            w => return Some((w, &args[i + 1..])),
        }
    }
    None
}

/// A `-c alias.<name>=<value>` whose value runs a shell (`!...`) or skips the hooks.
fn alias_skips_hooks(args: &[String]) -> bool {
    args.windows(2).filter(|p| p[0] == "-c").any(|p| {
        let Some((key, value)) = p[1].split_once('=') else {
            return false;
        };
        if !key.to_lowercase().starts_with("alias.") {
            return false;
        }
        let value = value.trim();
        let words: Vec<String> = value.split_whitespace().map(str::to_string).collect();
        value.starts_with('!')
            || words.iter().any(|w| w.starts_with("--no-v") && "--no-verify".starts_with(w.as_str()))
            || git_subcommand(&words).is_some_and(|(sub, rest)| sub == "commit" && commit_skips_hooks(rest))
    })
}

fn git_denial(inv: &[String]) -> Option<Denial> {
    if !GITS.contains(&base_name(&inv[0])) {
        return None;
    }
    let args = &inv[1..];
    if args.iter().any(|w| w.starts_with("--no-v") && "--no-verify".starts_with(w.as_str())) {
        return denial(Rule::GitHooks, "--no-verify skips the hooks that check the plan; run git commit and git push without it".into());
    }
    if alias_skips_hooks(args) {
        return denial(
            Rule::GitHooks,
            "a `-c alias` that runs a shell or skips the hooks hides what git runs; write the git command out in full".into(),
        );
    }
    let (sub, rest) = git_subcommand(args)?;
    (sub == "commit" && commit_skips_hooks(rest)).then(|| Denial {
        rule: Rule::GitHooks,
        reason: "git commit -n skips the hooks that check the plan; run git commit without -n".into(),
    })
}

fn decodes_text(inv: &[String]) -> bool {
    match base_name(&inv[0]) {
        "base64" => inv[1..].iter().any(|w| matches!(w.as_str(), "-d" | "-D" | "--decode")),
        "xxd" => inv[1..].iter().any(|w| w == "-r"),
        _ => false,
    }
}

/// A shell or `eval` that runs text it was handed rather than a file.
fn runs_given_text(inv: &[String]) -> bool {
    let name = base_name(&inv[0]);
    if name == "eval" {
        return true;
    }
    is_shell(name) && (inv[1..].iter().all(|w| w.starts_with('-')) || inv[1..].iter().any(|w| w.contains("$(") || w.contains('`')))
}

/// Programs that only read their input and print. Reading the stats, lease, yolo, cache or ledger file
/// is harmless; only a write can change what rvow trusts.
const READERS: &[&str] = &[
    "cat", "ls", "head", "tail", "grep", "rg", "wc", "stat", "file", "less", "more", "sort", "uniq", "cut", "tr", "nl", "column", "jq",
    "diff", "cmp", "du", "pwd", "echo", "printf",
];

/// True when every command of the line only reads and none writes a file.
fn only_reads(invs: &[&[String]], cmd: &str) -> bool {
    !invs.is_empty() && !writes_files(invs, cmd) && invs.iter().all(|inv| READERS.contains(&base_name(&inv[0])))
}

/// True when the whole `cmd` only reads, so a glob that expands to a file
/// rvow trusts may still run.
pub fn reads_only(cmd: &str) -> bool {
    let sh = parsed(cmd, false);
    let invs: Vec<&[String]> = sh.commands.iter().flat_map(|seg| invocations(seg)).collect();
    only_reads(&invs, cmd)
}

/// True when the line only prints text: `echo`, `printf`, or a `git commit` or `git tag` message.
fn only_text(invs: &[&[String]], cmd: &str) -> bool {
    !invs.is_empty()
        && !writes_files(invs, cmd)
        && invs.iter().all(|inv| match base_name(&inv[0]) {
            "echo" | "printf" => true,
            "git" => git_subcommand(&inv[1..]).is_some_and(|(sub, _)| matches!(sub, "commit" | "tag")),
            _ => false,
        })
}

/// Why a Bash command must not run, or `None`.
pub fn bash_rule(cmd: &str) -> Option<Denial> {
    let sh = parsed(cmd, false);
    let invs: Vec<&[String]> = sh.commands.iter().flat_map(|seg| invocations(seg)).collect();
    if let Some(reason) = invs.iter().find_map(|inv| rvow_denial(inv)) {
        return Some(reason);
    }
    let w = &sh.words;
    let rvow_answer = w.iter().any(|x| base_name(x) == "rvow") && w.iter().any(|x| x == "answer");
    let via_dialog = w.windows(2).any(|p| p[0] == "--via" && p[1] == "dialog") || w.iter().any(|x| x == "--via=dialog");
    if rvow_answer && via_dialog {
        return denial(
            Rule::OwnerCommand,
            "only the rvow_ask tool records an answer as given in a dialog; run `rvow answer <ref> \"<choice>\"` to record a chat answer"
                .into(),
        );
    }
    if let Some(reason) = invs.iter().find_map(|inv| git_denial(inv)) {
        return Some(reason);
    }
    if invs.iter().any(|inv| decodes_text(inv)) && invs.iter().any(|inv| runs_given_text(inv)) {
        return denial(Rule::OwnerCommand,
            "decoding text into a shell hides what runs; write the command out in the Bash call, and ask the user to run `! rvow accept <id>` if it is an accept"
                .into(),
        );
    }
    let joined = w.join(" ").to_lowercase();
    let lower = cmd.to_lowercase();
    let writes = writes_files(&invs, cmd);
    if invs.iter().any(|inv| sets_hooks_path(inv))
        || w.iter().any(|x| is_assignment(x) && x.to_lowercase().contains("core.hookspath"))
        || (writes && (lower.contains("core.hookspath") || joined.contains("core.hookspath")) && w.iter().any(|x| x.ends_with("config")))
    {
        return denial(Rule::GitHooks, "core.hooksPath decides which hooks run; changing it is for the user. If the plan needs it, run rvow block <task> --reason \"...\"".into());
    }
    let in_git_dir =
        w.iter().any(|x| x.ends_with(".git") || x.ends_with(".git/")) && w.iter().any(|x| x.to_lowercase().starts_with("hooks"));
    // Running or reading a hook while a writer such as `tee` targets another file is fine.
    if writes_to(&invs, cmd, |x| x.to_lowercase().contains(".git/hooks")) || (writes && in_git_dir) {
        return denial(Rule::GitHooks, ".git/hooks holds the pre-push gate; only rvow install writes it. Run rvow install instead".into());
    }
    let rotated = std::path::Path::new(ROTATED_FILE).file_name().and_then(|n| n.to_str()).unwrap_or(STATS_FILE);
    let names_stats = w.iter().any(|x| !x.contains(char::is_whitespace) && (base_name(x) == STATS_FILE || base_name(x) == rotated));
    let names_index = w.iter().any(|x| !x.contains(char::is_whitespace) && base_name(x) == INDEX_FILE);
    if names_index && !only_reads(&invs, cmd) && !sh.commands.first().and_then(|c| c.first()).is_some_and(|x| base_name(x) == "rvow") {
        return denial(
            Rule::CacheFile,
            format!("{INDEX_FILE} is written and read only by rvow; delete it to make rvow look the project up again"),
        );
    }
    if names_stats && !only_reads(&invs, cmd) && !sh.commands.first().and_then(|c| c.first()).is_some_and(|x| base_name(x) == "rvow") {
        return denial(
            Rule::StatsFile,
            format!("{STATS_FILE} is written and read only by rvow; run `rvow stats` to see the counts instead"),
        );
    }
    let names_yolo = w.iter().any(|x| !x.contains(char::is_whitespace) && base_name(x) == YOLO_FILE);
    if names_yolo && !only_reads(&invs, cmd) && !sh.commands.first().and_then(|c| c.first()).is_some_and(|x| base_name(x) == "rvow") {
        return denial(Rule::YoloFile, format!("{YOLO_FILE} is the user's switch; ask the user to run `! rvow yolo <tier>`"));
    }
    let names_lease = w.iter().any(|x| !x.contains(char::is_whitespace) && base_name(x) == LEASE_FILE);
    if names_lease && !only_reads(&invs, cmd) && !sh.commands.first().and_then(|c| c.first()).is_some_and(|x| base_name(x) == "rvow") {
        return denial(
            Rule::LeaseFile,
            format!("{LEASE_FILE} is written and read only by rvow; run `rvow delegate <task>` to lease a task instead"),
        );
    }
    let names_witness = w.iter().any(|x| !x.contains(char::is_whitespace) && base_name(x) == WITNESS_FILE);
    if names_witness && !only_reads(&invs, cmd) && !sh.commands.first().and_then(|c| c.first()).is_some_and(|x| base_name(x) == "rvow") {
        return denial(Rule::WitnessFile, witness::reason());
    }
    let names_cache = w.iter().any(|x| !x.contains(char::is_whitespace) && base_name(x) == CACHE_FILE);
    let only_rvow_first = sh.commands.first().and_then(|c| c.first()).is_some_and(|x| base_name(x) == "rvow");
    if names_cache && !only_rvow_first && !only_reads(&invs, cmd) {
        return denial(Rule::CacheFile, format!("{CACHE_FILE} is written and read only by rvow gate; run `rvow gate --rerun` instead"));
    }
    let names_rvow_file = w.iter().any(|x| {
        let b = base_name(x);
        !x.contains(char::is_whitespace) && (b == LEDGER || b == IDS_FILE)
    });
    let only_rvow = sh.commands.first().and_then(|c| c.first()).is_some_and(|x| base_name(x) == "rvow");
    if names_rvow_file && !only_rvow && !only_reads(&invs, cmd) {
        return denial(
            Rule::Ledger,
            format!(
                "{LEDGER} and {IDS_FILE} are written only by rvow; run `rvow show <change>` to read the plan or `rvow note` to record something"
            ),
        );
    }
    None
}

/// True when every `rvow` call in `cmd` that the user owns is an `accept` or a `confirm`, and the command
/// holds no other owner-only call. The `auto` tier lets such a command run.
pub fn only_accept_or_confirm(cmd: &str) -> bool {
    let sh = parsed(cmd, false);
    let subs: Vec<&str> = sh
        .commands
        .iter()
        .flat_map(|seg| invocations(seg))
        .filter(|inv| base_name(&inv[0]) == "rvow")
        .filter_map(|inv| inv[1..].iter().find(|w| !w.starts_with('-')).map(String::as_str))
        .collect();
    !subs.is_empty() && subs.iter().all(|s| matches!(*s, "accept" | "confirm" | "status" | "ls" | "show" | "check" | "validate" | "prove"))
}

/// What each `rvow prove` in `cmd` is given: the task words and flags after `prove`.
pub fn prove_args(cmd: &str) -> Vec<Vec<String>> {
    let sh = parsed(cmd, false);
    let calls = sh.commands.iter().flat_map(|seg| invocations(seg)).filter(|inv| base_name(&inv[0]) == "rvow");
    calls
        .filter_map(|inv| {
            let at = inv.iter().position(|w| w == "prove")?;
            Some(inv[at + 1..].iter().filter(|w| !matches!(w.as_str(), "--toon" | "--human")).cloned().collect())
        })
        .collect()
}

/// Why a Bash command must not run, or `None`.
pub fn bash_denial(cmd: &str) -> Option<String> {
    bash_rule(cmd).map(|d| d.reason)
}

/// How many paths one command's glob words may expand to, so a pattern such as
/// `**` cannot walk a whole tree.
const GLOB_LIMIT: usize = 4096;

/// How many folder entries the globs of one command read before they stop. The
/// hook has 10 s, and a shared Cargo `deps` folder can hold millions of entries.
const DIR_SCAN_LIMIT: usize = 16_384;

fn has_glob(word: &str) -> bool {
    word.contains(['*', '?', '['])
}

/// The words of `cmd` that name a path, with `~` and `$VAR` expanded as a
/// shell would: every word that holds a `/` or starts with `~` or `.`, without
/// the glob words, which `glob_paths` expands instead.
pub fn path_words(cmd: &str) -> Vec<String> {
    let home = std::env::var("HOME").ok();
    let sh = parsed(cmd, true);
    sh.words
        .iter()
        .filter(|w| !has_glob(w))
        .map(|w| expand_tilde(w, home.as_deref()))
        .filter(|w| w.starts_with('/') || w.starts_with('~') || w.starts_with('.') || w.contains('/'))
        .collect()
}

/// One program call of a command line, with the path words among its own
/// arguments and the directory it runs in after the `cd` calls before it.
pub struct Call {
    pub program: String,
    pub paths: Vec<String>,
    pub dir: PathBuf,
    pub moved: bool,
}

/// The program calls of `cmd` in order, each with its own path words. A `cd`
/// moves the directory of the calls after it; a wrapper such as `xargs` or
/// `env` adds a call for the program it starts.
pub fn calls(cmd: &str, cwd: &Path) -> Vec<Call> {
    let home = std::env::var("HOME").ok();
    let mut dir = cwd.to_path_buf();
    let mut moved = false;
    let mut out = Vec::new();
    for seg in &parsed(cmd, true).commands {
        for (n, inv) in invocations(seg).into_iter().enumerate() {
            let Some((program, args)) = inv.split_first() else {
                continue;
            };
            if n == 0 && base_name(program) == "cd" {
                if let Some(target) = args.iter().find(|a| !a.starts_with('-')) {
                    dir = real_path(Path::new(&expand_tilde(target, home.as_deref())), &dir);
                    moved = true;
                }
                continue;
            }
            let paths = args
                .iter()
                .filter(|w| !has_glob(w))
                .map(|w| expand_tilde(w, home.as_deref()))
                .filter(|w| w.starts_with('/') || w.starts_with('~') || w.starts_with('.') || w.contains('/'))
                .collect();
            out.push(Call { program: program.clone(), paths, dir: dir.clone(), moved });
        }
    }
    out
}

/// The words of `cmd` that may name a file it changes: the path words, and the
/// bare file names such as `out.txt` or `Cargo.toml`, which hold a dot but no `/`.
pub(super) fn changed_words(cmd: &str) -> Vec<String> {
    let mut out = path_words(cmd);
    let sh = parsed(cmd, false);
    let bare = |w: &&String| !w.starts_with('-') && !w.contains('/') && !has_glob(w) && w.trim_matches('.').contains('.');
    out.extend(sh.words.iter().filter(bare).cloned());
    out
}

/// Git subcommands that only read the repository, so their path words name
/// no file the command changes.
const GIT_READS: &[&str] = &[
    "diff",
    "log",
    "show",
    "status",
    "blame",
    "grep",
    "ls-files",
    "ls-tree",
    "rev-parse",
    "cat-file",
    "describe",
    "shortlog",
    "whatchanged",
];

/// What a Bash command that does not only read may change, as rvow records it
/// for the stop hook.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Changes {
    /// Shell words that may name a changed file, each with the directories a `cd`
    /// of the line moved to, so the caller resolves it from each. A word here may
    /// name a new file in an existing folder.
    pub paths: Vec<String>,
    /// Path-like text of heredoc bodies and inline scripts. It counts only where it
    /// names an existing file, or a path with a `/` whose folder exists.
    pub script_paths: Vec<String>,
    pub dirs: Vec<String>,
    /// `rvow put` ran, so the plan of the change may have changed.
    pub plan: bool,
}

/// The files a Bash command may change: its path and file-name words, the
/// expansions of its glob words, and the path-like text of heredoc bodies and
/// inline scripts such as `python3 -c "open('src/a.rs', 'w')"`. A line of git
/// read commands changes nothing.
pub fn bash_changes(cmd: &str, cwd: &Path) -> Changes {
    let sh = parsed(cmd, false);
    let invs: Vec<&[String]> = sh.commands.iter().flat_map(|seg| invocations(seg)).collect();
    let git_read = |inv: &&[String]| {
        matches!(base_name(&inv[0]), "git" | "rgit") && git_subcommand(&inv[1..]).is_some_and(|(sub, _)| GIT_READS.contains(&sub))
    };
    // `sed` without `-i` prints, and `cd` moves; neither changes a file unless
    // the line redirects into one.
    let quiet = |inv: &&[String]| {
        let name = base_name(&inv[0]);
        git_read(inv)
            || READERS.contains(&name)
            || name == "cd"
            || (name == "sed" && !inv.iter().any(|w| w.starts_with("-i") || w.starts_with("--in-place")))
    };
    let mut out = Changes {
        plan: invs.iter().any(|inv| base_name(&inv[0]) == "rvow" && inv.get(1).is_some_and(|w| w == "put")),
        ..Changes::default()
    };
    // An rvow command writes only its own store, never a repository file.
    let rvow = |inv: &&[String]| base_name(&inv[0]) == "rvow";
    if !invs.is_empty() && !writes_files(&invs, cmd) && invs.iter().all(|inv| quiet(inv) || rvow(inv)) {
        return out;
    }
    for inv in &invs {
        let name = base_name(&inv[0]);
        if name == "cd" {
            out.dirs.extend(inv.get(1).cloned());
        }
    }
    out.paths = changed_words(cmd);
    out.paths.extend(glob_paths(cmd, cwd).into_iter().filter_map(|p| p.to_str().map(String::from)));
    // The value of an option such as `--out=report.txt` may name the file it writes.
    for inv in invs.iter().filter(|inv| !rvow(inv)) {
        out.paths
            .extend(inv[1..].iter().filter(|w| w.starts_with('-')).filter_map(|w| w.split_once('=')).map(|(_, value)| value.to_string()));
    }
    // Heredoc bodies and quoted scripts are not shell words, so read the raw
    // text for anything shaped like a path, but only when an interpreter or a
    // shell runs that text: for `rvow put`, `cat` or `git commit -m` it is data.
    // The caller keeps only the ones that name an existing file, or a path with
    // a `/` whose folder exists.
    let runs_script = invs.iter().any(|inv| {
        let name = base_name(&inv[0]);
        is_interpreter(name) || is_shell(name) || name == "eval"
    });
    if !runs_script {
        return out;
    }
    let is_path_char = |c: char| c.is_alphanumeric() || matches!(c, '_' | '.' | '/' | '-');
    for piece in cmd.split(|c: char| !is_path_char(c)) {
        let piece = piece.strip_prefix("./").unwrap_or(piece).trim_end_matches(['.', '-']);
        let named = piece.contains('/')
            || piece.rsplit_once('.').is_some_and(|(stem, ext)| !stem.is_empty() && ext.chars().all(char::is_alphanumeric));
        if named && !piece.is_empty() {
            out.script_paths.push(piece.to_string());
        }
    }
    out
}

/// The filesystem paths that the glob words of `cmd` expand to from `cwd`.
/// A word without `*`, `?` or `[` is not expanded, and a path that does not
/// exist is dropped, so a pattern that matches nothing yields nothing.
pub fn glob_paths(cmd: &str, cwd: &Path) -> Vec<PathBuf> {
    // A name the command sets itself wins; any other `$NAME` or `${NAME}` falls back to
    // the process environment, as a shell does.
    let home = std::env::var("HOME").ok();
    let sh = parsed(cmd, true);
    let mut scan = Scan { out: Vec::new(), left: DIR_SCAN_LIMIT };
    for word in &sh.words {
        if !has_glob(word) {
            continue;
        }
        let word = expand_tilde(word, home.as_deref());
        let path = Path::new(&word);
        let (base, rest) = match path.strip_prefix("/") {
            Ok(rest) => (PathBuf::from("/"), rest),
            Err(_) => (cwd.to_path_buf(), path),
        };
        let parts: Vec<Part> = rest.components().map(|c| Part::new(&c.as_os_str().to_string_lossy())).collect();
        expand(base, &parts, &mut scan);
        if scan.out.len() >= GLOB_LIMIT {
            break;
        }
    }
    let mut out = scan.out;
    out.retain(|p| p.exists());
    out
}

/// A leading `~` or `~/x` as a shell writes it, with HOME from the environment.
fn expand_tilde(word: &str, home: Option<&str>) -> String {
    match home {
        Some(h) if word == "~" => h.to_string(),
        Some(h) => word.strip_prefix("~/").map_or_else(|| word.to_string(), |rest| format!("{h}/{rest}")),
        None => word.to_string(),
    }
}

/// One component of a glob word, with its matcher compiled once.
enum Part {
    Here,
    Up,
    Name(String),
    Glob(globset::GlobMatcher),
    Invalid,
}

impl Part {
    fn new(text: &str) -> Part {
        match text {
            "" | "." => Part::Here,
            ".." => Part::Up,
            _ if has_glob(text) => globset::Glob::new(text).map_or(Part::Invalid, |g| Part::Glob(g.compile_matcher())),
            _ => Part::Name(text.to_string()),
        }
    }
}

/// The expansions so far and the folder entries the call may still read.
struct Scan {
    out: Vec<PathBuf>,
    left: usize,
}

fn expand(prefix: PathBuf, parts: &[Part], scan: &mut Scan) {
    if scan.out.len() >= GLOB_LIMIT {
        return;
    }
    let Some((first, rest)) = parts.split_first() else {
        scan.out.push(prefix);
        return;
    };
    match first {
        Part::Here => expand(prefix, rest, scan),
        Part::Up => expand(prefix.parent().map(Path::to_path_buf).unwrap_or(prefix), rest, scan),
        Part::Name(name) => expand(prefix.join(name), rest, scan),
        Part::Invalid => {}
        Part::Glob(matcher) => {
            let Ok(entries) = std::fs::read_dir(&prefix) else {
                return;
            };
            let mut names = Vec::new();
            for entry in entries {
                // Out of budget: the folder stands for its matches, so a guard
                // still sees a glob inside a protected folder.
                if scan.left == 0 {
                    scan.out.push(prefix);
                    return;
                }
                scan.left -= 1;
                let Ok(entry) = entry else { continue };
                let name = entry.file_name();
                if name.to_str().is_some_and(|n| matcher.is_match(n)) {
                    names.push(name);
                }
            }
            names.sort();
            for name in names {
                if scan.out.len() >= GLOB_LIMIT {
                    return;
                }
                expand(prefix.join(name), rest, scan);
            }
        }
    }
}

/// The first line of the file at `path`, read up to `FIRST_LINE_LIMIT` bytes.
fn first_line(path: &Path) -> String {
    use std::io::{BufRead, Read};
    let Ok(file) = std::fs::File::open(path) else { return String::new() };
    let mut line = Vec::new();
    let _ = std::io::BufReader::new(file.take(FIRST_LINE_LIMIT)).read_until(b'\n', &mut line);
    let text = String::from_utf8_lossy(&line);
    text.trim_end_matches(['\n', '\r']).to_string()
}

/// How much of a script `first_line` reads to find its interpreter line.
const FIRST_LINE_LIMIT: u64 = 4096;

fn reads_no_file(name: &str, arg: &str) -> bool {
    if arg == "-" {
        return true;
    }
    if name.starts_with("python") {
        matches!(arg, "-c" | "-m")
    } else if name.starts_with("node") {
        matches!(arg, "-e" | "-p")
    } else if name.starts_with("perl") {
        matches!(arg, "-e" | "-E")
    } else if name.starts_with("ruby") {
        matches!(arg, "-e")
    } else if name.starts_with("php") {
        matches!(arg, "-r")
    } else {
        matches!(arg, "eval" | "-e")
    }
}

fn interpreter_script<'a>(name: &str, args: &'a [String]) -> Option<&'a String> {
    for w in args {
        if reads_no_file(name, w) {
            return None;
        }
        if !w.starts_with('-') {
            return Some(w);
        }
    }
    None
}

/// Why running a script that `cmd` names must not happen: the file holds an
/// `rvow accept`, `confirm` or `hook`, which belong to the user. A script that
/// the same command writes first cannot be read yet; that stays a known gap.
pub fn script_rule(cmd: &str, cwd: &Path) -> Option<Denial> {
    // No repository context here, so every script keeps the whole-file check.
    script_rule_committed(cmd, cwd, &|p| std::fs::read_to_string(p).ok())
}

/// The same rule with the caller's text selector for a committed test file:
/// when it returns `None` for the script path, the file is data and is not
/// read. When it returns text, only that text is checked; every other script
/// keeps the whole-file check.
pub fn script_rule_committed(cmd: &str, cwd: &Path, committed: &dyn Fn(&Path) -> Option<String>) -> Option<Denial> {
    let sh = parsed(cmd, false);
    for inv in sh.commands.iter().flat_map(|seg| invocations(seg)) {
        let name = base_name(&inv[0]);
        let file = if is_shell(name) {
            inv[1..].iter().find(|w| !w.starts_with('-'))
        } else if is_interpreter(name) {
            interpreter_script(name, &inv[1..])
        } else if name == "source" || name == "." {
            inv.get(1)
        } else if inv[0].contains('/') {
            Some(&inv[0])
        } else {
            None
        };
        let Some(file) = file else {
            continue;
        };
        let path = cwd.join(file);
        let Ok(meta) = std::fs::metadata(&path) else {
            continue;
        };
        if !meta.is_file() || meta.len() > 256 * 1024 || base_name(file) == "rvow" {
            continue;
        }
        let Some(text) = committed(&path) else {
            // A clean committed test file: the text is data and is not read.
            continue;
        };
        // The interpreter comes from the file's own first line, never from
        // the selected text, which may hold only the added lines.
        let first = first_line(&path);
        if let Some(inner) = check_text(name, &first, &text, file) {
            return inner;
        }
    }
    None
}

/// The denial for the text of a script that runs, or `None`.
fn check_text(name: &str, first: &str, text: &str, file: &str) -> Option<Option<Denial>> {
    let foreign = is_interpreter(name) || INTERPRETERS.iter().any(|i| first.starts_with("#!") && first.contains(i));
    let inner = if foreign { owned_in_code(text).and_then(|sub| sub_denial(sub, &[])) } else { bash_rule(text) };
    inner.map(|d| denial(d.rule, format!("{file} holds a command the user owns, so it does not run from here: {}", d.reason)))
}

pub fn script_denial(cmd: &str, cwd: &Path) -> Option<String> {
    script_rule(cmd, cwd).map(|d| d.reason)
}

const WRITERS: &[&str] = &["rm", "mv", "cp", "tee", "sed", "perl", "python", "python3", "chmod", "ln", "truncate", "dd", "install"];
const CONFIG_WRITE_FLAGS: &[&str] = &["--unset", "--unset-all", "--add", "--replace-all", "-e", "--edit"];

/// The files `cmd` redirects output to; `2>&1` and `>/dev/null` are not files.
fn redirect_targets(cmd: &str) -> Vec<String> {
    let chars: Vec<char> = cmd.chars().collect();
    let mut found = Vec::new();
    let mut i = 0;
    while i < chars.len() {
        if chars[i] != '>' {
            i += 1;
            continue;
        }
        let mut j = i + 1;
        while j < chars.len() && matches!(chars[j], '>' | '|') {
            j += 1;
        }
        if chars.get(j) == Some(&'&') && chars.get(j + 1).is_some_and(|c| c.is_ascii_digit() || *c == '-') {
            i = j + 1;
            continue;
        }
        if chars.get(j) == Some(&'&') {
            j += 1;
        }
        while j < chars.len() && chars[j].is_whitespace() {
            j += 1;
        }
        let target: String = chars[j..].iter().take_while(|c| !c.is_whitespace() && !SEPARATORS.contains(c)).collect();
        if target != "/dev/null" {
            found.push(target);
        }
        i = j.max(i + 1);
    }
    found
}

/// True when `sort` writes a file: `-o FILE`, `-oFILE`, or `--output` or any
/// unique prefix of it (`--o`, `--ou`, ...) with `=FILE` or a following word.
fn writes_sort_output(args: &[String]) -> bool {
    args.iter().any(|w| {
        if w.starts_with("--") {
            w.split('=').next().is_some_and(|flag| flag.len() >= 3 && "--output".starts_with(flag))
        } else {
            w.starts_with('-') && w[1..].contains('o')
        }
    })
}

/// Short `uniq` flags that take a value, and their long forms.
const UNIQ_VALUE_FLAGS: &[&str] = &["-f", "-s", "-w", "--skip-fields", "--skip-chars", "--check-chars"];

/// True when `uniq` writes a file: a second positional argument is its output.
fn writes_uniq_output(args: &[String]) -> bool {
    let mut positionals = 0;
    let mut skip_next = false;
    let mut ended = false;
    for w in args {
        if skip_next {
            skip_next = false;
            continue;
        }
        if ended {
            positionals += 1;
        } else if w == "--" {
            ended = true;
        } else if w.starts_with("--") {
            if !w.contains('=') && UNIQ_VALUE_FLAGS.contains(&w.as_str()) {
                skip_next = true;
            }
        } else if w.starts_with('-') && w.len() > 1 {
            // A value flag with nothing attached takes the next word;
            // the rest of the word is its attached value.
            let mut chars = w[1..].chars().peekable();
            while let Some(c) = chars.next() {
                if matches!(c, 'f' | 's' | 'w') {
                    skip_next = chars.peek().is_none();
                    break;
                }
            }
        } else {
            positionals += 1;
        }
        if positionals >= 2 {
            return true;
        }
    }
    false
}

/// A writer such as `rm` or `sed -i`.
fn is_writer(inv: &[String]) -> bool {
    let name = base_name(&inv[0]);
    if name == "sort" {
        return writes_sort_output(&inv[1..]);
    }
    if name == "uniq" {
        return writes_uniq_output(&inv[1..]);
    }
    WRITERS.contains(&name)
        && (name != "sed" || inv[1..].iter().any(|w| w == "--in-place" || (w.starts_with('-') && !w.starts_with("--") && w.contains('i'))))
}

/// The files that `cmd` names as write targets: its redirect targets and the
/// file arguments of a writer such as `cp`, `tee` or `sed -i`. An interpreter
/// is a writer only through a redirect, since its arguments name the script it
/// reads; a word in a commit message or a flag value is no target. Heredoc
/// text is data, and a target counts only once its variables are known: the
/// ones the command sets and the ones in the process environment.
pub fn written_paths(cmd: &str) -> Vec<String> {
    let text = without_heredocs(cmd);
    let mut vars = Vars::with_env();
    for seg in &tokenize(&text).commands {
        for w in seg.iter().take_while(|w| is_assignment(w)) {
            if let Some((name, value)) = w.split_once('=') {
                vars.insert(name, value);
            }
        }
    }
    let sh = parse(&text, 0, &vars);
    let mut out: Vec<String> = redirect_targets(&text).iter().map(|t| expand_vars(t, &vars)).collect();
    for inv in sh.commands.iter().flat_map(|seg| invocations(seg)) {
        let name = base_name(&inv[0]);
        if (is_writer(inv) && !is_interpreter(name)) || name == "touch" {
            out.extend(inv[1..].iter().filter(|w| !w.starts_with('-') && !w.contains('=')).cloned());
        }
    }
    out.retain(|t| !t.contains('$'));
    out
}

/// A command that changes files: a writer such as `rm` or `sed -i`, or a redirect to a file.
fn writes_files(invs: &[&[String]], cmd: &str) -> bool {
    !redirect_targets(cmd).is_empty() || invs.iter().any(|inv| is_writer(inv))
}

/// A command that changes a file `hit` accepts: a redirect to it, or an argument of a writer.
fn writes_to(invs: &[&[String]], cmd: &str, hit: impl Fn(&str) -> bool) -> bool {
    redirect_targets(cmd).iter().any(|t| hit(t)) || invs.iter().any(|inv| is_writer(inv) && inv[1..].iter().any(|w| hit(w)))
}

/// A `git` call that sets or removes `core.hooksPath`; reading it is fine.
fn sets_hooks_path(inv: &[String]) -> bool {
    if !GITS.contains(&base_name(&inv[0])) {
        return false;
    }
    let args = &inv[1..];
    let key = |w: &str| w.to_lowercase().starts_with("core.hookspath");
    let env_form = |w: &String| w.strip_prefix("--config-env=").is_some_and(key);
    if args.windows(2).any(|p| (p[0] == "-c" || p[0] == "--config-env") && key(&p[1])) || args.iter().any(env_form) {
        return true;
    }
    let Some(("config", rest)) = git_subcommand(args) else {
        return false;
    };
    let positional: Vec<&String> = rest.iter().filter(|w| !w.starts_with('-')).collect();
    positional.iter().any(|w| key(w)) && (positional.len() >= 2 || rest.iter().any(|w| CONFIG_WRITE_FLAGS.contains(&w.as_str())))
}

fn names_path(sh: &Shell, cmd: &str, path: &Path) -> bool {
    let home = std::env::var("HOME").unwrap_or_default();
    let full = path.display().to_string();
    let tilde = full.replacen(&home, "~", 1);
    let hit = |text: &str| text.contains(&full) || (!home.is_empty() && text.contains(&tilde));
    hit(cmd) || sh.words.iter().any(|w| hit(w))
}

/// Why a Bash command that names an enforcement file must not run: a write to
/// one of them could switch rvow off.
pub fn bash_enforcement_rule(cmd: &str, paths: &[PathBuf]) -> Option<Denial> {
    let sh = parsed(cmd, false);
    let named = paths.iter().find(|p| names_path(&sh, cmd, p))?;
    let invs: Vec<&[String]> = sh.commands.iter().flat_map(|seg| invocations(seg)).collect();
    let writes = writes_files(&invs, cmd);
    if !writes {
        return None;
    }
    denial(
        Rule::EnforcementFile,
        format!(
            "{} enforces rvow; changing it is for the user. If the plan needs it, run `rvow block <task> --reason \"...\"`",
            named.display()
        ),
    )
}

/// The text of the enforcement denial, for a caller that only prints it.
pub fn bash_enforcement_denial(cmd: &str, paths: &[PathBuf]) -> Option<String> {
    bash_enforcement_rule(cmd, paths).map(|d| d.reason)
}

/// Why a Bash command that names a secret path (the hidden checks) must not
/// run: the implementing agent may not read or write them.
pub fn bash_secret_rule(cmd: &str, paths: &[PathBuf]) -> Option<Denial> {
    let sh = parsed(cmd, false);
    let invs: Vec<&[String]> = sh.commands.iter().flat_map(|seg| invocations(seg)).collect();
    // Text that only names the folder reads nothing: an echo, or a commit or tag message.
    if only_text(&invs, cmd) {
        return None;
    }
    let joined = sh.words.join(" ");
    let named = paths.iter().find(|p| {
        let name = p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        names_path(&sh, cmd, p) || (!name.is_empty() && (cmd.contains(&name) || joined.contains(&name)))
    })?;
    denial(
        Rule::HiddenChecks,
        format!(
            "{} holds checks that stay hidden from the implementing agent; do not read or write them. If the plan needs them, run `rvow block <task> --reason \"...\"`",
            named.display()
        ),
    )
}

pub use rvow_proto::RUN_DIR;

/// The programs that open a Unix socket.
const UNIX_CLIENTS: &[&str] = &["nc", "ncat", "netcat", "socat", "curl"];

/// The `socat` addresses that hold the socket file after the colon.
const UNIX_ADDRESSES: &[&str] = &["UNIX-CONNECT:", "UNIX-LISTEN:", "UNIX-CLIENT:"];

/// The reason a denial of the engine's run folder carries, with `path` the path
/// the call names.
pub fn run_reason(path: &Path) -> String {
    format!(
        "{} reaches the rvow engine's run folder, which belongs to rvow: agents do not talk to the engine socket. Run rvow commands instead",
        path.display()
    )
}

/// The socket file a call of the client `name` is handed, or `None`: `nc -U
/// PATH`, `curl --unix-socket PATH` and `socat UNIX-CONNECT:PATH`.
fn socket_file<'a>(name: &str, args: &'a [String]) -> Option<&'a str> {
    match name {
        "nc" | "ncat" | "netcat" => {
            args.iter().position(|w| w == "-U" || w == "--unixsock").and_then(|at| args.get(at + 1)).map(String::as_str)
        }
        "curl" => args.iter().enumerate().find_map(|(at, w)| match w.strip_prefix("--unix-socket=") {
            Some(path) => Some(path),
            None if w == "--unix-socket" => args.get(at + 1).map(String::as_str),
            None => None,
        }),
        "socat" => args.iter().find_map(|w| UNIX_ADDRESSES.iter().find_map(|a| w.strip_prefix(a))),
        _ => None,
    }
}

/// Why a Bash command must not run: it names the engine's run folder at `run`,
/// runs code that names it, or hands the socket in it to a client such as
/// `nc -U`. The words, the redirect targets and the path words are the ones the
/// store guard reads, so a `~` or `$HOME` form, a relative path and a glob are
/// read the same way.
pub fn bash_run_rule(cmd: &str, cwd: &Path, run: &Path) -> Option<Denial> {
    // Every way in names the folder, runs a socket client, or starts inside the
    // folder; any other command skips the parses below, which copy the env.
    // A socket client counts only as a whole word, so `cargo run` or `rsync` is
    // no client.
    let words = cmd.split(|c: char| c.is_whitespace() || matches!(c, ';' | '|' | '&' | '(' | ')' | '`' | '\'' | '"'));
    let client = words.map(|w| w.rsplit('/').next().unwrap_or(w)).any(|w| UNIX_CLIENTS.contains(&w));
    // A path into the folder has a slash beside its name, which `cargo run` has not.
    let names_run = cmd.contains("/run") || cmd.contains("run/");
    if !client && !names_run && !cwd.as_os_str().as_encoded_bytes().ends_with(RUN_DIR.as_bytes()) {
        return None;
    }
    let run_key = key(&real_path(run, cwd));
    let inside = at_or_under(&key(&real_path(cwd, cwd)), &run_key);
    if !inside && !client && !names_run {
        return None;
    }
    let given = run.display().to_string();
    let home = std::env::var("HOME").ok().filter(|h| !h.is_empty());
    let tilde = home.as_deref().map(|h| given.replacen(h, "~", 1)).filter(|t| *t != given);
    let names = |text: &str| text.contains(&given) || tilde.as_deref().is_some_and(|t| text.contains(t));
    let sh = parsed(cmd, true);
    // A word or a redirect target that holds the folder's path as text, such as
    // the code of `python3 -c "... connect('<run>/engine.sock')"`.
    if sh.words.iter().any(|w| names(w)) {
        return denial(Rule::Store, run_reason(run));
    }
    // A path word at or under the folder, through `~`, `$HOME`, `.` or `..`.
    for word in path_words(cmd) {
        if at_or_under(&key(&real_path(Path::new(&word), cwd)), &run_key) {
            return denial(Rule::Store, run_reason(run));
        }
    }
    // A socket client handed a path in the folder, such as `nc -U engine.sock`
    // run from inside it.
    for seg in &sh.commands {
        for inv in invocations(seg) {
            let Some((program, args)) = inv.split_first() else {
                continue;
            };
            let name = base_name(program);
            if !UNIX_CLIENTS.contains(&name) {
                continue;
            }
            let Some(file) = socket_file(name, args) else {
                continue;
            };
            let file = expand_tilde(file, home.as_deref());
            if at_or_under(&key(&real_path(Path::new(&file), cwd)), &run_key) {
                return denial(Rule::Store, run_reason(run));
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rule_of(cmd: &str) -> Option<Rule> {
        bash_rule(cmd).map(|d| d.rule)
    }

    #[test]
    fn owner_install_is_denied_in_every_form() {
        for cmd in ["rvow install", "rvow install --all", "cd /tmp && rvow install --all", "rvow --human install", "cargo run -- install"] {
            assert_eq!(rule_of(cmd), Some(Rule::OwnerCommand), "{cmd}");
        }
    }

    #[test]
    fn owner_install_help_is_allowed() {
        for cmd in ["rvow install --help", "rvow install -h"] {
            assert_eq!(rule_of(cmd), None, "{cmd}");
        }
    }

    #[test]
    fn decide_pick_is_denied_to_agents() {
        for cmd in [
            "rvow decide TSK-x keep",
            "rvow decide TSK-x now",
            "rvow decide TSK-x archive",
            "cd /tmp && rvow decide TSK-x keep",
            "rvow --human decide TSK-x keep",
            "cargo run -- decide TSK-x keep",
            "python3 -c \"import os; os.system('rvow decide TSK-x keep')\"",
            "$RVOW decide TSK-x keep",
        ] {
            assert_eq!(rule_of(cmd), Some(Rule::OwnerCommand), "{cmd}");
        }
    }

    #[test]
    fn decide_list_and_help_are_allowed() {
        for cmd in ["rvow decide --list", "rvow decide --help", "rvow decide -h"] {
            assert_eq!(rule_of(cmd), None, "{cmd}");
        }
    }

    struct TempDir(PathBuf);

    impl TempDir {
        fn new(name: &str) -> TempDir {
            let dir = std::env::temp_dir().join(format!("rvow-shell-{}-{name}", std::process::id()));
            let _ = std::fs::remove_dir_all(&dir);
            std::fs::create_dir_all(&dir).unwrap();
            TempDir(dir)
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn glob_in_a_huge_folder_stands_for_the_folder() {
        let dir = TempDir::new("huge-glob");
        let deps = dir.0.join("deps");
        std::fs::create_dir(&deps).unwrap();
        for i in 0..=DIR_SCAN_LIMIT {
            std::fs::File::create(deps.join(format!("prepare-{i}"))).unwrap();
        }
        assert_eq!(glob_paths("ls deps/prepare-* | head -1", &dir.0), vec![deps]);
    }

    #[test]
    fn globs_over_many_folders_share_one_budget() {
        let dir = TempDir::new("many-glob");
        let per = 128;
        for d in 0..=DIR_SCAN_LIMIT / per {
            let sub = dir.0.join(format!("d{d:04}"));
            std::fs::create_dir(&sub).unwrap();
            for f in 0..per {
                std::fs::File::create(sub.join(format!("f{f}"))).unwrap();
            }
        }
        // 129 folders of 128: the budget runs out in the 127th, and it and the
        // two folders after it stand for their matches.
        let out = glob_paths("ls */missing-*", &dir.0);
        let names: Vec<_> = out.iter().filter_map(|p| p.file_name()?.to_str()).collect();
        assert_eq!(names, ["d0126", "d0127", "d0128"]);
    }

    #[test]
    fn scale_shell_parse_once_with_dollar() {
        let cmd = "cp $HOME/a.rs ./b.rs && cat ${HOME}/c";
        let before = PARSES.with(std::cell::Cell::get);
        for _ in 0..3 {
            path_words(cmd);
            glob_paths(cmd, Path::new("/"));
            bash_rule(cmd);
            reads_only(cmd);
        }
        assert_eq!(PARSES.with(std::cell::Cell::get) - before, 2, "one parse with the environment, one without");
    }

    #[test]
    fn scale_shell_first_line_of_a_big_script() {
        let dir = TempDir::new("first-line");
        let script = dir.0.join("run.sh");
        let mut text = String::from("#!/bin/bash\r\n");
        text.push_str(&"echo x\n".repeat(100_000));
        std::fs::write(&script, &text).unwrap();
        assert_eq!(first_line(&script), "#!/bin/bash");
        std::fs::write(&script, "z".repeat(10_000)).unwrap();
        assert_eq!(first_line(&script).len(), FIRST_LINE_LIMIT as usize);
    }

    #[test]
    fn glob_in_a_small_folder_expands() {
        let dir = TempDir::new("small-glob");
        std::fs::write(dir.0.join("a.rs"), "").unwrap();
        std::fs::write(dir.0.join("b.txt"), "").unwrap();
        assert_eq!(glob_paths("cat *.rs", &dir.0), vec![dir.0.join("a.rs")]);
    }

    #[test]
    fn hook_in_script_inline_code_is_denied() {
        for cmd in [
            "python3 -c \"import os; os.system('rvow hook post-tool-use')\"",
            "node -e \"require('child_process').execSync('rvow hook pre-tool-use')\"",
            "$RVOW hook post-tool-use",
            "python3 -c \"import subprocess; subprocess.run(['rvow', 'hook', 'stop'])\"",
            "python3 -c \"import subprocess; b='rvow'; subprocess.run([b, 'hook', 'stop'])\"",
            "python3 -c \"import os, subprocess; r=os.environ.get('RVOW_BIN', 'rvow'); subprocess.run([r, '--toon', '--human', 'hook'])\"",
            "python3 -c \"import subprocess; a=['hook', 'stop']; subprocess.run(['rvow'] + a)\"",
        ] {
            assert_eq!(rule_of(cmd), Some(Rule::WitnessFile), "{cmd}");
        }
    }

    #[test]
    fn hook_in_script_prose_and_other_programs_are_allowed() {
        for cmd in ["python3 -c \"print('rvow hooks')\"", "python3 -c \"print('a pre-push hook')\"", "git hook run pre-push"] {
            assert_eq!(rule_of(cmd), None, "{cmd}");
        }
    }

    /// A run folder in a temp folder, with the socket and the harness list in it.
    fn run_folder(name: &str) -> (TempDir, PathBuf) {
        let dir = TempDir::new(name);
        let run = dir.0.join(RUN_DIR);
        std::fs::create_dir_all(&run).unwrap();
        std::fs::write(run.join("engine.sock"), "").unwrap();
        std::fs::write(run.join("harnesses"), "").unwrap();
        (dir, run)
    }

    #[test]
    fn shell_engine_socket_a_command_that_names_the_run_folder_is_denied() {
        let (dir, run) = run_folder("engine-socket");
        let sock = run.join("engine.sock");
        let harnesses = run.join("harnesses");
        for cmd in [
            format!("nc -U {}", sock.display()),
            format!("curl --unix-socket {} http://x/v1/hook", sock.display()),
            format!("cat {}", harnesses.display()),
            format!("echo 1 > {}", harnesses.display()),
            format!("python3 -c \"import socket; s=socket.socket(socket.AF_UNIX); s.connect('{}')\"", sock.display()),
        ] {
            let d = bash_run_rule(&cmd, &dir.0, &run).unwrap_or_else(|| panic!("{cmd}"));
            assert_eq!(d.rule, Rule::Store);
            assert!(d.reason.contains("belongs to rvow") && d.reason.contains("engine socket"), "{}", d.reason);
        }
    }

    #[test]
    fn shell_engine_socket_a_relative_socket_file_is_denied() {
        let (dir, run) = run_folder("engine-socket-rel");
        for cmd in ["nc -U run/engine.sock", "socat UNIX-CONNECT:run/engine.sock -", "cat ./run/harnesses"] {
            assert_eq!(bash_run_rule(cmd, &dir.0, &run).map(|d| d.rule), Some(Rule::Store), "{cmd}");
        }
    }

    #[test]
    fn shell_engine_socket_a_home_form_is_denied() {
        let Some(home) = std::env::var("HOME").ok().filter(|h| !h.is_empty()) else { return };
        let run = std::path::PathBuf::from(&home).join(RUN_DIR);
        let dir = TempDir::new("engine-socket-home");
        for cmd in [format!("cat ~/{RUN_DIR}/harnesses"), format!("cat $HOME/{RUN_DIR}/engine.sock")] {
            assert_eq!(bash_run_rule(&cmd, &dir.0, &run).map(|d| d.rule), Some(Rule::Store), "{cmd}");
        }
        assert_eq!(bash_run_rule("cat $HOME/other", &dir.0, &run), None);
    }

    #[test]
    fn shell_engine_socket_another_socket_and_plain_reads_are_allowed() {
        let (dir, run) = run_folder("engine-socket-ok");
        for cmd in ["ls /tmp", "cat README.md", "nc -U /tmp/other.sock", "socat UNIX-CONNECT:/tmp/other.sock -"] {
            assert_eq!(bash_run_rule(cmd, &dir.0, &run), None, "{cmd}");
        }
    }

    #[test]
    fn daemon_is_denied_to_agents_in_every_form() {
        for cmd in [
            "rvow daemon",
            "rvow daemon --host-pipe",
            "rvow daemon --print-hello",
            "cargo run -q -p rvow -- daemon --host-pipe",
            "$RVOW daemon --host-pipe",
            "python3 -c \"import subprocess; subprocess.Popen(['rvow', 'daemon', '--host-pipe'])\"",
        ] {
            assert_eq!(rule_of(cmd), Some(Rule::OwnerCommand), "{cmd}");
        }
    }

    #[test]
    fn hook_in_script_file_is_denied() {
        let dir = TempDir::new("hook-file");
        std::fs::write(dir.0.join("probe.py"), "import os\nos.system(\"rvow hook stop\")\n").unwrap();
        let d = script_rule("python3 probe.py", &dir.0).expect("python3 probe.py");
        assert_eq!(d.rule, Rule::WitnessFile);
        assert!(d.reason.starts_with("probe.py holds a command the user owns"), "{}", d.reason);
    }
}
