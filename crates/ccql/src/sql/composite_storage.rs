//! Composite storage that merges multiple files as virtual tables
//!
//! Provides unified access to:
//! - Single-file tables (history, stats) via JsonStorage
//! - Virtual tables (jhistory/codex_history, transcripts, todos) via custom scanners

use crate::config::Config;
use crate::datasources::transcript::{discover_transcript_files, TranscriptFile};
use async_trait::async_trait;
use futures::stream;
use gluesql::core::ast::{ColumnDef, IndexOperator, OrderByExpr};
use gluesql::core::data::{CustomFunction as StructCustomFunction, Schema};
use gluesql::core::error::Error as GlueError;
use gluesql::core::store::{
    AlterTable, CustomFunction, CustomFunctionMut, DataRow, Index, IndexMut, Metadata, Planner,
    RowIter, Store, StoreMut, Transaction,
};
use gluesql::prelude::{Key, Result, Value};
use gluesql_json_storage::JsonStorage;
use serde_json::Value as JsonValue;
use std::collections::{BTreeMap, HashMap};
use std::fs;
use std::io::{BufRead, BufReader};

/// Storage that combines JsonStorage with virtual multi-file tables
pub struct CompositeStorage {
    json_storage: JsonStorage,
    config: Config,
}

impl CompositeStorage {
    /// Create a new composite storage
    pub fn new(config: Config) -> Result<Self> {
        let json_storage = JsonStorage::new(&config.data_dir)?;
        Ok(Self {
            json_storage,
            config,
        })
    }

    /// Check if a table is a virtual multi-file table
    fn is_virtual_table(&self, table_name: &str) -> bool {
        matches!(
            table_name,
            "jhistory" | "codex_history" | "transcripts" | "todos" | "sessions"
        )
    }

    /// Scan Codex jhistory and return all rows
    fn scan_jhistory(&self) -> Result<Vec<(Key, DataRow)>> {
        let jhistory_file = self.config.jhistory_file();
        if !jhistory_file.exists() {
            return Ok(Vec::new());
        }

        let mut rows = Vec::new();
        let mut row_id: i64 = 0;

        let file = fs::File::open(&jhistory_file)
            .map_err(|e| GlueError::StorageMsg(format!("Failed to open jhistory file: {}", e)))?;
        let reader = BufReader::new(file);

        for line in reader.lines().map_while(Result::ok) {
            if line.trim().is_empty() {
                continue;
            }

            if let Ok(json) = serde_json::from_str::<JsonValue>(&line) {
                if let Some(data_row) = jhistory_json_to_data_row(&json) {
                    rows.push((Key::I64(row_id), data_row));
                    row_id += 1;
                }
            }
        }

        Ok(rows)
    }

    /// Scan transcript files (projects layout + legacy) and return all rows
    fn scan_transcripts(&self) -> Result<Vec<(Key, DataRow)>> {
        let mut rows = Vec::new();
        let mut row_id: i64 = 0;

        for file in discover_transcript_files(&self.config) {
            if let Ok(handle) = fs::File::open(&file.path) {
                let reader = BufReader::new(handle);
                for line in reader.lines().map_while(Result::ok) {
                    if let Ok(json) = serde_json::from_str::<JsonValue>(&line) {
                        let data_row = json_to_data_row_with_meta(&json, &file);
                        rows.push((Key::I64(row_id), data_row));
                        row_id += 1;
                    }
                }
            }
        }

        Ok(rows)
    }

    /// Scan transcript files and aggregate one row per top-level session
    fn scan_sessions(&self) -> Result<Vec<(Key, DataRow)>> {
        let files = discover_transcript_files(&self.config);

        // Subagent file counts keyed by (project, parent session id)
        let mut subagent_counts: HashMap<(Option<String>, String), i64> = HashMap::new();
        for file in &files {
            if file.agent_id.is_some() {
                *subagent_counts
                    .entry((file.project.clone(), file.session_id.clone()))
                    .or_insert(0) += 1;
            }
        }

        let mut rows = Vec::new();
        let mut row_id: i64 = 0;

        for file in files.iter().filter(|f| f.agent_id.is_none()) {
            let Ok(handle) = fs::File::open(&file.path) else {
                continue;
            };

            let mut agg = SessionAggregate::default();
            let reader = BufReader::new(handle);
            for line in reader.lines().map_while(Result::ok) {
                if let Ok(json) = serde_json::from_str::<JsonValue>(&line) {
                    agg.observe(&json);
                }
            }

            let subagent_count = subagent_counts
                .get(&(file.project.clone(), file.session_id.clone()))
                .copied()
                .unwrap_or(0);

            rows.push((Key::I64(row_id), agg.into_data_row(file, subagent_count)));
            row_id += 1;
        }

        Ok(rows)
    }

    /// Scan todos directory and return all rows
    fn scan_todos(&self) -> Result<Vec<(Key, DataRow)>> {
        let todos_dir = self.config.todos_dir();
        if !todos_dir.exists() {
            return Ok(Vec::new());
        }

        let mut rows = Vec::new();
        let mut row_id: i64 = 0;

        let entries = fs::read_dir(&todos_dir)
            .map_err(|e| GlueError::StorageMsg(format!("Failed to read todos dir: {}", e)))?;

        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().is_some_and(|ext| ext == "json") {
                let source_file = path
                    .file_name()
                    .and_then(|n| n.to_str())
                    .unwrap_or("unknown")
                    .to_string();

                let (workspace_id, agent_id) = parse_todo_filename(&source_file);

                if let Ok(content) = fs::read_to_string(&path) {
                    if let Ok(json) = serde_json::from_str::<JsonValue>(&content) {
                        match json {
                            JsonValue::Array(items) => {
                                for item in items {
                                    let data_row = todo_json_to_data_row(
                                        &item,
                                        &source_file,
                                        &workspace_id,
                                        &agent_id,
                                    );
                                    rows.push((Key::I64(row_id), data_row));
                                    row_id += 1;
                                }
                            }
                            JsonValue::Object(_) => {
                                let data_row = todo_json_to_data_row(
                                    &json,
                                    &source_file,
                                    &workspace_id,
                                    &agent_id,
                                );
                                rows.push((Key::I64(row_id), data_row));
                                row_id += 1;
                            }
                            _ => {}
                        }
                    }
                }
            }
        }

