"""Check the actual Nix source filter against Cargo's local build inputs.

This does not evaluate Nix or claim a successful Nix build. It catches missing
workspace crates and embedded resources before an isolated build loses them.
"""
from pathlib import Path
import contextlib
import hashlib
import importlib.util
import io
import json
import os
import re
import subprocess
import sys
import tempfile
from unittest import mock
import tomllib
import unittest


ROOT = Path(__file__).resolve().parents[1]


class NixSourceClosureTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.manifest = tomllib.loads((ROOT / "Cargo.toml").read_text())
        source = (ROOT / "flake.nix").read_text()
        fileset = re.search(r"fileset\s*=\s*lib\.fileset\.unions\s*\[(.*?)\];", source, re.S)
        if fileset is None:
            raise AssertionError("Update source-closure checks for the new Nix source filter")
        literals = re.sub(r"#[^\n]*", "", fileset[1]).split()
        if not all(re.fullmatch(r"\./[\w./-]+", literal) for literal in literals):
            raise AssertionError("Nix source-closure check requires explicit source paths")
        cls.sources = [(ROOT / literal).resolve() for literal in literals]
        cls.members = [ROOT]
        for pattern in cls.manifest["workspace"]["members"]:
            members = list(ROOT.glob(pattern))
            if not members:
                raise AssertionError(f"Workspace member does not exist: {pattern}")
            cls.members.extend(members)

    def assert_in_source(self, path):
        path = path.resolve()
        self.assertTrue(
            any(path == source or path.is_relative_to(source) for source in self.sources),
            f"Nix source filter omits {path.relative_to(ROOT)}",
        )

    def test_nix_source_paths_exist_in_checkout(self):
        for source in self.sources:
            with self.subTest(source=source.relative_to(ROOT)):
                self.assertTrue(
                    source.exists(),
                    f"Nix source filter references a missing path: {source.relative_to(ROOT)}",
                )

    def test_nix_source_contains_every_workspace_crate(self):
        for member in self.members:
            with self.subTest(member=member.relative_to(ROOT)):
                self.assertTrue((member / "Cargo.toml").is_file())
                self.assert_in_source(member / "Cargo.toml")
                self.assert_in_source(member / "src")

    def test_nix_source_contains_local_path_dependencies_and_patches(self):
        def local_paths(value, in_dependencies=False):
            if isinstance(value, dict):
                for key, child in value.items():
                    if in_dependencies and key == "path" and isinstance(child, str):
                        yield child
                    else:
                        dependency_table = key in {
                            "dependencies", "dev-dependencies", "build-dependencies",
                            "patch", "replace",
                        }
                        yield from local_paths(child, in_dependencies or dependency_table)

        for member in self.members:
            manifest = tomllib.loads((member / "Cargo.toml").read_text())
            for relative in local_paths(manifest):
                dependency = (member / relative).resolve()
                with self.subTest(member=member.relative_to(ROOT), dependency=relative):
                    self.assertTrue((dependency / "Cargo.toml").is_file())
                    self.assert_in_source(dependency / "Cargo.toml")
                    self.assert_in_source(dependency / "src")

    def test_nix_source_preserves_literal_embedded_resources(self):
        for member in self.members:
            sources = list((member / "src").rglob("*.rs"))
            if (member / "build.rs").is_file():
                sources.append(member / "build.rs")
            for source in sources:
                for relative in re.findall(r'include_(?:bytes|str)!\s*\(\s*"([^"\n]+)"', source.read_text()):
                    with self.subTest(source=source.relative_to(ROOT), resource=relative):
                        resource = source.parent / relative
                        self.assertTrue(resource.is_file(), str(resource))
                        self.assert_in_source(resource)


