use std::path::Path;

use clap::Command;
use rvow_core::error::{Error, Result};

use crate::output::Reply;

/// Writes the command reference for `cli` under `out/commands`: an index page
/// and one page per visible command, nested commands as `<parent>-<child>.md`.
/// The pages come from the clap definitions, so they follow the CLI.
pub fn reference(mut cli: Command, out: &Path) -> Result<Reply> {
    cli.build();
    let dir = out.join("commands");
    if dir.exists() {
        std::fs::remove_dir_all(&dir).map_err(|e| Error::io(&dir, e))?;
    }
    std::fs::create_dir_all(&dir).map_err(|e| Error::io(&dir, e))?;
    let mut rows = Vec::new();
    let mut written = 0;
    for sub in visible(&cli) {
        written += write_pages(&dir, sub, &[], &mut rows)?;
    }
    let mut index =
        String::from("# Commands\n\nEvery rvow command. Each page comes from `rvow docs`, so it matches `rvow <command> --help`.\n\n");
    index.push_str("| Command | What it does |\n| --- | --- |\n");
    for (name, file, about) in &rows {
        index.push_str(&format!("| [`{name}`]({file}) | {} |\n", cell(about)));
    }
    let path = dir.join("index.md");
    std::fs::write(&path, index).map_err(|e| Error::io(&path, e))?;
    let mut r = Reply::new();
    r.line(format!("wrote {} command pages and the index under {}", written, dir.display()));
    r.set("pages", written.to_string());
    Ok(r)
}

fn visible(cmd: &Command) -> impl Iterator<Item = &Command> {
    cmd.get_subcommands().filter(|c| !c.is_hide_set() && c.get_name() != "help")
}

/// Writes the page of `cmd` and of its visible subcommands; returns how many pages it wrote.
fn write_pages(dir: &Path, cmd: &Command, parents: &[&str], rows: &mut Vec<(String, String, String)>) -> Result<usize> {
    let mut names: Vec<&str> = parents.to_vec();
    names.push(cmd.get_name());
    let file = format!("{}.md", names.join("-"));
    let about = cmd.get_about().map(ToString::to_string).unwrap_or_default();
    rows.push((names.join(" "), file.clone(), about));
    let path = dir.join(&file);
    std::fs::write(&path, page(cmd, &names)).map_err(|e| Error::io(&path, e))?;
    let mut written = 1;
    for sub in visible(cmd) {
        written += write_pages(dir, sub, &names, rows)?;
    }
    Ok(written)
}

fn page(cmd: &Command, names: &[&str]) -> String {
    let mut out = format!("# rvow {}\n\n", names.join(" "));
    if let Some(about) = cmd.get_long_about().or_else(|| cmd.get_about()) {
        out.push_str(&format!("{}\n\n", text(&about.to_string())));
    }
    let usage = cmd.clone().render_usage().to_string();
    let usage = usage.trim_start_matches("Usage: ").trim();
    out.push_str(&format!("## Usage\n\n```sh\n{usage}\n```\n"));
    // `--help` is on every page of clap's own help, so it says nothing here.
    let args: Vec<_> = cmd.get_arguments().filter(|a| !a.is_hide_set() && !a.is_global_set() && a.get_id() != "help").collect();
    let positional: Vec<_> = args.iter().filter(|a| a.is_positional()).collect();
    if !positional.is_empty() {
        out.push_str("\n## Arguments\n\n| Argument | Description |\n| --- | --- |\n");
        for a in positional {
            let name = a.get_value_names().and_then(|v| v.first()).map_or_else(|| a.get_id().to_string(), ToString::to_string);
            out.push_str(&format!("| `<{name}>` | {} |\n", cell(&help(a))));
        }
    }
    let options: Vec<_> = args.iter().filter(|a| !a.is_positional()).collect();
    if !options.is_empty() {
        out.push_str("\n## Options\n\n| Option | Description |\n| --- | --- |\n");
        for a in options {
            let mut flag = String::new();
            if let Some(s) = a.get_short() {
                flag.push_str(&format!("`-{s}`, "));
            }
            if let Some(l) = a.get_long() {
                flag.push_str(&format!("`--{l}`"));
            }
            if a.get_action().takes_values()
                && let Some(v) = a.get_value_names().and_then(|v| v.first())
            {
                flag.push_str(&format!(" `<{v}>`"));
            }
            out.push_str(&format!("| {} | {} |\n", flag.trim_end_matches(", "), cell(&help(a))));
        }
    }
    let subs: Vec<_> = visible(cmd).collect();
    if !subs.is_empty() {
        out.push_str("\n## Commands\n\n| Command | What it does |\n| --- | --- |\n");
        for s in subs {
            let file = format!("{}-{}.md", names.join("-"), s.get_name());
            let about = s.get_about().map(ToString::to_string).unwrap_or_default();
            out.push_str(&format!("| [`{} {}`]({file}) | {} |\n", names.join(" "), s.get_name(), cell(&about)));
        }
    }
    out
}

fn help(arg: &clap::Arg) -> String {
    let mut text = arg.get_long_help().or_else(|| arg.get_help()).map(ToString::to_string).unwrap_or_default();
    let values: Vec<String> =
        arg.get_possible_values().iter().filter(|v| !v.is_hide_set()).map(|v| format!("`{}`", v.get_name())).collect();
    if !values.is_empty() {
        text.push_str(&format!(" One of {}.", values.join(", ")));
    }
    text
}

/// `s` with `<` and `>` outside backticks written as entities, so a site renders `<id>` as text.
fn text(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut code = false;
    for c in s.chars() {
        match c {
            '`' => {
                code = !code;
                out.push(c);
            }
            '<' if !code => out.push_str("&lt;"),
            '>' if !code => out.push_str("&gt;"),
            _ => out.push(c),
        }
    }
    out
}

/// `s` fit for one table cell: one line, with `|` escaped.
fn cell(s: &str) -> String {
    text(&s.split_whitespace().collect::<Vec<_>>().join(" ")).replace('|', "\\|")
}
