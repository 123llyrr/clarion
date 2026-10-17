#!/usr/bin/env bash
# Restore the Apple-derived reference assets that were removed from this tree.
#
# Clarion is GPL-3.0-or-later. The icons, Apple wordmark and the SF / PingFang
# fonts are Apple property and cannot be redistributed, so they are not in git.
# Users who own a macOS image can extract them locally and re-import them here.
#
# This script never downloads anything and never modifies a tracked file except
# clarion-shell/pubspec.yaml. Imported assets are git-ignored.
#
# Usage:
#   scripts/import-macos-assets.sh --source <dir> [--dry-run]
#
# The source directory may either mirror repository paths
# (<dir>/clarion-shell/assets/macos/finder.png) or hold a flat set of extracted
# files, as long as every file name in the manifest is unique.
set -euo pipefail

repo_dir=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)
manifest="$repo_dir/resources/licenses/removed-apple-assets.sha256"
pubspec="$repo_dir/clarion-shell/pubspec.yaml"

source_dir=""
dry_run=0
while [[ $# -gt 0 ]]; do
  case "$1" in
    --source)
      [[ $# -ge 2 ]] || { printf 'error: --source needs a directory\n' >&2; exit 2; }
      source_dir="$2"
      shift 2
      ;;
    --source=*)
      source_dir="${1#*=}"
      shift
      ;;
    --dry-run)
      dry_run=1
      shift
      ;;
    -h|--help)
      sed -n '2,18p' "${BASH_SOURCE[0]}" | sed 's/^# \{0,1\}//'
      exit 0
      ;;
    *)
      printf 'error: unknown argument %s\n' "$1" >&2
      exit 2
      ;;
  esac
done

if [[ -z "$source_dir" ]]; then
  printf 'error: --source <dir> is required (a directory you extracted yourself)\n' >&2
  exit 2
fi
if [[ ! -d "$source_dir" ]]; then
  printf 'error: %s is not a directory\n' "$source_dir" >&2
  exit 2
fi
if [[ ! -f "$manifest" ]]; then
  printf 'error: manifest missing: %s\n' "$manifest" >&2
  exit 2
fi

# Duplicate basenames need an explicit directory layout in the source tree.
dupes=$(awk '!/^#/ && NF >= 2 {n = $2; sub(/.*\//, "", n); print n}' "$manifest" |
  sort | uniq -d || true)

restored=0
missing=0
mismatched=0
declare -a missing_list=()
declare -a mismatched_list=()

while read -r expected path; do
  [[ -z "${expected:-}" || "${expected:0:1}" == "#" ]] && continue

  candidate="$source_dir/$path"
  if [[ ! -f "$candidate" ]]; then
    base=${path##*/}
    if ! grep -qx "$base" <<<"$dupes"; then
      candidate="$source_dir/$base"
    fi
  fi

  if [[ ! -f "$candidate" ]]; then
    missing=$((missing + 1))
    missing_list+=("$path")
    continue
  fi

  actual=$(sha256sum "$candidate" | cut -d' ' -f1)
  if [[ "$actual" != "$expected" ]]; then
    mismatched=$((mismatched + 1))
    mismatched_list+=("$path")
    printf 'hash mismatch, skipped: %s\n' "$path" >&2
    continue
  fi

  if [[ "$dry_run" -eq 0 ]]; then
    mkdir -p "$repo_dir/$(dirname -- "$path")"
    install -m 0644 -- "$candidate" "$repo_dir/$path"
  fi
  restored=$((restored + 1))
done < <(awk '!/^#/ && NF >= 2 {print $1, $2}' "$manifest")

printf 'verified: %d, missing: %d, mismatched: %d\n' "$restored" "$missing" "$mismatched"

if [[ "$missing" -gt 0 ]]; then
  printf 'missing files (first 10):\n' >&2
  printf '  %s\n' "${missing_list[@]:0:10}" >&2
fi
if [[ "$mismatched" -gt 0 ]]; then
  printf 'mismatched files (first 10):\n' >&2
  printf '  %s\n' "${mismatched_list[@]:0:10}" >&2
fi

if [[ "$dry_run" -eq 1 ]]; then
  printf 'dry run: nothing written\n'
  exit 0
fi

if [[ "$restored" -eq 0 ]]; then
  printf 'nothing imported; pubspec.yaml left unchanged\n' >&2
  exit 1
fi

# Re-enable the Flutter asset and font declarations between the pubspec markers.
python3 - "$pubspec" <<'PY'
import pathlib
import sys

path = pathlib.Path(sys.argv[1])
text = path.read_text(encoding="utf-8")
begin = "  # BEGIN macOS assets\n"
end = "  # END macOS assets\n"
start = text.find(begin)
stop = text.find(end)
if start == -1 or stop == -1 or stop < start:
    print("pubspec.yaml: macOS asset markers not found; add the declarations manually", file=sys.stderr)
    raise SystemExit(1)
block = """  assets:
    - assets/macos/
  fonts:
    - family: macOSSystem
      fonts:
        - asset: assets/macos/SFNS.ttf
    - family: macOSChinese
      fonts:
        - asset: assets/macos/PingFangSC-Regular.otf
          weight: 400
        - asset: assets/macos/PingFangSC-Semibold.otf
          weight: 600
"""
path.write_text(text[:start] + begin + block + text[stop:], encoding="utf-8")
print("pubspec.yaml: assets/macos/ and both font families re-enabled")
PY

cat <<'NOTE'

Imported. Next steps:
  * scripts/build-shell.sh --release     # shell picks up icons and fonts
  * clarion-settings uses vector icons until the five include_bytes! entries in
    clarion-settings/src/icons.rs are restored; fonts resolve through fontconfig
    and need no change.
  * Do not commit the imported files. They are git-ignored on purpose.
NOTE
