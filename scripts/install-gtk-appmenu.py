#!/usr/bin/env python3
"""Install a staged GTK3 appmenu module in a user prefix without session writes.

The stage may be a DESTDIR root or its usr/ subdirectory. A nearby
source-provenance.json must identify the complete source tree used to build it.
Each release contains that source, license, schema and private parser library.
"""
import argparse
from datetime import datetime
import fcntl
import hashlib
import json
import os
from pathlib import Path
import shutil
import subprocess
import tarfile
import tempfile


def digest(path):
    return hashlib.sha256(path.read_bytes()).hexdigest()


def sync_directory(path):
    descriptor = os.open(path, os.O_RDONLY | os.O_DIRECTORY)
    try:
        os.fsync(descriptor)
    finally:
        os.close(descriptor)


def find_provenance(stage):
    for directory in [stage, *list(stage.parents)[:4]]:
        candidate = directory / "source-provenance.json"
        if candidate.is_file():
            return candidate
    raise ValueError("source-provenance.json is required beside the staged build")


def install(stage, prefix):
    if not (stage / "lib/gtk-3.0/modules/libappmenu-gtk-module.so").is_file():
        stage = stage / "usr"
    module = stage / "lib/gtk-3.0/modules/libappmenu-gtk-module.so"
    parser = stage / "lib/libappmenu-gtk3-parser.so.0"
    schema = stage / "share/glib-2.0/schemas/org.appmenu.gtk-module.gschema.xml"
    license_file = stage / "share/licenses/appmenu-gtk-module/LICENSE"
    for required in [module, parser, schema, license_file]:
        if not required.is_file():
            raise ValueError(f"Incomplete GTK3 module stage: {required}")
    provenance_file = find_provenance(stage)
    upstream = json.loads(provenance_file.read_text())
    source = Path(upstream["source"]).resolve(strict=True)
    if not source.is_dir() or not any(source.rglob("appmenu-gtk-module.c")):
        raise ValueError("Provenance must identify the complete GTK module source tree")
    for tool in ["patchelf", "glib-compile-schemas"]:
        if not shutil.which(tool):
            raise ValueError(f"Required build helper is unavailable: {tool}")

    root = prefix / "lib/clarion-appmenu"
    root.mkdir(parents=True, exist_ok=True)
    tag = datetime.now().strftime("%Y%m%d-%H%M%S-%f")
    with (root / ".install.lock").open("a") as lock:
        fcntl.flock(lock, fcntl.LOCK_EX)
        current = root / "current"
        if current.exists() and current.is_dir() and not current.is_symlink():
            raise ValueError(f"Refusing to replace an unmanaged current directory: {current}")
        previous = {"current": str(current), "absent": not current.exists() and not current.is_symlink()}
        if current.is_symlink():
            previous["target"] = os.readlink(current)
        elif current.exists():
            raise ValueError(f"Refusing to replace an unmanaged current file: {current}")
        backup = root / "backups" / tag
        backup.mkdir(parents=True)
        if current.is_symlink():
            (backup / "current").symlink_to((root / os.readlink(current)).resolve())
        with (backup / "previous.json").open("w") as record:
            record.write(json.dumps(previous, indent=2) + "\n")
            record.flush()
            os.fsync(record.fileno())
        sync_directory(backup)
        temporary = Path(tempfile.mkdtemp(prefix=".release-", dir=root))
        link = root / f".current-{tag}"
        release = root / "releases" / tag
        try:
            installed_module = temporary / "lib/gtk-3.0/modules/libappmenu-gtk-module.so"
            installed_module.parent.mkdir(parents=True)
            shutil.copy2(module, installed_module)
            shutil.copy2(parser.resolve(), temporary / "lib/libappmenu-gtk3-parser.so.0")
            # GTK loads the absolute module path. Resolve its parser privately
            # without LD_LIBRARY_PATH or changing the user's dynamic linker.
            subprocess.run(["patchelf", "--set-rpath", "$ORIGIN/../..", str(installed_module)], check=True)

            schemas = temporary / "share/glib-2.0/schemas"
            schemas.mkdir(parents=True)
            shutil.copy2(schema, schemas / schema.name)
            (schemas / "org.appmenu.gtk-module.gschema.override").write_text(
                "[org.appmenu.gtk-module]\nalways-show-inner-menu=true\n")
            subprocess.run(["glib-compile-schemas", "--strict", str(schemas)], check=True)
            licenses = temporary / "share/licenses/appmenu-gtk-module"
            licenses.mkdir(parents=True)
            shutil.copy2(license_file, licenses / "LICENSE")
            sources = temporary / "share/src/appmenu-gtk-module"
            sources.mkdir(parents=True)
            source_archive = sources / "source.tar.gz"
            # Archive the actual build source, preserving any local fixes along
            # with the upstream source. No external symlink target is followed.
            with tarfile.open(source_archive, "w:gz", dereference=False) as archive:
                archive.add(source, arcname="source", recursive=True)
            document = {
                "upstream": upstream,
                "source_archive_sha256": digest(source_archive),
                "staged_module_sha256": digest(module),
                "installed_module_sha256": digest(installed_module),
                "parser_sha256": digest(parser.resolve()),
                "license_sha256": digest(license_file),
                "schema_sha256": digest(schema),
                "install_changes": {"module_runpath": "$ORIGIN/../..",
                                    "schema_defaults": {"always-show-inner-menu": True}},
                "policy": "No persistent dconf, environment, service or session changes",
            }
            docs = temporary / "share/doc/clarion-gtk-appmenu"
            docs.mkdir(parents=True)
            (docs / "provenance.json").write_text(json.dumps(document, indent=2) + "\n")
            for path in temporary.rglob("*"):
                if path.is_file() and not path.is_symlink():
                    with path.open("rb") as contents:
                        os.fsync(contents.fileno())
            release.parent.mkdir(parents=True, exist_ok=True)
            temporary.rename(release)
            sync_directory(release.parent)
            link.symlink_to(Path("releases") / tag)
            os.replace(link, current)
            sync_directory(root)
        finally:
            if link.is_symlink():
                link.unlink()
            if temporary.exists():
                shutil.rmtree(temporary)
        return {
            "release": str(release),
            "backup": str(backup),
            "GTK_MODULES_append": str(current / "lib/gtk-3.0/modules/libappmenu-gtk-module.so"),
            "GSETTINGS_SCHEMA_DIR": str(current / "share/glib-2.0/schemas"),
            "provenance": str(current / "share/doc/clarion-gtk-appmenu/provenance.json"),
            "session_changed": False,
        }


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--source", type=Path, required=True, help="staged module DESTDIR or usr/ directory")
    parser.add_argument("--prefix", type=Path, default=Path.home() / ".local")
    arguments = parser.parse_args()
    if os.geteuid() == 0:
        parser.error("Run as the user; this installer does not use root privileges")
    try:
        installed = install(arguments.source.resolve(strict=True), arguments.prefix.resolve())
    except (ValueError, KeyError, OSError, subprocess.CalledProcessError) as error:
        parser.exit(1, f"GTK appmenu installation failed: {error}\n")
    print(json.dumps(installed, indent=2))


if __name__ == "__main__":
    main()
