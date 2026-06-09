#!/usr/bin/env bash
#
# claude-time installer. Downloads the appropriate binary from the latest
# GitHub Release and drops it in ~/.local/bin (or $CLAUDE_TIME_BIN_DIR).
#
# Usage:
#   curl -fsSL https://raw.githubusercontent.com/ayodm/claude-time/main/installer.sh | bash
#
set -euo pipefail

REPO="ayodm/claude-time"
BIN_NAME="claude-time"
BIN_DIR="${CLAUDE_TIME_BIN_DIR:-${HOME}/.local/bin}"

err() { printf '\033[31merror:\033[0m %s\n' "$*" >&2; exit 1; }
info() { printf '\033[32m==>\033[0m %s\n' "$*"; }

# Detect platform target triple.
OS="$(uname -s)"
ARCH="$(uname -m)"
case "${OS}-${ARCH}" in
  Darwin-arm64)   TARGET="aarch64-apple-darwin" ;;
  Darwin-x86_64)  TARGET="x86_64-apple-darwin" ;;
  Linux-x86_64)   TARGET="x86_64-unknown-linux-gnu" ;;
  Linux-aarch64)  TARGET="aarch64-unknown-linux-gnu" ;;
  *) err "unsupported platform: ${OS}-${ARCH}" ;;
esac

# Resolve latest release tag.
TAG="$(curl -fsSL "https://api.github.com/repos/${REPO}/releases/latest" \
  | grep -E '"tag_name":' | head -1 | cut -d'"' -f4 || true)"
[ -n "${TAG}" ] || err "no published releases yet at github.com/${REPO}"

ARCHIVE="${BIN_NAME}-${TAG}-${TARGET}.tar.gz"
URL="https://github.com/${REPO}/releases/download/${TAG}/${ARCHIVE}"

info "downloading ${URL}"
TMP="$(mktemp -d)"
trap 'rm -rf "${TMP}"' EXIT
curl -fsSL "${URL}" -o "${TMP}/${ARCHIVE}"
curl -fsSL "${URL}.sha256" -o "${TMP}/${ARCHIVE}.sha256"

(
  cd "${TMP}"
  if command -v shasum >/dev/null; then
    shasum -a 256 -c "${ARCHIVE}.sha256" >/dev/null
  else
    sha256sum -c "${ARCHIVE}.sha256" >/dev/null
  fi
)
info "checksum ok"

tar -xzf "${TMP}/${ARCHIVE}" -C "${TMP}"
mkdir -p "${BIN_DIR}"
install -m 0755 "${TMP}/${BIN_NAME}" "${BIN_DIR}/${BIN_NAME}"

info "installed ${BIN_NAME} ${TAG} → ${BIN_DIR}/${BIN_NAME}"

case ":${PATH}:" in
  *":${BIN_DIR}:"*) ;;
  *)
    echo
    echo "Add ${BIN_DIR} to your PATH:"
    echo "  export PATH=\"${BIN_DIR}:\$PATH\""
    ;;
esac

echo
echo "Next: '${BIN_NAME} install' to wire up the SessionStart + SessionEnd hooks."
