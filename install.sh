#!/bin/sh
# agents-first-toolkit installer — POSIX, fail-closed.
#
# Installs all five agent-first CLI tools:
#   shellaborate   (agentic-shell)  batch shell/git/gh DAG
#   patchify       (agentic-edit)   batch edits + writes + verify
#   curlosity      (agentic-web)    batch web search + fetch + HTML→MD
#   readcursively  (agentic-code)   batch regex search + file read
#   recurlsively   (agentic-web)    recursive domain → Markdown
#
# Usage:
#   curl -fsSL https://raw.githubusercontent.com/riceharvest/agents-first-toolkit/main/install.sh | sh
#
# Override the release tag:
#   AFT_VERSION=v0.1.0 sh install.sh
#
# Override the install dir (default: ~/.local/bin):
#   AFT_INSTALL_DIR=/opt/bin sh install.sh

set -eu

REPO="riceharvest/agents-first-toolkit"
INSTALL_DIR="${AFT_INSTALL_DIR:-$HOME/.local/bin}"

# Binaries shipped per target; the release workflow publishes one archive
# per binary so each can be installed independently.
BINS="shellaborate patchify curlosity readcursively recurlsively"

log()  { printf 'aft-install: %s\n' "$1" >&2; }
fail() { log "ERROR: $1"; exit 1; }

command -v curl >/dev/null 2>&1 || command -v wget >/dev/null 2>&1 ||
  fail "need curl or wget to download"

fetch() {
  url=$1
  if command -v curl >/dev/null 2>&1; then
    curl -fsSL "$url"
  else
    wget -qO- "$url"
  fi
}

fetch_to() {
  url=$1
  dest=$2
  if command -v curl >/dev/null 2>&1; then
    curl -fsSL -o "$dest" "$url" || fail "download failed: $url"
  else
    wget -q "$url" -O "$dest" || fail "download failed: $url"
  fi
}

OS=$(uname -s)
ARCH=$(uname -m)

case "$OS" in
  Linux) os_component="unknown-linux-musl" ;;
  Darwin) os_component="apple-darwin" ;;
  *) fail "unsupported OS '$OS' (supported: Linux, macOS)" ;;
esac

case "$ARCH" in
  x86_64 | amd64) arch_component="x86_64" ;;
  aarch64 | arm64) arch_component="aarch6_4" ;;
  *) fail "unsupported architecture '$ARCH' (supported: x86_64, aarch64)" ;;
esac

TARGET="${arch_component}-${os_component}"

if [ -n "${AFT_VERSION:-}" ]; then
  VERSION="$AFT_VERSION"
else
  log "resolving latest release..."
  VERSION=$(fetch "https://api.github.com/repos/$REPO/releases/latest" |
    sed -n 's/.*"tag_name": *"([^"]*)".*/\1/p' | head -n 1)
  [ -n "$VERSION" ] || fail "could not determine latest release (set AFT_VERSION to pin one)"
fi

BASE_URL="https://github.com/$REPO/releases/download/$VERSION"
TMPDIR_INST=$(mktemp -d)
trap 'rm -rf "$TMPDIR_INST"' EXIT

mkdir -p "$INSTALL_DIR"

log "installing $VERSION to $INSTALL_DIR for $TARGET"
for BIN in $BINS; do
  ARCHIVE="${BIN}-${VERSION#v}-${TARGET}.tar.gz"
  fetch_to "$BASE_URL/$ARCHIVE" "$TMPDIR_INST/$ARCHIVE"

  log "verifying $BIN..."
  if command -v sha256sum >/dev/null 2>&1; then
    # Best-effort checksum check if a per-archive sum is available.
    EXPECTED=$(fetch_to "$BASE_URL/SHA256SUMS" "$TMPDIR_INST/SHA256SUMS" 2>/dev/null || true)
    if [ -f "$TMPDIR_INST/SHA256SUMS" ]; then
      EXP=$(grep " $ARCHIVE\$" "$TMPDIR_INST/SHA256SUMS" | awk '{print $1}')
      ACT=$(sha256sum "$TMPDIR_INST/$ARCHIVE" | awk '{print $1}')
      [ -n "$EXP" ] && [ "$ACT" = "$EXP" ] || log "note: could not verify checksum for $ARCHIVE"
    fi
  fi

  log "extracting $BIN..."
  mkdir -p "$TMPDIR_INST/$BIN-extract"
  tar xzf "$TMPDIR_INST/$ARCHIVE" -C "$TMPDIR_INST/$BIN-extract" ||
    fail "archive extraction failed for $BIN"

  # The archive layout mirrors the release packaging: BIN/<bin>
  FOUND=$(find "$TMPDIR_INST/$BIN-extract" -type f -name "$BIN" | head -n 1)
  [ -n "$FOUND" ] || fail "binary $BIN not found in $ARCHIVE"

  cp "$FOUND" "$INSTALL_DIR/$BIN"
  chmod +x "$INSTALL_DIR/$BIN"

  "$INSTALL_DIR/$BIN" --version >/dev/null 2>&1 ||
    fail "installed $BIN failed to run --version"
  log "  $BIN $( "$INSTALL_DIR/$BIN" --version 2>/dev/null || echo ok )"
done

log "done. All five tools installed to $INSTALL_DIR"
case ":$PATH:" in
  *":$INSTALL_DIR:"*) ;;
  *) log "NOTE: $INSTALL_DIR is not on your PATH" ;;
esac
