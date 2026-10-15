#!/usr/bin/env python3
"""Publish the settings app locally, preserving its previous launcher and entry."""
import argparse
from datetime import datetime
import fcntl
import hashlib
import json
import os
from pathlib import Path
import shutil
import tempfile


def sha256(path):
    return hashlib.file_digest(path.open("rb"), "sha256").hexdigest()


def atomic_text(path, text):
    path.parent.mkdir(parents=True, exist_ok=True)
    descriptor, temporary = tempfile.mkstemp(prefix=".realand-settings-", dir=path.parent)
    try:
        with os.fdopen(descriptor, "w") as stream:
            stream.write(text)
            stream.flush()
            os.fsync(stream.fileno())
        os.chmod(temporary, 0o644)
        os.replace(temporary, path)
    finally:
        if os.path.exists(temporary):
            os.unlink(temporary)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", type=Path, required=True)
    parser.add_argument("--prefix", type=Path, default=Path.home() / ".local")
    arguments = parser.parse_args()
    binary = arguments.binary.resolve(strict=True)
    if not binary.is_file() or not os.access(binary, os.X_OK):
        parser.error("--binary must be an executable file")
    prefix = arguments.prefix.resolve()
    root = prefix / "lib/realand-settings"
    root.mkdir(parents=True, exist_ok=True)
    tag = datetime.now().strftime("%Y%m%d-%H%M%S-%f")
    paths = {
        "launcher": prefix / "bin/realand-settings",
        "desktop": prefix / "share/applications/realand-settings.desktop",
    }
    with (root / ".install.lock").open("a") as lock:
        fcntl.flock(lock, fcntl.LOCK_EX)
        backup = root / "backups" / tag
        backup.mkdir(parents=True)
        previous = {}
        for name, path in paths.items():
            if path.is_symlink():
                target = os.readlink(path)
                (backup / name).symlink_to(target)
                previous[name] = {"path": str(path), "target": target}
            elif path.exists():
                shutil.copy2(path, backup / name)
                previous[name] = {"path": str(path), "copy": str(backup / name)}
            else:
                previous[name] = {"path": str(path), "absent": True}
        release = root / "releases" / tag
        release.mkdir(parents=True)
        installed = release / "realand-settings"
        shutil.copy2(binary, installed)
        installed.chmod(0o755)
        digest = sha256(binary)
        if sha256(installed) != digest:
            raise RuntimeError("Copied executable differs from the build")
        launcher = paths["launcher"]
        launcher.parent.mkdir(parents=True, exist_ok=True)
        temporary = launcher.with_name(f".realand-settings-{tag}")
        temporary.symlink_to(installed)
        os.replace(temporary, launcher)
        escaped = str(launcher).replace("\\", "\\\\").replace('"', '\\"').replace("`", "\\`").replace("$", "\\$").replace("%", "%%")
        atomic_text(paths["desktop"], f'''[Desktop Entry]
Type=Application
Name=Realand System Preferences
Name[zh_CN]=系统偏好设置
Comment=Configure the Realand desktop
Comment[zh_CN]=配置 Realand 桌面
Exec="{escaped}"
Icon=preferences-system
Terminal=false
Categories=Settings;DesktopSettings;
StartupNotify=true
StartupWMClass=realand-settings
''')
        receipt = {
            "release": str(release), "binary": str(installed), "sha256": digest,
            "previous": previous, "backup": str(backup),
        }
        atomic_text(release / "installation.json", json.dumps(receipt, indent=2, ensure_ascii=False) + "\n")
        print(json.dumps(receipt, indent=2, ensure_ascii=False))


if __name__ == "__main__":
    main()
