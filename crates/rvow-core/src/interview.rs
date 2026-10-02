//! The interview before accept: open questions in the shape of Claude Code's
//! `AskUserQuestion`, and `rvow answer`, which records each decision in the
//! proposal's `## Clarifications` section.

use std::path::Path;

use serde_json::{Value, json};

use crate::change::{self, PROPOSAL, Stage};
use crate::error::{Error, Result};
use crate::lint::{Question, questions};
use crate::plan;
use crate::store::Project;

/// Questions per round: the most `AskUserQuestion` takes in one call.
pub const ROUND: usize = 4;
const HEADER_LEN: usize = 12;
/// The option offered when a question names only its recommended answer, so
/// every question has the two options `AskUserQuestion` needs.
pub const DECIDE_LATER: &str = "Decide later";

fn header(q: &str) -> String {
    let mut out = String::new();
    for w in q.split_whitespace() {
        if out.len() + w.len() + usize::from(!out.is_empty()) > HEADER_LEN {
            break;
        }
        if !out.is_empty() {
            out.push(' ');
        }
        out.push_str(w);
    }
    if out.is_empty() {
        q.chars().take(HEADER_LEN).collect()
    } else {
        out
    }
}

fn question_json(change: &str, q: &Question) -> Value {
    let mut options = Vec::new();
    if let Some(r) = &q.recommended {
        options.push(json!({
            "label": format!("{r} (Recommended)"),
            "description": q.why.clone().unwrap_or_else(|| "the recommended answer".into()),
        }));
    }
    for o in &q.options {
        options.push(json!({"label": o, "description": ""}));
    }
    if options.len() < 2 {
        options.push(json!({
            "label": DECIDE_LATER,
            "description": "keep the recommended answer as an assumption",
        }));
    }
    json!({
        "ref": format!("{change}:{}:{}", q.path, q.line),
        "question": q.question,
        "header": header(&q.question),
        "options": options,
        "multiSelect": false,
    })
}

/// The next round of open questions for the branch's plans (or `target`),
/// at most `ROUND`, with the count still waiting after it.
pub fn round(project: &Project, cwd: &Path, target: Option<&str>) -> Result<Value> {
    let mut all = Vec::new();
    for r in plan::check(project, cwd, target)? {
        for q in &r.questions {
            all.push(question_json(&r.change.id.to_string(), q));
        }
    }
    let remaining = all.len().saturating_sub(ROUND);
    all.truncate(ROUND);
    Ok(json!({"questions": all, "remaining": remaining}))
}

/// What `answer` recorded.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Answered {
    pub change: String,
    pub question: String,
    pub answer: String,
    pub recommended: bool,
}

