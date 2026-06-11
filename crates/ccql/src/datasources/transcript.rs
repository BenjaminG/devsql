use crate::config::Config;
use crate::error::Result;
use crate::streaming;
use std::path::{Path, PathBuf};
use walkdir::WalkDir;

/// A discovered transcript file with its derived metadata.
#[derive(Debug, Clone)]
pub struct TranscriptFile {
    pub path: PathBuf,
    /// File name, e.g. `sess-top.jsonl` or `agent-abc.jsonl`.
    pub source_file: String,
    /// Session id: file stem for top-level/legacy sessions; parent session
    /// directory name for subagent transcripts (legacy `ses_` prefix stripped).
    pub session_id: String,
    /// Project slug directory name, or `None` for legacy flat files.
    pub project: Option<String>,
    /// Agent file stem for subagent transcripts, or `None` otherwise.
    pub agent_id: Option<String>,
}

/// Discover all transcript files across the modern projects layout and the
/// legacy flat `transcripts/` directory.
///
/// - `<data_dir>/projects/<slug>/*.jsonl` (top-level sessions)
/// - `<data_dir>/projects/<slug>/<sessionId>/subagents/*.jsonl` (subagents)
/// - `<data_dir>/transcripts/*.jsonl` (legacy)
pub fn discover_transcript_files(config: &Config) -> Vec<TranscriptFile> {
    let mut files = Vec::new();

    let projects_dir = config.projects_dir();
    if projects_dir.exists() {
        for entry in WalkDir::new(&projects_dir)
            .into_iter()
            .filter_map(|e| e.ok())
        {
            let path = entry.path();
            if path.extension().is_none_or(|e| e != "jsonl") {
                continue;
            }
            if let Some(file) = classify_project_file(path, &projects_dir) {
                files.push(file);
            }
        }
    }

    let legacy_dir = config.transcripts_dir();
    if legacy_dir.exists() {
        for entry in WalkDir::new(&legacy_dir)
            .max_depth(1)
            .into_iter()
            .filter_map(|e| e.ok())
        {
            let path = entry.path();
            if path.extension().is_none_or(|e| e != "jsonl") {
                continue;
            }
            let source_file = file_name_string(path);
            let session_id = source_file
                .strip_prefix("ses_")
                .and_then(|s| s.strip_suffix(".jsonl"))
                .unwrap_or(&source_file)
                .to_string();
            files.push(TranscriptFile {
                path: path.to_path_buf(),
                source_file,
                session_id,
                project: None,
                agent_id: None,
            });
        }
    }

    files
}

fn file_name_string(path: &Path) -> String {
    path.file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("unknown")
        .to_string()
}

/// Classify a `.jsonl` file under `projects/` into transcript metadata.
fn classify_project_file(path: &Path, projects_dir: &Path) -> Option<TranscriptFile> {
    let rel = path.strip_prefix(projects_dir).ok()?;
    let components: Vec<&str> = rel
        .components()
        .filter_map(|c| c.as_os_str().to_str())
        .collect();

    let source_file = file_name_string(path);
    let stem = path.file_stem().and_then(|s| s.to_str())?.to_string();

    match components.as_slice() {
        // <slug>/<sessionId>.jsonl
        [slug, _file] => Some(TranscriptFile {
            path: path.to_path_buf(),
            source_file,
            session_id: stem,
            project: Some(slug.to_string()),
            agent_id: None,
        }),
        // <slug>/<sessionId>/subagents/agent-<id>.jsonl
        [slug, session_id, "subagents", _file] => Some(TranscriptFile {
            path: path.to_path_buf(),
            source_file,
            session_id: session_id.to_string(),
            project: Some(slug.to_string()),
            agent_id: Some(stem),
        }),
        _ => None,
    }
}

