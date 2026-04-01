#Requires -Version 5.1
<#
.SYNOPSIS
    Context Forge CLI (cf) — Windows Installer
.DESCRIPTION
    Downloads the cf binary from GitHub Releases (asvarnon/context-forge, private repo)
    and installs it to %LOCALAPPDATA%\context-forge\.
.PARAMETER Version
    Release tag to install (e.g. "v0.1.0"). Defaults to "latest".
.EXAMPLE
    .\install.ps1
    .\install.ps1 -Version v0.1.0
#>

param(
    [string]$Version = "latest"
)

$ErrorActionPreference = "Stop"

# ============================================================================
#  Config
# ============================================================================

$Repo       = "asvarnon/context-forge"
$AssetName  = "cf-windows-x64.exe"
$BinaryName = "cf.exe"
$InstallDir = Join-Path $env:LOCALAPPDATA "context-forge"

# ============================================================================
#  Helpers
# ============================================================================

function Write-Info  { param([string]$Msg) Write-Host "[info]  $Msg" -ForegroundColor Cyan }
function Write-Ok    { param([string]$Msg) Write-Host "[ok]    $Msg" -ForegroundColor Green }
function Write-Warn  { param([string]$Msg) Write-Host "[warn]  $Msg" -ForegroundColor Yellow }
function Write-Err   { param([string]$Msg) Write-Host "[error] $Msg" -ForegroundColor Red; exit 1 }

# ============================================================================
#  Banner
# ============================================================================

Write-Host ""
Write-Host "  +--------------------------------------+"
Write-Host "  |   Context Forge CLI - Installer      |"
Write-Host "  |   github.com/$Repo     |"
Write-Host "  +--------------------------------------+"
Write-Host ""

# ============================================================================
#  Check for gh CLI
# ============================================================================

$HasGh = $null -ne (Get-Command gh -ErrorAction SilentlyContinue)

if (-not $HasGh) {
    Write-Warn "gh CLI not found. Falling back to Invoke-WebRequest."
    Write-Warn "A GitHub Personal Access Token (PAT) is required for private repos."
    Write-Warn "Set `$env:GITHUB_TOKEN or install gh: https://cli.github.com"

    if ([string]::IsNullOrEmpty($env:GITHUB_TOKEN)) {
        Write-Err "GITHUB_TOKEN is not set and gh CLI is not available. Cannot access private repo."
    }
}

# ============================================================================
#  Resolve version
# ============================================================================

if ($Version -eq "latest") {
    Write-Info "Resolving latest release tag..."

    if ($HasGh) {
        try {
            $Version = gh release view --repo $Repo --json tagName --jq ".tagName" 2>&1
            if ($LASTEXITCODE -ne 0) { throw "gh failed" }
            $Version = $Version.Trim()
        } catch {
            Write-Err "Failed to resolve latest release via gh CLI. Are you authenticated? Run: gh auth login"
        }
    } else {
        try {
            $headers = @{
                "Authorization" = "token $env:GITHUB_TOKEN"
                "Accept"        = "application/vnd.github+json"
            }
            $release = Invoke-RestMethod -Uri "https://api.github.com/repos/$Repo/releases/latest" -Headers $headers
            $Version = $release.tag_name
        } catch {
            Write-Err "Failed to resolve latest release via GitHub API: $_"
        }
    }
}

if ([string]::IsNullOrEmpty($Version)) {
    Write-Err "Could not determine release version."
}

Write-Info "Version: $Version"

# ============================================================================
#  Download binary
# ============================================================================

$TmpDir  = Join-Path $env:TEMP "cf-install-$(Get-Random)"
$TmpFile = Join-Path $TmpDir $BinaryName
New-Item -ItemType Directory -Path $TmpDir -Force | Out-Null

Write-Info "Downloading $AssetName from release $Version..."

try {
    if ($HasGh) {
        $ghArgs = @("release", "download", $Version,
                     "--repo", $Repo,
                     "--pattern", $AssetName,
                     "--dir", $TmpDir,
                     "--clobber")
        & gh @ghArgs
        if ($LASTEXITCODE -ne 0) { throw "gh download failed" }

        $downloaded = Join-Path $TmpDir $AssetName
        Rename-Item -Path $downloaded -NewName $BinaryName
    } else {
        # Resolve asset URL from release metadata
        $headers = @{
            "Authorization" = "token $env:GITHUB_TOKEN"
            "Accept"        = "application/vnd.github+json"
        }
        $release = Invoke-RestMethod -Uri "https://api.github.com/repos/$Repo/releases/tags/$Version" -Headers $headers
        $asset   = $release.assets | Where-Object { $_.name -eq $AssetName } | Select-Object -First 1

        if ($null -eq $asset) {
            Write-Err "Asset $AssetName not found in release $Version."
        }

        $dlHeaders = @{
            "Authorization" = "token $env:GITHUB_TOKEN"
            "Accept"        = "application/octet-stream"
        }
        Invoke-WebRequest -Uri $asset.url -Headers $dlHeaders -OutFile $TmpFile
    }

    Write-Ok "Downloaded $AssetName"
} catch {
    # Clean up temp dir on failure
    Remove-Item -Path $TmpDir -Recurse -Force -ErrorAction SilentlyContinue
    Write-Err "Download failed: $_"
}

# ============================================================================
#  Install binary
# ============================================================================

if (-not (Test-Path $InstallDir)) {
    New-Item -ItemType Directory -Path $InstallDir -Force | Out-Null
    Write-Info "Created install directory: $InstallDir"
}

$DestPath = Join-Path $InstallDir $BinaryName
Move-Item -Path $TmpFile -Destination $DestPath -Force
Remove-Item -Path $TmpDir -Recurse -Force -ErrorAction SilentlyContinue
Write-Ok "Installed to $DestPath"

# ============================================================================
#  Add to user PATH (if not already present)
# ============================================================================

$userPath = [Environment]::GetEnvironmentVariable("Path", "User")

if ($userPath -split ";" | Where-Object { $_ -eq $InstallDir }) {
    Write-Info "$InstallDir is already in user PATH."
} else {
    $newPath = "$userPath;$InstallDir".TrimStart(";")
    [Environment]::SetEnvironmentVariable("Path", $newPath, "User")
    # Update current session too
    $env:Path = "$env:Path;$InstallDir"
    Write-Ok "Added $InstallDir to user PATH."
    Write-Warn "Restart your terminal for PATH changes to take effect in new sessions."
}

# ============================================================================
#  Verify
# ============================================================================

Write-Info "Verifying installation..."

try {
    $cfPath = Join-Path $InstallDir $BinaryName
    $result = & $cfPath --version 2>&1
    Write-Host $result
    Write-Ok "Context Forge CLI is ready!"
} catch {
    Write-Warn "cf is installed at $DestPath but verification failed: $_"
    Write-Warn "Try running: $DestPath --version"
}

Write-Host ""
