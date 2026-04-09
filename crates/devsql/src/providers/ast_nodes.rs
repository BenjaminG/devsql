#![cfg(feature = "tree-sitter-ast")]

use crate::Result;
use rusqlite::Connection;
use std::path::Path;

/// Placeholder loader until tree-sitter-backed AST extraction is implemented.
pub fn load(_conn: &Connection, _repo_path: &Path) -> Result<()> {
    Ok(())
}
