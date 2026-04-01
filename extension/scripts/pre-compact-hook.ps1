# Context Forge — PreCompact hook wrapper (Windows)
# Reads the VS Code hook JSON input from stdin, extracts the transcript,
# and saves it via the cf CLI before conversation compaction.
#
# Required environment variable:
#   CF_DB_PATH  — absolute path to the Context Forge SQLite database
#   CF_CLI      — (optional) path to the cf binary (defaults to "cf" on PATH)

$ErrorActionPreference = 'Stop'

$cfCli = if ($env:CF_CLI) { $env:CF_CLI } else { 'cf' }

# Read structured JSON that VS Code sends on stdin
$rawInput = [Console]::In.ReadToEnd()
$hookInput = $rawInput | ConvertFrom-Json

$transcriptPath = $hookInput.transcript_path

if (-not $transcriptPath -or -not (Test-Path $transcriptPath)) {
    # No transcript available — nothing to persist
    exit 0
}

if (-not $env:CF_DB_PATH) {
    Write-Error 'CF_DB_PATH not set — skipping pre-compact save'
    exit 1
}

# Pipe the transcript content to the CLI
Get-Content $transcriptPath -Raw | & $cfCli pre-compact --db $env:CF_DB_PATH

exit 0
