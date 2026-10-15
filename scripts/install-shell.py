#!/usr/bin/env python3
"""Install a complete shell bundle without changing the active desktop."""
import argparse
from datetime import datetime
import fcntl
import hashlib
import json
import os
from pathlib import Path
import shutil
import tempfile


def atomic_text(path, text, mode=0o644):
    path.parent.mkdir(parents=True, exist_ok=True)
    descriptor, temporary = tempfile.mkstemp(prefix=".realand-shell-", dir=path.parent)
    try:
        with os.fdopen(descriptor, "w") as output:
            output.write(text)
            output.flush()
            os.fsync(output.fileno())
        os.chmod(temporary, mode)
        os.replace(temporary, path)
    finally:
        if os.path.exists(temporary):
            os.unlink(temporary)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--bundle", type=Path, required=True)
    parser.add_argument("--prefix", type=Path, default=Path.home() / ".local")
    parser.add_argument("--config-root", type=Path)
    arguments = parser.parse_args()
    bundle = arguments.bundle.resolve(strict=True)
    for required in ["realand-shell", "lib/libflutter_linux_gtk.so", "data/icudtl.dat", "data/flutter_assets"]:
        if not (bundle / required).exists():
            parser.error(f"Incomplete bundle: {required}")
    prefix = arguments.prefix.resolve()
    root = prefix / "lib/realand-shell"
    root.mkdir(parents=True, exist_ok=True)
    xdg_config = os.environ.get("XDG_CONFIG_HOME", "")
    config = arguments.config_root or (Path(xdg_config) if xdg_config.startswith("/") else Path.home() / ".config")
    paths = {
        "launcher": prefix / "bin/realand-shell",
        "desktop": prefix / "share/applications/realand-shell.desktop",
        "service": config / "systemd/user/realand-shell.service",
    }
    tag = datetime.now().strftime("%Y%m%d-%H%M%S-%f")
    with (root / ".install.lock").open("a") as lock:
        fcntl.flock(lock, fcntl.LOCK_EX)
        backup = root / "backups" / tag
        backup.mkdir(parents=True)
        previous = {}
        for name, path in paths.items():
            if path.is_symlink():
                previous[name] = {"path": str(path), "target": os.readlink(path)}
                (backup / name).symlink_to(os.readlink(path))
            elif path.exists():
                shutil.copy2(path, backup / name)
                previous[name] = {"path": str(path), "copy": str(backup / name)}
            else:
                previous[name] = {"path": str(path), "absent": True}
        staging = Path(tempfile.mkdtemp(prefix=".bundle-", dir=root))
        release = root / "releases" / tag
        try:
            shutil.copytree(bundle, staging, dirs_exist_ok=True, symlinks=True)
            release.parent.mkdir(parents=True, exist_ok=True)
            staging.rename(release)
            launcher = paths["launcher"]
            launcher.parent.mkdir(parents=True, exist_ok=True)
            temporary = launcher.with_name(f".realand-shell-{tag}")
            temporary.symlink_to(release / "realand-shell")
            os.replace(temporary, launcher)
            # The application-menu entry deliberately opens a regular preview.
            # No unit is enabled or started by installing these files.
            escaped = str(launcher).replace("\\", "\\\\").replace('"', '\\"').replace("`", "\\`").replace("$", "\\$").replace("%", "%%")
            atomic_text(paths["desktop"], f'''[Desktop Entry]
Type=Application
Name=Realand Shell Preview
Name[zh_CN]=Realand 桌面外壳预览
Comment=Preview the Realand desktop shell
Exec="{escaped}" --preview
Icon=preferences-desktop
Terminal=false
Categories=System;Settings;
''')
            unit_executable = str(launcher).replace("\\", "\\\\").replace('"', '\\"').replace("%", "%%").replace("$", "$$")
            atomic_text(paths["service"], f'''[Unit]
Description=Realand Desktop Shell
ConditionEnvironment=XDG_CURRENT_DESKTOP=realand
PartOf=graphical-session.target
# GTK may synchronously activate xdg-desktop-portal while registering its
# application. Portal waits for graphical-session.target, so the shell must
# start after that target. Keep client autostart behind the menu registrar.
After=realand.service graphical-session.target
Before=xdg-desktop-autostart.target

[Service]
Type=notify
NotifyAccess=all
TimeoutStartSec=30s
ExecStart="{unit_executable}" --monitor=0
Restart=on-failure
RestartSec=2s

[Install]
WantedBy=graphical-session.target
''')
        except BaseException:
            for name, path in paths.items():
                if path.exists() or path.is_symlink():
                    path.unlink()
                record = previous[name]
                if "target" in record:
                    path.symlink_to(record["target"])
                elif "copy" in record:
                    shutil.copy2(record["copy"], path)
            if staging.exists():
                shutil.rmtree(staging)
            raise
        receipt = {
            "release": str(release), "backup": str(backup), "previous": previous,
            "sha256": hashlib.sha256((release / "realand-shell").read_bytes()).hexdigest(),
            "service_enabled_or_started": False, "existing_shell_changed": False,
        }
        atomic_text(backup / "receipt.json", json.dumps(receipt, ensure_ascii=False, indent=2) + "\n")
        print(json.dumps(receipt, ensure_ascii=False, indent=2))


if __name__ == "__main__":
    main()
