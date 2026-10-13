use std::path::PathBuf;

use rvow_core::change::{self, Stage};
use rvow_core::diag::{Rule, Severity};
use rvow_core::error::Result;
use rvow_core::git;
use rvow_core::import;
use rvow_core::interview;
use rvow_core::lint::Finding;
use rvow_core::plan;
use rvow_core::show;
use rvow_core::store;
use rvow_core::vow;

use super::{branch_label, current_branch, cwd, project};
use crate::ViaArg;
use crate::output::Reply;
use crate::toon::{Node, Obj};

pub fn import_openspec(from: Option<PathBuf>, dry_run: bool) -> Result<Reply> {
    let home = store::home_dir()?;
    let p = project(&home)?;
    let from = match from {
        Some(dir) => dir,
        None => git::toplevel(&cwd()?)?.join("openspec"),
    };
    let report = import::openspec(&p, &from, &current_branch()?, dry_run)?;
    Ok(Reply::lines(report.to_string()))
}

pub fn new(title: &str, small: bool) -> Result<Reply> {
    let home = store::home_dir()?;
    let p = project(&home)?;
    let branch = current_branch()?;
    let (changes, _) = change::list(&p)?;
    let busy = changes.iter().filter(|c| c.stage == Stage::Active).any(|c| {
        std::fs::read_to_string(c.path.join(change::PROPOSAL)).is_ok_and(|t| t.lines().any(|l| l.trim() == format!("branch: {branch}")))
    });
    let c = change::new(&p, title, &branch, small)?;
    let mut r = Reply::new();
    r.line(format!("created {} at {}", c.id, c.path.display()));
    r.set("created", c.id.to_string());
    r.set("path", c.path.display().to_string());
    if busy {
        let hint = format!("{branch} already has an active change; one change per branch is the standard");
        r.line(format!("hint: {hint}"));
        r.set("warning", hint);
    }
    r.help("Run `rvow guide proposal` for how to write the proposal");
    r.help(format!("Run `rvow check {}` when the plan is written", c.id));
    Ok(r)
}

/// One finding, stray folder or missing program, in the same shape for
/// `validate` and `check`.
struct Problem {
    at: String,
    level: &'static str,
    code: &'static str,
    message: String,
    fix: &'static str,
}

impl Problem {
    fn add(&self, r: &mut Reply, indent: &str) {
        r.line(format!("{indent}{}: {}[{}]: {}", self.at, self.level, self.code, self.message));
        if !self.fix.is_empty() {
            r.line(format!("{indent}  fix: {}", self.fix));
        }
    }

    fn row(&self) -> Obj {
        obj! {
            "at" => self.at,
            "level" => self.level,
            "code" => self.code,
            "message" => self.message,
            "fix" => self.fix,
        }
    }
}

fn problems(prefix: &str, findings: &[Finding]) -> Vec<Problem> {
    findings
        .iter()
        .map(|(path, d)| {
            let error = d.severity() == Severity::Error;
            Problem {
                at: format!("{prefix}{path}:{}", d.line),
                level: if error { "error" } else { "warning" },
                code: d.rule.code(),
                message: d.message.clone(),
                fix: if error { d.rule.fix() } else { "" },
            }
        })
        .collect()
}

pub fn validate() -> Result<Reply> {
    let home = store::home_dir()?;
    let p = project(&home)?;
    let (living, findings) = change::load_living(&p)?;
    let mut all = problems("", &findings);
    let (changes, stray) = change::list(&p)?;
    for path in stray {
        all.push(Problem {
            at: path.strip_prefix(&p.root).unwrap_or(&path).display().to_string(),
            level: "error",
            code: "stray",
            message: "not a change folder name, expected CHG-xxxxxx-<slug>".to_owned(),
            fix: "",
        });
    }
    let mut count = 0usize;
    for c in changes.iter().filter(|c| matches!(c.stage, Stage::Backlog | Stage::Active)) {
        count += 1;
        let loaded = change::load(&c.path, &living)?;
        let prefix = format!("{}/", c.path.strip_prefix(&p.root).unwrap_or(&c.path).display());
        all.extend(problems(&prefix, &loaded.findings));
    }
    let errors = all.iter().filter(|f| f.level == "error").count();
    let mut r = Reply::new();
    for f in &all {
        f.add(&mut r, "");
    }
    r.line(format!("checked {} specs and {count} open changes: {errors} errors", living.len()));
    r.set("specs", living.len());
    r.set("changes", count);
    r.set("errors", errors);
    r.table("findings", all.iter().map(Problem::row).collect(), "0 in the specs and open changes");
    r.ok = errors == 0;
    Ok(r)
}

