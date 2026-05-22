#!/usr/bin/env bash
# Build release tarballs and publish to GitHub + Homebrew tap.
# Run on macOS (Apple Silicon): native macOS builds + Docker for Linux.
#
#   jarvis tkr publish
#   make publish
#
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"

command -v docker >/dev/null || {
  echo "error: docker required for Linux release binaries" >&2
  exit 1
}
command -v gh >/dev/null || {
  echo "error: gh (GitHub CLI) required" >&2
  exit 1
}

exec make publish
