#!/usr/bin/env bash
# Phase 0 diagnostics are read-only unless --check, --build, or --run is requested.
set -euo pipefail

repo_dir=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)
output_name=eDP-1
quit_after=5
operation=diagnose

usage() {
    cat <<'EOF'
Usage: scripts/check-launcher-spike.sh [--check|--build|--run] [--output NAME] [--quit-after SECONDS]

Without a mode, print build/session prerequisites without opening any window.
  --check  cargo check --locked -p clarion-launcher
  --build  cargo build --locked -p clarion-launcher
  --run    Open the already-built Phase 0 window in an isolated test compositor.

--run requires CLARION_LAUNCHER_ISOLATED_TEST=1 and distinct parent/test
WAYLAND_DISPLAY and CLARION_SOCKET values. Set CLARION_LAUNCHER_PARENT_DISPLAY
and CLARION_LAUNCHER_PARENT_SOCKET to the parent session's original values.
Default output: eDP-1; default lifetime: 5 seconds. See docs/launcher-phase0.md.
EOF
}

while (($#)); do
    case "$1" in
        --check|--build|--run)
            [[ "$operation" == diagnose ]] || { echo 'Choose one mode.' >&2; exit 2; }
            operation=${1#--}
            shift
            ;;
        --output)
            [[ $# -ge 2 && -n "$2" && "$2" != -* ]] || { echo '--output needs a name.' >&2; exit 2; }
            output_name=$2
            shift 2
            ;;
        --quit-after)
            [[ $# -ge 2 && "$2" =~ ^[0-9]+$ ]] || { echo '--quit-after needs whole seconds.' >&2; exit 2; }
            quit_after=$2
            [[ ${#quit_after} -le 3 ]] || { echo '--quit-after must be between 1 and 120 seconds.' >&2; exit 2; }
            quit_after=$((10#$quit_after))
            [[ $quit_after -ge 1 && $quit_after -le 120 ]] || {
                echo '--quit-after must be between 1 and 120 seconds.' >&2
                exit 2
            }
            shift 2
            ;;
        -h|--help) usage; exit 0 ;;
        *) printf 'Unknown argument: %s\n' "$1" >&2; usage >&2; exit 2 ;;
    esac
done

cd -- "$repo_dir"
case "$operation" in
    check|build)
        exec cargo "$operation" --locked -p clarion-launcher
        ;;
    run)
        [[ "${CLARION_LAUNCHER_ISOLATED_TEST:-}" == 1 ]] || {
            echo '--run requires CLARION_LAUNCHER_ISOLATED_TEST=1 in an isolated compositor.' >&2
            exit 2
        }
        [[ -n "${WAYLAND_DISPLAY:-}" && -n "${CLARION_LAUNCHER_PARENT_DISPLAY:-}" &&
           "$WAYLAND_DISPLAY" != "$CLARION_LAUNCHER_PARENT_DISPLAY" &&
           -n "${CLARION_SOCKET:-}" && -n "${CLARION_LAUNCHER_PARENT_SOCKET:-}" &&
           "$CLARION_SOCKET" != "$CLARION_LAUNCHER_PARENT_SOCKET" ]] || {
            echo 'Test Wayland display and Clarion IPC socket must differ from the parent session.' >&2
            exit 2
        }
        [[ -x target/debug/clarion-launcher ]] || {
            echo 'Build first: scripts/check-launcher-spike.sh --build' >&2
            exit 1
        }
        exec target/debug/clarion-launcher --spike --output "$output_name" --quit-after "$quit_after"
        ;;
esac

printf 'Repository: %s\n' "$repo_dir"
for tool in rustc cargo pkg-config wayland-info; do
    if command -v "$tool" >/dev/null 2>&1; then
        command -v "$tool"
    else
        printf 'Missing tool: %s\n' "$tool"
    fi
done
if command -v rustc >/dev/null 2>&1; then rustc --version; fi
if command -v cargo >/dev/null 2>&1; then cargo --version; fi
if command -v rustup >/dev/null 2>&1; then rustup toolchain list; fi
if command -v pkg-config >/dev/null 2>&1; then
    for library in fontconfig freetype2 wayland-client xkbcommon vulkan egl; do
        if version=$(pkg-config --modversion "$library" 2>/dev/null); then
            printf '%s: %s\n' "$library" "$version"
        else
            printf 'Missing development library: %s\n' "$library"
        fi
    done
fi
printf 'Session type: %s; desktop: %s; display: %s\n' \
    "${XDG_SESSION_TYPE:-unset}" "${XDG_CURRENT_DESKTOP:-unset}" "${WAYLAND_DISPLAY:-unset}"
printf 'Clarion socket: %s\n' "${CLARION_SOCKET:-automatic discovery}"
if command -v wayland-info >/dev/null 2>&1 && [[ -n "${WAYLAND_DISPLAY:-}" ]]; then
    if globals=$(wayland-info 2>/dev/null); then
        printf '%s\n' "$globals" | sed -n \
            "/interface: 'zwlr_layer_shell_v1'/p; /interface: 'zwp_text_input_manager_v3'/p; /interface: 'zwp_input_method_manager_v2'/p; /interface: 'ext_background_effect_manager_v1'/p; /interface: 'wl_output'/p; /name: '/p; /logical_width:/p; /logical_height:/p; /scale:/p"
    else
        echo 'Cannot inspect the Wayland connection.'
    fi
fi
clarion_binary=''
for candidate in "$repo_dir/target/release/clarion" "$repo_dir/target/debug/clarion"; do
    if [[ -x "$candidate" ]]; then clarion_binary=$candidate; break; fi
done
if [[ -z "$clarion_binary" ]] && command -v clarion >/dev/null 2>&1; then
    clarion_binary=$(command -v clarion)
fi
if [[ -n "$clarion_binary" ]]; then
    echo 'Current layer surfaces (read-only IPC):'
    "$clarion_binary" msg -j layers || echo 'Cannot query Clarion layers.'
    echo 'Current outputs (read-only IPC):'
    "$clarion_binary" msg -j outputs || echo 'Cannot query Clarion outputs.'
fi
printf '%s\n' \
    'Build: cargo build --locked -p clarion-launcher' \
    'Compile check: cargo check --locked -p clarion-launcher' \
    'MSRV check: cargo +1.88.0 check --locked -p clarion-launcher' \
    'No launcher window was opened. Protocol availability does not establish Phase 0 acceptance.'
