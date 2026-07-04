//! JSON object-path diff adapter. Parses both sides and walks them in parallel,
//! reporting each leaf/branch that was added, removed, or changed by its dotted
//! path (e.g. `deps.serde`, `items[2].name`). Falls back (returns `None`) if
//! either side is not valid JSON, so the dispatcher can use the text adapter.

use super::super::adapters::{DiffEntry, DiffKind, FileChange, ObjChange};
use serde_json::Value;

/// Returns `None` if either input is not valid JSON.
pub fn diff(before: &str, after: &str) -> Option<FileChange> {
    let b: Value = serde_json::from_str(before).ok()?;
    let a: Value = serde_json::from_str(after).ok()?;

    let mut entries = Vec::new();
    walk("", &b, &a, &mut entries);

    // Formatting-only: the JSON is semantically identical but the text differs
    // (reordered keys, whitespace, indentation). No object-path entries, yet the
    // raw bytes changed.
    let formatting_only = entries.is_empty() && before != after;

    Some(FileChange {
        kind: DiffKind::Json,
        entries,
        formatting_only,
    })
}

/// Recursively compare `b` (before) and `a` (after) at dotted `path`.
fn walk(path: &str, b: &Value, a: &Value, out: &mut Vec<DiffEntry>) {
    match (b, a) {
        (Value::Object(bm), Value::Object(am)) => {
            // Keys in before but not after → removed; changed if present in both.
            for (k, bv) in bm {
                let child = join(path, k);
                match am.get(k) {
                    None => push(out, &child, ObjChange::Removed, Some(bv), None),
                    Some(av) if av != bv => walk(&child, bv, av, out),
                    Some(_) => {}
                }
            }
            // Keys only in after → added.
            for (k, av) in am {
                if !bm.contains_key(k) {
                    push(out, &join(path, k), ObjChange::Added, None, Some(av));
                }
            }
        }
        (Value::Array(ba), Value::Array(aa)) => {
            let n = ba.len().max(aa.len());
            for i in 0..n {
                let child = format!("{path}[{i}]");
                match (ba.get(i), aa.get(i)) {
                    (Some(bv), Some(av)) if av != bv => walk(&child, bv, av, out),
                    (Some(bv), None) => push(out, &child, ObjChange::Removed, Some(bv), None),
                    (None, Some(av)) => push(out, &child, ObjChange::Added, None, Some(av)),
                    _ => {}
                }
            }
        }
        // Scalars (or type mismatch) that differ → a leaf change.
        _ if b != a => push(out, path, ObjChange::Changed, Some(b), Some(a)),
        _ => {}
    }
}

fn push(out: &mut Vec<DiffEntry>, path: &str, change: ObjChange, before: Option<&Value>, after: Option<&Value>) {
    out.push(DiffEntry::ObjectPath {
        path: if path.is_empty() { "$".to_string() } else { path.to_string() },
        change,
        before: before.map(compact),
        after: after.map(compact),
    });
}

fn join(path: &str, key: &str) -> String {
    if path.is_empty() { key.to_string() } else { format!("{path}.{key}") }
}

fn compact(v: &Value) -> String {
    serde_json::to_string(v).unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn paths(c: &FileChange) -> Vec<(String, ObjChange)> {
        c.entries.iter().filter_map(|e| match e {
            DiffEntry::ObjectPath { path, change, .. } => Some((path.clone(), *change)),
            _ => None,
        }).collect()
    }

    #[test]
    fn detects_added_removed_changed_leaves() {
        let c = diff(
            r#"{"keep":1,"drop":2,"change":3}"#,
            r#"{"keep":1,"change":4,"add":5}"#,
        ).unwrap();
        let mut p = paths(&c);
        p.sort();
        assert_eq!(p, vec![
            ("add".into(), ObjChange::Added),
            ("change".into(), ObjChange::Changed),
            ("drop".into(), ObjChange::Removed),
        ]);
    }

    #[test]
    fn nested_and_array_paths() {
        let c = diff(
            r#"{"a":{"b":[1,2,3]}}"#,
            r#"{"a":{"b":[1,9,3,4]}}"#,
        ).unwrap();
        let p = paths(&c);
        assert!(p.iter().any(|(path, ch)| path == "a.b[1]" && *ch == ObjChange::Changed));
        assert!(p.iter().any(|(path, ch)| path == "a.b[3]" && *ch == ObjChange::Added));
    }

    #[test]
    fn reordered_keys_is_formatting_only() {
        let c = diff(r#"{"a":1,"b":2}"#, r#"{"b":2,"a":1}"#).unwrap();
        assert!(c.entries.is_empty());
        assert!(c.formatting_only, "same object, different key order → noise");
    }

    #[test]
    fn invalid_json_returns_none() {
        assert!(diff("nope", "{}").is_none());
    }
}
