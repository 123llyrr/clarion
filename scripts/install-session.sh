#!/bin/bash
# Run after: cargo build --locked --release -p clarion -p clarion-settings -p clarion-launcher
set -euo pipefail
repo_dir=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)
build_dir=${1:-"$repo_dir/target/release"}
staging_root=${DESTDIR:-}
if [[ -z "$staging_root" && $EUID -ne 0 ]]; then
    echo "Run with sudo: sudo bash $repo_dir/scripts/install-session.sh" >&2
    exit 1
fi
for binary in clarion clarion-settings clarion-launcher; do
    [[ -x "$build_dir/$binary" ]] || { echo "Build $binary first." >&2; exit 1; }
done
scratch_dir=$(mktemp -d)
trap 'rm -rf -- "$scratch_dir"' EXIT
sed 's|^Exec=clarion-session$|Exec=/usr/local/bin/clarion-session|' \
    "$repo_dir/resources/clarion.desktop" > "$scratch_dir/clarion.desktop"
sed 's|^ExecStart=clarion |ExecStart=/usr/local/bin/clarion |' \
    "$repo_dir/resources/clarion.service" > "$scratch_dir/clarion.service"
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
install_file "$build_dir/clarion" /usr/local/bin/clarion 755
install_file "$build_dir/clarion-settings" /usr/local/bin/clarion-settings 755
install_file "$build_dir/clarion-launcher" /usr/local/bin/clarion-launcher 755
install_file "$repo_dir/resources/clarion-session" /usr/local/bin/clarion-session 755
install_file "$repo_dir/scripts/desktop-health.py" /usr/local/bin/clarion-doctor 755
install_file "$repo_dir/resources/clarion-shell-msg" /usr/local/bin/clarion-shell-msg 755
for legacy in realand realand-session realand-settings realand-shell-msg; do
    install_file "$repo_dir/resources/compat/$legacy" "/usr/local/bin/$legacy" 755
done
install_file "$scratch_dir/clarion.desktop" /usr/share/wayland-sessions/clarion.desktop 644
install_file "$scratch_dir/clarion.service" /usr/lib/systemd/user/clarion.service 644
install_file "$repo_dir/resources/clarion-shutdown.target" /usr/lib/systemd/user/clarion-shutdown.target 644
install_file "$repo_dir/resources/clarion-portals.conf" /usr/share/xdg-desktop-portal/clarion-portals.conf 644
install_file "$repo_dir/LICENSE" /usr/share/licenses/clarion/LICENSE 644
for notice in "$repo_dir"/resources/licenses/*.txt; do
    install_file "$notice" "/usr/share/licenses/clarion/$(basename -- "$notice")" 644
done
install_file "$repo_dir/docs/denial-reference.txt" /usr/share/doc/clarion/denial-reference.txt 644
printf '%s\n' 'Installed Clarion session. Log out and select Clarion in the login screen.'
