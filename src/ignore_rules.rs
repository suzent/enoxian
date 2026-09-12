//! Which paths a Circle refuses to track.
//!
//! The watcher used to filter only dotfiles and editor scratch files, so a
//! build directory was synced like source. In a real Circle that meant 814 of
//! 1713 tracked documents lived under `target/`, and 1.6 GB of proposal blobs —
//! including `CACHEDIR.TAG`, the standard marker meaning *do not back this up* —
//! were replicated to every peer.
//!
//! Two sources of truth, in order of precedence:
//!
//! 1. `.gitignore` / `.ignore` / `.enoxignore` anywhere in the workspace. If a
//!    project already says what is derived, that is the answer.
//! 2. A short built-in list, because the project that prompted this had **no**
//!    `.gitignore` at all and still carried 1.6 GB of `target/`.
//!
//! **Ignoring is not deleting.** A path that becomes ignored stops being
//! tracked and stops being offered to peers; the file itself is left alone on
//! every device. Recording a deletion instead would erase a teammate's build
//! output from their disk, which is not what adding an ignore rule asks for.

use ignore::gitignore::{Gitignore, GitignoreBuilder};
use std::path::Path;

/// Directories that are never hand-authored, ignored even with no ignore file.
///
/// Deliberately short. A wrong entry here silently stops syncing something a
/// user wrote, which is far worse than tracking something they did not — so
/// this covers only cases where the directory is unambiguously derived, and
/// everything else is left to the project's own ignore files. `dist/` and
/// `build/` are excluded for exactly this reason: both are plausible
/// hand-written directories.
pub const DEFAULT_IGNORES: &[&str] = &[
    "target/",       // Rust
    "node_modules/", // npm/yarn/pnpm
    "__pycache__/",  // Python bytecode
    ".venv/",        // Python virtualenv (also a dotdir)
    "venv/",         // Python virtualenv, undotted convention
];

/// Ignore files honoured, in addition to the built-ins.
const IGNORE_FILES: &[&str] = &[".enoxignore", ".gitignore", ".ignore"];

/// A compiled matcher for one workspace.
///
/// One `Gitignore` per directory that holds an ignore file, kept with that
/// directory's workspace-relative path. A single flattened matcher would let
/// `frontend/.gitignore` silently govern `backend/` — patterns in a nested
/// ignore file are relative to the directory containing it.
pub struct IgnoreRules {
    /// `(dir_rel, matcher)`, shallowest first. `dir_rel` is "" for the root.
    matchers: Vec<(String, Gitignore)>,
}

impl IgnoreRules {
    /// Compile the rules for `workspace`, walking it for ignore files.
    ///
    /// Never fails: a malformed pattern is skipped rather than taking the
    /// daemon down, since these files are user-authored.
    pub fn build(workspace: &Path) -> Self {
        let mut matchers = vec![("".to_string(), defaults_matcher())];
        collect_ignore_files(workspace, workspace, &mut matchers, 0);
        // Shallowest first, so a deeper ignore file can override a shallower
        // one — matching gitignore precedence.
        matchers.sort_by_key(|(dir, _)| dir.matches('/').count() + usize::from(!dir.is_empty()));
        Self { matchers }
    }

    /// An empty ruleset — built-ins only, no workspace scan. For tests and for
    /// the window before a workspace exists.
    pub fn defaults_only() -> Self {
        Self {
            matchers: vec![("".to_string(), defaults_matcher())],
        }
    }

