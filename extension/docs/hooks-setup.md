# Context Forge — PreCompact Hook Setup Guide

> **Status:** Temporary manual setup while auto-registration is pending ([Issue #18](https://github.com/AustinGTI/context-forge/issues/18)).

VS Code fires a **PreCompact** hook every time the conversation context is about
to be compacted (truncated to fit the prompt budget). Context Forge uses this
hook to snapshot the full transcript *before* compaction, ensuring nothing is
lost.

The hook invokes the `cf` CLI, which reads the transcript from stdin and
persists it in the same SQLite database the extension queries.

---

## Prerequisites

| Requirement | How to verify |
|---|---|
| **Rust toolchain** | `cargo --version` |
| **`cf` CLI built** | `cargo build -p cf-cli` → binary at `target/debug/cf` (or `target/debug/cf.exe` on Windows) |
| **`jq` installed** (Linux/macOS only) | `jq --version` — used by the bash wrapper to parse JSON |
| **Context Forge extension loaded** | Activate the extension at least once so the database file is created |

### Finding the database path

The extension stores its database at:

| Platform | Path |
|---|---|
| **Windows** | `%APPDATA%\Code\User\globalStorage\asvarnon.context-forge\context-forge.db` |
| **macOS** | `~/Library/Application Support/Code/User/globalStorage/asvarnon.context-forge/context-forge.db` |
| **Linux** | `~/.config/Code/User/globalStorage/asvarnon.context-forge/context-forge.db` |

> **Tip:** After activating the extension, open the **Context Forge** output
> channel — the database path is printed on startup.

---

## Approach 1 — Workspace hooks *(recommended)*

Workspace hooks live in `.github/hooks/` and apply to every agent session in
the workspace. This is the simplest setup.

### 1. Create the hook configuration

Create `.github/hooks/pre-compact.json` in your workspace root:

```json
{
  "hooks": {
    "PreCompact": [
      {
        "type": "command",
        "command": "./extension/scripts/pre-compact-hook.sh",
        "windows": "powershell -ExecutionPolicy Bypass -File extension\\scripts\\pre-compact-hook.ps1",
        "timeout": 15,
        "env": {
          "CF_DB_PATH": "<YOUR_DB_PATH>",
          "CF_CLI": "./target/debug/cf"
        }
      }
    ]
  }
}
```

Replace `<YOUR_DB_PATH>` with the full path from the table above.  
Adjust `CF_CLI` to point to your built `cf` binary.

### 2. Make the bash script executable (Linux/macOS)

```bash
chmod +x extension/scripts/pre-compact-hook.sh
```

### 3. Verify

1. Open VS Code in the workspace.
2. Check **View → Output → GitHub Copilot Chat Hooks** — the hook should appear
   in the "Load Hooks" log.
3. Start a long chat session and let compaction trigger, or wait for the
   `PreCompact` event.
4. Check the **Context Forge** output channel for a new entry ID.

---

## Approach 2 — Agent-scoped hooks

Agent-scoped hooks only fire when a specific custom agent is active. Useful if
you want the PreCompact save limited to a particular workflow.

### 1. Enable agent-scoped hooks

Add to your VS Code **settings.json**:

```json
{
  "chat.useCustomAgentHooks": true
}
```

### 2. Create a custom agent with hooks

Create `.github/agents/context-aware.agent.md` (or any `.agent.md` file):

```markdown
---
name: "Context Aware"
description: "Agent with automatic context persistence before compaction"
hooks:
  PreCompact:
    - type: command
      command: "./extension/scripts/pre-compact-hook.sh"
      windows: "powershell -ExecutionPolicy Bypass -File extension\\scripts\\pre-compact-hook.ps1"
      timeout: 15
      env:
        CF_DB_PATH: "<YOUR_DB_PATH>"
        CF_CLI: "./target/debug/cf"
---

You are a coding assistant. Your conversation context is automatically saved
before compaction by Context Forge.
```

### 3. Activate the agent

Select the **Context Aware** agent from the agent picker (or `@context-aware`
in Chat) — the PreCompact hook fires only during sessions with this agent.

---

## Approach 3 — VS Code settings

You can also configure hook file locations via settings if your hook files live
outside the default `.github/hooks/` directory.

Add to **settings.json**:

```json
{
  "chat.hookFilesLocations": {
    ".github/hooks": true,
    "extension/scripts/hooks.json": true
  }
}
```

Then create `extension/scripts/hooks.json` with the same structure as the
workspace hook configuration in Approach 1.

---

## How it works

```
Compaction triggered
        │
        ▼
VS Code fires PreCompact hook
        │
        ▼
Wrapper script receives JSON on stdin:
  { "transcript_path": "/tmp/.../transcript.json",
    "sessionId": "...",
    "trigger": "auto" }
        │
        ▼
Wrapper reads transcript file from disk
        │
        ▼
Pipes content to:  cf pre-compact --db <DB_PATH>
        │
        ▼
CLI saves snapshot with EntryKind::PreCompact
        │
        ▼
Next Copilot request → extension's contextProvider
  calls assemble("*") → returns persisted context
```

---

## Troubleshooting

| Symptom | Fix |
|---|---|
| Hook not loading | Check **Output → GitHub Copilot Chat Hooks** for "Load Hooks" entries. Verify the `.json` file is in `.github/hooks/` with a valid `hooks` object. |
| `CF_DB_PATH not set` error | Set the `CF_DB_PATH` environment variable in the hook config's `env` block. |
| `jq: command not found` | Install jq: `brew install jq` (macOS), `apt install jq` (Debian/Ubuntu). |
| Permission denied (Linux/macOS) | Run `chmod +x extension/scripts/pre-compact-hook.sh`. |
| Timeout errors | Increase `timeout` in the hook config (default: 30s). |
| No transcript saved | The `transcript_path` field may be empty for very short sessions. This is expected — there is nothing to persist. |
| Using `/hooks` command | Type `/hooks` in Chat to use the interactive hook configuration UI. |

---

## Generating hooks with AI

VS Code provides a `/create-hook` slash command. Try:

```
/create-hook Run the Context Forge CLI to save a pre-compact snapshot
```

This generates a hook configuration file you can customize.

---

## References

- [VS Code Agent Hooks documentation](https://code.visualstudio.com/docs/copilot/customization/hooks)
- [Issue #18 — chatHooks auto-registration](https://github.com/AustinGTI/context-forge/issues/18)
- [PreCompact hook stub](../src/hooks/preCompact.ts)
