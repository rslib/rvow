use crate::id::Id;

const STEPS: [&str; 4] = ["GIVEN", "WHEN", "THEN", "AND"];

#[derive(Clone)]
pub(super) struct OsScenario {
    pub(super) title: String,
    pub(super) steps: Vec<String>,
}

#[derive(Clone)]
pub(super) struct OsReq {
    pub(super) title: String,
    pub(super) text: String,
    pub(super) scenarios: Vec<OsScenario>,
}

#[derive(Default)]
pub(super) struct OsSpec {
    pub(super) purpose: String,
    /// Requirements by the name of their `##` group.
    pub(super) groups: Vec<(String, Vec<OsReq>)>,
    /// RENAMED pairs, FROM then TO.
    pub(super) renames: Vec<(String, String)>,
}

impl OsSpec {
    pub(super) fn group(&self, name: &str) -> &[OsReq] {
        self.groups.iter().find(|(g, _)| g == name).map_or(&[], |(_, r)| r)
    }
}

pub(super) fn step(line: &str) -> Option<String> {
    let rest = line.trim_start().strip_prefix("- **")?;
    let (word, text) = rest.split_once("**")?;
    STEPS.contains(&word).then(|| format!("{word} {}", text.trim()))
}

pub(super) fn requirement_title(s: &str) -> Option<&str> {
    let s = s.trim().trim_matches('`');
    Some(s.strip_prefix("### Requirement:")?.trim())
}

pub(super) fn parse_openspec(src: &str) -> OsSpec {
    let mut out = OsSpec::default();
    let mut group = String::new();
    let mut fenced = false;
    for line in src.lines() {
        if line.trim_start().starts_with("```") {
            fenced = !fenced;
        }
        if !fenced && let Some(name) = line.strip_prefix("## ") {
            group = name.trim().to_string();
            if group.ends_with("Requirements") {
                out.groups.push((group.clone(), Vec::new()));
            }
            continue;
        }
        if group == "Purpose" {
            out.purpose.push_str(line);
            out.purpose.push('\n');
        } else if group == "RENAMED Requirements" {
            let item = line.trim().trim_start_matches("- ");
            if let Some(from) = item.strip_prefix("FROM:") {
                out.renames.push((requirement_title(from).unwrap_or("").to_string(), String::new()));
            } else if let Some(to) = item.strip_prefix("TO:")
                && let Some(last) = out.renames.last_mut()
            {
                last.1 = requirement_title(to).unwrap_or("").to_string();
            }
        } else if let Some((_, reqs)) = out.groups.last_mut().filter(|_| !group.is_empty()) {
            if !fenced && let Some(title) = line.strip_prefix("### Requirement:") {
                reqs.push(OsReq { title: title.trim().to_string(), text: String::new(), scenarios: Vec::new() });
            } else if let Some(req) = reqs.last_mut() {
                if !fenced && let Some(title) = line.strip_prefix("#### Scenario:") {
                    req.scenarios.push(OsScenario { title: title.trim().to_string(), steps: Vec::new() });
                } else if let Some(sc) = req.scenarios.last_mut() {
                    match (step(line), sc.steps.last_mut()) {
                        (Some(s), _) => sc.steps.push(s),
                        (None, Some(prev)) if !line.trim().is_empty() => {
                            prev.push(' ');
                            prev.push_str(line.trim());
                        }
                        _ => {}
                    }
                } else if !line.trim().is_empty() && !line.trim_start().starts_with("<!--") && line.trim() != "(deprecated)" {
                    if !req.text.is_empty() {
                        req.text.push(' ');
                    }
                    req.text.push_str(line.trim());
                }
            }
        }
    }
    out.purpose = out.purpose.trim().to_string();
    out
}

pub(super) fn render_req(heading: &str, req: &OsReq) -> String {
    let mut s = format!("## {heading}\n\n{}\n", req.text);
    for sc in &req.scenarios {
        s.push_str(&format!("\n### Scenario: {}\n", sc.title));
        for st in &sc.steps {
            s.push_str(&format!("- {st}\n"));
        }
    }
    s
}

pub(super) fn find_title<'a>(reqs: &'a [(Id, OsReq)], title: &str) -> Option<&'a (Id, OsReq)> {
    reqs.iter().find(|(_, r)| r.title == title).or_else(|| reqs.iter().find(|(_, r)| r.title.eq_ignore_ascii_case(title)))
}