/// Flattened `model` / `usage_*` column values for an assistant transcript
/// record, extracted from `message.model` and `message.usage.*`.
///
/// Returns only pairs whose source value exists and is non-null. Empty for
/// non-assistant records.
pub fn flattened_usage_fields(json: &serde_json::Value) -> Vec<(&'static str, &serde_json::Value)> {
    let mut fields = Vec::new();
    if json.get("type").and_then(|t| t.as_str()) != Some("assistant") {
        return fields;
    }
    let Some(message) = json.get("message") else {
        return fields;
    };

    fn put<'a>(
        fields: &mut Vec<(&'static str, &'a serde_json::Value)>,
        key: &'static str,
        value: Option<&'a serde_json::Value>,
    ) {
        if let Some(value) = value {
            if !value.is_null() {
                fields.push((key, value));
            }
        }
    }

    put(&mut fields, "model", message.get("model"));

    let usage = message.get("usage");
    let get = |path: &str| usage.and_then(|u| u.get(path));
    put(&mut fields, "usage_input_tokens", get("input_tokens"));
    put(&mut fields, "usage_output_tokens", get("output_tokens"));
    put(
        &mut fields,
        "usage_cache_read_input_tokens",
        get("cache_read_input_tokens"),
    );
    put(
        &mut fields,
        "usage_cache_creation_input_tokens",
        get("cache_creation_input_tokens"),
    );
    put(&mut fields, "usage_service_tier", get("service_tier"));

    let cache_creation = usage.and_then(|u| u.get("cache_creation"));
    put(
        &mut fields,
        "usage_ephemeral_5m_input_tokens",
        cache_creation.and_then(|c| c.get("ephemeral_5m_input_tokens")),
    );
    put(
        &mut fields,
        "usage_ephemeral_1h_input_tokens",
        cache_creation.and_then(|c| c.get("ephemeral_1h_input_tokens")),
    );

    fields
}

/// Per-session aggregation over a session's transcript records, used to build
/// the `sessions` virtual table (and reused by the devsql binary).
#[derive(Default)]
pub struct SessionAggregate {
    pub cwd: Option<String>,
    pub git_branch: Option<String>,
    pub version: Option<String>,
    pub title: Option<String>,
    pub first_timestamp: Option<String>,
    pub last_timestamp: Option<String>,
    pub user_message_count: i64,
    pub assistant_message_count: i64,
    pub total_input_tokens: i64,
    pub total_output_tokens: i64,
    pub total_cache_read_input_tokens: i64,
    pub total_cache_creation_input_tokens: i64,
    pub pr_url: Option<String>,
    pub pr_number: Option<i64>,
}

impl SessionAggregate {
    /// Fold one transcript record into the aggregate.
    pub fn observe(&mut self, json: &serde_json::Value) {
        let get_str = |key: &str| json.get(key).and_then(|v| v.as_str()).map(String::from);

        if let Some(ts) = get_str("timestamp") {
            // ISO 8601 timestamps compare correctly as strings
            if self
                .first_timestamp
                .as_deref()
                .is_none_or(|f| ts.as_str() < f)
            {
                self.first_timestamp = Some(ts.clone());
            }
            if self
                .last_timestamp
                .as_deref()
                .is_none_or(|l| ts.as_str() > l)
            {
                self.last_timestamp = Some(ts);
            }
        }
        if self.cwd.is_none() {
            self.cwd = get_str("cwd");
        }
        if self.git_branch.is_none() {
            self.git_branch = get_str("gitBranch");
        }
        if let Some(version) = get_str("version") {
            self.version = Some(version); // last seen wins
        }

        match json.get("type").and_then(|t| t.as_str()) {
            Some("user") => self.user_message_count += 1,
            Some("assistant") => {
                self.assistant_message_count += 1;
                if let Some(usage) = json.get("message").and_then(|m| m.get("usage")) {
                    let tok = |key: &str| {
                        usage
                            .get(key)
                            .and_then(serde_json::Value::as_i64)
                            .unwrap_or(0)
                    };
                    self.total_input_tokens += tok("input_tokens");
                    self.total_output_tokens += tok("output_tokens");
                    self.total_cache_read_input_tokens += tok("cache_read_input_tokens");
                    self.total_cache_creation_input_tokens += tok("cache_creation_input_tokens");
                }
            }
            Some("ai-title") => {
                if let Some(title) = get_str("aiTitle") {
                    self.title = Some(title);
                }
            }
            Some("pr-link") => {
                if let Some(url) = get_str("prUrl") {
                    self.pr_url = Some(url);
                }
                if let Some(n) = json.get("prNumber").and_then(serde_json::Value::as_i64) {
                    self.pr_number = Some(n);
                }
            }
            _ => {}
        }
    }
}

pub struct TranscriptDataSource {
    config: Config,
}

impl TranscriptDataSource {
    pub fn new(config: Config) -> Self {
        Self { config }
    }

    pub fn list_sessions(&self) -> Result<Vec<SessionInfo>> {
        let mut sessions = Vec::new();
        for file in discover_transcript_files(&self.config) {
            let metadata = std::fs::metadata(&file.path).ok();
            sessions.push(SessionInfo {
                session_id: file.source_file.trim_end_matches(".jsonl").to_string(),
                path: file.path,
                size_bytes: metadata.as_ref().map(|m| m.len()).unwrap_or(0),
                modified: metadata.and_then(|m| m.modified().ok()),
            });
        }

        sessions.sort_by_key(|s| std::cmp::Reverse(s.modified));
        Ok(sessions)
    }

