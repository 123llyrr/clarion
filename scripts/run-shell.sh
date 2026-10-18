#!/usr/bin/env bash
# A second non-preview instance must be refused by the Shell's instance lock.
set -euo pipefail
script_dir="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
repo_dir="$(cd -- "$script_dir/.." && pwd)"
build_mode="release"
if [[ "${1:-}" == "--help" || "${1:-}" == "-h" ]]; then
  printf '%s\n' 'Usage: scripts/run-shell.sh [--release|--debug] [--preview] [--monitor=N] [--output-name=NAME]' \
    'Uses target/{release,debug}/clarion-shell. This script never stops the active Shell.'
  exit 0
fi
if [[ "${1:-}" == "--debug" || "${1:-}" == "--release" ]]; then
  build_mode="${1#--}"
  shift
fi
target_dir="${CARGO_TARGET_DIR:-${repo_dir}/target}"
if [[ "$target_dir" != /* ]]; then
  target_dir="${repo_dir}/${target_dir}"
fi
shell_binary="${CLARION_SHELL_BINARY:-${REALAND_SHELL_BINARY:-${target_dir}/${build_mode}/clarion-shell}}"
if [[ ! -x "$shell_binary" ]]; then
  printf 'Shell binary not found. Run scripts/build-shell.sh --%s first.\n' "$build_mode" >&2
  exit 1
fi
exec "$shell_binary" "$@"
