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
        "matcher": "",
        "hooks": [
          {
            "type": "command",
            "command": "cf query --format text --top-k 10",
            "timeout": 10000
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
| **SessionStart** | When a new Claude Code session starts | `cf query` outputs previous context to stdout, which Claude sees as context |

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
| Timeout errors | Increase `timeout` in hook config (default 10000ms) or use `cf --timeout-ms 15000` |
