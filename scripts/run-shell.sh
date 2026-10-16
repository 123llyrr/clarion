#!/usr/bin/env bash
set -euo pipefail

script_dir="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
project_dir="$(cd -- "$script_dir/../clarion-shell" && pwd)"
build_mode="release"
if [[ "${1:-}" == "--help" || "${1:-}" == "-h" ]]; then
  printf '%s\n' 'Usage: scripts/run-shell.sh [--release|--profile|--debug] [--demo|--preview] [--monitor=N] [--output-name=NAME]' \
    'The full Flutter bundle must be built first; this script does not replace the running desktop shell.'
  exit 0
fi
if [[ "${1:-}" == "--debug" || "${1:-}" == "--profile" || "${1:-}" == "--release" ]]; then
  build_mode="${1#--}"
  shift
fi
case "$(uname -m)" in
  x86_64) target_arch="x64" ;;
  aarch64|arm64) target_arch="arm64" ;;
  *) printf 'Unsupported shell architecture: %s\n' "$(uname -m)" >&2; exit 1 ;;
esac
shell_binary="${CLARION_SHELL_BINARY:-${REALAND_SHELL_BINARY:-${project_dir}/build/linux/${target_arch}/${build_mode}/bundle/clarion-shell}}"
if [[ ! -x "$shell_binary" ]]; then
  printf 'Shell bundle not found. Run scripts/build-shell.sh --%s first.\n' "$build_mode" >&2
  exit 1
fi
# Preserve argument boundaries (including output names containing spaces).
# Launching this process does not stop or replace any running desktop shell.
exec "$shell_binary" "$@"
