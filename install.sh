#!/usr/bin/env bash
set -euo pipefail

REPO="${JKR_REPO:-einyx/jkr}"
BIN_NAME="jkr"
INSTALL_DIR="${JKR_INSTALL_DIR:-${HOME}/.local/bin}"

OS=$(uname -s | tr '[:upper:]' '[:lower:]')
ARCH=$(uname -m)

case "${OS}-${ARCH}" in
  darwin-arm64)   ARCHIVE="jkr-aarch64-apple-darwin.tar.gz" ;;
  darwin-x86_64)  ARCHIVE="jkr-x86_64-apple-darwin.tar.gz" ;;
  linux-x86_64)   ARCHIVE="jkr-x86_64-unknown-linux-gnu.tar.gz" ;;
  linux-aarch64)  ARCHIVE="jkr-aarch64-unknown-linux-gnu.tar.gz" ;;
  *)
    echo "Unsupported platform: ${OS}-${ARCH}" >&2
    echo "Windows x86_64: get jkr-x86_64-pc-windows-msvc.tar.gz from https://github.com/${REPO}/releases/latest" >&2
    exit 1
    ;;
esac

VERSION=$(curl -sfL "https://api.github.com/repos/${REPO}/releases/latest" \
  | grep '"tag_name"' | head -1 | cut -d'"' -f4)

if [ -z "${VERSION}" ]; then
  echo "Could not determine latest release version for ${REPO}" >&2
  exit 1
fi

URL="https://github.com/${REPO}/releases/download/${VERSION}/${ARCHIVE}"

echo "Installing jkr ${VERSION} for ${OS}-${ARCH}..."
mkdir -p "${INSTALL_DIR}"
curl -fsSL "${URL}" | tar xz -C "${INSTALL_DIR}" "${BIN_NAME}"
chmod +x "${INSTALL_DIR}/${BIN_NAME}"

# macOS: strip provenance/quarantine xattrs and re-apply an ad-hoc signature.
# Without this, AppleSystemPolicy can refuse to launch the binary with
# "load code signature error 2" / SIGKILL on first run after install.
# The --identifier flag pins the codesign identity to com.einyx.jkr so the
# Keychain ACL on jkr's vault master key stays valid across upgrades —
# without it, each install gets a hash-derived identifier and the prior
# install loses access to its own keychain entry.
if [ "${OS}" = "darwin" ]; then
  xattr -c "${INSTALL_DIR}/${BIN_NAME}" 2>/dev/null || true
  codesign --force --sign - --identifier com.einyx.jkr \
    "${INSTALL_DIR}/${BIN_NAME}" 2>/dev/null || true
fi

echo "Installed to ${INSTALL_DIR}/jkr"
echo "Make sure ${INSTALL_DIR} is in your PATH."
