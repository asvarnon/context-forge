# Claude Code Integration

Context Forge integrates with [Claude Code](https://code.claude.com/) via its CLI hooks system, persisting context across sessions without VS Code.

## Prerequisites

- Claude Code installed and working (`claude` command available)
- `cf` binary on your `PATH`

## Install the CLI

Download the latest `cf` binary from [GitHub Releases](https://github.com/asvarnon/context-forge/releases) for your platform:

| Platform | Binary |
|----------|--------|
| Linux x64 | `cf-linux-x64` |
| macOS ARM | `cf-darwin-arm64` |
| Windows x64 | `cf-windows-x64.exe` |

```bash
# Example: Linux/macOS
chmod +x cf-linux-x64
sudo mv cf-linux-x64 /usr/local/bin/cf

# Verify
cf --version
```

On Windows, rename to `cf.exe` and add its directory to `PATH`.

## Configure Hooks

Copy the hook configuration into your Claude Code settings. You can use either:

- **Global**: `~/.claude/settings.json` (applies to all projects)
- **Project**: `.claude/settings.json` (checked into repo)
- **Local**: `.claude/settings.local.json` (gitignored, per-machine)

Add the following to your chosen settings file:

```json
{
  "hooks": {
    "PreCompact": [
      {
        "matcher": "",
        "hooks": [
          {
            "type": "command",
            "command": "cf pre-compact",
            "timeout": 10000
          }
        ]
      }
    ],
    "PostCompact": [
      {
        "matcher": "",
        "hooks": [
          {
            "type": "command",
            "command": "cf save --kind auto",
            "timeout": 10000
          }
        ]
      }
    ],
    "SessionStart": [
      {
        "matcher": "compact",
        "hooks": [
          {
            "type": "command",
            "command": "cf query --format text --source compact",
            "timeout": 15000
          }
        ]
      },
      {
        "matcher": "",
        "hooks": [
          {
            "type": "command",
            "command": "cf query --format text --source startup",
            "timeout": 15000
          }
        ]
      }
    ]
  }
}
```

> **Tip**: A ready-to-use template is in [`docs/claude-code-hooks.json`](claude-code-hooks.json).

## How It Works

| Hook | When | What happens |
|------|------|-------------|
| **PreCompact** | Before Claude compacts the conversation | Full transcript is piped via stdin to `cf pre-compact`, saved to SQLite |
| **PostCompact** | After compaction | JSON with `compact_summary` is piped to `cf save`, summary extracted and saved |
| **SessionStart** | When a new Claude Code session starts | `cf query` outputs previous context to stdout. With `--source`, an importance block is prepended before BM25 results — passages that recur frequently across sessions, ranked by importance score. Post-compaction sessions (`source=compact`) use progressive injection: the importance budget scales up based on compaction depth to fight context drift. |

The database defaults to `~/.context-forge/context.db`. The directory is created automatically on first use.

## Verify Setup

```bash
# Check the database (creates it if needed)
cf info

# Manually save a test entry
echo "test context entry" | cf save

# Query it back
cf query --format text --top-k 5

# Clean up test data
cf clear
```

## Query Filter

The `cf query` command accepts an optional `--query` flag to filter entries by FTS5 full-text search:

```bash
# Return only entries matching "security"
cf query --query "security" --format text

# FTS5 syntax: AND, OR, NOT, NEAR, quoted phrases
cf query --query "security AND hardening" --format text
```

Multi-word queries without explicit FTS5 operators are automatically expanded with OR — `security hardening` becomes `security OR hardening`.

Omit `--query` to return all entries ranked by recency (the default behavior).

## Configuration File

You can set defaults for `cf query` in an optional TOML config file at `~/.context-forge/config.toml`:

```toml
token_budget = 16000
top_k = 10
recency_half_life_hours = 72.0
```

| Key | Type | Default | Description |
|-----|------|---------|-------------|
| `token_budget` | integer | 16000 | Max tokens to assemble |
| `top_k` | integer | 10 | Max entries to consider |
| `recency_half_life_hours` | float | 72.0 | Recency decay half-life in hours |

For options that have corresponding CLI flags (`--token-budget`, `--top-k`), precedence is: CLI flags > config file > compile-time defaults. `recency_half_life_hours` is config-file only. The config file is created manually — Context Forge does not write to it.

## Token Budget

The `cf query` command assembles context entries into a token budget — the maximum number of tokens to return. Entries are ranked by BM25 relevance and recency, then greedily packed until the budget is exhausted.

The default budget is **16,000 tokens**. To increase it:

```bash
# In your hook config
"command": "cf query --format text --top-k 10 --token-budget 32000"

# Or manually
cf query --format text --token-budget 32000
```

Larger budgets retrieve more context but consume more of Claude's context window. A budget of 16,000–32,000 tokens works well for most workflows.

## Importance Detection

Context Forge analyzes your conversation history to surface high-value passages that recur frequently across sessions. These are injected as a dedicated block before BM25 results at `SessionStart`.

### How It Works

The importance pipeline runs at query time and identifies four categories of passages:

| Category | What it captures |
|---|---|
| **Corrective** | Things the model was told NOT to do — prevents repeated mistakes |
| **Stateful** | Named values and settings that change over time — always injects the latest |
| **Decisive** | Design decisions with explicit reasoning — preserves architectural choices |
| **Reinforcing** | Patterns confirmed across multiple sessions — behavioral anchors |

### Hook Setup

Use two `SessionStart` hooks to branch on trigger type:

```json
"SessionStart": [
  {
    "matcher": "compact",
    "hooks": [
      {
        "type": "command",
        "command": "cf query --format text --source compact",
        "timeout": 15000
      }
    ]
  },
  {
    "matcher": "",
    "hooks": [
      {
        "type": "command",
        "command": "cf query --format text --source startup",
        "timeout": 15000
      }
    ]
  }
]
```

The `matcher` field matches the `source` value Claude Code sends with `SessionStart`:

| Source | Matcher | Strategy |
|---|---|---|
| `startup` | `""` (catch-all) | Broad injection — default weights and budget |
| `resume` | `""` (catch-all) | Same as startup |
| `compact` | `"compact"` | Progressive injection — budget scales with compaction depth, weights shift toward reinforcing and stateful categories |
| `clear` | `""` (catch-all) | Same as startup (importance block will be empty after a clear anyway) |

### Importance Budget

The importance block uses a dedicated token budget separate from the BM25 budget. Use `--importance-budget` to control it:

```bash
# Default: 512 tokens for importance, remainder for BM25
cf query --format text --source startup

# Larger importance budget (useful for long-running projects)
cf query --format text --source startup --importance-budget 1024

# Post-compaction with larger base budget (scales further with compaction depth)
cf query --format text --source compact --importance-budget 1024
```

The total token budget (`--token-budget`, default 16,000) is split:
- Importance block: up to `--importance-budget` tokens (default 512)
- BM25 results: remaining tokens

For `source=compact`, the effective importance budget scales automatically based on how many times the session has been compacted — 25% more per compaction level (e.g., count=2 → 1.25×, count=3 → 1.5×).

### Output Format

The importance block is prepended to the BM25 output:

```
=== Important Context ===

[CORRECTIVE] (recurring across 4 sessions)
You should NOT use unwrap in library crates. Use Result with thiserror instead.

[DECISIVE] (recurring across 3 sessions)
Chose system OpenSSL over vendored because system OpenSSL gets security patches via dnf update. Set OPENSSL_NO_VENDOR=1 in ~/.bashrc.

---
<BM25 context entries>
```

When no importance data exists (empty store or fewer than 2 sessions), only the BM25 block is returned — identical to the pre-importance behavior.

### Backward Compatibility

Existing hook configs that omit `--source` continue to work unchanged — they get BM25-only output, same as before. Add `--source` when you're ready to enable importance injection.

## Custom Database Path

All subcommands accept `--db <path>` to use a different database:

```json
{
  "hooks": {
    "SessionStart": [
      {
        "matcher": "",
        "hooks": [
          {
            "type": "command",
            "command": "cf query --db /path/to/project.db --format text --top-k 10"
          }
        ]
      }
    ]
  }
}
```

## Troubleshooting

| Problem | Solution |
|---------|----------|
| `cf: command not found` | Ensure the binary is on your `PATH` |
| Empty context on session start | Run `cf info` to check entry count; run `cf query --format text` manually |
| Permission denied | Check file permissions on `~/.context-forge/` |
| Hooks not firing | Verify `~/.claude/settings.json` is valid JSON; check Claude Code logs |
| Timeout errors | Increase `timeout` in hook config (default 10000ms) or use `cf query --timeout-ms 15000 --format text` |