    /// Whether `rel` (forward-slashed, workspace-relative) should be skipped.
    pub fn is_ignored(&self, rel: &str) -> bool {
        if always_ignored(rel) {
            return true;
        }
        // Last match wins, and matchers are ordered shallowest first, so a
        // nested negation can re-include something a parent excluded.
        let mut ignored = false;
        for (dir, matcher) in &self.matchers {
            let Some(scoped) = scope(rel, dir) else {
                continue;
            };
            // Directory rules such as `target/` only match when the matcher is
            // told the path is a directory, so test each ancestor as one.
            let mut prefix = String::new();
            for part in scoped.split('/') {
                if !prefix.is_empty() {
                    prefix.push('/');
                }
                prefix.push_str(part);
                let is_dir = prefix.len() < scoped.len();
                let m = matcher.matched(&prefix, is_dir);
                if m.is_ignore() {
                    ignored = true;
                } else if m.is_whitelist() {
                    ignored = false;
                }
            }
        }
        ignored
    }
}

/// Whether a write to this path should invalidate the compiled rules.
pub fn is_ignore_file(rel: &str) -> bool {
    let name = rel.rsplit('/').next().unwrap_or(rel);
    IGNORE_FILES.contains(&name)
}

/// `rel` expressed relative to `dir`, or `None` if it is not under it.
fn scope<'a>(rel: &'a str, dir: &str) -> Option<&'a str> {
    if dir.is_empty() {
        return Some(rel);
    }
    rel.strip_prefix(dir)?.strip_prefix('/')
}

fn defaults_matcher() -> Gitignore {
    let mut builder = GitignoreBuilder::new("");
    for pattern in DEFAULT_IGNORES {
        let _ = builder.add_line(None, pattern);
    }
    builder.build().unwrap_or_else(|_| Gitignore::empty())
}

fn collect_ignore_files(
    root: &Path,
    dir: &Path,
    matchers: &mut Vec<(String, Gitignore)>,
    depth: usize,
) {
    // A deep tree is usually a dependency directory the built-ins already cover;
    // bounding the walk keeps startup predictable on a large workspace.
    if depth > 12 {
        return;
    }
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let Ok(file_type) = entry.file_type() else {
            continue;
        };
        if file_type.is_dir() {
            let name = entry.file_name();
            let name = name.to_string_lossy();
            // Don't descend into what we already know is derived, and never
            // into `.git` — this is where the time goes on a big checkout.
            if DEFAULT_IGNORES.contains(&format!("{name}/").as_str()) || name.starts_with('.') {
                continue;
            }
            collect_ignore_files(root, &path, matchers, depth + 1);
        } else if IGNORE_FILES.contains(&entry.file_name().to_string_lossy().as_ref()) {
            let dir_rel = dir
                .strip_prefix(root)
                .map(|p| p.to_string_lossy().replace('\\', "/"))
                .unwrap_or_default();
            let mut builder = GitignoreBuilder::new(dir);
            // add() returns any parse error; a bad pattern is skipped, not fatal.
            let _ = builder.add(&path);
            if let Ok(m) = builder.build() {
                matchers.push((dir_rel, m));
            }
        }
    }
}