        Ok(rows)
    }

    /// Create a virtual schema for jhistory table (schemaless)
    fn jhistory_schema(&self) -> Schema {
        self.codex_history_schema_for("jhistory")
    }

    /// Create a virtual schema for codex_history alias table (schemaless)
    fn codex_history_alias_schema(&self) -> Schema {
        self.codex_history_schema_for("codex_history")
    }

    fn codex_history_schema_for(&self, table_name: &str) -> Schema {
        Schema {
            table_name: table_name.to_string(),
            column_defs: None, // Schemaless
            indexes: Vec::new(),
            engine: None,
            foreign_keys: Vec::new(),
            comment: Some("Virtual table for Codex CLI history.jsonl".to_string()),
        }
    }

    /// Create a virtual schema for transcripts table (schemaless)
    fn transcripts_schema(&self) -> Schema {
        Schema {
            table_name: "transcripts".to_string(),
            column_defs: None, // Schemaless
            indexes: Vec::new(),
            engine: None,
            foreign_keys: Vec::new(),
            comment: Some("Virtual table merging all transcript files".to_string()),
        }
    }

    /// Create a virtual schema for sessions table (schemaless)
    fn sessions_schema(&self) -> Schema {
        Schema {
            table_name: "sessions".to_string(),
            column_defs: None, // Schemaless
            indexes: Vec::new(),
            engine: None,
            foreign_keys: Vec::new(),
            comment: Some("Virtual table with one row per session".to_string()),
        }
    }

    /// Create a virtual schema for todos table (schemaless)
    fn todos_schema(&self) -> Schema {
        Schema {
            table_name: "todos".to_string(),
            column_defs: None, // Schemaless
            indexes: Vec::new(),
            engine: None,
            foreign_keys: Vec::new(),
            comment: Some("Virtual table merging all todo files".to_string()),
        }
    }
}

/// Per-session aggregation state for the `sessions` virtual table
#[derive(Default)]
struct SessionAggregate {
    cwd: Option<String>,
    git_branch: Option<String>,
    version: Option<String>,
    title: Option<String>,
    first_timestamp: Option<String>,
    last_timestamp: Option<String>,
    user_message_count: i64,
    assistant_message_count: i64,
    total_input_tokens: i64,
    total_output_tokens: i64,
    total_cache_read_input_tokens: i64,
    total_cache_creation_input_tokens: i64,
    pr_url: Option<String>,
    pr_number: Option<i64>,
}

impl SessionAggregate {
    fn observe(&mut self, json: &JsonValue) {
        let get_str = |key: &str| json.get(key).and_then(|v| v.as_str()).map(String::from);

        if let Some(ts) = get_str("timestamp") {
            // ISO 8601 timestamps compare correctly as strings
            if self.first_timestamp.as_deref().is_none_or(|f| ts.as_str() < f) {
                self.first_timestamp = Some(ts.clone());
            }
            if self.last_timestamp.as_deref().is_none_or(|l| ts.as_str() > l) {
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
                    let tok = |key: &str| usage.get(key).and_then(json_value_as_i64).unwrap_or(0);
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
                if let Some(n) = json.get("prNumber").and_then(json_value_as_i64) {
                    self.pr_number = Some(n);
                }
            }
            _ => {}
        }
    }

    fn into_data_row(self, file: &TranscriptFile, subagent_count: i64) -> DataRow {
        let mut map = BTreeMap::new();
        map.insert(
            "session_id".to_string(),
            Value::Str(file.session_id.clone()),
        );
        if let Some(project) = &file.project {
            map.insert("project".to_string(), Value::Str(project.clone()));
        }

        let mut put_str = |key: &str, value: Option<String>| {
            if let Some(value) = value {
                map.insert(key.to_string(), Value::Str(value));
            }
        };
        put_str("cwd", self.cwd);
        put_str("git_branch", self.git_branch);
        put_str("version", self.version);
        put_str("title", self.title);
        put_str("first_timestamp", self.first_timestamp);
        put_str("last_timestamp", self.last_timestamp);
        put_str("pr_url", self.pr_url);

        map.insert(
            "user_message_count".to_string(),
            Value::I64(self.user_message_count),
        );
        map.insert(
            "assistant_message_count".to_string(),
            Value::I64(self.assistant_message_count),
        );
        map.insert("subagent_count".to_string(), Value::I64(subagent_count));
        map.insert(
            "total_input_tokens".to_string(),
            Value::I64(self.total_input_tokens),
        );
        map.insert(
            "total_output_tokens".to_string(),
            Value::I64(self.total_output_tokens),
        );
        map.insert(
            "total_cache_read_input_tokens".to_string(),
            Value::I64(self.total_cache_read_input_tokens),
        );
        map.insert(
            "total_cache_creation_input_tokens".to_string(),
            Value::I64(self.total_cache_creation_input_tokens),
        );
        if let Some(n) = self.pr_number {
            map.insert("pr_number".to_string(), Value::I64(n));
        }

        DataRow::Map(map)
    }
}

