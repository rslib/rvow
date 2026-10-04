#[derive(Debug)]
struct Token<'a> {
    text: &'a str,
    start: usize,
    end: usize,
}

fn tokens(src: &str) -> Option<Vec<Token<'_>>> {
    let b = src.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    while i < b.len() {
        if b[i].is_ascii_whitespace() {
            i += 1;
            continue;
        }
        if b[i..].starts_with(b"//") {
            while i < b.len() && b[i] != b'\n' {
                i += 1;
            }
            continue;
        }
        if b[i..].starts_with(b"/*") {
            i += 2;
            let mut depth = 1;
            while i < b.len() && depth > 0 {
                if b[i..].starts_with(b"/*") {
                    depth += 1;
                    i += 2;
                } else if b[i..].starts_with(b"*/") {
                    depth -= 1;
                    i += 2;
                } else {
                    i += 1;
                }
            }
            if depth != 0 {
                return None;
            }
            continue;
        }
        let start = i;
        let raw = if b[i..].starts_with(b"br") || b[i..].starts_with(b"cr") {
            i + 2
        } else if b[i] == b'r' {
            i + 1
        } else {
            i
        };
        let mut quote = raw;
        while quote < b.len() && b[quote] == b'#' {
            quote += 1;
        }
        if raw > i && quote < b.len() && b[quote] == b'"' {
            let hashes = quote - raw;
            i = quote + 1;
            loop {
                if i >= b.len() {
                    return None;
                }
                if b[i] == b'"' && b.get(i + 1..i + 1 + hashes).is_some_and(|s| s.iter().all(|x| *x == b'#')) {
                    i += 1 + hashes;
                    break;
                }
                i += 1;
            }
        } else {
            let quote = if b[i..].starts_with(b"b\"") || b[i..].starts_with(b"c\"") || b[i..].starts_with(b"b'") { i + 1 } else { i };
            let character = b[quote] == b'\''
                && (b.get(quote + 1) == Some(&b'\\')
                    || src
                        .get(quote + 1..)
                        .and_then(|s| s.chars().next())
                        .is_some_and(|c| b.get(quote + 1 + c.len_utf8()) == Some(&b'\'')));
            if b[quote] == b'"' || character {
                let delimiter = b[quote];
                i = quote + 1;
                loop {
                    if i >= b.len() {
                        return None;
                    }
                    if b[i] == b'\\' {
                        i += 2;
                    } else if b[i] == delimiter {
                        i += 1;
                        break;
                    } else {
                        i += 1;
                    }
                }
            } else if b[i].is_ascii_alphanumeric() || b[i] == b'_' || b[i] >= 128 {
                i += 1;
                while i < b.len() && (b[i].is_ascii_alphanumeric() || b[i] == b'_' || b[i] >= 128) {
                    i += 1;
                }
            } else {
                i += 1;
            }
        }
        out.push(Token { text: src.get(start..i)?, start, end: i });
    }
    Some(out)
}

