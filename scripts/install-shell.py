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
    descriptor, temporary = tempfile.mkstemp(prefix=".clarion-shell-", dir=path.parent)
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



def file_sha256(path):
    digest = hashlib.sha256()
    with path.open("rb") as source:
        for chunk in iter(lambda: source.read(512 * 1024), b""):
            digest.update(chunk)
    return digest.hexdigest()

def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--bundle", type=Path, required=True)
    parser.add_argument("--launcher-binary", type=Path,
                        help="Built GPUI launcher; defaults to the bundled executable or target/release/clarion-launcher")
    parser.add_argument("--prefix", type=Path, default=Path.home() / ".local")
    parser.add_argument("--config-root", type=Path)
    arguments = parser.parse_args()
    bundle = arguments.bundle.resolve(strict=True)
    for required in ["clarion-shell", "lib/libflutter_linux_gtk.so", "data/icudtl.dat", "data/flutter_assets"]:
        if not (bundle / required).exists():
            parser.error(f"Incomplete bundle: {required}")
    if arguments.launcher_binary is not None:
        launcher_binary = arguments.launcher_binary
    elif (bundle / "clarion-launcher").exists():
        launcher_binary = bundle / "clarion-launcher"
    else:
        launcher_binary = Path(__file__).resolve().parent.parent / "target/release/clarion-launcher"
    if not launcher_binary.is_file() or not os.access(launcher_binary, os.X_OK):
        parser.error("Build clarion-launcher first with cargo build --locked --release -p clarion-launcher, "
                     "or supply an executable --launcher-binary")
    launcher_binary = launcher_binary.resolve(strict=True)
    prefix = arguments.prefix.resolve()
    root = prefix / "lib/clarion-shell"
    root.mkdir(parents=True, exist_ok=True)
    xdg_config = os.environ.get("XDG_CONFIG_HOME", "")
    config = arguments.config_root or (Path(xdg_config) if xdg_config.startswith("/") else Path.home() / ".config")
    paths = {
        "launcher": prefix / "bin/clarion-shell",
        "legacy_launcher": prefix / "bin/realand-shell",
        "apps_launcher": prefix / "bin/clarion-launcher",
        "command": prefix / "bin/clarion-shell-msg",
        "legacy_command": prefix / "bin/realand-shell-msg",
        "desktop": prefix / "share/applications/clarion-shell.desktop",
        "service": config / "systemd/user/clarion-shell.service",
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
            # Pin Apps to the running Shell's release. A later bin symlink switch
            # cannot make an already-running Shell spawn a different generation.
            bundled_launcher = staging / "clarion-launcher"
            if bundled_launcher.exists() or bundled_launcher.is_symlink():
                bundled_launcher.unlink()
            shutil.copy2(launcher_binary, bundled_launcher)
            bundled_launcher.chmod(0o755)
            release.parent.mkdir(parents=True, exist_ok=True)
            shutil.copy2(Path(__file__).resolve().parent.parent / "resources/clarion-shell-msg",
                         staging / "clarion-shell-msg")
            (staging / "clarion-shell-msg").chmod(0o755)
            staging.rename(release)
            launcher = paths["launcher"]
            launcher.parent.mkdir(parents=True, exist_ok=True)
            temporary = launcher.with_name(f".clarion-shell-{tag}")
            temporary.symlink_to(release / "clarion-shell")
            os.replace(temporary, launcher)
            legacy = paths["legacy_launcher"]
            temporary = legacy.with_name(f".realand-shell-{tag}")
            temporary.symlink_to(launcher)
            os.replace(temporary, legacy)
            for key, target in (("apps_launcher", release / "clarion-launcher"),
                                ("command", release / "clarion-shell-msg"),
                                ("legacy_command", paths["command"])):
                path = paths[key]
                temporary = path.with_name(f".{path.name}-{tag}")
                temporary.symlink_to(target)
                os.replace(temporary, path)
            # The application-menu entry deliberately opens a regular preview.
            # No unit is enabled or started by installing these files.
            escaped = str(launcher).replace("\\", "\\\\").replace('"', '\\"').replace("`", "\\`").replace("$", "\\$").replace("%", "%%")
            atomic_text(paths["desktop"], f'''[Desktop Entry]
Type=Application
Name=Clarion Shell Preview
Comment=Preview the Clarion desktop shell
Exec="{escaped}" --preview
Icon=preferences-desktop
Terminal=false
Categories=System;Settings;
''')
            unit_executable = str(launcher).replace("\\", "\\\\").replace('"', '\\"').replace("%", "%%").replace("$", "$$")
            atomic_text(paths["service"], f'''[Unit]
Description=Clarion Desktop Shell
ConditionEnvironment=XDG_CURRENT_DESKTOP=clarion
PartOf=graphical-session.target
# GTK may synchronously activate xdg-desktop-portal while registering its
# application. Portal waits for graphical-session.target, so the shell must
# start after that target. Keep client autostart behind the menu registrar.
After=clarion.service graphical-session.target
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
            # Finalization is part of the transaction too. A failed hash read or
            # receipt write must restore the public paths just like a copy or
            # symlink failure, rather than leave an unreported new generation.
            receipt = {
                "release": str(release), "backup": str(backup), "previous": previous,
                "sha256": file_sha256(release / "clarion-shell"),
                "launcher_source": str(launcher_binary),
                "launcher_sha256": file_sha256(release / "clarion-launcher"),
                "service_enabled_or_started": False, "existing_shell_changed": False,
            }
            atomic_text(backup / "receipt.json", json.dumps(receipt, ensure_ascii=False, indent=2) + "\n")
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
        print(json.dumps(receipt, ensure_ascii=False, indent=2))


if __name__ == "__main__":
    main()