pub fn review(
    target: &str,
    packet: bool,
    record: Option<&std::path::Path>,
    judge: bool,
    harness: Option<&str>,
    model: Option<&str>,
) -> Result<Reply> {
    let home = store::home_dir()?;
    let p = project(&home)?;
    if packet {
        let packet = rvow_core::review::packet_for(&p, &home, &cwd()?, target)?;
        let mut reply = Reply::new();
        reply.line(format!("identity: {}\n{}", packet.identity, packet.text));
        reply.set("identity", packet.identity);
        reply.set("packet", packet.text);
        reply.set("evidence_bytes", packet.evidence_bytes);
        reply.set("truncated", packet.truncated);
        return Ok(reply);
    }
    if let Some(file) = record {
        if judge {
            let harness = harness.unwrap_or("");
            let model = model.unwrap_or("");
            let recorded = rvow_core::review::record_judge_for(&p, &home, &cwd()?, target, file, harness, model)?;
            let mut reply = Reply::new();
            reply.line(format!("judge review: {} of {} findings (event {})", recorded.kept, recorded.total, recorded.event));
            reply.set("event", recorded.event);
            reply.set("total", recorded.total);
            reply.set("kept", recorded.kept);
            reply.set("truncated", recorded.truncated);
            reply.set("harness", recorded.harness);
            reply.set("model", recorded.model);
            let rows = recorded
                .findings
                .iter()
                .map(|finding| {
                    reply.line(format!("  judge: {}", finding.message));
                    obj! { "message" => finding.message, "path" => finding.path, "line" => finding.line }
                })
                .collect();
            reply.table("findings", rows, "0 judge findings");
            return Ok(reply);
        }
        let recorded = rvow_core::review::record_for(&p, &home, &cwd()?, target, file)?;
        let mut reply = Reply::new();
        reply.line(format!("advisory review: {} of {} findings (event {})", recorded.kept, recorded.total, recorded.event));
        reply.set("event", recorded.event);
        reply.set("total", recorded.total);
        reply.set("kept", recorded.kept);
        reply.set("truncated", recorded.truncated);
        let rows = recorded
            .findings
            .iter()
            .map(|finding| {
                reply.line(format!("  advisory: {}", finding.message));
                obj! { "message" => finding.message, "path" => finding.path, "line" => finding.line }
            })
            .collect();
        reply.table("findings", rows, "0 advisory findings");
        return Ok(reply);
    }
    let text = show::review(&p, target)?;
    let mut warnings = String::new();
    for report in plan::check(&p, &cwd()?, Some(target))? {
        for (_, d) in report.findings.iter().filter(|(_, d)| d.rule == Rule::WeakVerify) {
            warnings.push_str(&format!("warning[weak-verify]: {}\n", d.message));
        }
    }
    Ok(Reply::lines(format!("{warnings}{text}\n\n{REVIEW}")))
}

/// `rvow put`: replace one part of a change, or one task or requirement block,
/// with the text on stdin, and print `ok` or the check findings.
pub fn put(target: &str, part: Option<&str>) -> Result<Reply> {
    let home = store::home_dir()?;
    let p = project(&home)?;
    let text = rvow_core::put::stdin()?;
    let out = rvow_core::put::put(&p, &cwd()?, target, part, &text)?;
    Ok(Reply::lines(out.lines.join("\n")))
}

pub fn ids(target: Option<&str>) -> Result<Reply> {
    let home = store::home_dir()?;
    let p = project(&home)?;
    let minted = vow::mint_active(&p, &cwd()?, target)?;
    let mut r = Reply::new();
    let mut rows = Vec::new();
    for (from, to) in &minted {
        r.line(format!("{from} -> {to}"));
        rows.push(obj! { "from" => from.to_string(), "to" => to.to_string() });
    }
    if minted.is_empty() {
        r.line("no placeholders");
    }
    r.table("minted", rows, format!("0 placeholders on branch {}", branch_label()));
    Ok(r)
}

const REVIEW: &str = "\
before you accept, check:
  1. Does each requirement say what the system does, not how?
  2. Does each task's check fail today for the reason the task fixes?
  3. Is anything in Out of scope that you need now?
  4. Is every Assumption one you agree with?
  5. Is the change small enough to review its diff in one sitting?";

