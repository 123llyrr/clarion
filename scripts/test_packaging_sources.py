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
import socket
import subprocess
import sys
import tempfile
import threading
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


class NativeShellPackagingClosureTests(unittest.TestCase):
    def test_nix_build_and_runtime_closure_includes_gpui_libraries(self):
        source = (ROOT / "flake.nix").read_text()
        inputs = re.search(r"buildInputs\s*=\s*\[(.*?)\]", source, re.S)
        self.assertIsNotNone(inputs)
        for dependency in ("fontconfig", "freetype", "wayland", "libxkbcommon", "vulkan-headers", "vulkan-loader", "libGL"):
            self.assertIn(dependency, inputs[1].split())
        shell = tomllib.loads((ROOT / "clarion-shell/Cargo.toml").read_text())
        gpui = tomllib.loads((ROOT / "third_party/gpui/Cargo.toml").read_text())
        # Headless tests compile GPUI's X11 backend even though the shipped Shell
        # client selects Wayland. Keep its native link dependencies in strictDeps.
        self.assertIn("test-support", shell["dev-dependencies"]["gpui"]["features"])
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
        self.assertIn("/clarion-shell $out/bin/clarion-shell", source)

    def test_rpm_and_deb_ship_the_built_shell_with_assets_and_service(self):
        manifest = tomllib.loads((ROOT / "Cargo.toml").read_text())
        rpm = manifest["package"]["metadata"]["generate-rpm"]["assets"]
        deb = manifest["package"]["metadata"]["deb"]["assets"]
        self.assertTrue(any(asset["source"] == "target/release/clarion-shell" for asset in rpm))
        self.assertTrue(any(asset[0] == "target/release/clarion-shell" for asset in deb))
        self.assertFalse(any(asset["source"] == "target/release/clarion-launcher" for asset in rpm))
        self.assertFalse(any(asset[0] == "target/release/clarion-launcher" for asset in deb))
        for resource in ("resources/clarion-shell.service", "resources/clarion-shell.desktop",
                         "clarion-shell/assets/icons/*"):
            self.assertTrue(any(asset["source"] == resource for asset in rpm), resource)
            self.assertTrue(any(asset[0] == resource for asset in deb), resource)
        spec = (ROOT / "clarion.spec.rpkg").read_text()
        self.assertIn("--package clarion-shell", spec)
        self.assertIn("target/rpm/clarion-shell", spec)
        self.assertIn("%{_bindir}/clarion-shell", spec)

    def test_session_installer_stages_all_three_binaries_without_executing_them(self):
        with tempfile.TemporaryDirectory(prefix="clarion session package ") as temporary:
            root = Path(temporary)
            built = root / "built"
            built.mkdir()
            marker = root / "unexpected-execution"
            for binary in ("clarion", "clarion-settings", "clarion-shell"):
                path = built / binary
                path.write_text(f"#!/bin/sh\ntouch '{marker}'\n")
                path.chmod(0o755)
            stage = root / "stage"
            subprocess.run(["bash", str(ROOT / "scripts/install-session.sh"), str(built)],
                           env={**os.environ, "DESTDIR": str(stage)}, check=True,
                           stdout=subprocess.PIPE, stderr=subprocess.PIPE, timeout=10)
            for binary in ("clarion", "clarion-settings", "clarion-shell"):
                self.assertEqual((stage / "usr/local/bin" / binary).read_bytes(), (built / binary).read_bytes())
                self.assertTrue(os.access(stage / "usr/local/bin" / binary, os.X_OK))
            self.assertFalse(marker.exists())
            self.assertFalse((stage / "usr/local/bin/clarion-launcher").exists())
            self.assertTrue((stage / "usr/local/share/clarion-shell/icons/LICENSE").is_file())
            self.assertTrue((stage / "usr/lib/systemd/user/clarion-shell.service").is_file())
            self.assertFalse((stage / "usr/lib/systemd/user/graphical-session.target.wants").exists())

    def test_session_installer_rejects_missing_shell_before_writing(self):
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
            self.assertIn(b"Build clarion-shell first", result.stderr)
            self.assertFalse(stage.exists())


class NativeShellSmokeIsolationTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        spec = importlib.util.spec_from_file_location("native_shell_smoke", ROOT / "scripts/test-native-shell.py")
        cls.smoke = importlib.util.module_from_spec(spec)
        spec.loader.exec_module(cls.smoke)

    def test_private_environment_discards_parent_session_endpoints(self):
        with tempfile.TemporaryDirectory() as temporary:
            private = Path(temporary)
            inherited = {"CLARION_SOCKET": "/parent/core", "CLARION_SHELL_SOCKET": "/parent/shell",
                         "REALAND_SOCKET": "/parent/legacy", "WAYLAND_DISPLAY": "/parent/wayland",
                         "DBUS_SESSION_BUS_ADDRESS": "unix:path=/parent/bus", "NOTIFY_SOCKET": "/parent/notify",
                         "GTK_MODULES": "/parent/module", "HOME": "/parent/home"}
            with mock.patch.dict(os.environ, inherited):
                env = self.smoke.isolated_environment(private)
            for endpoint in inherited.keys() - {"HOME"}:
                self.assertNotIn(endpoint, env)
            self.assertEqual(env["HOME"], str(private / "home"))
            self.assertEqual(Path(env["XDG_RUNTIME_DIR"]).stat().st_mode & 0o777, 0o700)

    def test_worker_requires_owned_marker_private_bus_and_isolated_paths(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            private = root / "owned"
            private.mkdir()
            parent = root / "parent-wayland"
            with socket.socket(socket.AF_UNIX) as surface:
                surface.bind(str(parent))
                env = self.smoke.isolated_environment(private)
                (private / self.smoke.MARKER).write_text("fixture-token")
                env.update(CLARION_NATIVE_SMOKE_ROOT=str(private), CLARION_NATIVE_SMOKE_TOKEN="fixture-token",
                           CLARION_NATIVE_SMOKE_PARENT=str(parent),
                           DBUS_SESSION_BUS_ADDRESS=f"unix:path={private}/runtime/bus,guid=fixture")
                with mock.patch.dict(os.environ, env, clear=True):
                    actual, runtime, actual_parent = self.smoke.guarded_worker_environment(private)
                self.assertEqual(runtime, private / "runtime")
                self.assertEqual(actual_parent, parent)
                for replacement in ({"DBUS_SESSION_BUS_ADDRESS": "unix:path=/parent/bus"},
                                    {"CLARION_NATIVE_SMOKE_TOKEN": "wrong"},
                                    {"CLARION_SOCKET": "/parent/core"},
                                    {"HOME": "/parent/home"}):
                    with self.subTest(replacement=replacement), mock.patch.dict(os.environ, env | replacement, clear=True):
                        with self.assertRaises(RuntimeError):
                            self.smoke.guarded_worker_environment(private)

    def test_native_catalog_fixture_is_nonempty_chinese_and_disposable(self):
        with tempfile.TemporaryDirectory() as temporary:
            private = Path(temporary)
            env = self.smoke.isolated_environment(private)
            identifiers = self.smoke.seed_catalog(private, env)
            self.assertEqual(len(identifiers), 20)
            applications = Path(env["XDG_DATA_HOME"]) / "applications"
            self.assertEqual(len(list(applications.glob("*.desktop"))), 20)
            for identifier in identifiers:
                text = (applications / identifier).read_text()
                self.assertIn("Name=测试应用", text)
                self.assertIn("Exec=/usr/bin/true\n", text)
                icon = Path(re.search(r"^Icon=(.+)$", text, re.M)[1])
                self.assertTrue(icon.is_relative_to(private) and icon.is_file())
            preferences = json.loads((Path(env["XDG_CONFIG_HOME"]) / "clarion-shell/preferences.json").read_text())
            self.assertEqual(preferences["pinned_app_ids"], identifiers[:6])
            self.assertTrue((private / "home/Desktop/测试文档.txt").is_file())

    def test_native_protocol_trace_requires_effect_and_metadata_only_identity(self):
        effect = ('-> ext_background_effect_manager_v1#7.get_background_effect(new id ext_background_effect_surface_v1#8, wl_surface#9)\n'
                  '-> ext_background_effect_surface_v1#8.set_blur_region(wl_region#10)\n')
        identity = ('-> gtk_surface1@13.set_dbus_properties("io.github.clarion.Desktop.MacOS", nil, nil, nil, nil, nil)\n')
        evidence = self.smoke.protocol_evidence(effect + identity)
        self.assertEqual(evidence["metadata_only_desktop_identities"], 1)
        exported_menu = identity.replace('nil, nil, nil, nil, nil', '"/menu", nil, nil, nil, nil')
        for trace in (effect, identity, effect + exported_menu, effect + identity + exported_menu):
            with self.subTest(trace=trace), self.assertRaises(RuntimeError):
                self.smoke.protocol_evidence(trace)

    def test_ipc_probe_reads_newline_replies_without_waiting_for_eof(self):
        with tempfile.TemporaryDirectory() as temporary:
            path = Path(temporary) / "owned.sock"
            requests = []
            failures = []
            with socket.socket(socket.AF_UNIX, socket.SOCK_STREAM) as listener:
                listener.bind(str(path))
                listener.listen(4)
                listener.settimeout(3)
                def serve():
                    try:
                        for _ in range(4):
                            peer, _address = listener.accept()
                            with peer, peer.makefile("rb") as lines:
                                requests.append(json.loads(lines.readline()))
                                peer.sendall(b'{"Ok":{"Version":"fixture"}}\n')
                    except Exception as error:
                        failures.append(error)
                worker = threading.Thread(target=serve, daemon=True)
                worker.start()
                result = self.smoke.probe_ipc(path)
                worker.join(timeout=3)
                self.assertFalse(worker.is_alive())
            self.assertFalse(failures)
            self.assertEqual(requests, ["Version", "Version", "Outputs", "Outputs"])
            self.assertTrue(all(item["ok"] for item in result), result)
            self.assertEqual([item["half_close"] for item in result], [False, True, False, True])


class ShellGenerationTests(unittest.TestCase):
    @staticmethod
    def fixture(root, revision):
        binary = root / ("shell-" + revision)
        binary.write_text("shell-" + revision)
        binary.chmod(0o755)
        icons = root / ("icons-" + revision)
        icons.mkdir()
        (icons / "LICENSE").write_text("open fixture license\n")
        (icons / "app.svg").write_text('<svg xmlns="http://www.w3.org/2000/svg"><title>' + revision + '</title></svg>')
        return binary, icons

    @staticmethod
    def command(root, binary, icons):
        return [sys.executable, str(ROOT / "scripts/install-shell.py"),
                "--binary", str(binary), "--assets", str(icons),
                "--prefix", str(root / "prefix"), "--config-root", str(root / "config")]

    def install(self, root, binary, icons):
        result = subprocess.run(self.command(root, binary, icons), check=True,
                                text=True, stdout=subprocess.PIPE, stderr=subprocess.PIPE, timeout=10)
        return json.loads(result.stdout)

    def test_native_shell_and_open_icons_share_generation_and_old_release_remains(self):
        with tempfile.TemporaryDirectory(prefix="clarion generation ") as temporary:
            root = Path(temporary)
            binary1, icons1 = self.fixture(root, "one")
            first = self.install(root, binary1, icons1)
            release1 = Path(first["release"])
            self.assertEqual((release1 / "bin/clarion-shell").read_bytes(), binary1.read_bytes())
            self.assertEqual((release1 / "assets/icons/app.svg").read_bytes(), (icons1 / "app.svg").read_bytes())
            self.assertEqual(first["sha256"], hashlib.sha256(binary1.read_bytes()).hexdigest())
            binary2, icons2 = self.fixture(root, "two")
            second = self.install(root, binary2, icons2)
            release2 = Path(second["release"])
            self.assertNotEqual(release1, release2)
            self.assertEqual((root / "prefix/bin/clarion-shell").resolve(), release2 / "bin/clarion-shell")
            self.assertEqual((root / "prefix/bin/clarion-shell-msg").resolve(), release2 / "bin/clarion-shell-msg")
            self.assertEqual((release1 / "bin/clarion-shell").read_bytes(), binary1.read_bytes())
            self.assertFalse((root / "prefix/bin/clarion-launcher").exists())
            self.assertFalse(second["standalone_launcher_installed"])
            self.assertFalse(second["service_enabled_or_started"])
            self.assertFalse(second["existing_shell_changed"])
            self.assertFalse((root / "config/systemd/user/graphical-session.target.wants").exists())
            service = (root / "config/systemd/user/clarion-shell.service").read_text()
            self.assertIn("Type=notify", service)
            self.assertIn("After=clarion.service graphical-session.target", service)
            self.assertIn("Before=xdg-desktop-autostart.target", service)
            manifest = json.loads((release2 / "manifest.json").read_text())
            self.assertEqual(manifest["toolkit"], "gpui")
            self.assertEqual(manifest["icons"]["app.svg"], hashlib.sha256((icons2 / "app.svg").read_bytes()).hexdigest())

    def test_non_executable_shell_or_linked_assets_are_rejected_before_install(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            binary, icons = self.fixture(root, "one")
            binary.chmod(0o644)
            result = subprocess.run(self.command(root, binary, icons), capture_output=True, timeout=10)
            self.assertNotEqual(result.returncode, 0)
            self.assertIn(b"Build an executable clarion-shell first", result.stderr)
            self.assertFalse((root / "prefix").exists())
            binary.chmod(0o755)
            (icons / "outside.svg").symlink_to(root / "not-owned.svg")
            result = subprocess.run(self.command(root, binary, icons), capture_output=True, timeout=10)
            self.assertNotEqual(result.returncode, 0)
            self.assertIn(b"Asset links are not accepted", result.stderr)
            self.assertFalse((root / "prefix").exists())

    def test_check_only_performs_no_writes_or_execution(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            binary, icons = self.fixture(root, "one")
            marker = root / "executed"
            binary.write_text(f"#!/bin/sh\ntouch '{marker}'\n")
            result = subprocess.run([*self.command(root, binary, icons), "--check-only"],
                                    capture_output=True, text=True, check=True, timeout=10)
            self.assertEqual(json.loads(result.stdout)["status"], "checked")
            self.assertFalse((root / "prefix").exists())
            self.assertFalse((root / "config").exists())
            self.assertFalse(marker.exists())

    def test_install_never_overwrites_preferences_or_unrelated_legacy_launcher(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            binary, icons = self.fixture(root, "one")
            preferences = root / "config/realand-shell/preferences.json"
            preferences.parent.mkdir(parents=True)
            preferences.write_bytes(b'{"pinned_app_ids":["mine.desktop"]}\n')
            legacy_apps = root / "prefix/bin/clarion-launcher"
            legacy_apps.parent.mkdir(parents=True)
            legacy_apps.write_text("unmanaged application entry")
            self.install(root, binary, icons)
            self.assertEqual(preferences.read_bytes(), b'{"pinned_app_ids":["mine.desktop"]}\n')
            self.assertEqual(legacy_apps.read_text(), "unmanaged application entry")

    def assert_failure_restores_paths(self, failure, target):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            binary1, icons1 = self.fixture(root, "one")
            first = self.install(root, binary1, icons1)
            previous = {}
            for record in first["previous"].values():
                path = Path(record["path"])
                previous[path] = ("link", os.readlink(path)) if path.is_symlink() else ("file", path.read_bytes())
            binary2, icons2 = self.fixture(root, "two")
            spec = importlib.util.spec_from_file_location("install_native_shell_test", ROOT / "scripts/install-shell.py")
            installer = importlib.util.module_from_spec(spec)
            spec.loader.exec_module(installer)
            original = getattr(installer, failure)

            def inject(path, *args, **kwargs):
                if path.name == target and (root / "prefix/lib/clarion-shell/current").resolve() != Path(first["release"]):
                    raise OSError("injected finalization failure")
                return original(path, *args, **kwargs)

            argv = self.command(root, binary2, icons2)[1:]
            with mock.patch.object(sys, "argv", argv), mock.patch.object(installer, failure, side_effect=inject), contextlib.redirect_stdout(io.StringIO()):
                with self.assertRaisesRegex(OSError, "finalization failure"):
                    installer.main()
            for path, expected in previous.items():
                actual = ("link", os.readlink(path)) if path.is_symlink() else ("file", path.read_bytes())
                self.assertEqual(actual, expected, str(path))
            self.assertEqual((Path(first["release"]) / "bin/clarion-shell").read_bytes(), binary1.read_bytes())

    def test_desktop_write_failure_restores_every_public_path(self):
        self.assert_failure_restores_paths("atomic_text", "clarion-shell.desktop")

    def test_receipt_write_failure_restores_every_public_path(self):
        self.assert_failure_restores_paths("atomic_text", "receipt.json")

    def test_final_hash_read_failure_restores_every_public_path(self):
        self.assert_failure_restores_paths("file_sha256", "clarion-shell")


if __name__ == "__main__":
    unittest.main()
