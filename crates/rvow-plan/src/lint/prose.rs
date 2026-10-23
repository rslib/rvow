use std::borrow::Cow;

use crate::diag::{Diagnostic, Rule};

pub const MAX_WORDS: usize = 60;

const VAGUE: &[&str] = &[
    "fast",
    "quick",
    "quickly",
    "efficient",
    "efficiently",
    "appropriate",
    "appropriately",
    "user friendly",
    "easy",
    "easily",
    "some",
    "several",
    "sufficient",
    "reasonable",
    "optimal",
    "robust",
    "seamless",
    "flexible",
];

const ESCAPE: &[&str] = &[
    "if necessary",
    "if possible",
    "where possible",
    "as appropriate",
    "as required",
    "as needed",
    "etc",
    "including but not limited to",
    "and so on",
];

const PRONOUNS: &[&str] = &["it", "this", "they", "these", "those", "that"];

const TRIGGERS: &[&str] = &["when", "while", "if", "where"];

/// The text with every backtick span replaced by a space.
pub(super) fn prose(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for (i, part) in text.split('`').enumerate() {
        out.push_str(if i % 2 == 0 { part } else { " " });
    }
    out
}

fn code_spans(text: &str) -> impl Iterator<Item = &str> {
    text.split('`').skip(1).step_by(2)
}

fn words(text: &str) -> impl Iterator<Item = &str> {
    text.split(|c: char| !c.is_alphanumeric()).filter(|w| !w.is_empty())
}

/// Lowercase words joined by single spaces and padded, so `" if necessary "`
/// matches whole words only.
pub(super) fn phrase_text(text: &str) -> String {
    let mut out = String::with_capacity(text.len() + 2);
    out.push(' ');
    for w in words(text) {
        for c in w.chars() {
            out.extend(c.to_lowercase());
        }
        out.push(' ');
    }
    out
}

pub(super) fn has_phrase(haystack: &str, phrase: &str) -> bool {
    let bytes = haystack.as_bytes();
    haystack
        .match_indices(phrase)
        .any(|(start, _)| start > 0 && bytes.get(start - 1) == Some(&b' ') && bytes.get(start + phrase.len()) == Some(&b' '))
}

pub(super) fn lint_keywords(text: &str, line: usize, out: &mut Vec<Diagnostic>) {
    let mut strong = 0;
    let mut weak = Vec::new();
    for w in words(text) {
        match w {
            "SHALL" | "MUST" => strong += 1,
            "SHOULD" | "MAY" => weak.push(w),
            _ if w.eq_ignore_ascii_case("shall") || w.eq_ignore_ascii_case("must") => {
                out.push(Diagnostic::new(Rule::Keyword, line, format!("{w:?} carries no meaning unless it is written in capitals")));
            }
            _ => {}
        }
    }
    let total = strong + weak.len();
    if total != 1 {
        out.push(Diagnostic::new(Rule::Keyword, line, format!("found {total} of SHALL, MUST, SHOULD or MAY, expected exactly one")));
    } else if let Some(w) = weak.first() {
        out.push(Diagnostic::new(Rule::WeakKeyword, line, format!("{w} makes the requirement optional")));
    }
}

pub(super) fn lint_ears(text: &str, line: usize, out: &mut Vec<Diagnostic>) {
    let mut ears = |msg: &str| out.push(Diagnostic::new(Rule::Ears, line, msg));
    let first = words(text).next().unwrap_or("");
    let is_trigger = TRIGGERS.iter().any(|t| first.eq_ignore_ascii_case(t));
    if is_trigger && first != first.to_uppercase() {
        ears("write the EARS keyword in capitals, as in WHEN");
    }
    if words(text).filter(|w| *w == "WHEN" || *w == "IF").count() > 1 {
        ears("more than one WHEN or IF trigger, split the requirement");
    }
    if is_trigger {
        match text.find(',') {
            None => ears("end the condition with a comma before the main clause"),
            Some(comma) => {
                if !words(&text[comma..]).any(|w| matches!(w, "SHALL" | "MUST" | "SHOULD" | "MAY")) {
                    ears("the keyword belongs in the main clause, after the condition");
                }
            }
        }
        if first == "IF" && !text.contains(", THEN ") {
            ears("an IF condition takes the form IF <condition>, THEN <system> SHALL");
        }
    }
    let plain: Cow<str> = if text.contains("e.g.") || text.contains("i.e.") {
        Cow::Owned(text.replace("e.g.", "eg").replace("i.e.", "ie"))
    } else {
        Cow::Borrowed(text)
    };
    let breaks =
        plain.char_indices().filter(|(i, c)| matches!(c, '.' | '!' | '?') && plain[i + 1..].starts_with(char::is_whitespace)).count();
    if breaks > 0 {
        ears("write one sentence per requirement");
    }
    if !plain.ends_with('.') {
        ears("end the sentence with a period");
    }
}

pub(super) fn lint_words(raw: &str, text: &str, line: usize, out: &mut Vec<Diagnostic>) {
    let phrases = phrase_text(text);
    for w in VAGUE.iter().filter(|w| has_phrase(&phrases, w)) {
        out.push(Diagnostic::new(Rule::Vague, line, format!("{w:?} is not measurable")));
    }
    for w in ESCAPE.iter().filter(|w| has_phrase(&phrases, w)) {
        out.push(Diagnostic::new(Rule::EscapeClause, line, format!("{w:?} leaves the requirement open")));
    }
    if text.contains('/') {
        out.push(Diagnostic::new(Rule::AndOr, line, "\"/\" or \"and/or\" makes the requirement ambiguous"));
    }
    let subject_after_comma = text.split_once(',').and_then(|(_, rest)| words(rest).next());
    for w in [words(text).next(), subject_after_comma].into_iter().flatten() {
        if PRONOUNS.contains(&w.to_lowercase().as_str()) {
            out.push(Diagnostic::new(Rule::Pronoun, line, format!("{w:?} as a subject hides what it refers to")));
        }
    }
    for span in code_spans(raw) {
        if looks_like_design(span) {
            out.push(Diagnostic::new(Rule::DesignInSpec, line, format!("`{span}` looks like a path or a code name")));
        }
    }
    let count = words(raw).count();
    if count > MAX_WORDS {
        out.push(Diagnostic::new(Rule::Size, line, format!("{count} words, more than {MAX_WORDS}")));
    }
}

fn looks_like_design(span: &str) -> bool {
    let has_ext = span
        .rsplit_once('.')
        .is_some_and(|(stem, ext)| !stem.is_empty() && (1..=4).contains(&ext.len()) && ext.chars().all(|c| c.is_ascii_alphanumeric()));
    span.contains('/') || span.contains("::") || span.contains("()") || has_ext
}

#[cfg(test)]
mod tests {
    use super::{has_phrase, phrase_text};

    #[test]
    fn has_phrase_matches_whole_words_only() {
        assert!(has_phrase(" use as needed here ", "as needed"));
        assert!(!has_phrase(" use as neededly here ", "as needed"));
        assert!(!has_phrase(" has needed ", "as needed"));
        assert!(has_phrase(" fast ", "fast"));
        assert!(!has_phrase(" breakfast ", "fast"));
    }

    #[test]
    fn phrase_text_lowercases_and_pads() {
        assert_eq!(phrase_text(" Hello, World! "), " hello world ");
        assert_eq!(phrase_text("\u{c4}"), " \u{e4} ");
    }
}
