//! Markdown heading/section diff adapter. Splits each document into sections by
//! ATX heading (`#`…`######`), then reports which sections were added, removed,
//! or modified — a more reviewable view than raw line noise for prose docs. The
//! body of a modified section is still line-diffed underneath.

use super::super::adapters::{DiffEntry, DiffKind, FileChange, SectionChange};
use std::collections::BTreeMap;

pub fn diff(before: &str, after: &str) -> FileChange {
    let bs = sections(before);
    let as_ = sections(after);

    let mut entries = Vec::new();
    // Removed / modified (present in before).
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
                // Line-level detail for the modified section body.
                for hunk in super::text::line_hunks(bbody, abody) {
                    entries.push(DiffEntry::LineHunk(hunk));
                }
            }
            Some(_) => {}
        }
    }
    // Added (only in after).
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
        kind: DiffKind::Markdown,
        entries,
        formatting_only,
    }
}

/// Split into `heading text → section body`. Content before the first heading is
/// keyed as `(preamble)`. Duplicate headings get a disambiguating suffix.
fn sections(md: &str) -> BTreeMap<String, String> {
    let mut out: BTreeMap<String, String> = BTreeMap::new();
    let mut current = "(preamble)".to_string();
    let mut body = String::new();
    let mut seen: BTreeMap<String, usize> = BTreeMap::new();

    let flush = |name: &str,
                 body: &mut String,
                 seen: &mut BTreeMap<String, usize>,
                 out: &mut BTreeMap<String, String>| {
        if body.is_empty() && name == "(preamble)" {
            return;
        }
        let count = seen.entry(name.to_string()).or_insert(0);
        let key = if *count == 0 {
            name.to_string()
        } else {
            format!("{name} ({})", *count + 1)
        };
        *count += 1;
        out.insert(key, std::mem::take(body));
    };

    for line in md.lines() {
        if let Some(h) = heading(line) {
            flush(&current, &mut body, &mut seen, &mut out);
            current = h;
        } else {
            body.push_str(line);
            body.push('\n');
        }
    }
    flush(&current, &mut body, &mut seen, &mut out);
    out
}

/// The heading text if `line` is an ATX heading, else `None`.
fn heading(line: &str) -> Option<String> {
    let t = line.trim_start();
    if !t.starts_with('#') {
        return None;
    }
    let hashes = t.chars().take_while(|c| *c == '#').count();
    if hashes == 0 || hashes > 6 {
        return None;
    }
    let rest = t[hashes..].trim();
    // Require a space after the hashes (ATX rule) — avoids matching `#tag`.
    if !t[hashes..].starts_with(' ') && !rest.is_empty() {
        return None;
    }
    Some(rest.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn section_changes(c: &FileChange) -> Vec<(String, SectionChange)> {
        c.entries
            .iter()
            .filter_map(|e| match e {
                DiffEntry::Section { name, change } => Some((name.clone(), *change)),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn detects_section_add_remove_modify() {
        let before = "# Intro\nhello\n## Old\ngone\n## Keep\nsame\n";
        let after = "# Intro\nHELLO\n## Keep\nsame\n## New\nfresh\n";
        let c = diff(before, after);
        let ch = section_changes(&c);
        assert!(ch.contains(&("Intro".into(), SectionChange::Modified)));
        assert!(ch.contains(&("Old".into(), SectionChange::Removed)));
        assert!(ch.contains(&("New".into(), SectionChange::Added)));
        assert!(!ch.iter().any(|(n, _)| n == "Keep")); // unchanged section absent
    }

    #[test]
    fn heading_detection() {
        assert_eq!(heading("## Title").as_deref(), Some("Title"));
        assert_eq!(heading("text"), None);
        assert_eq!(heading("#notaheading"), None); // no space
        assert_eq!(heading("####### too many"), None);
    }

    #[test]
    fn identical_markdown_no_entries() {
        let c = diff("# A\nx\n", "# A\nx\n");
        assert!(c.entries.is_empty());
    }
}
