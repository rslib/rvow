//! Reads a Bash command the way a shell would, well enough to spot the calls
//! that the hooks deny: quotes and backslashes that split a word are removed,
//! a plain variable assignment is expanded, and the code inside `sh -c`,
//! `eval` and `$(...)` is read as commands too. Anything a shell computes at
//! run time (a generated name, a downloaded script) stays a known gap that the
//! final gate covers.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use crate::change::LEDGER;
use crate::gate::cache::CACHE_FILE;
use crate::store::IDS_FILE;

const SEPARATORS: &[char] = &[';', '&', '|', '(', ')', '`', '\n'];
const SHELLS: &[&str] = &["sh", "bash", "zsh", "dash", "ksh", "ash"];
const INTERPRETERS: &[&str] = &["python", "python3", "node", "perl", "ruby", "php", "deno", "bun"];
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

fn expand_vars(word: &str, vars: &HashMap<String, String>) -> String {
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
                out.push_str(v);
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
    let at = inv.iter().position(|w| w.starts_with('-') && !w.starts_with("--") && w.contains('c'))?;
    inv.get(at + 1).cloned()
}

fn parse(cmd: &str, depth: u8, outer: &HashMap<String, String>) -> Shell {
    let tokens = tokenize(&without_heredocs(cmd));
    let mut vars = outer.clone();
    for seg in &tokens.commands {
        for w in seg {
            if let Some((name, value)) = w.split_once('=').filter(|_| is_assignment(w)) {
                vars.insert(name.to_string(), value.to_string());
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

fn mentions_accept(code: &str) -> bool {
    let tokens: Vec<&str> = code.split(|c: char| !c.is_alphanumeric()).collect();
    tokens.contains(&"rvow") && tokens.iter().any(|t| matches!(*t, "accept" | "confirm"))
}

fn owner_command(sub: &str) -> String {
    format!("rvow {sub} belongs to the user; the hooks deny it for agents. Ask the user to run /rvow-{sub} <id>, or to type ! rvow {sub}")
}

/// The subcommand of an `rvow` or `cargo run` call that the user owns.
fn rvow_denial(inv: &[String]) -> Option<String> {
    let name = base_name(&inv[0]);
    let args: &[String] = if name == "rvow" {
        &inv[1..]
    } else if name == "cargo" {
        let run = inv.iter().position(|w| w == "run")?;
        &inv[run + 1..]
    } else if inv[0].starts_with('$') {
        // A command named by a variable that is not known here.
        return inv[1..].iter().any(|w| matches!(w.as_str(), "accept" | "confirm")).then(|| owner_command("accept"));
    } else if is_interpreter(name) {
        return inv[1..].iter().any(|w| mentions_accept(w)).then(|| owner_command("accept"));
    } else {
        return None;
    };
    let sub = args.iter().find(|x| !x.starts_with('-') && *x != "rvow")?;
    match sub.as_str() {
        "accept" | "confirm" => Some(owner_command(sub)),
        "ingest" => Some(
            "rvow ingest records proofs from a CI file and belongs to the user; ask the user to run `! rvow ingest <file>`, or run `rvow prove <task>` to prove it here"
                .into(),
        ),
        _ => None,
    }
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

fn git_denial(inv: &[String]) -> Option<String> {
    if base_name(&inv[0]) != "git" {
        return None;
    }
    let args = &inv[1..];
    if args.iter().any(|w| w.starts_with("--no-v") && "--no-verify".starts_with(w.as_str())) {
        return Some("--no-verify skips the hooks that check the plan; run git commit and git push without it".into());
    }
    let (sub, rest) = git_subcommand(args)?;
    (sub == "commit" && commit_skips_hooks(rest))
        .then(|| "git commit -n skips the hooks that check the plan; run git commit without -n".into())
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

/// Why a Bash command must not run, or `None`.
pub fn bash_denial(cmd: &str) -> Option<String> {
    let sh = parse(cmd, 0, &HashMap::new());
    let invs: Vec<&[String]> = sh.commands.iter().flat_map(|seg| invocations(seg)).collect();
    if let Some(reason) = invs.iter().find_map(|inv| rvow_denial(inv)) {
        return Some(reason);
    }
    let w = &sh.words;
    let rvow_answer = w.iter().any(|x| base_name(x) == "rvow") && w.iter().any(|x| x == "answer");
    let via_dialog = w.windows(2).any(|p| p[0] == "--via" && p[1] == "dialog") || w.iter().any(|x| x == "--via=dialog");
    if rvow_answer && via_dialog {
        return Some(
            "only the rvow_ask tool records an answer as given in a dialog; run `rvow answer <ref> \"<choice>\"` to record a chat answer"
                .into(),
        );
    }
    if let Some(reason) = invs.iter().find_map(|inv| git_denial(inv)) {
        return Some(reason);
    }
    if invs.iter().any(|inv| decodes_text(inv)) && invs.iter().any(|inv| runs_given_text(inv)) {
        return Some(
            "decoding text into a shell hides what runs; write the command out in the Bash call, and ask the user to run `! rvow accept <id>` if it is an accept"
                .into(),
        );
    }
    let joined = w.join(" ").to_lowercase();
    let lower = cmd.to_lowercase();
    if lower.contains("core.hookspath") || joined.contains("core.hookspath") {
        return Some("core.hooksPath decides which hooks run; changing it is for the user. If the plan needs it, run rvow block <task> --reason \"...\"".into());
    }
    let in_git_dir =
        w.iter().any(|x| x.ends_with(".git") || x.ends_with(".git/")) && w.iter().any(|x| x.to_lowercase().starts_with("hooks"));
    if lower.contains(".git/hooks") || joined.contains(".git/hooks") || in_git_dir {
        return Some(".git/hooks holds the pre-push gate; only rvow install writes it. Run rvow install instead".into());
    }
    let names_cache = w.iter().any(|x| !x.contains(char::is_whitespace) && base_name(x) == CACHE_FILE);
    let only_rvow_first = sh.commands.first().and_then(|c| c.first()).is_some_and(|x| base_name(x) == "rvow");
    if names_cache && !only_rvow_first {
        return Some(format!("{CACHE_FILE} is written and read only by rvow gate; run `rvow gate --rerun` instead"));
    }
    let names_rvow_file = w.iter().any(|x| {
        let b = base_name(x);
        !x.contains(char::is_whitespace) && (b == LEDGER || b == IDS_FILE)
    });
    let only_rvow = sh.commands.first().and_then(|c| c.first()).is_some_and(|x| base_name(x) == "rvow");
    if names_rvow_file && !only_rvow {
        return Some(format!(
            "{LEDGER} and {IDS_FILE} are written only by rvow; run `rvow show <change>` to read the plan or `rvow note` to record something"
        ));
    }
    None
}

/// Why running a script that `cmd` names must not happen: the file holds an
/// `rvow accept` or `rvow confirm`, which belong to the user. A script that the
/// same command writes first cannot be read yet; that stays a known gap.
pub fn script_denial(cmd: &str, cwd: &Path) -> Option<String> {
    let sh = parse(cmd, 0, &HashMap::new());
    for inv in sh.commands.iter().flat_map(|seg| invocations(seg)) {
        let name = base_name(&inv[0]);
        let file = if is_shell(name) || is_interpreter(name) {
            inv[1..].iter().find(|w| !w.starts_with('-'))
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
        let Ok(text) = std::fs::read_to_string(&path) else {
            continue;
        };
        let first = text.lines().next().unwrap_or("");
        let foreign = is_interpreter(name) || INTERPRETERS.iter().any(|i| first.starts_with("#!") && first.contains(i));
        let inner = if foreign { mentions_accept(&text).then(|| owner_command("accept")) } else { bash_denial(&text) };
        if let Some(inner) = inner {
            return Some(format!("{file} holds a command the user owns, so it does not run from here: {inner}"));
        }
    }
    None
}

const WRITERS: &[&str] = &["rm", "mv", "cp", "tee", "sed", "perl", "python", "python3", "chmod", "ln", "truncate", "dd", "install"];

fn names_path(sh: &Shell, cmd: &str, path: &Path) -> bool {
    let home = std::env::var("HOME").unwrap_or_default();
    let full = path.display().to_string();
    let tilde = full.replacen(&home, "~", 1);
    let hit = |text: &str| text.contains(&full) || (!home.is_empty() && text.contains(&tilde));
    hit(cmd) || sh.words.iter().any(|w| hit(w))
}

/// Why a Bash command that names an enforcement file must not run: a write to
/// one of them could switch rvow off.
pub(super) fn bash_enforcement_denial(cmd: &str, paths: &[PathBuf]) -> Option<String> {
    let sh = parse(cmd, 0, &HashMap::new());
    let named = paths.iter().find(|p| names_path(&sh, cmd, p))?;
    let writes = cmd.contains('>') || sh.words.iter().any(|w| WRITERS.contains(&base_name(w)));
    writes.then(|| {
        format!(
            "{} enforces rvow; changing it is for the user. If the plan needs it, run `rvow block <task> --reason \"...\"`",
            named.display()
        )
    })
}

/// Why a Bash command that names a secret path (the hidden checks) must not
/// run: the implementing agent may not read or write them.
pub(super) fn bash_secret_denial(cmd: &str, paths: &[PathBuf]) -> Option<String> {
    let sh = parse(cmd, 0, &HashMap::new());
    let joined = sh.words.join(" ");
    let named = paths.iter().find(|p| {
        let name = p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        names_path(&sh, cmd, p) || (!name.is_empty() && (cmd.contains(&name) || joined.contains(&name)))
    })?;
    Some(format!(
        "{} holds checks that stay hidden from the implementing agent; do not read or write them. If the plan needs them, run `rvow block <task> --reason \"...\"`",
        named.display()
    ))
}