/// Convert a JSON object to a DataRow with metadata columns
fn json_to_data_row_with_meta(json: &JsonValue, file: &TranscriptFile) -> DataRow {
    let mut map = BTreeMap::new();

    map.insert(
        "_source_file".to_string(),
        Value::Str(file.source_file.clone()),
    );
    map.insert(
        "_session_id".to_string(),
        Value::Str(file.session_id.clone()),
    );
    if let Some(project) = &file.project {
        map.insert("_project".to_string(), Value::Str(project.clone()));
    }
    if let Some(agent_id) = &file.agent_id {
        map.insert("_agent_id".to_string(), Value::Str(agent_id.clone()));
    }

    if let JsonValue::Object(obj) = json {
        for (key, value) in obj {
            map.insert(key.clone(), json_value_to_glue_value(value));
        }
    }

    flatten_usage_columns(json, &mut map);

    DataRow::Map(map)
}

/// Inject flattened `model` / `usage_*` columns for assistant rows.
///
/// Values come from `message.model` and `message.usage.*`. A column is only
/// added when the source value exists and the key is not already present at
/// the top level of the record (existing JSON keys always win).
fn flatten_usage_columns(json: &JsonValue, map: &mut BTreeMap<String, Value>) {
    if json.get("type").and_then(|t| t.as_str()) != Some("assistant") {
        return;
    }
    let Some(message) = json.get("message") else {
        return;
    };

    let mut put = |key: &str, value: Option<&JsonValue>| {
        if let Some(value) = value {
            if !value.is_null() && !map.contains_key(key) {
                map.insert(key.to_string(), json_value_to_glue_value(value));
            }
        }
    };

    put("model", message.get("model"));

    let usage = message.get("usage");
    let get = |path: &str| usage.and_then(|u| u.get(path));
    put("usage_input_tokens", get("input_tokens"));
    put("usage_output_tokens", get("output_tokens"));
    put("usage_cache_read_input_tokens", get("cache_read_input_tokens"));
    put(
        "usage_cache_creation_input_tokens",
        get("cache_creation_input_tokens"),
    );
    put("usage_service_tier", get("service_tier"));

    let cache_creation = usage.and_then(|u| u.get("cache_creation"));
    put(
        "usage_ephemeral_5m_input_tokens",
        cache_creation.and_then(|c| c.get("ephemeral_5m_input_tokens")),
    );
    put(
        "usage_ephemeral_1h_input_tokens",
        cache_creation.and_then(|c| c.get("ephemeral_1h_input_tokens")),
    );
}

/// Convert a todo JSON object to a DataRow
fn todo_json_to_data_row(
    json: &JsonValue,
    source_file: &str,
    workspace_id: &str,
    agent_id: &str,
) -> DataRow {
    let mut map = BTreeMap::new();

    map.insert(
        "_source_file".to_string(),
        Value::Str(source_file.to_string()),
    );
    map.insert(
        "_workspace_id".to_string(),
        Value::Str(workspace_id.to_string()),
    );
    map.insert("_agent_id".to_string(), Value::Str(agent_id.to_string()));

    if let JsonValue::Object(obj) = json {
        for (key, value) in obj {
            map.insert(key.clone(), json_value_to_glue_value(value));
        }
    }

    DataRow::Map(map)
}

/// Convert a codex jhistory JSON object to a normalized DataRow
fn jhistory_json_to_data_row(json: &JsonValue) -> Option<DataRow> {
    let obj = json.as_object()?;

    let text = obj
        .get("text")
        .or_else(|| obj.get("display"))
        .and_then(json_value_as_string)
        .unwrap_or_default();

    let session_id = obj
        .get("session_id")
        .or_else(|| obj.get("sessionId"))
        .and_then(json_value_as_string)
        .unwrap_or_default();

    let ts_seconds = obj
        .get("ts")
        .and_then(json_value_as_i64)
        .or_else(|| {
            obj.get("timestamp")
                .and_then(json_value_as_i64)
                .map(normalize_ts_seconds)
        })
        .unwrap_or(0);

    let timestamp_millis = ts_seconds.saturating_mul(1000);

    let mut map = BTreeMap::new();
    map.insert("display".to_string(), Value::Str(text.clone()));
    map.insert("timestamp".to_string(), Value::I64(timestamp_millis));
    map.insert("session_id".to_string(), Value::Str(session_id.clone()));
    map.insert("sessionId".to_string(), Value::Str(session_id));
    map.insert("text".to_string(), Value::Str(text));
    map.insert("ts".to_string(), Value::I64(ts_seconds));

    // Preserve any extra fields from codex output.
    for (key, value) in obj {
        if matches!(
            key.as_str(),
            "display" | "timestamp" | "session_id" | "sessionId" | "text" | "ts"
        ) {
            continue;
        }
        map.insert(key.clone(), json_value_to_glue_value(value));
    }

    Some(DataRow::Map(map))
}

fn normalize_ts_seconds(raw_ts: i64) -> i64 {
    // Convert epoch milliseconds into seconds when needed.
    if raw_ts > 10_000_000_000 {
        raw_ts / 1000
    } else {
        raw_ts
    }
}

fn json_value_as_i64(value: &JsonValue) -> Option<i64> {
    match value {
        JsonValue::Number(n) => n
            .as_i64()
            .or_else(|| n.as_u64().and_then(|u| i64::try_from(u).ok())),
        JsonValue::String(s) => s.parse::<i64>().ok(),
        _ => None,
    }
}

