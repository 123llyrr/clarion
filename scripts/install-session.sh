#!/bin/bash
# Run after: cargo build --release -p realand -p realand-settings
set -euo pipefail
repo_dir=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)
build_dir=${1:-"$repo_dir/target/release"}
staging_root=${DESTDIR:-}
if [[ -z "$staging_root" && $EUID -ne 0 ]]; then
    echo "Run with sudo: sudo bash $repo_dir/scripts/install-session.sh" >&2
    exit 1
fi
for binary in realand realand-settings; do
    [[ -x "$build_dir/$binary" ]] || { echo "Build $binary first." >&2; exit 1; }
done
scratch_dir=$(mktemp -d)
trap 'rm -rf -- "$scratch_dir"' EXIT
sed 's|^Exec=realand-session$|Exec=/usr/local/bin/realand-session|' \
    "$repo_dir/resources/realand.desktop" > "$scratch_dir/realand.desktop"
sed 's|^ExecStart=realand |ExecStart=/usr/local/bin/realand |' \
    "$repo_dir/resources/realand.service" > "$scratch_dir/realand.service"
install_file() {
    local source=$1 destination="$staging_root$2" mode=$3
    install -d -- "$(dirname -- "$destination")"
    if [[ -e "$destination" && ! -e "$destination.bak" ]]; then
        cp -p -- "$destination" "$destination.bak"
    fi
    # Replace atomically, including when the executable is currently running.
    install -m "$mode" -- "$source" "$destination.new"
    mv -f -- "$destination.new" "$destination"
}
install_file "$build_dir/realand" /usr/local/bin/realand 755
install_file "$build_dir/realand-settings" /usr/local/bin/realand-settings 755
install_file "$repo_dir/resources/realand-session" /usr/local/bin/realand-session 755
install_file "$repo_dir/scripts/desktop-health.py" /usr/local/bin/realand-doctor 755
install_file "$scratch_dir/realand.desktop" /usr/share/wayland-sessions/realand.desktop 644
install_file "$scratch_dir/realand.service" /usr/lib/systemd/user/realand.service 644
install_file "$repo_dir/resources/realand-shutdown.target" /usr/lib/systemd/user/realand-shutdown.target 644
install_file "$repo_dir/resources/realand-portals.conf" /usr/share/xdg-desktop-portal/realand-portals.conf 644
install_file "$repo_dir/LICENSE" /usr/share/licenses/realand/LICENSE 644
for notice in "$repo_dir"/resources/licenses/*.txt; do
    install_file "$notice" "/usr/share/licenses/realand/$(basename -- "$notice")" 644
done
install_file "$repo_dir/docs/denial-reference.txt" /usr/share/doc/realand/denial-reference.txt 644
printf '%s\n' 'Installed Realand session. Log out and select Realand in the login screen.'
