#!/usr/bin/env python3
"""Install a native Rust Shell generation without changing the active desktop.

The launcher is part of this binary. Only open icons under the supplied icons
folder are copied; old releases and user preferences are never deleted.
"""
import argparse
from datetime import datetime
import fcntl
import hashlib
import json
import os
from pathlib import Path
import shutil
import tempfile

ROOT = Path(__file__).resolve().parent.parent
ASSET_SUFFIXES = {".png", ".svg", ".md", ".txt"}


def sync_directory(path):
    descriptor = os.open(path, os.O_RDONLY | os.O_DIRECTORY)
    try:
        os.fsync(descriptor)
    finally:
        os.close(descriptor)


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
        sync_directory(path.parent)
    finally:
        if os.path.lexists(temporary):
            os.unlink(temporary)


def file_sha256(path):
    digest = hashlib.sha256()
    with path.open("rb") as source:
        for chunk in iter(lambda: source.read(512 * 1024), b""):
            digest.update(chunk)
    return digest.hexdigest()


def checked_inputs(binary, assets):
    binary = binary.resolve(strict=True)
    if not binary.is_file() or not os.access(binary, os.X_OK):
        raise ValueError("Build an executable clarion-shell first with scripts/build-shell.sh --release")
    assets = assets.resolve(strict=True)
    if not assets.is_dir() or not (assets / "LICENSE").is_file():
        raise ValueError("The open icon directory must include its LICENSE")
    files = []
    total = 0
    for path in sorted(assets.rglob("*")):
        if path.is_symlink():
            raise ValueError(f"Asset links are not accepted: {path}")
        if path.is_dir():
            continue
        if not path.is_file() or (path.suffix.lower() not in ASSET_SUFFIXES and "LICENSE" not in path.name):
            raise ValueError(f"Unsupported icon asset: {path}")
        size = path.stat().st_size
        total += size
        if size > 16 * 1024 * 1024 or total > 64 * 1024 * 1024:
            raise ValueError("Icon assets exceed the bounded installation budget")
        files.append((path.relative_to(assets), file_sha256(path)))
    if not any(path.suffix.lower() in {".png", ".svg"} for path, _ in files):
        raise ValueError("No open icon files were supplied")
    return binary, assets, files


def replace_link(path, target, tag):
    path.parent.mkdir(parents=True, exist_ok=True)
    temporary = path.with_name(f".{path.name}-{tag}")
    try:
        temporary.symlink_to(target)
        os.replace(temporary, path)
        sync_directory(path.parent)
    finally:
        if temporary.is_symlink():
            temporary.unlink()


