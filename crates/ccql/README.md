# ccql

**Claude Code Query Language** - SQL query engine for Claude Code and Codex CLI data.

## Installation

### Homebrew (macOS/Linux)

```bash
brew install douglance/tap/ccql
```

### Shell script (macOS/Linux)

```bash
curl --proto '=https' --tlsv1.2 -LsSf https://github.com/douglance/ccql/releases/latest/download/ccql-installer.sh | sh
```

### PowerShell (Windows)

```powershell
powershell -ExecutionPolicy ByPass -c "irm https://github.com/douglance/ccql/releases/latest/download/ccql-installer.ps1 | iex"
```

### npm

```bash
npm install -g ccql
```

### Cargo

```bash
cargo install ccql
```

### From source

```bash
git clone https://github.com/douglance/ccql
cd ccql
cargo install --path .
```

## Quick Start

```bash
# SQL is the default command - just pass a query
ccql "SELECT display FROM history ORDER BY timestamp DESC LIMIT 5"
ccql "SELECT session_id, text FROM jhistory ORDER BY ts DESC LIMIT 5"
ccql "SELECT tool_name, COUNT(*) as n FROM transcripts WHERE type='tool_use' GROUP BY tool_name"
ccql "SELECT content FROM todos WHERE status='pending'"

# Get help
ccql -h              # Quick reference
ccql --help          # Full documentation
ccql tables          # Show table schemas
ccql examples        # Show query examples
```

## Tables

| Table | Source | Description |
|-------|--------|-------------|
| `history` | `history.jsonl` | User prompts |
| `jhistory` | `~/.codex/history.jsonl` | Codex CLI prompt history (virtual) |
| `codex_history` | Alias of `jhistory` | Codex CLI prompt history (virtual) |
| `transcripts` | `projects/<slug>/**/*.jsonl` (+ legacy `transcripts/*.jsonl`) | Conversation logs (virtual) |
| `sessions` | Same files as `transcripts` | One aggregated row per session (virtual) |
| `todos` | `todos/*.json` | Task items (virtual) |

### history

| Column | Type | Description |
|--------|------|-------------|
| `display` | TEXT | The prompt text |
| `timestamp` | INTEGER | Unix timestamp (ms) |
| `project` | TEXT | Project directory |
| `pastedContents` | OBJECT | Pasted content (JSON) |

### jhistory

| Column | Type | Description |
|--------|------|-------------|
| `session_id` | TEXT | Codex session ID |
| `ts` | INTEGER | Raw Unix timestamp (seconds) |
| `text` | TEXT | Raw prompt text |
| `display` | TEXT | Prompt text (normalized alias for `text`) |
| `timestamp` | INTEGER | Unix timestamp (milliseconds) |

`codex_history` exposes the exact same columns as `jhistory`.

### transcripts

Sourced from the modern Claude Code layout, scanned recursively:

- `projects/<slug>/<sessionId>.jsonl` — top-level sessions
- `projects/<slug>/<sessionId>/subagents/agent-<id>.jsonl` — subagent transcripts
- `transcripts/*.jsonl` — legacy flat layout (still read for backward compatibility)

The table is schemaless: every top-level JSON key in each record becomes a
column, so newer fields (`message.usage.*`, `compactMetadata`, record `type`s
like `ai-title` / `pr-link` / `mode`, etc.) surface automatically. Nested
objects are exposed as GlueSQL `MAP` values — drill into them with
`UNWRAP(<column>, '<dotted.path>')`.

| Column | Type | Description |
|--------|------|-------------|
| `_source_file` | TEXT | Source file name (e.g. `<sessionId>.jsonl`, `agent-<id>.jsonl`, legacy `ses_xxx.jsonl`) |
| `_session_id` | TEXT | Session ID (parent session ID for subagent files; `ses_` prefix stripped for legacy) |
| `_project` | TEXT | Project slug directory name (e.g. `-Users-you-Developer-app`); NULL for legacy flat files |
| `_agent_id` | TEXT | Subagent file stem (e.g. `agent-abc123`); NULL for non-subagent rows |
| `type` | TEXT | Record type: `user`, `assistant`, `tool_use`, `tool_result`, `ai-title`, `pr-link`, `mode`, … |
| `timestamp` | TEXT | ISO 8601 timestamp |
| `message` | OBJECT | Full message object, including `message.usage.*` token/cache counts |
| `content` | TEXT | Message text (type='user') |
| `tool_name` | TEXT | Tool name (type='tool_*') |
| `tool_input` | OBJECT | Tool parameters |
| `tool_output` | OBJECT | Tool response (type='tool_result') |