/// Rules that hold regardless of ignore files: dotfiles, editor scratch, and
/// the sync engine's own conflict copies. Preserved from the original filter.
fn always_ignored(rel: &str) -> bool {
    let name = rel.split('/').next_back().unwrap_or(rel);
    if rel.split('/').any(|part| part.starts_with('.')) {
        return true;
    }
    if name.ends_with('~')
        || name.ends_with(".swp")
        || name.ends_with(".swx")
        || name.ends_with(".swo")
        || name.ends_with(".tmp")
    {
        return true;
    }
    // Sublime Text safe-write: test.txt.sb-<hex>-<random>
    if name.contains(".sb-") {
        return true;
    }
    // Conflict copies written by the sync engine: file.txt.conflict.agent-id
    if name.contains(".conflict.") {
        return true;
    }
    // Vim temp files (numeric names like 4913)
    if !name.is_empty() && name.chars().all(|c| c.is_ascii_digit()) {
        return true;
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn built_ins_cover_derived_directories_without_any_ignore_file() {
        let rules = IgnoreRules::defaults_only();
        assert!(rules.is_ignored("repo/src-tauri/target/debug/build/x/output"));
        assert!(rules.is_ignored("target/CACHEDIR.TAG"));
        assert!(rules.is_ignored("frontend/node_modules/react/index.js"));
        assert!(rules.is_ignored("app/__pycache__/mod.cpython-311.pyc"));
    }

    #[test]
    fn source_is_never_ignored_by_the_built_ins() {
        let rules = IgnoreRules::defaults_only();
        assert!(!rules.is_ignored("src/main.rs"));
        assert!(!rules.is_ignored("repo/src/lib.rs"));
        assert!(!rules.is_ignored("README.md"));
        // Near-misses on the directory names must not be swept up.
        assert!(!rules.is_ignored("targets.md"));
        assert!(!rules.is_ignored("src/target_selection.rs"));
        assert!(!rules.is_ignored("docs/node_modules.md"));
        // `dist/` and `build/` are plausible hand-written directories, so the
        // built-ins deliberately leave them alone.
        assert!(!rules.is_ignored("dist/index.html"));
        assert!(!rules.is_ignored("build/notes.md"));
    }

    #[test]
    fn a_gitignore_in_the_workspace_is_honoured() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join(".gitignore"), "*.log\nsecret/\n").unwrap();
        std::fs::create_dir_all(dir.path().join("secret")).unwrap();
        let rules = IgnoreRules::build(dir.path());

        assert!(rules.is_ignored("debug.log"));
        assert!(rules.is_ignored("secret/keys.txt"));
        assert!(!rules.is_ignored("debug.txt"));
        assert!(!rules.is_ignored("src/main.rs"));
    }

    #[test]
    fn a_nested_gitignore_applies_to_its_own_subtree() {
        let dir = tempfile::tempdir().unwrap();
        let sub = dir.path().join("frontend");
        std::fs::create_dir_all(&sub).unwrap();
        std::fs::write(sub.join(".gitignore"), "coverage/\n").unwrap();
        let rules = IgnoreRules::build(dir.path());

        assert!(rules.is_ignored("frontend/coverage/report.html"));
        // The nested rule must not leak to a sibling subtree.
        assert!(!rules.is_ignored("backend/coverage/report.html"));
    }

    #[test]
    fn a_negation_can_re_include_a_path() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join(".gitignore"), "*.log\n!keep.log\n").unwrap();
        let rules = IgnoreRules::build(dir.path());

        assert!(rules.is_ignored("debug.log"));
        assert!(!rules.is_ignored("keep.log"));
    }

    #[test]
    fn enoxignore_is_honoured_too() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join(".enoxignore"), "scratch/\n").unwrap();
        let rules = IgnoreRules::build(dir.path());
        assert!(rules.is_ignored("scratch/notes.txt"));
    }

    #[test]
    fn editor_scratch_and_dotfiles_still_ignored() {
        let rules = IgnoreRules::defaults_only();
        assert!(rules.is_ignored(".git/config"));
        assert!(rules.is_ignored("notes.txt~"));
        assert!(rules.is_ignored("notes.txt.swp"));
        assert!(rules.is_ignored("a.txt.sb-1234-abcd"));
        assert!(rules.is_ignored("a.txt.conflict.agent"));
        assert!(rules.is_ignored("4913"));
        assert!(!rules.is_ignored("notes.txt"));
    }

    #[test]
    fn ignore_file_writes_are_recognised() {
        assert!(is_ignore_file(".gitignore"));
        assert!(is_ignore_file("frontend/.gitignore"));
        assert!(is_ignore_file(".enoxignore"));
        assert!(!is_ignore_file("src/gitignore.rs"));
    }

    #[test]
    fn a_malformed_pattern_does_not_take_the_daemon_down() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join(".gitignore"), "[unclosed\n*.log\n").unwrap();
        let rules = IgnoreRules::build(dir.path());
        // The valid rule still applies; the bad one is skipped.
        assert!(!rules.is_ignored("src/main.rs"));
    }
}
