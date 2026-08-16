//! Code-aware diff adapter. Without embedding a parser per language, it uses a
//! lightweight, language-aware heuristic: recognize top-level definition lines
//! (functions, classes, structs, etc.) as section boundaries, then report which
//! definitions were added / removed / had their body modified. Modified bodies
//! are line-diffed underneath.
//!
//! This is intentionally approximate — it groups changes by the nearest
//! preceding definition, which is far more reviewable than a flat line diff for
//! source files, while staying robust to any language via the fallback.

use super::super::adapters::{DiffEntry, DiffKind, FileChange, SectionChange};
use std::collections::BTreeMap;

pub fn diff(ext: &str, before: &str, after: &str) -> FileChange {
    let bs = sections(ext, before);
    let as_ = sections(ext, after);

    let mut entries = Vec::new();
    for (name, bbody) in &bs {
        match as_.get(name) {
            None => entries.push(DiffEntry::Section {
                name: name.clone(),
                change: SectionChange::Removed,
            }),
            Some(abody) if abody != bbody => {
                entries.push(DiffEntry::Section {
                    name: name.clone(),
                    change: SectionChange::Modified,
                });
                for hunk in super::text::line_hunks(bbody, abody) {
                    entries.push(DiffEntry::LineHunk(hunk));
                }
            }
            Some(_) => {}
        }
    }
    for name in as_.keys() {
        if !bs.contains_key(name) {
            entries.push(DiffEntry::Section {
                name: name.clone(),
                change: SectionChange::Added,
            });
        }
    }

    let formatting_only =
        entries.is_empty() && super::super::adapters::whitespace_normalized_eq(before, after);

    FileChange {
        kind: DiffKind::Code,
        entries,
        formatting_only,
    }
}

/// Split source into `definition name → body`, grouping lines under the nearest
/// preceding recognized definition. Lines before the first definition go under
/// `(top-level)`.
fn sections(ext: &str, src: &str) -> BTreeMap<String, String> {
    let mut out: BTreeMap<String, String> = BTreeMap::new();
    let mut current = "(top-level)".to_string();
    let mut body = String::new();
    let mut seen: BTreeMap<String, usize> = BTreeMap::new();

    for line in src.lines() {
        if let Some(name) = definition_name(ext, line) {
            if !(current == "(top-level)" && body.trim().is_empty()) {
                insert(&mut out, &mut seen, &current, std::mem::take(&mut body));
            } else {
                body.clear();
            }
            current = name;
        }
        body.push_str(line);
        body.push('\n');
    }
    insert(&mut out, &mut seen, &current, body);
    out
}

fn insert(
    out: &mut BTreeMap<String, String>,
    seen: &mut BTreeMap<String, usize>,
    name: &str,
    body: String,
) {
    if body.trim().is_empty() {
        return;
    }
    let count = seen.entry(name.to_string()).or_insert(0);
    let key = if *count == 0 {
        name.to_string()
    } else {
        format!("{name} #{}", *count + 1)
    };
    *count += 1;
    out.insert(key, body);
}

/// Recognize a top-level definition and return a label. Heuristic keyword match
/// at (roughly) the start of a line — deliberately simple and language-family
/// based rather than a real parser.
fn definition_name(ext: &str, line: &str) -> Option<String> {
    let t = line.trim_start();
    // Keywords that introduce a named definition across the supported languages.
    const KWS: &[&str] = &[
        "fn ",
        "pub fn ",
        "async fn ",
        "pub async fn ",
        "struct ",
        "enum ",
        "trait ",
        "impl ",
        "class ",
        "def ",
        "func ",
        "function ",
        "interface ",
        "type ",
    ];
    for kw in KWS {
        if let Some(rest) = t.strip_prefix(kw) {
            let name: String = rest
                .chars()
                .take_while(|c| c.is_alphanumeric() || *c == '_' || *c == '<' || *c == ':')
                .collect();
            if !name.is_empty() {
                let kind = kw.trim().split(' ').next_back().unwrap_or(kw.trim());
                return Some(format!("{kind} {name}"));
            }
        }
    }
    // Language-specific: JS/TS `const foo = (…) =>` / method shorthand is common
    // but noisy to detect reliably; the keyword set above covers the primary
    // cases. `ext` is reserved for future per-language refinement.
    let _ = ext;
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn changes(c: &FileChange) -> Vec<(String, SectionChange)> {
        c.entries
            .iter()
            .filter_map(|e| match e {
                DiffEntry::Section { name, change } => Some((name.clone(), *change)),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn groups_change_by_function() {
        let before = "fn a() {\n  1\n}\nfn b() {\n  2\n}\n";
        let after = "fn a() {\n  1\n}\nfn b() {\n  99\n}\n";
        let c = diff("rs", before, after);
        let ch = changes(&c);
        // Only b changed.
        assert!(ch.contains(&("fn b".into(), SectionChange::Modified)));
        assert!(!ch.iter().any(|(n, _)| n == "fn a"));
    }

    #[test]
    fn detects_added_and_removed_defs() {
        let before = "fn keep() {}\nfn gone() {}\n";
        let after = "fn keep() {}\nfn added() {}\n";
        let ch = changes(&diff("rs", before, after));
        assert!(ch.contains(&("fn gone".into(), SectionChange::Removed)));
        assert!(ch.contains(&("fn added".into(), SectionChange::Added)));
    }

    #[test]
    fn recognizes_multiple_languages() {
        assert_eq!(
            definition_name("py", "def foo():").as_deref(),
            Some("def foo")
        );
        assert_eq!(
            definition_name("go", "func Bar() {").as_deref(),
            Some("func Bar")
        );
        assert_eq!(
            definition_name("java", "class Baz {").as_deref(),
            Some("class Baz")
        );
        assert_eq!(definition_name("rs", "    let x = 1;"), None);
    }
}
