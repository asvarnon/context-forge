#!/usr/bin/env bash
set -euo pipefail

# ============================================================================
#  Context Forge CLI (cf) — Install Script
#  Repo: asvarnon/context-forge (private)
#  Supports: Linux x64, macOS ARM64
# ============================================================================

REPO="asvarnon/context-forge"
BINARY_NAME="cf"
VERSION="${1:-latest}"

# --- Helpers ----------------------------------------------------------------

info()  { printf "\033[1;34m[info]\033[0m  %s\n" "$*"; }
ok()    { printf "\033[1;32m[ok]\033[0m    %s\n" "$*"; }
warn()  { printf "\033[1;33m[warn]\033[0m  %s\n" "$*"; }
error() { printf "\033[1;31m[error]\033[0m %s\n" "$*" >&2; exit 1; }

# --- Banner -----------------------------------------------------------------

echo ""
echo "  ┌──────────────────────────────────────┐"
echo "  │   Context Forge CLI — Installer       │"
echo "  │   github.com/${REPO}      │"
echo "  └──────────────────────────────────────┘"
echo ""

# --- Detect platform --------------------------------------------------------

OS="$(uname -s)"
ARCH="$(uname -m)"

case "${OS}" in
    Linux)  PLATFORM="linux" ;;
    Darwin) PLATFORM="darwin" ;;
    *)      error "Unsupported OS: ${OS}. Only Linux and macOS are supported." ;;
esac

case "${ARCH}" in
    x86_64|amd64)       MAPPED_ARCH="x64" ;;
    aarch64|arm64)       MAPPED_ARCH="arm64" ;;
    *)                   error "Unsupported architecture: ${ARCH}" ;;
esac

ASSET_NAME="cf-${PLATFORM}-${MAPPED_ARCH}"

# Validate supported combinations
if [[ "${PLATFORM}" == "linux" && "${MAPPED_ARCH}" != "x64" ]]; then
    error "Only x64 is supported on Linux. Detected: ${MAPPED_ARCH}"
fi
if [[ "${PLATFORM}" == "darwin" && "${MAPPED_ARCH}" != "arm64" ]]; then
    error "Only ARM64 (Apple Silicon) is supported on macOS. Detected: ${MAPPED_ARCH}"
fi

info "Detected: ${OS} ${ARCH} → asset ${ASSET_NAME}"

# --- Resolve version --------------------------------------------------------

if [[ "${VERSION}" == "latest" ]]; then
    info "Resolving latest release tag..."
    if command -v gh &>/dev/null; then
        VERSION="$(gh release view --repo "${REPO}" --json tagName --jq '.tagName')" \
            || error "Failed to resolve latest release via gh CLI. Are you authenticated?"
    else
        warn "gh CLI not found — trying GitHub API via curl."
        warn "This requires authentication for private repos."
        warn "Set GITHUB_TOKEN or install gh: https://cli.github.com"
        if [[ -z "${GITHUB_TOKEN:-}" ]]; then
            error "GITHUB_TOKEN is not set and gh CLI is not available. Cannot access private repo."
        fi
        VERSION="$(curl -fsSL \
            -H "Authorization: token ${GITHUB_TOKEN}" \
            -H "Accept: application/vnd.github+json" \
            "https://api.github.com/repos/${REPO}/releases/latest" \
            | grep '"tag_name"' | head -1 | cut -d'"' -f4)" \
            || error "Failed to resolve latest release via GitHub API."
    fi
fi

[[ -z "${VERSION}" ]] && error "Could not determine release version."
info "Version: ${VERSION}"

# --- Download binary --------------------------------------------------------

TMPDIR="$(mktemp -d)"
TMPFILE="${TMPDIR}/${BINARY_NAME}"
trap 'rm -rf "${TMPDIR}"' EXIT

info "Downloading ${ASSET_NAME} from release ${VERSION}..."

if command -v gh &>/dev/null; then
    gh release download "${VERSION}" \
        --repo "${REPO}" \
        --pattern "${ASSET_NAME}" \
        --dir "${TMPDIR}" \
        --clobber \
        || error "Download failed. Check that release ${VERSION} exists and contains ${ASSET_NAME}."
    mv "${TMPDIR}/${ASSET_NAME}" "${TMPFILE}"
