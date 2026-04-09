//! Semantic diff: symbol-level change classification between two Git commits.

use crate::providers::symbols::{extract_symbols, SymbolMatch};

/// A symbol-level change detected between two commits.
pub struct SymbolChange {
    pub name: String,
    pub kind: String,
    pub change_type: ChangeType,
    pub file_path: String,
    pub line_start: usize,
}

/// The type of change that happened to a symbol.
pub enum ChangeType {
    Added,
    Removed,
    Modified,
}

/// Read file content at a specific git commit.
pub fn read_file_at_commit(
    repo: &git2::Repository,
    oid: git2::Oid,
    path: &str,
) -> Option<String> {
    let commit = repo.find_commit(oid).ok()?;
    let tree = commit.tree().ok()?;
    let entry = tree.get_path(std::path::Path::new(path)).ok()?;
    let blob = repo.find_blob(entry.id()).ok()?;
    if blob.is_binary() {
        return None;
    }
    String::from_utf8(blob.content().to_vec()).ok()
}

/// Compute semantic diff for a single file by comparing symbols between two
/// versions and checking which symbols overlap with changed hunks.
pub fn diff_file_symbols(
    base_content: Option<&str>,
    head_content: Option<&str>,
    file_path: &str,
    language: &str,
    hunks: &[(usize, usize)], // (start_line, line_count) of changed regions in head
) -> Vec<SymbolChange> {
    let base_symbols: Vec<SymbolMatch> = base_content
        .map(|c| extract_symbols(language, c))
        .unwrap_or_default();
    let head_symbols: Vec<SymbolMatch> = head_content
        .map(|c| extract_symbols(language, c))
        .unwrap_or_default();

    let mut changes = Vec::new();

    // Find added symbols (in head but not base, by name+kind)
    for sym in &head_symbols {
        if !base_symbols
            .iter()
            .any(|b| b.name == sym.name && b.kind == sym.kind)
        {
            changes.push(SymbolChange {
                name: sym.name.clone(),
                kind: sym.kind.clone(),
                change_type: ChangeType::Added,
                file_path: file_path.to_string(),
                line_start: sym.line_start,
            });
        }
    }

    // Find removed symbols (in base but not head)
    for sym in &base_symbols {
        if !head_symbols
            .iter()
            .any(|h| h.name == sym.name && h.kind == sym.kind)
        {
            changes.push(SymbolChange {
                name: sym.name.clone(),
                kind: sym.kind.clone(),
                change_type: ChangeType::Removed,
                file_path: file_path.to_string(),
                line_start: sym.line_start,
            });
        }
    }

    // Find modified symbols (in both, but hunks overlap the head symbol's line range)
    for sym in &head_symbols {
        let in_base = base_symbols
            .iter()
            .any(|b| b.name == sym.name && b.kind == sym.kind);
        let already_added = changes
            .iter()
            .any(|c| c.name == sym.name && matches!(c.change_type, ChangeType::Added));
        if in_base && !already_added {
            let sym_start = sym.line_start;
            let sym_end = sym.line_end;
            let is_touched = hunks.iter().any(|&(hunk_start, hunk_count)| {
                let hunk_end = hunk_start + hunk_count;
                // Overlap check: two ranges [sym_start, sym_end] and [hunk_start, hunk_end]
                sym_start <= hunk_end && hunk_start <= sym_end
            });
            if is_touched {
                changes.push(SymbolChange {
                    name: sym.name.clone(),
                    kind: sym.kind.clone(),
                    change_type: ChangeType::Modified,
                    file_path: file_path.to_string(),
                    line_start: sym.line_start,
                });
            }
        }
    }

    changes
}