    pub async fn load_session(&self, session_id: &str) -> Result<Vec<serde_json::Value>> {
        let path = discover_transcript_files(&self.config)
            .into_iter()
            .find(|f| f.source_file.trim_end_matches(".jsonl") == session_id)
            .map(|f| f.path)
            .unwrap_or_else(|| {
                self.config
                    .transcripts_dir()
                    .join(format!("{}.jsonl", session_id))
            });
        streaming::read_jsonl_raw(path).await
    }

    pub async fn load_all_sessions(&self) -> Result<Vec<(String, Vec<serde_json::Value>)>> {
        let sessions = self.list_sessions()?;
        let mut all = Vec::new();

        for session in sessions {
            match streaming::read_jsonl_raw(&session.path).await {
                Ok(entries) => all.push((session.session_id, entries)),
                Err(e) => tracing::debug!("Failed to load session {}: {}", session.session_id, e),
            }
        }

        Ok(all)
    }

    pub async fn search_in_sessions(&self, pattern: &regex::Regex) -> Result<Vec<SearchResult>> {
        let sessions = self.list_sessions()?;
        let mut results = Vec::new();

        for session in sessions {
            match streaming::read_jsonl_raw(&session.path).await {
                Ok(entries) => {
                    for (idx, entry) in entries.iter().enumerate() {
                        let entry_str = serde_json::to_string(entry).unwrap_or_default();
                        if pattern.is_match(&entry_str) {
                            results.push(SearchResult {
                                session_id: session.session_id.clone(),
                                entry_index: idx,
                                entry: entry.clone(),
                            });
                        }
                    }
                }
                Err(e) => tracing::debug!("Failed to load session {}: {}", session.session_id, e),
            }
        }

        Ok(results)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write(path: &std::path::Path, contents: &str) {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).expect("mkdir");
        }
        std::fs::write(path, contents).expect("write");
    }

    #[test]
    fn list_sessions_discovers_projects_subagents_and_legacy() {
        let temp = tempfile::tempdir().expect("temp");
        let root = temp.path();
        let slug = "-Users-douglance-Developer-lv-devsql";

        write(
            &root.join("projects").join(slug).join("sess-top.jsonl"),
            r#"{"type":"user"}"#,
        );
        write(
            &root
                .join("projects")
                .join(slug)
                .join("sess-parent")
                .join("subagents")
                .join("agent-abc.jsonl"),
            r#"{"type":"assistant"}"#,
        );
        write(
            &root.join("transcripts").join("ses_legacy.jsonl"),
            r#"{"type":"user"}"#,
        );

        let config = Config::new_with_codex_data_dir(root.to_path_buf(), root.join("codex"))
            .expect("config");
        let ds = TranscriptDataSource::new(config);
        let sessions = ds.list_sessions().expect("sessions");

        assert_eq!(sessions.len(), 3, "all three transcript files discovered");
        let ids: Vec<&str> = sessions.iter().map(|s| s.session_id.as_str()).collect();
        assert!(ids.contains(&"sess-top"));
        assert!(ids.contains(&"agent-abc"));
        assert!(ids.contains(&"ses_legacy"));
    }
}

#[derive(Debug, Clone)]
pub struct SessionInfo {
    pub session_id: String,
    pub path: PathBuf,
    pub size_bytes: u64,
    pub modified: Option<std::time::SystemTime>,
}

impl SessionInfo {
    pub fn formatted_time(&self) -> String {
        self.modified
            .and_then(|m| m.duration_since(std::time::UNIX_EPOCH).ok())
            .map(|d| {
                chrono::DateTime::from_timestamp(d.as_secs() as i64, 0)
                    .map(|dt| dt.format("%Y-%m-%d %H:%M").to_string())
                    .unwrap_or_else(|| "unknown".to_string())
            })
            .unwrap_or_else(|| "unknown".to_string())
    }

    pub fn size_human(&self) -> String {
        if self.size_bytes < 1024 {
            format!("{} B", self.size_bytes)
        } else if self.size_bytes < 1024 * 1024 {
            format!("{:.1} KB", self.size_bytes as f64 / 1024.0)
        } else {
            format!("{:.1} MB", self.size_bytes as f64 / (1024.0 * 1024.0))
        }
    }
}

#[derive(Debug, Clone)]
pub struct SearchResult {
    pub session_id: String,
    pub entry_index: usize,
    pub entry: serde_json::Value,
}