Assistant rows additionally get flattened convenience columns (only added when
the source value exists; a top-level JSON key with the same name is never
overwritten):

| Column | Type | Source |
|--------|------|--------|
| `model` | TEXT | `message.model` |
| `usage_input_tokens` | INTEGER | `message.usage.input_tokens` |
| `usage_output_tokens` | INTEGER | `message.usage.output_tokens` |
| `usage_cache_read_input_tokens` | INTEGER | `message.usage.cache_read_input_tokens` |
| `usage_cache_creation_input_tokens` | INTEGER | `message.usage.cache_creation_input_tokens` |
| `usage_ephemeral_5m_input_tokens` | INTEGER | `message.usage.cache_creation.ephemeral_5m_input_tokens` |
| `usage_ephemeral_1h_input_tokens` | INTEGER | `message.usage.cache_creation.ephemeral_1h_input_tokens` |
| `usage_service_tier` | TEXT | `message.usage.service_tier` |

#### Querying cache / usage tokens

`message.usage` carries the token accounting. Use `UNWRAP` with a dotted path to
read nested values directly in SQL:

```sql
SELECT _project,
       _session_id,
       UNWRAP(message, 'usage.input_tokens')                          AS input_tokens,
       UNWRAP(message, 'usage.output_tokens')                         AS output_tokens,
       UNWRAP(message, 'usage.cache_read_input_tokens')               AS cache_read,
       UNWRAP(message, 'usage.cache_creation_input_tokens')           AS cache_creation,
       UNWRAP(message, 'usage.cache_creation.ephemeral_5m_input_tokens') AS ephemeral_5m
FROM transcripts
WHERE type = 'assistant'
  AND UNWRAP(message, 'usage.cache_read_input_tokens') IS NOT NULL;
```

### sessions

One row per top-level session file (subagent files contribute only to
`subagent_count`; legacy flat files get a row with NULL `project`). Read-only,
like the other virtual tables.

| Column | Type | Description |
|--------|------|-------------|
| `session_id` | TEXT | Session ID (file stem; `ses_` prefix stripped for legacy files) |
| `project` | TEXT | Project slug directory name; NULL for legacy flat files |
| `cwd` | TEXT | Working directory (first seen in the session) |
| `git_branch` | TEXT | Git branch (first seen) |
| `version` | TEXT | Claude Code version (last seen) |
| `title` | TEXT | AI-generated session title (from the `ai-title` record, if any) |
| `first_timestamp` | TEXT | Earliest record timestamp (ISO 8601) |
| `last_timestamp` | TEXT | Latest record timestamp (ISO 8601) |
| `user_message_count` | INTEGER | Number of `type='user'` records |
| `assistant_message_count` | INTEGER | Number of `type='assistant'` records |
| `subagent_count` | INTEGER | Number of subagent transcript files for this session |
| `total_input_tokens` | INTEGER | Sum of `message.usage.input_tokens` over assistant records |
| `total_output_tokens` | INTEGER | Sum of `message.usage.output_tokens` |
| `total_cache_read_input_tokens` | INTEGER | Sum of `message.usage.cache_read_input_tokens` |
| `total_cache_creation_input_tokens` | INTEGER | Sum of `message.usage.cache_creation_input_tokens` |
| `pr_url` | TEXT | Linked PR URL (from the `pr-link` record, if any) |
| `pr_number` | INTEGER | Linked PR number (if any) |

Token totals cover the top-level session file only (not subagent transcripts).