class LauncherPackagingClosureTests(unittest.TestCase):
    def test_nix_build_and_runtime_closure_includes_gpui_libraries(self):
        source = (ROOT / "flake.nix").read_text()
        inputs = re.search(r"buildInputs\s*=\s*\[(.*?)\]", source, re.S)
        self.assertIsNotNone(inputs)
        for dependency in ("fontconfig", "freetype", "wayland", "libxkbcommon", "vulkan-headers", "vulkan-loader", "libGL"):
            self.assertIn(dependency, inputs[1].split())
        launcher = tomllib.loads((ROOT / "clarion-launcher/Cargo.toml").read_text())
        gpui = tomllib.loads((ROOT / "third_party/gpui/Cargo.toml").read_text())
        # Headless tests compile GPUI's X11 backend even though the shipped Apps
        # client selects Wayland. Keep its native link dependencies in strictDeps.
        self.assertIn("test-support", launcher["dev-dependencies"]["gpui"]["features"])
        self.assertIn("x11", gpui["features"]["test-support"])
        self.assertIn("xorg.libxcb", inputs[1].split())
        xkb = gpui["target"]['cfg(any(target_os = "linux", target_os = "freebsd"))']["dependencies"]["xkbcommon"]
        self.assertIn("x11", xkb["features"])
        self.assertIn("libxkbcommon", inputs[1].split())
        for library in ("-lfontconfig", "-lfreetype", "-lwayland-client", "-lxkbcommon", "-lvulkan", "-lEGL"):
            self.assertIn('"' + library + '"', source)
        for flag_set in ("cargoBuildFlags", "cargoTestFlags"):
            flags = re.search(flag_set + r"\s*=\s*\[(.*?)\]", source, re.S)[1]
            self.assertIn('"--locked"', flags)
            self.assertIn('"--workspace"', flags)
        self.assertIn("/clarion-launcher $out/bin/clarion-launcher", source)

    def test_rpm_and_deb_ship_the_built_launcher(self):
        manifest = tomllib.loads((ROOT / "Cargo.toml").read_text())
        rpm = manifest["package"]["metadata"]["generate-rpm"]["assets"]
        deb = manifest["package"]["metadata"]["deb"]["assets"]
        self.assertTrue(any(asset["source"] == "target/release/clarion-launcher" for asset in rpm))
        self.assertTrue(any(asset[0] == "target/release/clarion-launcher" for asset in deb))
        spec = (ROOT / "clarion.spec.rpkg").read_text()
        self.assertIn("--package clarion-launcher", spec)
        self.assertIn("target/rpm/clarion-launcher", spec)
        self.assertIn("%{_bindir}/clarion-launcher", spec)

    def test_session_installer_stages_all_three_binaries_without_executing_them(self):
        with tempfile.TemporaryDirectory(prefix="clarion session package ") as temporary:
            root = Path(temporary)
            built = root / "built"
            built.mkdir()
            marker = root / "unexpected-execution"
            for binary in ("clarion", "clarion-settings", "clarion-launcher"):
                path = built / binary
                path.write_text(f"#!/bin/sh\ntouch '{marker}'\n")
                path.chmod(0o755)
            stage = root / "stage"
            subprocess.run(["bash", str(ROOT / "scripts/install-session.sh"), str(built)],
                           env={**os.environ, "DESTDIR": str(stage)}, check=True,
                           stdout=subprocess.PIPE, stderr=subprocess.PIPE, timeout=10)
            for binary in ("clarion", "clarion-settings", "clarion-launcher"):
                self.assertEqual((stage / "usr/local/bin" / binary).read_bytes(), (built / binary).read_bytes())
                self.assertTrue(os.access(stage / "usr/local/bin" / binary, os.X_OK))
            self.assertFalse(marker.exists())
            self.assertFalse((stage / "usr/lib/systemd/user/graphical-session.target.wants").exists())

    def test_session_installer_rejects_missing_launcher_before_writing(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            for binary in ("clarion", "clarion-settings"):
                path = root / binary
                path.write_text("#!/bin/sh\nexit 0\n")
                path.chmod(0o755)
            stage = root / "stage"
            result = subprocess.run(["bash", str(ROOT / "scripts/install-session.sh"), str(root)],
                                    env={**os.environ, "DESTDIR": str(stage)},
                                    stdout=subprocess.PIPE, stderr=subprocess.PIPE, timeout=10)
            self.assertNotEqual(result.returncode, 0)
            self.assertIn(b"Build clarion-launcher first", result.stderr)
            self.assertFalse(stage.exists())


class ShellGenerationTests(unittest.TestCase):
    @staticmethod
    def fixture(root, revision):
        bundle = root / ("bundle-" + revision)
        (bundle / "lib").mkdir(parents=True)
        (bundle / "data/flutter_assets").mkdir(parents=True)
        for relative in ("clarion-shell", "lib/libflutter_linux_gtk.so", "data/icudtl.dat"):
            (bundle / relative).write_text("shell-" + revision)
        (bundle / "clarion-shell").chmod(0o755)
        launcher = root / ("apps-" + revision)
        launcher.write_text("apps-" + revision)
        launcher.chmod(0o755)
        return bundle, launcher

    @staticmethod
    def command(root, bundle, launcher):
        return [sys.executable, str(ROOT / "scripts/install-shell.py"),
                "--bundle", str(bundle), "--launcher-binary", str(launcher),
                "--prefix", str(root / "prefix"), "--config-root", str(root / "config")]

    def install(self, root, bundle, launcher):
        result = subprocess.run(self.command(root, bundle, launcher), check=True,
                                text=True, stdout=subprocess.PIPE, stderr=subprocess.PIPE, timeout=10)
        return json.loads(result.stdout)

    def test_shell_and_apps_are_copied_into_one_release_and_old_release_is_retained(self):
        with tempfile.TemporaryDirectory(prefix="clarion generation ") as temporary:
            root = Path(temporary)
            bundle1, launcher1 = self.fixture(root, "one")
            first = self.install(root, bundle1, launcher1)
            release1 = Path(first["release"])
            self.assertEqual((release1 / "clarion-launcher").read_bytes(), launcher1.read_bytes())
            self.assertFalse((release1 / "clarion-launcher").is_symlink())
            self.assertEqual(first["launcher_sha256"], hashlib.sha256(launcher1.read_bytes()).hexdigest())
            bundle2, launcher2 = self.fixture(root, "two")
            second = self.install(root, bundle2, launcher2)
            release2 = Path(second["release"])
            self.assertNotEqual(release1, release2)
            self.assertEqual((root / "prefix/bin/clarion-shell").resolve().parent, release2)
            self.assertEqual((root / "prefix/bin/clarion-launcher").resolve().parent, release2)
            self.assertEqual((release1 / "clarion-launcher").read_bytes(), launcher1.read_bytes())
            self.assertEqual(second["previous"]["apps_launcher"]["target"], str(release1 / "clarion-launcher"))
            self.assertFalse(second["service_enabled_or_started"])
            self.assertFalse(second["existing_shell_changed"])
            self.assertFalse((root / "config/systemd/user/graphical-session.target.wants").exists())

    def test_incomplete_apps_binary_is_rejected_before_any_installation(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            bundle, launcher = self.fixture(root, "one")
            launcher.chmod(0o644)
            result = subprocess.run(self.command(root, bundle, launcher),
                                    stdout=subprocess.PIPE, stderr=subprocess.PIPE, timeout=10)
            self.assertNotEqual(result.returncode, 0)
            self.assertIn(b"Build clarion-launcher first", result.stderr)
            self.assertFalse((root / "prefix").exists())

    def test_failed_install_restores_shell_apps_and_command_aliases_together(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            bundle1, launcher1 = self.fixture(root, "one")
            first = self.install(root, bundle1, launcher1)
            release1 = Path(first["release"])
            bundle2, launcher2 = self.fixture(root, "two")
            spec = importlib.util.spec_from_file_location("install_shell_packaging_test", ROOT / "scripts/install-shell.py")
            installer = importlib.util.module_from_spec(spec)
            spec.loader.exec_module(installer)
            original = installer.atomic_text
            def fail_desktop(path, *args, **kwargs):
                if path.suffix == ".desktop":
                    raise OSError("injected finalization failure")
                return original(path, *args, **kwargs)
            argv = self.command(root, bundle2, launcher2)[1:]
            with mock.patch.object(sys, "argv", argv), mock.patch.object(installer, "atomic_text", side_effect=fail_desktop), contextlib.redirect_stdout(io.StringIO()):
                with self.assertRaisesRegex(OSError, "injected finalization"):
                    installer.main()
            for binary in ("clarion-shell", "clarion-launcher", "clarion-shell-msg"):
                self.assertEqual((root / "prefix/bin" / binary).resolve(), release1 / binary)
            self.assertEqual((release1 / "clarion-launcher").read_bytes(), launcher1.read_bytes())

    def assert_late_failure_restores_paths(self, failure):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            bundle1, launcher1 = self.fixture(root, "one")
            first = self.install(root, bundle1, launcher1)
            # Include aliases, the desktop file and the service definition. A
            # rollback which restores only the two executables is incomplete.
            previous = {}
            for record in first["previous"].values():
                path = Path(record["path"])
                previous[path] = ("link", os.readlink(path)) if path.is_symlink() else ("file", path.read_bytes())
            bundle2, launcher2 = self.fixture(root, "two")
            spec = importlib.util.spec_from_file_location("install_shell_late_failure_test", ROOT / "scripts/install-shell.py")
            installer = importlib.util.module_from_spec(spec)
            spec.loader.exec_module(installer)
            original = getattr(installer, failure)

            def inject(path, *args, **kwargs):
                # Both targets are reached after every public path switched.
                target = "receipt.json" if failure == "atomic_text" else "clarion-launcher"
                if path.name == target:
                    self.assertNotEqual((root / "prefix/bin/clarion-shell").resolve().parent,
                                        Path(first["release"]))
                    raise OSError("injected post-commit finalization failure")
                return original(path, *args, **kwargs)

            argv = self.command(root, bundle2, launcher2)[1:]
            with mock.patch.object(sys, "argv", argv), mock.patch.object(installer, failure, side_effect=inject), contextlib.redirect_stdout(io.StringIO()):
                with self.assertRaisesRegex(OSError, "post-commit finalization"):
                    installer.main()
            for path, expected in previous.items():
                actual = ("link", os.readlink(path)) if path.is_symlink() else ("file", path.read_bytes())
                self.assertEqual(actual, expected, str(path))
            self.assertEqual((Path(first["release"]) / "clarion-launcher").read_bytes(), launcher1.read_bytes())

    def test_receipt_write_failure_rolls_back_every_public_path(self):
        self.assert_late_failure_restores_paths("atomic_text")

    def test_final_hash_read_failure_rolls_back_every_public_path(self):
        self.assert_late_failure_restores_paths("file_sha256")


if __name__ == "__main__":
    unittest.main()
