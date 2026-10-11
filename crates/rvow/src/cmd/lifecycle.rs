use std::path::Path;

use rvow_core::amend;
use rvow_core::archive;
use rvow_core::brief;
use rvow_core::change;
use rvow_core::error::{Error, Result};
use rvow_core::notes;
use rvow_core::proof;
use rvow_core::store;
use rvow_core::vow;

use super::{cwd, project};
use crate::NoteArg;
use crate::output::Reply;
use crate::toon::Node;

/// The `next:` part of the brief, so the agent learns what to do as soon as the user decides.
pub fn next_for_agent(p: &store::Project, home: &Path) -> String {
    let text = brief::brief(p, home, &cwd().unwrap_or_default()).unwrap_or_default();
    text.find("next:").and_then(|i| text[i..].lines().next()).unwrap_or("next: run rvow brief").to_string()
}

pub fn accept(target: &str) -> Result<bool> {
    let home = store::home_dir()?;
    let p = project(&home)?;
    let c = change::resolve(&p, target)?;
    let prepared = vow::prepare(&p, &home, c, &cwd()?, &mut std::io::stderr())?;
    for b in &prepared.baseline {
        if b.outcome.exit.is_none() {
            println!("warning: {} baseline ended by a signal or timeout", b.task);
        }
    }
    let tasks = prepared.loaded.tasks.len();
    let base = prepared.base_commit[..12.min(prepared.base_commit.len())].to_string();
    let vowed = vow::commit(&p, prepared)?;
    println!("accepted {} at {} (vow {}): {tasks} tasks, base {base}", vowed.id, vowed.path.display(), vowed.event);
    println!("{}", next_for_agent(&p, &home));
    Ok(true)
}

pub fn archive(target: &str) -> Result<Reply> {
    let home = store::home_dir()?;
    let p = project(&home)?;
    // The judge note is advisory: a failed lookup counts as no note and never stops the archive.
    let judged = matches!(rvow_core::review::judge_for(&p, &cwd()?, target), Ok(Some(_)));
    let a = archive::archive(&p, &home, &cwd()?, target)?;
    let mut r = Reply::new();
    let mut rows = Vec::new();
    for (op, id) in &a.merged {
        r.line(format!("  {} {id}", op.word()));
        rows.push(obj! { "op" => op.word(), "id" => id });
    }
    r.line(format!("archived {} to {}", a.id, a.path.display()));
    if !judged {
        let hint = format!("note: no judge note for the current packet; ask the user to run /rvow-judge {}", a.id);
        r.line(&hint);
        r.set("judge", hint);
    }
    r.line("next: pick the next plan with rvow ls");
    r.set("archived", a.id.to_string());
    r.set("path", a.path.display().to_string());
    r.table("merged", rows, "0 spec deltas");
    r.set("next", "pick the next plan with rvow ls");
    Ok(r)
}

pub fn drop_change(target: &str, reason: &str) -> Result<Reply> {
    let home = store::home_dir()?;
    let path = archive::drop_change(&project(&home)?, target, reason)?;
    let mut r = Reply::new();
    r.line(format!("dropped to {}", path.display()));
    r.set("dropped", path.display().to_string());
    Ok(r)
}

pub fn supersede(old: &str, by: &str, reason: &str) -> Result<Reply> {
    let home = store::home_dir()?;
    let path = archive::supersede(&project(&home)?, old, by, reason)?;
    let mut r = Reply::new();
    r.line(format!("superseded to {}", path.display()));
    r.set("superseded", path.display().to_string());
    Ok(r)
}

pub fn amend(reason: &str, target: Option<&str>, unlock: Option<&str>) -> Result<Reply> {
    let home = store::home_dir()?;
    let p = project(&home)?;
    let mut r = Reply::new();
    let mut items = Vec::new();
    for a in amend::amend(&p, &home, &cwd()?, target, reason, unlock)? {
        r.line(format!("{} for {}:", a.id, a.change));
        let mut rows = Vec::new();
        for i in &a.items {
            r.line(format!("  {}: {}", i.class.name(), i.what));
            rows.push(Node::Obj(obj! { "class" => i.class.name(), "what" => i.what }));
        }
        let mut item = obj! { "id" => a.id.to_string(), "change" => a.change.to_string() };
        item.push(("items".to_owned(), Node::List(rows)));
        items.push(Node::Obj(item));
    }
    r.line("ask the user to run /rvow-confirm (or ! rvow confirm)");
    r.set("amendments", items);
    r.help("Ask the user to run /rvow-confirm (or ! rvow confirm) to decide them");
    Ok(r)
}