fn json_value_as_string(value: &JsonValue) -> Option<String> {
    match value {
        JsonValue::String(s) => Some(s.clone()),
        JsonValue::Null => None,
        other => Some(other.to_string()),
    }
}

/// Parse todo filename to extract workspace_id and agent_id
fn parse_todo_filename(filename: &str) -> (String, String) {
    let name = filename.strip_suffix(".json").unwrap_or(filename);

    if let Some(idx) = name.find("-agent-") {
        let workspace_id = name[..idx].to_string();
        let agent_id = name[idx + 7..].to_string();
        (workspace_id, agent_id)
    } else {
        (name.to_string(), "unknown".to_string())
    }
}

/// Convert serde_json Value to GlueSQL Value
fn json_value_to_glue_value(value: &JsonValue) -> Value {
    match value {
        JsonValue::Null => Value::Null,
        JsonValue::Bool(b) => Value::Bool(*b),
        JsonValue::Number(n) => {
            if let Some(i) = n.as_i64() {
                Value::I64(i)
            } else if let Some(f) = n.as_f64() {
                Value::F64(f)
            } else {
                Value::Str(n.to_string())
            }
        }
        JsonValue::String(s) => Value::Str(s.clone()),
        JsonValue::Array(arr) => Value::List(arr.iter().map(json_value_to_glue_value).collect()),
        JsonValue::Object(obj) => {
            let map: BTreeMap<String, Value> = obj
                .iter()
                .map(|(k, v)| (k.clone(), json_value_to_glue_value(v)))
                .collect();
            Value::Map(map)
        }
    }
}

/// Convert a vector of rows to a RowIter (pinned boxed stream)
fn rows_to_iter(rows: Vec<(Key, DataRow)>) -> RowIter<'static> {
    let stream = stream::iter(rows.into_iter().map(Ok));
    Box::pin(stream)
}

// Implement the Store trait
#[async_trait]
impl Store for CompositeStorage {
    async fn fetch_schema(&self, table_name: &str) -> Result<Option<Schema>> {
        match table_name {
            "jhistory" => Ok(Some(self.jhistory_schema())),
            "codex_history" => Ok(Some(self.codex_history_alias_schema())),
            "transcripts" => Ok(Some(self.transcripts_schema())),
            "sessions" => Ok(Some(self.sessions_schema())),
            "todos" => Ok(Some(self.todos_schema())),
            _ => self.json_storage.fetch_schema(table_name).await,
        }
    }

    async fn fetch_all_schemas(&self) -> Result<Vec<Schema>> {
        let mut schemas = self.json_storage.fetch_all_schemas().await?;

        if self.config.jhistory_file().exists() {
            schemas.push(self.jhistory_schema());
            schemas.push(self.codex_history_alias_schema());
        }
        if self.config.transcripts_dir().exists() || self.config.projects_dir().exists() {
            schemas.push(self.transcripts_schema());
            schemas.push(self.sessions_schema());
        }
        if self.config.todos_dir().exists() {
            schemas.push(self.todos_schema());
        }

        Ok(schemas)
    }

    async fn fetch_data(&self, table_name: &str, key: &Key) -> Result<Option<DataRow>> {
        if self.is_virtual_table(table_name) {
            let rows = match table_name {
                "jhistory" | "codex_history" => self.scan_jhistory()?,
                "transcripts" => self.scan_transcripts()?,
                "sessions" => self.scan_sessions()?,
                "todos" => self.scan_todos()?,
                _ => return Ok(None),
            };

            for (k, row) in rows {
                if &k == key {
                    return Ok(Some(row));
                }
            }
            Ok(None)
        } else {
            self.json_storage.fetch_data(table_name, key).await
        }
    }

    async fn scan_data(&self, table_name: &str) -> Result<RowIter<'_>> {
        if self.is_virtual_table(table_name) {
            let rows = match table_name {
                "jhistory" | "codex_history" => self.scan_jhistory()?,
                "transcripts" => self.scan_transcripts()?,
                "sessions" => self.scan_sessions()?,
                "todos" => self.scan_todos()?,
                _ => Vec::new(),
            };

            Ok(rows_to_iter(rows))
        } else {
            self.json_storage.scan_data(table_name).await
        }
    }
}

// Implement StoreMut (delegate to JsonStorage for non-virtual tables)
#[async_trait]
impl StoreMut for CompositeStorage {
    async fn insert_schema(&mut self, schema: &Schema) -> Result<()> {
        if self.is_virtual_table(&schema.table_name) {
            Err(GlueError::StorageMsg(
                "Cannot create schema for virtual table".to_string(),
            ))
        } else {
            self.json_storage.insert_schema(schema).await
        }
    }

    async fn delete_schema(&mut self, table_name: &str) -> Result<()> {
        if self.is_virtual_table(table_name) {
            Err(GlueError::StorageMsg(
                "Cannot delete virtual table schema".to_string(),
            ))
        } else {
            self.json_storage.delete_schema(table_name).await
        }
    }

    async fn append_data(&mut self, table_name: &str, rows: Vec<DataRow>) -> Result<()> {
        if self.is_virtual_table(table_name) {
            Err(GlueError::StorageMsg(
                "Write operations on virtual multi-file tables not yet supported".to_string(),
            ))
        } else {
            self.json_storage.append_data(table_name, rows).await
        }
    }

