//! Provider for the `source_lines` table.

use crate::Result;
use rusqlite::{params, Connection};
use std::path::Path;

use super::walk_source_files;

/// Maximum file size (in bytes) to ingest line-by-line. Files larger than this
/// are skipped to keep the in-memory database manageable.
const MAX_FILE_SIZE: u64 = 1_048_576; // 1 MB

/// Create and populate the `source_lines` table.
pub fn load(conn: &Connection, repo_path: &Path) -> Result<()> {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS source_lines (
            file_path TEXT,
            line_number INTEGER,
            content TEXT,
            is_blank INTEGER,
            PRIMARY KEY (file_path, line_number)
        )",
    )?;

    let files = walk_source_files(repo_path);

    conn.execute_batch("BEGIN")?;

    let mut stmt = conn.prepare(
        "INSERT OR IGNORE INTO source_lines
         (file_path, line_number, content, is_blank)
         VALUES (?1, ?2, ?3, ?4)",
    )?;

    for file_info in &files {
        if file_info.size > MAX_FILE_SIZE {
            continue;
        }

        let abs_path = repo_path.join(&file_info.path);
        let Ok(bytes) = std::fs::read(&abs_path) else {
            continue;
        };
        let content = String::from_utf8_lossy(&bytes);

        for (i, line) in content.lines().enumerate() {
            let line_number = (i + 1) as i64;
            let is_blank: i64 = if line.trim().is_empty() { 1 } else { 0 };
            stmt.execute(params![file_info.path, line_number, line, is_blank])?;
        }
    }

    drop(stmt);
    conn.execute_batch("COMMIT")?;

    // Create index after bulk insert for better performance.
    conn.execute_batch(
        "CREATE INDEX IF NOT EXISTS idx_source_lines_content ON source_lines(content)",
    )?;

    Ok(())
}