```sql
-- Top 10 sessions by cache-read tokens
SELECT title, project, total_cache_read_input_tokens, last_timestamp
FROM sessions
ORDER BY total_cache_read_input_tokens DESC
LIMIT 10;

-- Sessions that opened a PR
SELECT title, pr_number, pr_url
FROM sessions
WHERE pr_url IS NOT NULL
ORDER BY last_timestamp DESC;

-- Daily output token totals by model (via flattened transcript columns)
SELECT SUBSTR(timestamp, 1, 10) AS day,
       model,
       SUM(usage_output_tokens) AS output_tokens
FROM transcripts
WHERE type = 'assistant' AND usage_output_tokens IS NOT NULL
GROUP BY SUBSTR(timestamp, 1, 10), model
ORDER BY SUBSTR(timestamp, 1, 10) DESC;
```

### todos

| Column | Type | Description |
|--------|------|-------------|
| `_source_file` | TEXT | Source filename |
| `_workspace_id` | TEXT | Workspace ID |
| `_agent_id` | TEXT | Agent ID |
| `content` | TEXT | Todo description |
| `status` | TEXT | `pending` \| `in_progress` \| `completed` |
| `activeForm` | TEXT | Display text when active |

## Examples

### Filter by Current Project

Use the `project` column to limit queries to the folder you're working in:

```bash
# Only prompts from current project
ccql "SELECT display FROM history WHERE project = '$(pwd)' ORDER BY timestamp DESC LIMIT 10"

# Transcripts from current project (via session join)
ccql "SELECT t.tool_name, COUNT(*) as n FROM transcripts t
      JOIN history h ON t._session_id = h.session_id
      WHERE h.project = '$(pwd)' AND t.type='tool_use'
      GROUP BY t.tool_name ORDER BY n DESC"
```

### History Queries

```bash
# Recent prompts
ccql "SELECT display FROM history ORDER BY timestamp DESC LIMIT 10"

# Search prompts
ccql "SELECT display FROM history WHERE display LIKE '%error%'"

# Prompts by project
ccql "SELECT project, COUNT(*) as n FROM history GROUP BY project ORDER BY n DESC"

# Long prompts (likely pasted code)
ccql "SELECT LENGTH(display) as len, SUBSTR(display, 1, 60) as preview
      FROM history ORDER BY len DESC LIMIT 10"

# Recent Codex prompts
ccql "SELECT datetime(timestamp/1000, 'unixepoch') as time, display
      FROM jhistory ORDER BY timestamp DESC LIMIT 10"
```

### Transcript Queries

```bash
# Tool usage stats
ccql "SELECT tool_name, COUNT(*) as n FROM transcripts
      WHERE type='tool_use' GROUP BY tool_name ORDER BY n DESC"

# Most active sessions
ccql "SELECT _session_id, COUNT(*) as n FROM transcripts
      GROUP BY _session_id ORDER BY n DESC LIMIT 10"

# Find sessions mentioning a topic
ccql "SELECT DISTINCT _session_id FROM transcripts
      WHERE content LIKE '%authentication%'"
```

### Todo Queries

```bash
# Pending todos
ccql "SELECT content FROM todos WHERE status='pending'"

# Todo counts by status
ccql "SELECT status, COUNT(*) as n FROM todos GROUP BY status"
```

## Output Formats

```bash
ccql -f table "SELECT ..."    # Pretty table (default)
ccql -f json "SELECT ..."     # JSON array
ccql -f jsonl "SELECT ..."    # JSON lines
ccql -f raw "SELECT ..."      # Raw output
```

## Write Operations

Write operations require explicit flags for safety:

```bash
# Preview changes (dry run)
ccql --dry-run "DELETE FROM history WHERE timestamp < 1700000000000"

# Execute with backup
ccql --write "DELETE FROM history WHERE timestamp < 1700000000000"
```

## Other Commands

```bash
ccql prompts                  # Extract prompts with filtering
ccql sessions                 # List sessions
ccql search "term"            # Full-text search
ccql todos --status pending   # List todos
ccql stats                    # Usage statistics
ccql duplicates               # Find repeated prompts
ccql query '.[]' history      # jq-style queries
ccql query '.[]' jhistory     # jq-style queries for Codex prompts
ccql query '.[]' codex_history
```

## Configuration

```bash
# Set data directory
export CLAUDE_DATA_DIR=~/.claude
export CODEX_HOME=~/.codex

# Or via flag
ccql --data-dir ~/.claude "SELECT ..."
```

## License

MIT