fn close(ts: &[Token<'_>], start: usize) -> Option<usize> {
    let mut stack = Vec::new();
    for (i, t) in ts.iter().enumerate().skip(start) {
        match t.text {
            "{" => stack.push("}"),
            "[" => stack.push("]"),
            "(" => stack.push(")"),
            "}" | "]" | ")" => {
                if stack.pop()? != t.text {
                    return None;
                }
                if stack.is_empty() {
                    return Some(i);
                }
            }
            _ => {}
        }
    }
    None
}

fn ranges(ts: &[Token<'_>]) -> Option<Vec<(usize, usize)>> {
    let mut out = Vec::new();
    let mut i = 0;
    while i < ts.len() {
        if ts[i].text == "#" && ts.get(i + 1).is_some_and(|t| t.text == "[") {
            let start = i;
            let mut attr = i;
            let mut test = false;
            loop {
                let end = close(ts, attr + 1)?;
                test |= ts[attr..=end].iter().map(|t| t.text).eq(["#", "[", "cfg", "(", "test", ")", "]"]);
                i = end + 1;
                if ts.get(i).is_some_and(|t| t.text == "#") && ts.get(i + 1).is_some_and(|t| t.text == "[") {
                    attr = i;
                } else {
                    break;
                }
            }
            if test {
                // A #[cfg(test)] that prefixes an inline test module (mod name
                // with a body) is what we guard. A #[cfg(test)] on a `use`, a
                // `fn`, a field or a `mod name;` submodule include is not a
                // guardable test module, so skip past the attribute and keep
                // scanning instead of treating it as a lexer failure.
                let mut t = i;
                while ts.get(t).is_some_and(|x| x.text == "pub") {
                    if ts.get(t + 1).is_some_and(|x| x.text == "(") {
                        t = close(ts, t + 1)? + 1;
                    } else {
                        t += 1;
                    }
                }
                if !ts.get(t).is_some_and(|x| x.text == "mod") || !ts.get(t + 2).is_some_and(|x| x.text == "{") {
                    continue;
                }
                let open = t + 2;
                let end = close(ts, open)?;
                // Include enclosing attributes and headers so disabling a parent is drift.
                for parent in (0..start).rev() {
                    if ts[parent].text == "{" && close(ts, parent).is_some_and(|e| e > end) {
                        return Some(vec![(0, ts.len() - 1)]);
                    }
                }
                out.push((start, end));
                i = end + 1;
            }
        } else {
            i += 1;
        }
    }
    Some(out)
}

pub(super) fn modules(src: &str) -> Option<String> {
    let ts = tokens(src)?;
    let mut out = String::new();
    let mut i = 0;
    while ts.get(i..i + 3).is_some_and(|part| part.iter().map(|t| t.text).eq(["#", "!", "["])) {
        let end = close(&ts, i + 2)?;
        out.push_str(&src[ts[i].start..ts[end].end]);
        out.push('\n');
        i = end + 1;
    }
    for (start, end) in ranges(&ts)? {
        let start = src[..ts[start].start].rfind('\n').map_or(0, |p| p + 1);
        let end = src[ts[end].end..].find('\n').map_or(src.len(), |p| ts[end].end + p + 1);
        out.push_str(&src[start..end]);
        if !out.ends_with('\n') {
            out.push('\n');
        }
    }
    Some(out)
}

/// The names of `mod <name>;` submodule includes behind a `#[cfg(test)]`
/// attribute. These are not inline test modules; they pull a separate test file
/// into the crate under test. The caller resolves each name to a file and
/// guards its content the same way it guards a test file.
pub(super) fn test_submodules(src: &str) -> Vec<String> {
    let Some(ts) = tokens(src) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    let mut i = 0;
    while i < ts.len() {
        let mut attr = i;
        let mut test = false;
        if !(ts[i].text == "#" && ts.get(i + 1).is_some_and(|t| t.text == "[")) {
            i += 1;
            continue;
        }
        let mut head;
        loop {
            let Some(end) = close(&ts, attr + 1) else {
                return out;
            };
            test |= ts[attr..=end].iter().map(|t| t.text).eq(["#", "[", "cfg", "(", "test", ")", "]"]);
            head = end + 1;
            if ts.get(head).is_some_and(|t| t.text == "#") && ts.get(head + 1).is_some_and(|t| t.text == "[") {
                attr = head;
            } else {
                break;
            }
        }
        if !test {
            i = head;
            continue;
        }
        // A bodyless `mod name;` after the attribute is a submodule include.
        let t = head;
        if ts.get(t).is_some_and(|x| x.text == "mod")
            && ts.get(t + 1).is_some_and(|x| !matches!(x.text, "{" | ";"))
            && ts.get(t + 2).is_some_and(|x| x.text == ";")
        {
            out.push(ts[t + 1].text.to_string());
            i = t + 3;
        } else {
            i = head;
        }
    }
    out
}

fn items<'a>(ts: &'a [Token<'a>]) -> Option<Vec<Vec<&'a str>>> {
    let mut out = Vec::new();
    let mut start = 0;
    let mut i = 0;
    while i < ts.len() {
        match ts[i].text {
            "[" | "(" => i = close(ts, i)? + 1,
            "{" => {
                i = close(ts, i)? + 1;
                if ts.get(i).is_some_and(|t| t.text == ";") {
                    i += 1;
                }
                out.push(ts[start..i].iter().map(|t| t.text).collect());
                start = i;
            }
            ";" => {
                i += 1;
                out.push(ts[start..i].iter().map(|t| t.text).collect());
                start = i;
            }
            _ => i += 1,
        }
    }
    (start == ts.len()).then_some(out)
}

fn added_test(item: &[&str]) -> bool {
    let mut i = 0;
    let mut test = false;
    while item.get(i..i + 2) == Some(&["#", "["]) {
        let Some(end) = item[i + 2..].iter().position(|t| *t == "]") else {
            return false;
        };
        test |= item.get(i..i + 4) == Some(&["#", "[", "test", "]"]);
        i += end + 3;
    }
    test && item.get(i) == Some(&"fn")
}

pub(super) fn preserved(before: &str, now: &str) -> bool {
    let (Some(old), Some(new)) = (tokens(before), tokens(now)) else {
        return false;
    };
    let (Some(old_ranges), Some(new_ranges)) = (ranges(&old), ranges(&new)) else {
        return false;
    };
    if !old_ranges.is_empty() {
        let old_prefix = &old[..old_ranges[0].0];
        let Some(&(new_start, _)) = new_ranges.first() else {
            return false;
        };
        if !old_prefix.iter().map(|t| t.text).eq(new[..new_start].iter().map(|t| t.text)) {
            return false;
        }
    }
    if old_ranges.is_empty() {
        let (Some(old_items), Some(new_items)) = (items(&old), items(&new)) else {
            return false;
        };
        let mut rest = new_items.iter();
        return old_items.iter().all(|item| {
            rest.by_ref().find(|candidate| *candidate == item || !added_test(candidate)).is_some_and(|candidate| candidate == item)
        }) && rest.all(|item| added_test(item));
    }
    let mut remaining = new_ranges.iter();
    old_ranges.iter().all(|&(start, end)| {
        let Some(open) = (start..end).find(|&i| old[i].text == "{") else {
            return false;
        };
        let header: Vec<_> = old[start..=open].iter().map(|t| t.text).collect();
        remaining.any(|&(ns, ne)| {
            let Some(no) = (ns..ne).find(|&i| new[i].text == "{") else {
                return false;
            };
            if !new[ns..=no].iter().map(|t| t.text).eq(header.iter().copied()) {
                return false;
            }
            let (Some(old_items), Some(new_items)) = (items(&old[open + 1..end]), items(&new[no + 1..ne])) else {
                return false;
            };
            let mut rest = new_items.iter();
            old_items.iter().all(|item| {
                rest.by_ref().find(|candidate| *candidate == item || !added_test(candidate)).is_some_and(|candidate| candidate == item)
            }) && rest.all(|item| added_test(item))
        })
    })
}
