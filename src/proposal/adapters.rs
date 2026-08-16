//! Document-aware diff adapters (M16).
//!
//! The snapshot layer (`diff.rs`) only says *which paths* changed. This layer
//! answers *how* a modified file changed, in a form meaningful for review —
//! without requiring the agent that made the change to produce a structured
//! patch. Given `(path, before, after)`, [`diff_file`] picks an adapter by file
//! type and content and returns a [`FileChange`].
//!
//! Adapters, in dispatch-preference order:
//!
//! | Adapter | Selected for | Produces |
//! |---------|--------------|----------|
//! | binary  | non-UTF-8 content | hash-only summary |
//! | json    | `.json` that parses | object-path add/remove/change |
//! | markdown| `.md`/`.markdown` | per-heading-section changes |
//! | code    | source extensions | function/class-level changed spans |
//! | text    | everything else | unified line hunks |
//!
//! Every adapter also reports whether the change is **formatter noise** — a
//! diff whose only difference is whitespace/formatting — so review UIs can
//! de-emphasize it.

use serde::Serialize;

mod binary;
mod code;
mod json;
mod markdown;
mod text;

pub use text::LineHunk;

/// The kind of adapter that produced a change (for the UI to render).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum DiffKind {
    Text,
    Markdown,
    Json,
    Code,
    Binary,
}

/// A structured entry within a document-aware diff. The shape depends on the
/// adapter; the UI switches on `kind`.
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum DiffEntry {
    /// A unified line hunk (text/markdown/code fall back to this for bodies).
    LineHunk(LineHunk),
    /// A JSON/object path changed. `path` is dotted (e.g. `a.b[0].c`).
    ObjectPath {
        path: String,
        change: ObjChange,
        before: Option<String>,
        after: Option<String>,
    },
    /// A named section changed (markdown heading, code function/class).
    Section { name: String, change: SectionChange },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ObjChange {
    Added,
    Removed,
    Changed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SectionChange {
    Added,
    Removed,
    Modified,
}

/// The document-aware diff of one modified file.
#[derive(Debug, Clone, Serialize)]
pub struct FileChange {
    pub kind: DiffKind,
    pub entries: Vec<DiffEntry>,
    /// True when the change is only whitespace/formatting (noise). Review UIs
    /// can collapse these.
    pub formatting_only: bool,
}

/// Choose an adapter for `path` + content and produce the document-aware diff.
/// `before`/`after` are the raw file bytes.
pub fn diff_file(path: &str, before: &[u8], after: &[u8]) -> FileChange {
    // Binary first: anything not valid UTF-8 gets the hash-only treatment.
    let (Ok(before_s), Ok(after_s)) = (std::str::from_utf8(before), std::str::from_utf8(after))
    else {
        return binary::diff(before, after);
    };

    let ext = path.rsplit('.').next().unwrap_or("").to_ascii_lowercase();
    match ext.as_str() {
        "json" => json::diff(before_s, after_s).unwrap_or_else(|| text::diff(before_s, after_s)),
        "md" | "markdown" => markdown::diff(before_s, after_s),
        // Common source extensions get the code adapter.
        "rs" | "ts" | "tsx" | "js" | "jsx" | "py" | "go" | "java" | "c" | "cc" | "cpp" | "h"
        | "hpp" | "rb" | "cs" | "swift" | "kt" | "php" => code::diff(&ext, before_s, after_s),
        _ => text::diff(before_s, after_s),
    }
}

/// Whether two texts differ only in whitespace (used by several adapters for the
/// `formatting_only` flag). Collapses all runs of whitespace and compares.
pub(crate) fn whitespace_normalized_eq(a: &str, b: &str) -> bool {
    fn norm(s: &str) -> String {
        s.split_whitespace().collect::<Vec<_>>().join(" ")
    }
    a != b && norm(a) == norm(b)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dispatches_by_extension() {
        assert_eq!(
            diff_file("a.json", b"{}", br#"{"x":1}"#).kind,
            DiffKind::Json
        );
        assert_eq!(diff_file("a.md", b"# a", b"# b").kind, DiffKind::Markdown);
        assert_eq!(
            diff_file("a.rs", b"fn a(){}", b"fn a(){1}").kind,
            DiffKind::Code
        );
        assert_eq!(diff_file("a.txt", b"x", b"y").kind, DiffKind::Text);
    }

    #[test]
    fn non_utf8_is_binary() {
        assert_eq!(
            diff_file("a.bin", &[0xff, 0xfe], &[0x00]).kind,
            DiffKind::Binary
        );
    }

    #[test]
    fn invalid_json_falls_back_to_text() {
        // `.json` that doesn't parse should still produce a text diff, not panic.
        assert_eq!(
            diff_file("a.json", b"not json", b"also not").kind,
            DiffKind::Text
        );
    }

    #[test]
    fn whitespace_normalized_eq_detects_formatting() {
        assert!(whitespace_normalized_eq("a  b", "a b"));
        assert!(whitespace_normalized_eq("a\nb", "a b"));
        assert!(!whitespace_normalized_eq("a b", "a b")); // identical -> false
        assert!(!whitespace_normalized_eq("a b", "a c")); // real change
    }
}
