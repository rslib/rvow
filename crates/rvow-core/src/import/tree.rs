use std::collections::HashSet;
use std::path::{Path, PathBuf};

use super::openspec::{OsReq, OsScenario, parse_openspec};
use crate::change;
use crate::error::{Error, Result};
use crate::id::{Id, Kind};
use crate::spec::parse_spec;

pub(super) struct Cap {
    pub(super) name: String,
    pub(super) reqs: Vec<(Id, OsReq)>,
}

pub(super) type Mint<'a> = dyn FnMut(Kind, Option<&Id>) -> Result<Id> + 'a;

pub(super) fn read(path: &Path) -> Result<String> {
    std::fs::read_to_string(path).map_err(|e| Error::io(path, e))
}

pub(super) fn sorted_dirs(dir: &Path) -> Result<Vec<PathBuf>> {
    let mut dirs: Vec<PathBuf> = match std::fs::read_dir(dir) {
        Ok(entries) => entries.flatten().map(|e| e.path()).filter(|p| p.is_dir()).collect(),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Vec::new(),
        Err(e) => return Err(Error::io(dir, e)),
    };
    dirs.sort();
    Ok(dirs)
}

pub(super) fn existing_caps(root: &Path) -> Result<Vec<Cap>> {
    let mut caps = Vec::new();
    for dir in sorted_dirs(&root.join("specs"))? {
        let path = dir.join("spec.md");
        if !path.is_file() {
            continue;
        }
        let (spec, _) = parse_spec(&read(&path)?);
        let reqs = spec
            .requirements
            .into_iter()
            .filter_map(|r| {
                let crate::id::Ref::Id(id) = r.id else { return None };
                Some((
                    id,
                    OsReq {
                        title: r.title,
                        text: r.text,
                        scenarios: r
                            .scenarios
                            .into_iter()
                            .map(|s| OsScenario {
                                title: s.title,
                                steps: s.steps.into_iter().map(|step| format!("{} {}", step.kind.word(), step.text)).collect(),
                            })
                            .collect(),
                    },
                ))
            })
            .collect();
        caps.push(Cap { name: change::file_name(&dir)?, reqs });
    }
    Ok(caps)
}

pub(super) fn preflight(from: &Path, root: &Path) -> Result<()> {
    let caps = existing_caps(root)?;
    let mut conflicts = Vec::new();
    for dir in sorted_dirs(&from.join("specs"))? {
        let path = dir.join("spec.md");
        if !path.is_file() {
            continue;
        }
        let name = change::file_name(&dir)?;
        let mut titles: HashSet<String> =
            caps.iter().find(|c| c.name == name).into_iter().flat_map(|c| &c.reqs).map(|(_, r)| r.title.to_ascii_lowercase()).collect();
        for req in parse_openspec(&read(&path)?).group("Requirements") {
            if !titles.insert(req.title.to_ascii_lowercase()) {
                conflicts.push(format!("{name}: {}", req.title));
            }
        }
    }
    if !conflicts.is_empty() {
        return Err(Error::new(
            "import",
            format!("requirement title conflicts:\n{}", conflicts.join("\n")),
            "rename or remove every conflicting incoming requirement",
        ));
    }
    Ok(())
}

pub(super) fn validate_schema(from: &Path) -> Result<()> {
    let path = from.join("config.yaml");
    if !path.is_file() {
        return Ok(());
    }
    let src = read(&path)?;
    let mut schemas = 0;
    let mut block_indent = None;
    for line in src.lines() {
        let text = line.trim();
        if text.is_empty() || text.starts_with('#') {
            continue;
        }
        let indent = line.len() - line.trim_start().len();
        if block_indent.is_some_and(|parent| indent > parent) {
            continue;
        }
        block_indent = None;
        // Flow mappings, aliases and explicit keys need a YAML parser to establish the root schema.
        if text.starts_with(['{', '[', '&', '*', '?', '!']) || text.starts_with("<<:") || text.starts_with("- ") {
            return Err(Error::new(
                "import",
                format!("{} has unsupported schema syntax", path.display()),
                "use the default spec-driven OpenSpec schema",
            ));
        }
        let Some((key, value)) = text.split_once(':') else { continue };
        if key.contains('\\') {
            return Err(Error::new(
                "import",
                format!("{} has unsupported schema key syntax", path.display()),
                "use the default spec-driven OpenSpec schema",
            ));
        }
        let value = value.split(" #").next().unwrap_or("").trim();
        if value.starts_with(['|', '>']) {
            block_indent = Some(indent);
        }
        if key.trim().trim_matches(['\'', '"']) == "schema" {
            schemas += 1;
            let value = value.trim_matches(['\'', '"']);
            if value != "spec-driven" || schemas > 1 {
                return Err(Error::new(
                    "import",
                    format!("{} requires the default spec-driven schema", path.display()),
                    "use the default spec-driven OpenSpec schema",
                ));
            }
        }
    }
    Ok(())
}

pub(super) fn copy_history(from: &Path, to: &Path, files: &mut Vec<(PathBuf, String)>) -> Result<()> {
    let mut entries: Vec<_> = std::fs::read_dir(from)
        .map_err(|e| Error::io(from, e))?
        .collect::<std::result::Result<Vec<_>, _>>()
        .map_err(|e| Error::io(from, e))?;
    entries.sort_by_key(|entry| entry.file_name());
    for entry in entries {
        let path = entry.path();
        let target = to.join(entry.file_name());
        let kind = entry.file_type().map_err(|e| Error::io(&path, e))?;
        if kind.is_dir() {
            copy_history(&path, &target, files)?;
        } else if kind.is_file() && path.extension().is_some_and(|ext| ext == "md") {
            let text = read(&path)?;
            if target.exists() && read(&target)? != text {
                return Err(Error::new(
                    "import",
                    format!("{} conflicts with imported history", target.display()),
                    "keep the original history under a distinct archive name",
                ));
            }
            files.push((target, text));
        }
    }
    Ok(())
}