    async fn insert_data(&mut self, table_name: &str, rows: Vec<(Key, DataRow)>) -> Result<()> {
        if self.is_virtual_table(table_name) {
            Err(GlueError::StorageMsg(
                "Write operations on virtual multi-file tables not yet supported".to_string(),
            ))
        } else {
            self.json_storage.insert_data(table_name, rows).await
        }
    }

    async fn delete_data(&mut self, table_name: &str, keys: Vec<Key>) -> Result<()> {
        if self.is_virtual_table(table_name) {
            Err(GlueError::StorageMsg(
                "Write operations on virtual multi-file tables not yet supported".to_string(),
            ))
        } else {
            self.json_storage.delete_data(table_name, keys).await
        }
    }
}

// Implement Metadata (delegate to JsonStorage)
#[async_trait]
impl Metadata for CompositeStorage {}

// Implement Index (delegate to JsonStorage)
#[async_trait]
impl Index for CompositeStorage {
    async fn scan_indexed_data(
        &self,
        table_name: &str,
        index_name: &str,
        asc: Option<bool>,
        cmp_value: Option<(&IndexOperator, Value)>,
    ) -> Result<RowIter<'_>> {
        if self.is_virtual_table(table_name) {
            // Virtual tables don't support indexes, fall back to full scan
            self.scan_data(table_name).await
        } else {
            self.json_storage
                .scan_indexed_data(table_name, index_name, asc, cmp_value)
                .await
        }
    }
}

// Implement IndexMut (delegate to JsonStorage)
#[async_trait]
impl IndexMut for CompositeStorage {
    async fn create_index(
        &mut self,
        table_name: &str,
        index_name: &str,
        column: &OrderByExpr,
    ) -> Result<()> {
        if self.is_virtual_table(table_name) {
            Err(GlueError::StorageMsg(
                "Cannot create index on virtual table".to_string(),
            ))
        } else {
            self.json_storage
                .create_index(table_name, index_name, column)
                .await
        }
    }

    async fn drop_index(&mut self, table_name: &str, index_name: &str) -> Result<()> {
        if self.is_virtual_table(table_name) {
            Err(GlueError::StorageMsg(
                "Cannot drop index on virtual table".to_string(),
            ))
        } else {
            self.json_storage.drop_index(table_name, index_name).await
        }
    }
}

// Implement AlterTable (delegate to JsonStorage)
#[async_trait]
impl AlterTable for CompositeStorage {
    async fn rename_schema(&mut self, table_name: &str, new_table_name: &str) -> Result<()> {
        if self.is_virtual_table(table_name) || self.is_virtual_table(new_table_name) {
            Err(GlueError::StorageMsg(
                "Cannot rename virtual table".to_string(),
            ))
        } else {
            self.json_storage
                .rename_schema(table_name, new_table_name)
                .await
        }
    }

    async fn rename_column(
        &mut self,
        table_name: &str,
        old_column_name: &str,
        new_column_name: &str,
    ) -> Result<()> {
        if self.is_virtual_table(table_name) {
            Err(GlueError::StorageMsg(
                "Cannot alter virtual table".to_string(),
            ))
        } else {
            self.json_storage
                .rename_column(table_name, old_column_name, new_column_name)
                .await
        }
    }

    async fn add_column(&mut self, table_name: &str, column_def: &ColumnDef) -> Result<()> {
        if self.is_virtual_table(table_name) {
            Err(GlueError::StorageMsg(
                "Cannot alter virtual table".to_string(),
            ))
        } else {
            self.json_storage.add_column(table_name, column_def).await
        }
    }

    async fn drop_column(
        &mut self,
        table_name: &str,
        column_name: &str,
        if_exists: bool,
    ) -> Result<()> {
        if self.is_virtual_table(table_name) {
            Err(GlueError::StorageMsg(
                "Cannot alter virtual table".to_string(),
            ))
        } else {
            self.json_storage
                .drop_column(table_name, column_name, if_exists)
                .await
        }
    }
}

// Implement Transaction (delegate to JsonStorage)
#[async_trait]
impl Transaction for CompositeStorage {
    async fn begin(&mut self, autocommit: bool) -> Result<bool> {
        self.json_storage.begin(autocommit).await
    }

    async fn rollback(&mut self) -> Result<()> {
        self.json_storage.rollback().await
    }

    async fn commit(&mut self) -> Result<()> {
        self.json_storage.commit().await
    }
}

// Implement CustomFunction (delegate to JsonStorage)
#[async_trait]
impl CustomFunction for CompositeStorage {
    async fn fetch_function(&self, func_name: &str) -> Result<Option<&StructCustomFunction>> {
        self.json_storage.fetch_function(func_name).await
    }

    async fn fetch_all_functions(&self) -> Result<Vec<&StructCustomFunction>> {
        self.json_storage.fetch_all_functions().await
    }
}

// Implement CustomFunctionMut (delegate to JsonStorage)
#[async_trait]
impl CustomFunctionMut for CompositeStorage {
    async fn insert_function(&mut self, func: StructCustomFunction) -> Result<()> {
        self.json_storage.insert_function(func).await
    }

    async fn delete_function(&mut self, func_name: &str) -> Result<()> {
        self.json_storage.delete_function(func_name).await
    }
}

