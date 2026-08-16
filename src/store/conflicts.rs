use std::path::Path;

/// Generates the relative path for a conflict copy of `rel_path`.
/// Format: `<rel_path>.conflict.<agent_id>`
pub fn conflict_rel_path(rel_path: &str, agent_id: &str) -> String {
    format!("{rel_path}.conflict.{agent_id}")
}

/// Walk the workspace directory tree and return all relative paths that look
/// like conflict copies (filename contains ".conflict.").
/// Hidden directories (starting with '.') are skipped.
pub fn scan_conflicts(workspace: &Path) -> Vec<String> {
    let mut out = Vec::new();
    scan_dir(workspace, workspace, &mut out);
    out.sort();
    out
}

fn scan_dir(base: &Path, dir: &Path, acc: &mut Vec<String>) {
    let rd = match std::fs::read_dir(dir) {
        Ok(rd) => rd,
        Err(_) => return,
    };
    for entry in rd.flatten() {
        let path = entry.path();
        let name = entry.file_name().to_string_lossy().to_string();
        if name.starts_with('.') {
            continue;
        }
        if path.is_dir() {
            scan_dir(base, &path, acc);
        } else if name.contains(".conflict.") {
            if let Ok(rel) = path.strip_prefix(base) {
                acc.push(rel.to_string_lossy().replace('\\', "/"));
            }
        }
    }
}
