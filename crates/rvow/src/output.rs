//! One command result in two shapes: human text for a terminal, and a value
//! that the agent mode prints as TOON.

use crate::toon::{self, Node, Obj};

/// Long text in TOON is cut to this many chars unless `--full` is given.
const TRUNCATE_AT: usize = 1500;

pub struct Reply {
    pub text: String,
    pub data: Obj,
    pub help: Vec<String>,
    pub ok: bool,
}

impl Reply {
    pub fn new() -> Reply {
        Reply { text: String::new(), data: Obj::new(), help: Vec::new(), ok: true }
    }

    /// A command that printed its own natural text and has nothing more to say.
    pub fn done(ok: bool) -> Reply {
        Reply { ok, ..Reply::new() }
    }

    /// Plain text: as is for a person, `lines[N]` in TOON.
    pub fn lines(text: String) -> Reply {
        let mut r = Reply::new();
        r.set("lines", text.lines().collect::<Vec<_>>());
        r.text = text;
        r
    }

    /// Like `lines`, but TOON keeps only the first `TRUNCATE_AT` chars unless
    /// `full`, and then names the size and the `rerun --full` command.
    pub fn long(text: String, full: bool, rerun: &str) -> Reply {
        let total = text.chars().count();
        if full || total <= TRUNCATE_AT {
            return Reply::lines(text);
        }
        let cut: String = text.chars().take(TRUNCATE_AT).collect();
        let mut lines: Vec<String> = cut.lines().map(str::to_owned).collect();
        lines.push(format!("... (truncated, {total} chars total)"));
        let mut r = Reply::new();
        r.set("lines", lines);
        r.help(format!("Run `{rerun} --full` to see all {total} chars"));
        r.text = text;
        r
    }

    pub fn line(&mut self, line: impl AsRef<str>) {
        self.text.push_str(line.as_ref());
        self.text.push('\n');
    }

    pub fn set(&mut self, key: &str, value: impl Into<Node>) {
        self.data.push((key.to_owned(), value.into()));
    }

    /// A table under `key`, or `key: <empty>` so that "nothing" is explicit.
    pub fn table(&mut self, key: &str, rows: Vec<Obj>, empty: impl Into<String>) {
        if rows.is_empty() {
            self.set(key, empty.into());
        } else {
            self.set(key, Node::List(rows.into_iter().map(Node::Obj).collect()));
        }
    }

    pub fn help(&mut self, line: impl Into<String>) {
        self.help.push(line.into());
    }

    pub fn print(mut self, toon: bool) {
        if !toon {
            print!("{}", self.text);
            return;
        }
        if !self.help.is_empty() {
            let help = std::mem::take(&mut self.help);
            self.set("help", help);
        }
        if !self.data.is_empty() {
            println!("{}", toon::encode(&self.data));
        }
    }
}

/// An error record for the agent: `error:`, then one `help:` line or `help[N]:`.
pub fn error(message: &str, mut help: Vec<String>) -> String {
    let mut value = obj! { "error" => message };
    match help.len() {
        0 => {}
        1 => value.push(("help".to_owned(), help.remove(0).into())),
        _ => value.push(("help".to_owned(), help.into())),
    }
    toon::encode(&value)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn long_text_is_cut_with_its_size_and_the_full_hint() {
        let text = "a\n".repeat(1000);
        let r = Reply::long(text.clone(), false, "rvow show REQ-x");
        let Some((_, Node::List(lines))) = r.data.first() else { panic!("no lines") };
        assert_eq!(lines.len(), 751);
        assert_eq!(lines.last(), Some(&Node::from("... (truncated, 2000 chars total)")));
        assert_eq!(r.help, ["Run `rvow show REQ-x --full` to see all 2000 chars"]);
        assert_eq!(r.text, text);
        assert!(Reply::long(text, true, "x").help.is_empty());
    }

    #[test]
    fn errors_print_one_help_line_or_a_list() {
        assert_eq!(error("bad", vec![]), "error: bad");
        assert_eq!(error("bad", vec!["run rvow init".into()]), "error: bad\nhelp: run rvow init");
        assert_eq!(error("x: y", vec!["a".into(), "b".into()]), "error: \"x: y\"\nhelp[2]:\n  a\n  b");
    }

    #[test]
    fn empty_tables_say_so() {
        let mut r = Reply::new();
        r.table("changes", vec![], "0 on branch main");
        r.table("rows", vec![obj! { "id" => "a", "n" => 1usize }], "");
        assert_eq!(toon::encode(&r.data), "changes: 0 on branch main\nrows[1]{id,n}:\n  a,1");
    }
}
