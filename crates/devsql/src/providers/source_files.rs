//! Provider for the `source_files` table.

use crate::Result;
use rusqlite::{params, Connection};
use std::io::{BufRead, BufReader};
use std::path::Path;

use super::{detect_language, walk_source_files};

/// Create and populate the `source_files` table.
pub fn load(conn: &Connection, repo_path: &Path) -> Result<()> {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS source_files (
            path TEXT PRIMARY KEY,
            name TEXT,
            extension TEXT,
            directory TEXT,
            size_bytes INTEGER,
            line_count INTEGER,
            modified_at TEXT,
            language TEXT
        )",
    )?;

    let files = walk_source_files(repo_path);

    conn.execute_batch("BEGIN")?;

    let mut stmt = conn.prepare(
        "INSERT OR IGNORE INTO source_files
         (path, name, extension, directory, size_bytes, line_count, modified_at, language)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
    )?;

    for file_info in &files {
        let abs_path = repo_path.join(&file_info.path);
        let line_count = count_lines(&abs_path);
        let language = detect_language(&file_info.extension);

        stmt.execute(params![
            file_info.path,
            file_info.name,
            file_info.extension,
            file_info.directory,
            file_info.size as i64,
            line_count as i64,
            file_info.modified_at,
            language,
        ])?;
    }

    drop(stmt);
    conn.execute_batch("COMMIT")?;

    Ok(())
}

/// Count lines in a file using a buffered reader without holding the entire
/// file contents in memory.
fn count_lines(path: &Path) -> usize {
    let Ok(file) = std::fs::File::open(path) else {
        return 0;
    };
    let reader = BufReader::new(file);
    reader.lines().count()
}