pub fn list_pending(p: &store::Project, ids: &[String]) -> Result<bool> {
    let (living, _) = change::load_living(p)?;
    let pending = amend::pending(&proof::active(p, &living)?);
    if pending.is_empty() {
        println!("no pending amendments");
        return Ok(true);
    }
    let shown: Vec<&amend::Pending> = if ids.is_empty() {
        pending.iter().collect()
    } else {
        ids.iter()
            .map(|id| {
                pending.iter().find(|a| a.id.to_string() == *id).ok_or_else(|| {
                    Error::new("no-amendment", format!("{id} is not a pending amendment"), "run rvow confirm to list the pending ones")
                })
            })
            .collect::<Result<_>>()?
    };
    for a in shown {
        for line in a.render() {
            println!("{line}");
        }
    }
    if ids.is_empty() {
        println!("digest {}", amend::digest(&pending));
    }
    println!("decide with: rvow confirm <id>... --accept|--reject, or --accept --all <digest>");
    Ok(true)
}

pub fn confirm(ids: &[String], accept: bool, reject: bool, all: Option<&str>, sign: bool, signer: Option<&str>) -> Result<bool> {
    let home = store::home_dir()?;
    let p = project(&home)?;
    if !accept && !reject {
        if all.is_some() {
            return Err(Error::new("decision", "say what to do with the amendments", "add --accept or --reject"));
        }
        return list_pending(&p, ids);
    }
    let decision = if accept { amend::Decision::Accept } else { amend::Decision::Reject };
    let cfg = rvow_core::config::load(&p, &home)?;
    if cfg.require_signature && !sign {
        return Err(Error::new(
            "signature",
            format!("{} requires a signed confirmation", p.root.display()),
            "add --sign --signer <identity> to the decision",
        ));
    }
    let signing = if sign {
        let key = cfg.signing_key.as_deref().ok_or_else(|| {
            Error::new(
                "signing-key",
                "no global signing_key is configured",
                format!("set signing_key in {}", home.join(store::CONFIG_FILE).display()),
            )
        })?;
        let signer = signer
            .filter(|s| !s.trim().is_empty())
            .ok_or_else(|| Error::new("signer", "a signed decision needs a signer identity", "pass --signer <identity>"))?;
        Some(amend::Signing { key: std::path::Path::new(key), signer })
    } else {
        None
    };
    let decided = amend::confirm_signed(&p, ids, decision, all, signing).inspect_err(|e| {
        if e.code == "digest" {
            let _ = list_pending(&p, &[]);
        }
    })?;
    for id in decided {
        println!("{id}: {}", if accept { "accepted" } else { "rejected" });
    }
    println!(
        "{}",
        if accept {
            "next: the plan edit stands; continue the tasks"
        } else {
            "next: undo the rejected plan edit, or amend it again with a different edit"
        }
    );
    Ok(true)
}

pub fn block(task: &str, reason: &str) -> Result<Reply> {
    let home = store::home_dir()?;
    let p = project(&home)?;
    let (change, task) = notes::block(&p, task, reason)?;
    let mut r = Reply::new();
    r.line(format!("{task} blocked in {change}"));
    r.line("the gate refuses it until the user confirms an amendment that changes or drops it");
    r.set("blocked", task.as_str());
    r.set("change", change.to_string());
    r.help("Run `rvow amend --reason \"<why>\"` after editing the task, for the user to confirm");
    Ok(r)
}

pub fn note(kind: NoteArg, text: &str) -> Result<Reply> {
    let home = store::home_dir()?;
    let p = project(&home)?;
    let kind = match kind {
        NoteArg::Decision => notes::NoteKind::Decision,
        NoteArg::Surprise => notes::NoteKind::Surprise,
        NoteArg::Handoff => notes::NoteKind::Handoff,
    };
    let (change, cut) = notes::note(&p, &cwd()?, kind, text)?;
    let mut r = Reply::new();
    r.line(format!("{} noted in {change}", kind.name()));
    r.set("noted", kind.name());
    r.set("change", change.to_string());
    if cut {
        let w = format!("the handoff kept only its first {} lines", notes::HANDOFF_LINES);
        r.line(format!("warning: {w}"));
        r.set("warning", w);
    }
    Ok(r)
}