else
    if [[ -z "${GITHUB_TOKEN:-}" ]]; then
        error "GITHUB_TOKEN is not set and gh CLI is not available. Cannot download from private repo."
    fi
    # Resolve the asset download URL from the release
    RELEASE_DATA="$(curl -fsSL \
        -H "Authorization: token ${GITHUB_TOKEN}" \
        -H "Accept: application/vnd.github+json" \
        "https://api.github.com/repos/${REPO}/releases/tags/${VERSION}")" \
        || error "Failed to fetch release ${VERSION} metadata."

    ASSET_URL="$(echo "${RELEASE_DATA}" \
        | grep -o "\"url\": *\"[^\"]*${ASSET_NAME}[^\"]*\"" \
        | head -1 | cut -d'"' -f4)"

    [[ -z "${ASSET_URL}" ]] && error "Asset ${ASSET_NAME} not found in release ${VERSION}."

    curl -fsSL \
        -H "Authorization: token ${GITHUB_TOKEN}" \
        -H "Accept: application/octet-stream" \
        -o "${TMPFILE}" \
        "${ASSET_URL}" \
        || error "Download failed for ${ASSET_URL}."
fi

chmod +x "${TMPFILE}"
ok "Downloaded ${ASSET_NAME}"

# --- Verify checksum --------------------------------------------------------

CHECKSUM_FILE="checksums.sha256"
info "Downloading checksums for integrity verification..."

if command -v gh &>/dev/null; then
    gh release download "${VERSION}" \
        --repo "${REPO}" \
        --pattern "${CHECKSUM_FILE}" \
        --dir "${TMPDIR}" \
        --clobber 2>/dev/null
else
    CHECKSUM_URL="$(echo "${RELEASE_DATA}" \
        | grep -o "\"url\": *\"[^\"]*${CHECKSUM_FILE}[^\"]*\"" \
        | head -1 | cut -d'"' -f4)"

    if [[ -n "${CHECKSUM_URL}" ]]; then
        curl -fsSL \
            -H "Authorization: token ${GITHUB_TOKEN}" \
            -H "Accept: application/octet-stream" \
            -o "${TMPDIR}/${CHECKSUM_FILE}" \
            "${CHECKSUM_URL}" 2>/dev/null
    fi
fi

if [[ -f "${TMPDIR}/${CHECKSUM_FILE}" ]]; then
    EXPECTED="$(grep "${ASSET_NAME}" "${TMPDIR}/${CHECKSUM_FILE}" | awk '{print $1}')"
    ACTUAL="$(sha256sum "${TMPFILE}" | awk '{print $1}')"
    if [[ "${EXPECTED}" != "${ACTUAL}" ]]; then
        error "Checksum verification FAILED. Binary may be corrupted or tampered with.\n  Expected: ${EXPECTED}\n  Got:      ${ACTUAL}"
    fi
    ok "Checksum verified (SHA-256)"
else
    warn "No checksums.sha256 found in release ${VERSION} — skipping integrity check."
    warn "Releases from v0.3.0+ include checksums. Consider upgrading."
fi

# --- Install binary ---------------------------------------------------------

if [[ -w "/usr/local/bin" ]]; then
    INSTALL_DIR="/usr/local/bin"
elif command -v sudo &>/dev/null; then
    INSTALL_DIR="/usr/local/bin"
    info "Requires sudo to install to ${INSTALL_DIR}"
    sudo mv "${TMPFILE}" "${INSTALL_DIR}/${BINARY_NAME}"
    ok "Installed to ${INSTALL_DIR}/${BINARY_NAME}"
else
    INSTALL_DIR="${HOME}/.local/bin"
    mkdir -p "${INSTALL_DIR}"
    info "No sudo available — installing to ${INSTALL_DIR}"
    # Check if ~/.local/bin is in PATH
    if [[ ":${PATH}:" != *":${INSTALL_DIR}:"* ]]; then
        warn "${INSTALL_DIR} is not in your PATH."
        warn "Add it:  export PATH=\"${INSTALL_DIR}:\$PATH\""
    fi
fi

# Move unless sudo branch already handled it
if [[ -f "${TMPFILE}" ]]; then
    mv "${TMPFILE}" "${INSTALL_DIR}/${BINARY_NAME}"
    ok "Installed to ${INSTALL_DIR}/${BINARY_NAME}"
fi

# --- Verify -----------------------------------------------------------------

info "Verifying installation..."
if command -v cf &>/dev/null; then
    cf --version
    ok "Context Forge CLI is ready!"
else
    warn "cf is installed at ${INSTALL_DIR}/${BINARY_NAME} but not found in PATH."
    warn "You may need to restart your shell or add ${INSTALL_DIR} to PATH."
fi

echo ""