/// Records the user's answer to the question at `reference`
/// (`CHG-id:file:line`): removes its marker in place, so the references of the
/// other open questions stay valid, and appends the decision to the
/// proposal's `## Clarifications`.
pub fn answer(project: &Project, reference: &str, choice: &str) -> Result<Answered> {
    let bad_ref = || {
        Error::new(
            "ref",
            format!("{reference:?} is not a question reference"),
            "use the ref that rvow clarify prints, as CHG-xxxxxx:proposal.md:12",
        )
    };
    let mut parts = reference.rsplitn(3, ':');
    let line: usize = parts
        .next()
        .and_then(|l| l.parse().ok())
        .ok_or_else(bad_ref)?;
    let file = parts.next().ok_or_else(bad_ref)?;
    let change_id = parts.next().ok_or_else(bad_ref)?;
    let choice = choice.trim();
    if choice.is_empty() {
        return Err(Error::new(
            "answer",
            "the answer is empty",
            "pass the user's choice",
        ));
    }
    let c = change::resolve(project, change_id)?;
    if c.stage != Stage::Backlog {
        return Err(Error::new(
            "stage",
            format!("{} is {}; questions end at accept", c.id, c.stage.name()),
            "record a new ambiguity in an accepted plan with rvow amend",
        ));
    }
    let _lock = project.lock()?;
    let path = c.path.join(file);
    let text = std::fs::read_to_string(&path).map_err(|e| Error::io(&path, e))?;
    let found = questions(&[(file.to_string(), text.clone())])
        .into_iter()
        .find(|q| q.line == line)
        .ok_or_else(|| {
            Error::new(
                "no-question",
                format!("{reference} holds no open question"),
                "run rvow clarify for the current refs",
            )
        })?;
    let mut lines: Vec<String> = text.lines().map(String::from).collect();
    let target = &mut lines[line - 1];
    *target = target.replacen(&found.marker, "", 1).trim_end().to_string();
    let mut new_text = lines.join("\n");
    if text.ends_with('\n') {
        new_text.push('\n');
    }
    std::fs::write(&path, &new_text).map_err(|e| Error::io(&path, e))?;

    let stripped = choice.trim_end_matches("(Recommended)").trim();
    let (answer, recommended) = if stripped == DECIDE_LATER {
        (
            found
                .recommended
                .clone()
                .unwrap_or_else(|| "not decided".into()),
            true,
        )
    } else {
        let rec = found
            .recommended
            .as_deref()
            .is_some_and(|r| r.eq_ignore_ascii_case(stripped));
        (stripped.to_string(), rec)
    };
    let how = if recommended {
        "recommended"
    } else {
        "overridden"
    };
    let entry = format!(
        "- Q: {} -> A: {answer} ({how}, {})",
        found.question,
        crate::archive::today()
    );
    let proposal = c.path.join(PROPOSAL);
    let mut prop = std::fs::read_to_string(&proposal).map_err(|e| Error::io(&proposal, e))?;
    if !prop.ends_with('\n') {
        prop.push('\n');
    }
    if !prop.lines().any(|l| l.trim() == "## Clarifications") {
        prop.push_str("\n## Clarifications\n");
    }
    // The section is last, so appending at the end of the file places the
    // entry inside it.
    if prop
        .lines()
        .rev()
        .find(|l| l.starts_with("## "))
        .map(str::trim)
        == Some("## Clarifications")
    {
        prop.push_str(&entry);
        prop.push('\n');
    } else {
        let at = prop.find("## Clarifications").expect("section exists");
        let end = prop[at + 3..]
            .find("\n## ")
            .map_or(prop.len(), |i| at + 3 + i + 1);
        prop.insert_str(end, &format!("{entry}\n"));
    }
    std::fs::write(&proposal, prop).map_err(|e| Error::io(&proposal, e))?;
    Ok(Answered {
        change: c.id.to_string(),
        question: found.question,
        answer,
        recommended,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::git::testrepo::*;
    use crate::store::{Mode, init};

    #[test]
    fn headers_fit_twelve_chars() {
        assert_eq!(header("Where do ids live?"), "Where do ids");
        assert_eq!(header("Supercalifragilistic"), "Supercalifra");
    }

    #[test]
    fn a_round_and_answers_keep_refs_stable() {
        let tmp = TempDir::new();
        let repo = repo_with_commit(&tmp.0, "r");
        let p = init(&repo, &tmp.0.join("home"), Mode::Home).unwrap();
        let c = change::new(&p, "T", "main", true).unwrap();
        let prop = c.path.join(PROPOSAL);
        let text = std::fs::read_to_string(&prop).unwrap();
        let marks = (1..=5)
            .map(|n| {
                format!(
                    "[NEEDS CLARIFICATION: Q{n}? | recommended: a{n} | options: b{n} | why: w{n}]\n"
                )
            })
            .collect::<String>();
        std::fs::write(&prop, text.replace("## Why\n", &format!("## Why\n{marks}"))).unwrap();

        let r = round(&p, &repo, None).unwrap();
        assert_eq!(r["questions"].as_array().unwrap().len(), ROUND);
        assert_eq!(r["remaining"], 1);
        let q0 = &r["questions"][0];
        assert_eq!(q0["options"][0]["label"], "a1 (Recommended)");
        assert_eq!(q0["options"][0]["description"], "w1");
        assert_eq!(q0["options"][1]["label"], "b1");
        let first = q0["ref"].as_str().unwrap().to_string();
        let second = r["questions"][1]["ref"].as_str().unwrap().to_string();

        let a = answer(&p, &first, "a1 (Recommended)").unwrap();
        assert!(a.recommended);
        let b = answer(&p, &second, "my own").unwrap();
        assert!(!b.recommended);
        assert_eq!(answer(&p, &first, "x").unwrap_err().code, "no-question");

        let text = std::fs::read_to_string(&prop).unwrap();
        assert!(
            text.contains("- Q: Q1? -> A: a1 (recommended,")
                && !text.contains("[NEEDS CLARIFICATION: Q1?"),
            "{text}"
        );
        assert!(
            text.contains("- Q: Q2? -> A: my own (overridden,"),
            "{text}"
        );
        assert_eq!(
            round(&p, &repo, None).unwrap()["questions"]
                .as_array()
                .unwrap()
                .len(),
            3
        );
        assert!(
            crate::proposal::parse_proposal(&text).1.is_empty(),
            "{text}"
        );
    }

    #[test]
    fn decide_later_keeps_the_recommended_answer() {
        let tmp = TempDir::new();
        let repo = repo_with_commit(&tmp.0, "r");
        let p = init(&repo, &tmp.0.join("home"), Mode::Home).unwrap();
        let c = change::new(&p, "T", "main", true).unwrap();
        let prop = c.path.join(PROPOSAL);
        let text = std::fs::read_to_string(&prop).unwrap();
        std::fs::write(
            &prop,
            text + "[NEEDS CLARIFICATION: Size? | recommended: 20 lines]\n",
        )
        .unwrap();
        let r = round(&p, &repo, None).unwrap();
        assert_eq!(r["questions"][0]["options"][1]["label"], DECIDE_LATER);
        let reference = r["questions"][0]["ref"].as_str().unwrap().to_string();
        let a = answer(&p, &reference, DECIDE_LATER).unwrap();
        assert_eq!(a.answer, "20 lines");
        assert!(a.recommended);
    }
}
