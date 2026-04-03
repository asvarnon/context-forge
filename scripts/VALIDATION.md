# scripts/

This directory contains utility and validation scripts. Validation scripts are **throwaway diagnostics** — they are not committed to the repository and are gitignored.

## Validation Scripts

| Script | Purpose | Status |
|---|---|---|
| `validate_heuristics.py` | Tests importance detection heuristics against real cf database. Runs tokenization, recurrence scoring, context extraction, category classification, and scoring against live data. Includes content pre-filtering. | Throwaway — not committed |

## Install Scripts

| Script | Purpose |
|---|---|
| `install.sh` | Unix install script for context-forge CLI |
| `install.ps1` | Windows install script for context-forge CLI |