pub fn check(target: Option<&str>) -> Result<Reply> {
    let home = store::home_dir()?;
    let p = project(&home)?;
    let reports = plan::check(&p, &cwd()?, target)?;
    let mut r = Reply::new();
    if reports.is_empty() {
        r.line("no open change on this branch");
        r.set("changes", format!("0 open on branch {}", branch_label()));
        r.help("Run `rvow new \"<title>\"` to plan a change");
        return Ok(r);
    }
    let mut items = Vec::new();
    for c in &reports {
        let id = &c.change.id;
        r.line(format!("{id} ({})", c.change.stage.name()));
        let mut found = problems("", &c.findings);
        found.extend(c.slow.iter().map(|s| Problem {
            at: s.task.clone(),
            level: "warning",
            code: "full-suite",
            message: s.message.clone(),
            fix: "move the whole-suite commands into the project `checks`, keep a targeted verify on the task",
        }));
        found.extend(c.missing.iter().map(|(task, prog)| Problem {
            at: task.clone(),
            level: "error",
            code: "missing-program",
            message: format!("{prog} is neither on PATH nor in the repository"),
            fix: "",
        }));
        if c.no_tasks {
            found.push(Problem {
                at: "tasks.md".to_string(),
                level: "error",
                code: "no-tasks",
                message: "tasks.md has no task".to_string(),
                fix: "add tasks as `- TSK-new1 <title>` with verify: and files:",
            });
        }
        for f in &found {
            f.add(&mut r, "  ");
        }
        let mut questions = Vec::new();
        for q in &c.questions {
            r.line(format!("  {}:{}: open question: {}", q.path, q.line, q.question));
            questions.push(Node::Obj(obj! {
                "at" => format!("{}:{}", q.path, q.line),
                "question" => q.question,
            }));
        }
        let ready = !c.blocking();
        let mut item = obj! {
            "id" => id.to_string(),
            "stage" => c.change.stage.name(),
            "ready" => ready,
        };
        if !found.is_empty() {
            let rows = found.iter().map(|f| Node::Obj(f.row())).collect();
            item.push(("findings".to_owned(), Node::List(rows)));
        }
        if !questions.is_empty() {
            item.push(("questions".to_owned(), Node::List(questions)));
            r.help(format!("Run `rvow clarify {id}` for the questions with recommended answers"));
        }
        items.push(Node::Obj(item));
        if ready {
            r.line(format!("  ready to accept: ask the user to run /rvow-accept {id} (or ! rvow accept {id})"));
            r.help(format!("Ask the user to run /rvow-accept {id} (or ! rvow accept {id})"));
        } else {
            r.ok = false;
            r.line("  not ready to accept");
            r.help(format!("Run `rvow check {id}` after fixing the plan"));
        }
    }
    r.set("changes", items);
    Ok(r)
}

pub fn clarify(target: Option<&str>, json: bool) -> Result<Reply> {
    let home = store::home_dir()?;
    let p = project(&home)?;
    let mut r = Reply::new();
    if json {
        let config = rvow_core::config::load(&p, &home)?;
        println!("{}", interview::round_with_cap(&p, &cwd()?, target, config.interview_cap as usize)?);
        return Ok(r);
    }
    let mut rows = Vec::new();
    for c in plan::check(&p, &cwd()?, target)? {
        for q in &c.questions {
            let rec = q.recommended.as_deref().unwrap_or("none given");
            r.line(format!("{}:{}:{}: {} (recommended: {rec})", c.change.id, q.path, q.line, q.question));
            rows.push(obj! {
                "ref" => format!("{}:{}:{}", c.change.id, q.path, q.line),
                "question" => q.question,
                "recommended" => rec,
            });
        }
    }
    if rows.is_empty() {
        r.line("no open questions");
    } else {
        let ask = "ask the user with the rvow_ask tool, or ask in chat and record each answer with rvow answer <ref> \"<choice>\"";
        r.line(ask);
        r.set("count", rows.len());
        r.help(ask);
    }
    r.table("questions", rows, format!("0 open on branch {}", branch_label()));
    Ok(r)
}

pub fn answer(reference: &str, choice: &str, via: ViaArg) -> Result<bool> {
    let home = store::home_dir()?;
    let p = project(&home)?;
    let via = match via {
        ViaArg::Dialog => interview::Via::Dialog,
        ViaArg::Chat => interview::Via::Chat,
    };
    let a = interview::answer(&p, reference, choice, via)?;
    let how = if a.recommended { "the recommended answer" } else { "an answer other than the recommended one" };
    println!("Recorded for {}: {} -> {} ({how}).", a.change, a.question, a.answer);
    println!("Now update the requirement or task this answer affects, and remove any text it contradicts.");
    Ok(true)
}

pub fn claim(target: &str) -> Result<Reply> {
    let home = store::home_dir()?;
    let p = project(&home)?;
    let from = plan::claim(&p, &cwd()?, target)?;
    let branch = branch_label();
    let mut r = Reply::new();
    r.line(format!("moved {target} from {from} to {branch}"));
    r.set("claimed", target.to_string());
    r.set("from", from);
    r.set("to", branch);
    r.help(format!("Run `rvow check {target}` to see if the plan is ready"));
    Ok(r)
}
