#!/usr/bin/env bash
# Build the native GPUI Shell without changing the installed/running desktop.
set -euo pipefail
script_dir="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
repo_dir="$(cd -- "$script_dir/.." && pwd)"
mode="release"
if [[ "${1:-}" == "--release" || "${1:-}" == "--debug" ]]; then
  mode="${1#--}"
  shift
fi
if [[ "${1:-}" == "--help" || "${1:-}" == "-h" ]]; then
  printf '%s\n' 'Usage: scripts/build-shell.sh [--release|--debug] [Cargo build options]' \
    'Builds the Rust/GPUI clarion-shell, including its application launcher.' \
    'No Flutter SDK, separate launcher executable or running-session changes.'
  exit 0
fi
cd -- "$repo_dir"
if [[ "$mode" == release ]]; then
  exec cargo build --locked --release -p clarion-shell "$@"
fi
exec cargo build --locked -p clarion-shell "$@"