// Implement Planner (uses default impl which delegates to Store)
#[async_trait]
impl Planner for CompositeStorage {}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::config::Config;

    fn write(path: &std::path::Path, contents: &str) {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).expect("mkdir");
        }
        fs::write(path, contents).expect("write");
    }

    fn config_for(dir: &std::path::Path) -> Config {
        Config::new_with_codex_data_dir(dir.to_path_buf(), dir.join("codex")).expect("config")
    }

    #[tokio::test]
    async fn transcripts_scan_reads_projects_layout_and_legacy() {
        let temp = tempfile::tempdir().expect("temp");
        let root = temp.path();

        // Top-level session under projects/<slug>/<sessionId>.jsonl
        write(
            &root
                .join("projects")
                .join("-Users-douglance-Developer-lv-devsql")
                .join("sess-top.jsonl"),
            r#"{"type":"user","content":"hi"}"#,
        );
        // Subagent transcript under projects/<slug>/<sessionId>/subagents/agent-<id>.jsonl
        write(
            &root
                .join("projects")
                .join("-Users-douglance-Developer-lv-devsql")
                .join("sess-parent")
                .join("subagents")
                .join("agent-abc123.jsonl"),
            r#"{"type":"assistant","content":"sub"}"#,
        );
        // Legacy flat transcript
        write(
            &root.join("transcripts").join("ses_legacy1.jsonl"),
            r#"{"type":"user","content":"old"}"#,
        );

        let storage = CompositeStorage::new(config_for(root)).expect("storage");
        let rows = storage.scan_transcripts().expect("scan");
        assert_eq!(rows.len(), 3, "expected 3 rows across both layouts");
    }

    #[tokio::test]
    async fn transcripts_metadata_columns() {
        let temp = tempfile::tempdir().expect("temp");
        let root = temp.path();
        let slug = "-Users-douglance-Developer-lv-devsql";

        write(
            &root.join("projects").join(slug).join("sess-top.jsonl"),
            r#"{"type":"user","content":"hi"}"#,
        );
        write(
            &root
                .join("projects")
                .join(slug)
                .join("sess-parent")
                .join("subagents")
                .join("agent-abc123.jsonl"),
            r#"{"type":"assistant","content":"sub"}"#,
        );
        write(
            &root.join("transcripts").join("ses_legacy1.jsonl"),
            r#"{"type":"user","content":"old"}"#,
        );

        let storage = CompositeStorage::new(config_for(root)).expect("storage");
        let rows = storage.scan_transcripts().expect("scan");

        let get = |needle: &str| -> &BTreeMap<String, Value> {
            rows.iter()
                .find_map(|(_, r)| match r {
                    DataRow::Map(m) => {
                        if matches!(m.get("_source_file"), Some(Value::Str(s)) if s == needle) {
                            Some(m)
                        } else {
                            None
                        }
                    }
                    _ => None,
                })
                .unwrap_or_else(|| panic!("row {} not found", needle))
        };

        let top = get("sess-top.jsonl");
        assert_eq!(top.get("_session_id"), Some(&Value::Str("sess-top".into())));
        assert_eq!(top.get("_project"), Some(&Value::Str(slug.into())));
        assert!(top.get("_agent_id").is_none());

        let sub = get("agent-abc123.jsonl");
        assert_eq!(
            sub.get("_session_id"),
            Some(&Value::Str("sess-parent".into())),
            "subagent session id is parent dir name"
        );
        assert_eq!(sub.get("_project"), Some(&Value::Str(slug.into())));
        assert_eq!(
            sub.get("_agent_id"),
            Some(&Value::Str("agent-abc123".into()))
        );

        let legacy = get("ses_legacy1.jsonl");
        assert_eq!(
            legacy.get("_session_id"),
            Some(&Value::Str("legacy1".into())),
            "legacy ses_ prefix stripped"
        );
        assert!(legacy.get("_project").is_none());
        assert!(legacy.get("_agent_id").is_none());
    }

    #[tokio::test]
    async fn transcripts_preserve_all_keys_and_cache_tokens_queryable() {
        use crate::sql::{SqlEngine, SqlOptions};
        let temp = tempfile::tempdir().expect("temp");
        let root = temp.path();
        let slug = "-Users-douglance-Developer-lv-devsql";

        let assistant = r#"{"type":"assistant","message":{"role":"assistant","usage":{"input_tokens":6,"output_tokens":127,"cache_read_input_tokens":26378,"cache_creation_input_tokens":26449,"cache_creation":{"ephemeral_5m_input_tokens":0,"ephemeral_1h_input_tokens":26449}}}}"#;
        let pr_link = r#"{"type":"pr-link","url":"https://example.com/pr/1"}"#;
        write(
            &root.join("projects").join(slug).join("sess-top.jsonl"),
            &format!("{}\n{}\n", assistant, pr_link),
        );

        let mut engine = SqlEngine::new(config_for(root), SqlOptions::default()).expect("engine");

        // pr-link record round-trips
        let pr = engine
            .execute("SELECT url FROM transcripts WHERE type = 'pr-link'")
            .await
            .expect("pr query");
        assert_eq!(pr.len(), 1);
        assert_eq!(pr[0]["url"], serde_json::json!("https://example.com/pr/1"));

        // cache_read_input_tokens queryable via native nested Map access (UNWRAP dotted path)
        let cache = engine
            .execute(
                "SELECT UNWRAP(message, 'usage.cache_read_input_tokens') AS crit, \
                 UNWRAP(message, 'usage.cache_creation.ephemeral_5m_input_tokens') AS eph \
                 FROM transcripts WHERE type = 'assistant'",
            )
            .await
            .expect("cache query");
        assert_eq!(cache.len(), 1);
        assert_eq!(cache[0]["crit"], serde_json::json!(26378));
        assert_eq!(cache[0]["eph"], serde_json::json!(0));
    }

    #[tokio::test]
    async fn transcripts_flatten_usage_columns_for_assistant_rows() {
        let temp = tempfile::tempdir().expect("temp");
        let root = temp.path();
        let slug = "-Users-douglance-Developer-lv-devsql";

        let assistant = r#"{"type":"assistant","message":{"model":"claude-opus-4-7","usage":{"input_tokens":6,"output_tokens":127,"cache_read_input_tokens":26378,"cache_creation_input_tokens":26449,"service_tier":"standard","cache_creation":{"ephemeral_5m_input_tokens":11,"ephemeral_1h_input_tokens":26449}}}}"#;
        write(
            &root.join("projects").join(slug).join("sess-top.jsonl"),
            assistant,
        );

        let storage = CompositeStorage::new(config_for(root)).expect("storage");
        let rows = storage.scan_transcripts().expect("scan");
        assert_eq!(rows.len(), 1);
        let DataRow::Map(map) = &rows[0].1 else {
            panic!("expected map row");
        };

        assert_eq!(
            map.get("model"),
            Some(&Value::Str("claude-opus-4-7".into()))
        );
        assert_eq!(map.get("usage_input_tokens"), Some(&Value::I64(6)));
        assert_eq!(map.get("usage_output_tokens"), Some(&Value::I64(127)));
        assert_eq!(
            map.get("usage_cache_read_input_tokens"),
            Some(&Value::I64(26378))
        );
        assert_eq!(
            map.get("usage_cache_creation_input_tokens"),
            Some(&Value::I64(26449))
        );
        assert_eq!(
            map.get("usage_ephemeral_5m_input_tokens"),
            Some(&Value::I64(11))
        );
        assert_eq!(
            map.get("usage_ephemeral_1h_input_tokens"),
            Some(&Value::I64(26449))
        );
        assert_eq!(
            map.get("usage_service_tier"),
            Some(&Value::Str("standard".into()))
        );
        // Full message Map column is preserved as-is
        assert!(matches!(map.get("message"), Some(Value::Map(_))));
    }

    #[tokio::test]
    async fn transcripts_no_usage_columns_when_absent() {
        let temp = tempfile::tempdir().expect("temp");
        let root = temp.path();
        write(
            &root.join("projects").join("slug").join("s1.jsonl"),
            r#"{"type":"user","content":"hi"}"#,
        );

        let storage = CompositeStorage::new(config_for(root)).expect("storage");
        let rows = storage.scan_transcripts().expect("scan");
        let DataRow::Map(map) = &rows[0].1 else {
            panic!("expected map row");
        };
        assert!(map.get("model").is_none());
        assert!(map.get("usage_input_tokens").is_none());
        assert!(map.get("usage_service_tier").is_none());
    }

    #[tokio::test]
    async fn transcripts_flattened_columns_never_overwrite_existing_keys() {
        let temp = tempfile::tempdir().expect("temp");
        let root = temp.path();

        // Record already carries top-level "model" and "usage_input_tokens" keys.
        let record = r#"{"type":"assistant","model":"top-level-wins","usage_input_tokens":"original","message":{"model":"claude-opus-4-7","usage":{"input_tokens":6,"output_tokens":1}}}"#;
        write(&root.join("projects").join("slug").join("s1.jsonl"), record);

        let storage = CompositeStorage::new(config_for(root)).expect("storage");
        let rows = storage.scan_transcripts().expect("scan");
        let DataRow::Map(map) = &rows[0].1 else {
            panic!("expected map row");
        };
        assert_eq!(
            map.get("model"),
            Some(&Value::Str("top-level-wins".into())),
            "existing top-level JSON key must not be overwritten"
        );
        assert_eq!(
            map.get("usage_input_tokens"),
            Some(&Value::Str("original".into())),
            "existing top-level JSON key must not be overwritten"
        );
        // Non-colliding flattened column still appears
        assert_eq!(map.get("usage_output_tokens"), Some(&Value::I64(1)));
    }

    #[tokio::test]
    async fn sessions_table_aggregates_per_session() {
        use crate::sql::{SqlEngine, SqlOptions};
        let temp = tempfile::tempdir().expect("temp");
        let root = temp.path();
        let slug = "-Users-douglance-Developer-lv-devsql";

        // Rich session: 2 user msgs, 2 assistant msgs, ai-title, pr-link, 2 subagents
        let rich = [
            r#"{"type":"user","content":"hi","timestamp":"2026-06-01T10:00:00.000Z","cwd":"/Users/douglance/Developer/lv/devsql","gitBranch":"main","version":"2.1.100"}"#,
            r#"{"type":"assistant","timestamp":"2026-06-01T10:00:05.000Z","version":"2.1.100","message":{"model":"claude-opus-4-7","usage":{"input_tokens":6,"output_tokens":127,"cache_read_input_tokens":100,"cache_creation_input_tokens":200}}}"#,
            r#"{"type":"ai-title","aiTitle":"Fix the widget","sessionId":"sess-rich"}"#,
            r#"{"type":"pr-link","sessionId":"sess-rich","prNumber":42,"prUrl":"https://github.com/org/repo/pull/42","timestamp":"2026-06-01T10:01:00.000Z"}"#,
            r#"{"type":"user","content":"more","timestamp":"2026-06-01T10:02:00.000Z","cwd":"/elsewhere","gitBranch":"other","version":"2.1.101"}"#,
            r#"{"type":"assistant","timestamp":"2026-06-01T10:02:30.000Z","version":"2.1.101","message":{"model":"claude-opus-4-7","usage":{"input_tokens":4,"output_tokens":3,"cache_read_input_tokens":50,"cache_creation_input_tokens":25}}}"#,
        ]
        .join("\n");
        write(
            &root.join("projects").join(slug).join("sess-rich.jsonl"),
            &rich,
        );
        // Subagent files (their tokens must NOT count toward the session totals)
        let sub = r#"{"type":"assistant","timestamp":"2026-06-01T10:03:00.000Z","message":{"usage":{"input_tokens":999,"output_tokens":999,"cache_read_input_tokens":999,"cache_creation_input_tokens":999}}}"#;
        write(
            &root
                .join("projects")
                .join(slug)
                .join("sess-rich")
                .join("subagents")
                .join("agent-one.jsonl"),
            sub,
        );
        write(
            &root
                .join("projects")
                .join(slug)
                .join("sess-rich")
                .join("subagents")
                .join("agent-two.jsonl"),
            sub,
        );
        // Minimal legacy flat session
        write(
            &root.join("transcripts").join("ses_old.jsonl"),
            r#"{"type":"user","content":"legacy","timestamp":"2025-01-01T00:00:00.000Z"}"#,
        );

        let mut engine = SqlEngine::new(config_for(root), SqlOptions::default()).expect("engine");

        let rows = engine
            .execute("SELECT * FROM sessions ORDER BY session_id")
            .await
            .expect("sessions query");
        assert_eq!(rows.len(), 2, "one row per session file");

        let legacy = &rows[0];
        assert_eq!(legacy["session_id"], serde_json::json!("old"));
        assert_eq!(legacy["project"], serde_json::Value::Null);
        assert_eq!(legacy["user_message_count"], serde_json::json!(1));
        assert_eq!(legacy["assistant_message_count"], serde_json::json!(0));
        assert_eq!(legacy["subagent_count"], serde_json::json!(0));
        assert_eq!(legacy["total_input_tokens"], serde_json::json!(0));

        let rich = &rows[1];
        assert_eq!(rich["session_id"], serde_json::json!("sess-rich"));
        assert_eq!(rich["project"], serde_json::json!(slug));
        assert_eq!(
            rich["cwd"],
            serde_json::json!("/Users/douglance/Developer/lv/devsql"),
            "cwd is first seen"
        );
        assert_eq!(rich["git_branch"], serde_json::json!("main"), "first seen");
        assert_eq!(rich["version"], serde_json::json!("2.1.101"), "last seen");
        assert_eq!(rich["title"], serde_json::json!("Fix the widget"));
        assert_eq!(
            rich["first_timestamp"],
            serde_json::json!("2026-06-01T10:00:00.000Z")
        );
        assert_eq!(
            rich["last_timestamp"],
            serde_json::json!("2026-06-01T10:02:30.000Z")
        );
        assert_eq!(rich["user_message_count"], serde_json::json!(2));
        assert_eq!(rich["assistant_message_count"], serde_json::json!(2));
        assert_eq!(rich["subagent_count"], serde_json::json!(2));
        assert_eq!(rich["total_input_tokens"], serde_json::json!(10));
        assert_eq!(rich["total_output_tokens"], serde_json::json!(130));
        assert_eq!(rich["total_cache_read_input_tokens"], serde_json::json!(150));
        assert_eq!(
            rich["total_cache_creation_input_tokens"],
            serde_json::json!(225)
        );
        assert_eq!(
            rich["pr_url"],
            serde_json::json!("https://github.com/org/repo/pull/42")
        );
        assert_eq!(rich["pr_number"], serde_json::json!(42));
    }

    #[test]
    fn test_parse_todo_filename() {
        let (workspace, agent) = parse_todo_filename("abc123-agent-def456.json");
        assert_eq!(workspace, "abc123");
        assert_eq!(agent, "def456");

        let (workspace, agent) = parse_todo_filename("simple.json");
        assert_eq!(workspace, "simple");
        assert_eq!(agent, "unknown");
    }

    #[test]
    fn test_json_value_to_glue_value() {
        assert_eq!(
            json_value_to_glue_value(&JsonValue::String("test".to_string())),
            Value::Str("test".to_string())
        );
        assert_eq!(
            json_value_to_glue_value(&JsonValue::Bool(true)),
            Value::Bool(true)
        );
        assert_eq!(
            json_value_to_glue_value(&serde_json::json!(42)),
            Value::I64(42)
        );
    }

    #[test]
    fn test_jhistory_json_to_data_row() {
        let json = serde_json::json!({
            "session_id": "abc123",
            "ts": 1754402102,
            "text": "hello codex"
        });

        let Some(DataRow::Map(ref map)) = jhistory_json_to_data_row(&json) else {
            panic!("expected jhistory row");
        };

        assert_eq!(
            map.get("display"),
            Some(&Value::Str("hello codex".to_string()))
        );
        assert_eq!(map.get("ts"), Some(&Value::I64(1754402102)));
        assert_eq!(map.get("timestamp"), Some(&Value::I64(1_754_402_102_000)));
    }

    #[test]
    fn test_jhistory_json_to_data_row_with_string_numbers() {
        let json = serde_json::json!({
            "session_id": "abc123",
            "ts": "1754402102",
            "text": "hello codex"
        });

        let Some(DataRow::Map(ref map)) = jhistory_json_to_data_row(&json) else {
            panic!("expected jhistory row");
        };

        assert_eq!(map.get("ts"), Some(&Value::I64(1_754_402_102)));
        assert_eq!(map.get("timestamp"), Some(&Value::I64(1_754_402_102_000)));
    }

    #[test]
    fn test_normalize_ts_seconds() {
        assert_eq!(normalize_ts_seconds(1_754_402_102), 1_754_402_102);
        assert_eq!(normalize_ts_seconds(1_754_402_102_000), 1_754_402_102);
    }
}
