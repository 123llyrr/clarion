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
#   scripts/import-shell-assets.sh --source <dir> [--dry-run]
#
# The source directory may either mirror repository paths
# (<dir>/clarion-shell/assets/shell/finder.png) or hold a flat set of extracted
# files, as long as every file name in the manifest is unique. Both the
# neutral tree names and the original Apple file names are accepted.
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

# The tree stores neutral asset names, but a macOS extraction keeps the
# original Apple file names. Accept either in the source directory.
declare -A source_alias=(
  [logo-dark.png]=apple-logo-black.png
  [logo-dark.svg]=apple-logo-black.svg
  [logo-dark-tight.png]=apple-logo-black-tight.png
  [logo-light.png]=apple-logo-white.png
  [logo-light.svg]=apple-logo-white.svg
  [logo-light-tight.png]=apple-logo-white-tight.png
  [shell-badge-1024.png]=catalina-badge-1024.png
  [shell-badge-512.png]=catalina-badge-512.png
  [shell-badge-round.png]=catalina-badge-round.png
  [search-background.png]=ectl_search_background.png
  [search-background@2x.png]=ectl_search_background@2x.png
  [search-cancel.png]=ectl_search_cancel.png
  [search-cancel-pressed.png]=ectl_search_cancel_pressed.png
  [search-close.png]=ectl_search_close.png
  [search-close@2x.png]=ectl_search_close@2x.png
  [search-loupe.png]=ectl_search_loupe.png
  [search-loupe@2x.png]=ectl_search_loupe@2x.png
  [search-loupe-rtl.png]=ectl_search_loupe_rtl.png
  [search-loupe-rtl@2x.png]=ectl_search_loupe_rtl@2x.png
  [search-magnify.png]=ectl_search_magnify.png
  [closebox.png]=ecsb_closebox.png
  [closebox@2x.png]=ecsb_closebox@2x.png
  [closebox-pressed.png]=ecsb_closebox_pressed.png
  [closebox-pressed@2x.png]=ecsb_closebox_pressed@2x.png
  [cjk-regular.otf]=PingFangSC-Regular.otf
  [cjk-semibold.otf]=PingFangSC-Semibold.otf
  [ui-text.ttf]=SFNS.ttf
  [app-icon.icns]=ProductPageIcon.icns
  [app-icon-256.tiff]=ProductPageIcon_256x256.tiff
  [app-icon-512.tiff]=ProductPageIcon_512x512.tiff
  [logo-dark.pdf]=AKAppleLogoBlack.pdf
  [logo-light.pdf]=AKAppleLogoWhite.pdf
)

restored=0
missing=0
mismatched=0
declare -a missing_list=()
declare -a mismatched_list=()

while read -r expected path; do
  [[ -z "${expected:-}" || "${expected:0:1}" == "#" ]] && continue

  candidate="$source_dir/$path"
  base=${path##*/}
  if [[ ! -f "$candidate" ]]; then
    if ! grep -qx "$base" <<<"$dupes"; then
      candidate="$source_dir/$base"
    fi
  fi
  if [[ ! -f "$candidate" ]]; then
    alias=${source_alias[$base]:-}
    if [[ -n "$alias" ]]; then
      candidate="$source_dir/$alias"
      if [[ ! -f "$candidate" ]]; then
        candidate="$source_dir/${path%/*}/$alias"
      fi
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
begin = "  # BEGIN shell assets\n"
end = "  # END shell assets\n"
start = text.find(begin)
stop = text.find(end)
if start == -1 or stop == -1 or stop < start:
    print("pubspec.yaml: shell asset markers not found; add the declarations manually", file=sys.stderr)
    raise SystemExit(1)
# The assets list stays live (the shipped icons always apply); an import only
# appends the shell line to it and uncomments the fonts. Replacing the block
# with another assets: key would duplicate the YAML mapping key.
block = """  fonts:
    - family: ClarionSystem
      fonts:
        - asset: assets/shell/ui-text.ttf
    - family: ClarionChinese
      fonts:
        - asset: assets/shell/cjk-regular.otf
          weight: 400
        - asset: assets/shell/cjk-semibold.otf
          weight: 600
"""
text = text[:start] + begin + block + text[stop:]
if "    - assets/shell/\n" not in text:
    text = text.replace(
        "    - assets/icons/\n",
        "    - assets/icons/\n    - assets/shell/\n",
    )
path.write_text(text, encoding="utf-8")
print("pubspec.yaml: assets/shell/ and both font families re-enabled")
PY

cat <<'NOTE'

Imported. Next steps:
  * scripts/build-shell.sh --release     # shell picks up icons and fonts
  * clarion-settings uses vector icons until the five include_bytes! entries in
    clarion-settings/src/icons.rs are restored; fonts resolve through fontconfig
    and need no change.
  * Do not commit the imported files. They are git-ignored on purpose.
NOTE
