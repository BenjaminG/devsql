# DevSQL

**Query your AI coding history and codebase to become a better prompter.**

DevSQL lets you analyze your Claude Code and Codex CLI conversations alongside your Git commits and source code. Find your most productive prompts, identify patterns in successful coding sessions, search symbols across your codebase, and understand the impact of changes.

## Why?

Your `~/.claude/` and `~/.codex/` folders contain a goldmine of data: every prompt you've written, every tool used, every conversation that led to shipped code. DevSQL turns that—plus your Git history and source code—into queryable insights.

**Ask questions like:**
- "Which of my prompts led to the most commits?"
- "What patterns do my successful coding sessions have?"
- "When do I struggle most—and what prompts help me recover?"
- "Which tools does Claude use most when I'm productive?"
- "What symbols are defined in this file, and who imports them?"
- "What changed between these two commits at the symbol level?"

## What You Can Do

### Find Your Most Productive Prompts
```bash
devsql "SELECT h.display as prompt, COUNT(c.id) as commits_after
FROM history h
LEFT JOIN commits c ON DATE(datetime(h.timestamp/1000, 'unixepoch')) = DATE(c.authored_at)
GROUP BY h.display
HAVING commits_after > 0
ORDER BY commits_after DESC
LIMIT 20"
```

### Identify Struggle Sessions
```bash
devsql "SELECT
  DATE(datetime(h.timestamp/1000, 'unixepoch')) as day,
  COUNT(*) as prompts,
  COUNT(DISTINCT c.id) as commits,
  CAST(COUNT(*) AS FLOAT) / MAX(1, COUNT(DISTINCT c.id)) as struggle_ratio
FROM history h
LEFT JOIN commits c ON DATE(datetime(h.timestamp/1000, 'unixepoch')) = DATE(c.authored_at)
GROUP BY day
ORDER BY struggle_ratio DESC
LIMIT 10"
```

### Search Symbols Across Your Codebase
```bash
# Find all functions matching a pattern
devsql search "parse"

# Filter by kind
devsql search "Error" --kind struct

# SQL query for full control
devsql "SELECT name, kind, file_path, line_start
FROM symbols WHERE kind = 'function'
ORDER BY name LIMIT 20"
```

### Explore File Context
```bash
# Get file metadata and all symbols defined in it
devsql context src/engine.rs

# See what a file exports and who depends on it
devsql impact src/lib.rs
```

### Compare Commits with Semantic Diffs
```bash
# See what changed between two refs—files and symbols
devsql diff main~5 HEAD
```

### View File History
```bash
# Git history for a specific file with diff stats
devsql history src/engine.rs
```

### Analyze Your Prompting Patterns
```bash
ccql "SELECT tool_name, COUNT(*) as uses
FROM transcripts
WHERE type = 'tool_use'
GROUP BY tool_name
ORDER BY uses DESC"
```

### Train Your AI Agent
Tell Claude Code to query your history:

> "Use devsql to find my 10 most effective prompts from the past month—the ones that led to commits the same day. Then analyze what they have in common."

> "Query my Claude history to find sessions where I used many prompts but made few commits. What was I struggling with?"

> "Use devsql search to find all error handling functions in the codebase, then show me which files import them."

## Installation

### Claude Code Plugin (Recommended)

Install the plugin to use devsql directly within Claude Code:

```
/plugin marketplace add douglance/devsql
/plugin install devsql@devsql
```

Restart Claude Code to load the plugin. The plugin auto-installs the devsql binary on first use.

**Usage:**
- `/devsql:query SELECT * FROM history LIMIT 10` — Direct SQL queries
- `/devsql:query SELECT * FROM symbols WHERE kind = 'function'` — Code intelligence queries
- Or just ask Claude: "Show my most productive prompts from last week"

### Homebrew (macOS/Linux)
```bash
brew install douglance/tap/devsql
```

### Direct Download
Download from [GitHub Releases](https://github.com/douglance/devsql/releases) for macOS or Linux.

### Build from Source
```bash
git clone https://github.com/douglance/devsql.git
cd devsql && cargo install --path crates/devsql
```

To enable tree-sitter-based AST analysis (richer symbol extraction and import parsing):
```bash
cargo install --path crates/devsql --features tree-sitter-ast
```

## The Three Tools

| Tool | What It Queries |
|------|-----------------|
| `ccql` | Your Claude Code + Codex CLI data (`~/.claude/`, `~/.codex/`) |
| `vcsql` | Your Git repositories |
| `devsql` | All of the above—join conversations, commits, and code |

## Commands

### SQL Query (default)
```bash
devsql "<SQL>"              # Query any table with raw SQL
devsql -f json "<SQL>"      # Output as JSON
devsql -f csv "<SQL>"       # Output as CSV
```

### Agent Tool Commands

These structured commands return JSON, designed for use by AI agents:

| Command | Description |
|---------|-------------|
| `devsql search <query>` | Find symbols by name across the codebase |
| `devsql context <file>` | Get file metadata and symbols for a given path |
| `devsql history <file>` | Show Git commit history for a specific file |
| `devsql diff <base> <head>` | Compare two Git refs with file-level and symbol-level stats |
| `devsql impact <file>` | Analyze a file's exports and find potential dependents |

Options common to all commands: `--repo` / `-r` (repo path, default `.`), `--data-dir` / `-d` (Claude data dir, default `~/.claude`).

## Available Tables

**Claude Code**: `history` (your prompts), `transcripts` (full conversations), `todos` (tasks)

**Codex CLI**: `jhistory` (prompt history from `history.jsonl`), `codex_history` (alias)

**Git**: `commits`, `branches`, `diffs` (commit-level stats), `diff_files` (per-file stats with status)

**Code** (source analysis): `source_files` (file inventory), `source_lines` (line-level content), `symbols` (functions, classes, structs, etc.), `imports`\*, `ast_nodes`\*

\* Requires the `tree-sitter-ast` feature for full extraction. Without it, `symbols` uses regex-based extraction (Rust, TypeScript, JavaScript, Python, Go) and `imports`/`ast_nodes` are empty.

### Code Table Schemas

| Table | Key Columns |
|-------|------------|
| `source_files` | `path`, `name`, `extension`, `directory`, `size_bytes`, `line_count`, `modified_at`, `language` |
| `source_lines` | `file_path`, `line_number`, `content`, `is_blank` |
| `symbols` | `file_path`, `name`, `kind`, `line_start`, `line_end`, `signature`, `visibility`, `parameters`, `return_type`, `language` |
| `imports` | `file_path`, `line_number`, `module`, `name`, `alias`, `kind`, `is_default`, `is_wildcard` |

## License

MIT