def public_paths(prefix, config):
    return {
        "current": prefix / "lib/clarion-shell/current",
        "launcher": prefix / "bin/clarion-shell",
        "legacy_launcher": prefix / "bin/realand-shell",
        "command": prefix / "bin/clarion-shell-msg",
        "legacy_command": prefix / "bin/realand-shell-msg",
        "desktop": prefix / "share/applications/clarion-shell.desktop",
        "service": config / "systemd/user/clarion-shell.service",
    }


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", type=Path, default=ROOT / "target/release/clarion-shell")
    parser.add_argument("--assets", type=Path, default=ROOT / "clarion-shell/assets/icons",
                        help="Open icon directory containing LICENSE; never the old private artwork folder")
    parser.add_argument("--prefix", type=Path, default=Path.home() / ".local")
    parser.add_argument("--config-root", type=Path)
    parser.add_argument("--check-only", action="store_true")
    arguments = parser.parse_args()
    try:
        binary, assets, asset_files = checked_inputs(arguments.binary, arguments.assets)
    except (OSError, ValueError) as error:
        parser.error(str(error))
    prefix = arguments.prefix.resolve()
    config_env = os.environ.get("XDG_CONFIG_HOME", "")
    config = (arguments.config_root or
              (Path(config_env) if config_env.startswith("/") else Path.home() / ".config")).resolve()
    paths = public_paths(prefix, config)
    for path in paths.values():
        if path.exists() and not path.is_symlink() and not path.is_file():
            parser.error(f"Refusing to replace an unmanaged directory or special file: {path}")
    source_hash = file_sha256(binary)
    if arguments.check_only:
        print(json.dumps({"status": "checked", "binary": str(binary), "sha256": source_hash,
                          "assets": len(asset_files), "existing_shell_changed": False}, indent=2))
        return

    root = prefix / "lib/clarion-shell"
    root.mkdir(parents=True, exist_ok=True)
    tag = "native-" + datetime.now().strftime("%Y%m%d-%H%M%S-%f")
    with (root / ".install.lock").open("a") as lock:
        fcntl.flock(lock, fcntl.LOCK_EX)
        for path in paths.values():
            if path.exists() and not path.is_symlink() and not path.is_file():
                raise ValueError(f"Refusing to replace an unmanaged directory or special file: {path}")
        backup = root / "backups" / tag
        backup.mkdir(parents=True)
        previous = {}
        for name, path in paths.items():
            if path.is_symlink():
                previous[name] = {"path": str(path), "target": os.readlink(path)}
            elif path.is_file():
                shutil.copy2(path, backup / name)
                previous[name] = {"path": str(path), "copy": str(backup / name)}
            else:
                previous[name] = {"path": str(path), "absent": True}
        atomic_text(backup / "previous.json", json.dumps(previous, ensure_ascii=False, indent=2) + "\n")
        staging = Path(tempfile.mkdtemp(prefix=".native-", dir=root))
        release = root / "releases" / tag
        mutated = []
        try:
            (staging / "bin").mkdir()
            shutil.copy2(binary, staging / "bin/clarion-shell")
            (staging / "bin/clarion-shell").chmod(0o755)
            shutil.copy2(ROOT / "resources/clarion-shell-msg", staging / "bin/clarion-shell-msg")
            (staging / "bin/clarion-shell-msg").chmod(0o755)
            for relative, expected_hash in asset_files:
                destination = staging / "assets/icons" / relative
                destination.parent.mkdir(parents=True, exist_ok=True)
                shutil.copy2(assets / relative, destination)
                destination.chmod(0o644)
                if file_sha256(destination) != expected_hash:
                    raise ValueError(f"Icon changed during installation: {relative}")
            if file_sha256(staging / "bin/clarion-shell") != source_hash:
                raise ValueError("Shell binary changed during installation")
            manifest = {"toolkit": "gpui", "binary_sha256": source_hash,
                        "icons": {str(path): digest for path, digest in asset_files}}
            atomic_text(staging / "manifest.json", json.dumps(manifest, indent=2) + "\n")
            for path in staging.rglob("*"):
                if path.is_file():
                    with path.open("rb") as source:
                        os.fsync(source.fileno())
            for path in sorted((path for path in staging.rglob("*") if path.is_dir()),
                               key=lambda path: len(path.parts), reverse=True):
                sync_directory(path)
            sync_directory(staging)
            release.parent.mkdir(parents=True, exist_ok=True)
            staging.rename(release)
            sync_directory(release.parent)
            links = {
                "current": Path("releases") / tag,
                "launcher": root / "current/bin/clarion-shell",
                "legacy_launcher": paths["launcher"],
                "command": root / "current/bin/clarion-shell-msg",
                "legacy_command": paths["command"],
            }
            for name, target in links.items():
                mutated.append(name)
                replace_link(paths[name], target, tag)
            # Desktop entry escaping follows the Desktop Entry executable rules.
            executable = str(paths["launcher"]).replace("\\", "\\\\").replace('"', '\\"').replace("`", "\\`").replace("$", "\\$").replace("%", "%%")
            desktop = (ROOT / "resources/clarion-shell.desktop").read_text()
            mutated.append("desktop")
            atomic_text(paths["desktop"], desktop.replace("Exec=clarion-shell --preview", f'Exec="{executable}" --preview'))
            unit_executable = str(paths["launcher"]).replace("\\", "\\\\").replace('"', '\\"').replace("%", "%%").replace("$", "$$")
            unit = (ROOT / "resources/clarion-shell.service").read_text()
            mutated.append("service")
            atomic_text(paths["service"], unit.replace("ExecStart=clarion-shell --monitor=0", f'ExecStart="{unit_executable}" --monitor=0'))
            # Finalization failures roll back every public entry, including the
            # single generation pointer, without touching any previous release.
            receipt = {
                "status": "installed", "toolkit": "gpui", "release": str(release),
                "backup": str(backup), "previous": previous,
                "sha256": file_sha256(release / "bin/clarion-shell"),
                "assets": len(asset_files), "standalone_launcher_installed": False,
                "service_enabled_or_started": False, "existing_shell_changed": False,
                "preferences_changed": False,
            }
            atomic_text(backup / "receipt.json", json.dumps(receipt, ensure_ascii=False, indent=2) + "\n")
        except BaseException:
            for name in reversed(mutated):
                path = paths[name]
                if path.exists() or path.is_symlink():
                    path.unlink()
                record = previous[name]
                if "target" in record:
                    path.symlink_to(record["target"])
                elif "copy" in record:
                    shutil.copy2(record["copy"], path)
                sync_directory(path.parent)
            raise
        finally:
            if staging.exists():
                shutil.rmtree(staging)
        print(json.dumps(receipt, ensure_ascii=False, indent=2))


if __name__ == "__main__":
    main()
