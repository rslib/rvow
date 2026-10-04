//! Source checks for unfinished Rust implementations.

#[cfg(test)]
mod tests {
    /// The repository root, from the rvow crate's manifest directory.
    fn repo_root() -> std::path::PathBuf {
        std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../..")
            .canonicalize()
            .unwrap_or_else(|_| std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../.."))
    }
    /// A scan for unfinished stubs in the Rust crates: none may be left.
    #[test]
    fn w18_no_todo_or_unimplemented_stubs_in_crates() {
        let root = repo_root();
        let todo = concat!("todo", "!");
        let unimplemented = concat!("unimplemented", "!");
        let mut bad: Vec<String> = Vec::new();
        for path in rust_files(&root.join("crates")) {
            let Ok(text) = std::fs::read_to_string(&path) else { continue };
            // Ignore this file itself, whose needles are built from fragments.
            let is_self = path.ends_with("src/w18.rs");
            for (i, line) in text.lines().enumerate() {
                let t = line.trim();
                if !is_self && (t.contains(todo) || t.contains(unimplemented)) {
                    let rel = path.strip_prefix(&root).unwrap_or(&path);
                    bad.push(format!("{}:{}: {}", rel.display(), i + 1, t));
                }
            }
        }
        assert!(bad.is_empty(), "crates contain unfinished stubs:\n{}", bad.join("\n"));
    }

    fn rust_files(dir: &std::path::Path) -> Vec<std::path::PathBuf> {
        let mut out = Vec::new();
        let mut stack = vec![dir.to_path_buf()];
        while let Some(d) = stack.pop() {
            let Ok(entries) = std::fs::read_dir(&d) else { continue };
            for e in entries.flatten() {
                let p = e.path();
                if p.is_dir() {
                    let name = p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
                    if !["target", "node_modules", ".git"].contains(&name.as_str()) {
                        stack.push(p);
                    }
                } else if p.extension().is_some_and(|x| x == "rs" || x == "tsx") {
                    out.push(p);
                }
            }
        }
        out.sort();
        out
    }
}
