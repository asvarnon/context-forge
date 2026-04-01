#!/usr/bin/env bash
# Context Forge — PreCompact hook wrapper
# Reads the VS Code hook JSON input from stdin, extracts the transcript,
# and saves it via the cf CLI before conversation compaction.
#
# Required environment variable:
#   CF_DB_PATH  — absolute path to the Context Forge SQLite database
#   CF_CLI      — (optional) path to the cf binary (defaults to "cf" on $PATH)

set -euo pipefail

CF_CLI="${CF_CLI:-cf}"

# Read structured JSON that VS Code sends on stdin
INPUT=$(cat)

# Extract transcript_path from the hook input
TRANSCRIPT_PATH=$(echo "$INPUT" | jq -r '.transcript_path // empty')

if [ -z "$TRANSCRIPT_PATH" ] || [ ! -f "$TRANSCRIPT_PATH" ]; then
  # No transcript available — nothing to persist
  exit 0
fi

if [ -z "${CF_DB_PATH:-}" ]; then
  echo '{"systemMessage":"CF_DB_PATH not set — skipping pre-compact save"}' >&2
  exit 1
fi

# Pipe the transcript content to the CLI
cat "$TRANSCRIPT_PATH" | "$CF_CLI" pre-compact --db "$CF_DB_PATH"

exit 0
