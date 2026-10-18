#!/usr/bin/env bash
set -euo pipefail

script_dir="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
project_dir="$(cd -- "$script_dir/../clarion-shell" && pwd)"
sdk_dir="${CLARION_FLUTTER_SDK:-${REALAND_FLUTTER_SDK:-${HOME}/.cache/clarion/toolchains/flutter}}"
if [[ -z "${CLARION_FLUTTER_SDK:-}" && -z "${REALAND_FLUTTER_SDK:-}" &&
      ! -x "${sdk_dir}/bin/flutter" && -x "${HOME}/.cache/realand/toolchains/flutter/bin/flutter" ]]; then
  sdk_dir="${HOME}/.cache/realand/toolchains/flutter"
fi
flutter_bin="${CLARION_FLUTTER_BIN:-${REALAND_FLUTTER_BIN:-${sdk_dir}/bin/flutter}}"
build_mode="release"
if [[ "${1:-}" == "--debug" || "${1:-}" == "--profile" || "${1:-}" == "--release" ]]; then
  build_mode="${1#--}"
  shift
fi
if [[ "${1:-}" == "--help" || "${1:-}" == "-h" ]]; then
  printf '%s\n' 'Usage: scripts/build-shell.sh [--release|--profile|--debug] [Flutter build options]' \
    'CLARION_FLUTTER_SDK selects an independent official Flutter SDK.'
  exit 0
fi
if [[ ! -x "$flutter_bin" ]]; then
  printf 'Flutter SDK not found at %s. Set CLARION_FLUTTER_SDK to an official SDK.\n' "$sdk_dir" >&2
  exit 1
fi
for dependency in clang cmake ninja pkg-config; do
  if ! command -v "$dependency" >/dev/null 2>&1; then
    printf 'Missing Linux build tool: %s\n' "$dependency" >&2
    exit 1
  fi
done
if ! pkg-config --exists gtk+-3.0 'gtk-layer-shell-0 >= 0.6' 'wayland-client >= 1.20' dbusmenu-gtk3-0.4; then
  printf '%s\n' 'Missing development libraries: GTK3, gtk-layer-shell >= 0.6, wayland-client >= 1.20, and libdbusmenu-gtk3.' >&2
  exit 1
fi

cd -- "$project_dir"
# A checked-in lockfile protects release builds from dependency changes.
if [[ -f pubspec.lock ]]; then
  "$flutter_bin" pub get --enforce-lockfile
else
  "$flutter_bin" pub get
fi
exec "$flutter_bin" build linux "--${build_mode}" --no-pub "$@"
