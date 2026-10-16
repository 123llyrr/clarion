#!/usr/bin/env python3
"""Identity migration uses one executable and preserves literal CLI arguments and user data."""
import json
import importlib.machinery
import importlib.util
import socket
from pathlib import Path
import shutil
import subprocess
import sys
import tempfile
import unittest


ROOT = Path(__file__).resolve().parent.parent

loader = importlib.machinery.SourceFileLoader("clarion_shell_msg", str(ROOT / "resources/clarion-shell-msg"))
shell_msg_spec = importlib.util.spec_from_loader(loader.name, loader)
shell_msg = importlib.util.module_from_spec(shell_msg_spec)
loader.exec_module(shell_msg)


class IdentityCompatibilityTests(unittest.TestCase):
    def test_legacy_commands_forward_literal_arguments_to_the_sibling_binary(self):
        with tempfile.TemporaryDirectory(prefix="clarion wrappers ") as temporary:
            root = Path(temporary)
            for suffix in ("", "-session", "-settings", "-shell", "-shell-msg"):
                legacy = root / ("realand" + suffix)
                canonical = root / ("clarion" + suffix)
                shutil.copy2(ROOT / "resources/compat" / legacy.name, legacy)
                canonical.write_text(
                    "#!/usr/bin/env python3\nimport json, sys\n"
                    "print(json.dumps(sys.argv))\n"
                )
                canonical.chmod(0o755)
                arguments = ["spaces stay together", "$(touch should-not-exist)", "\"'`", "line\nbreak"]
                result = subprocess.run(
                    [str(legacy), *arguments], check=True, text=True,
                    stdout=subprocess.PIPE, stderr=subprocess.PIPE,
                    cwd=root, timeout=5,
                )
                self.assertEqual(json.loads(result.stdout), [str(canonical), *arguments])
                self.assertFalse((root / "should-not-exist").exists())

    def test_settings_install_retains_old_launcher_backup_and_one_new_binary(self):
        with tempfile.TemporaryDirectory(prefix="clarion settings install ") as temporary:
            root = Path(temporary)
            binary = root / "built-settings"
            binary.write_text("#!/bin/sh\nexit 0\n")
            binary.chmod(0o755)
            prefix = root / "local"
            old = prefix / "bin/realand-settings"
            old.parent.mkdir(parents=True)
            old.write_text("original launcher\n")
            result = subprocess.run(
                [sys.executable, str(ROOT / "scripts/install-settings.py"),
                 "--binary", str(binary), "--prefix", str(prefix)],
                check=True, text=True, stdout=subprocess.PIPE, stderr=subprocess.PIPE, timeout=10,
            )
            receipt = json.loads(result.stdout)
            canonical = prefix / "bin/clarion-settings"
            self.assertTrue(old.is_symlink())
            self.assertEqual(old.resolve(), canonical.resolve())
            self.assertEqual(Path(receipt["previous"]["legacy_launcher"]["copy"]).read_text(),
                             "original launcher\n")
            self.assertEqual(canonical.read_bytes(), binary.read_bytes())

    def test_shell_install_preserves_alias_and_does_not_start_a_service(self):
        with tempfile.TemporaryDirectory(prefix="clarion shell install ") as temporary:
            root = Path(temporary)
            bundle = root / "bundle"
            (bundle / "lib").mkdir(parents=True)
            (bundle / "data/flutter_assets").mkdir(parents=True)
            for relative in ("clarion-shell", "lib/libflutter_linux_gtk.so", "data/icudtl.dat"):
                (bundle / relative).write_bytes(b"fixture")
            (bundle / "clarion-shell").chmod(0o755)
            prefix = root / "local"
            result = subprocess.run(
                [sys.executable, str(ROOT / "scripts/install-shell.py"),
                 "--bundle", str(bundle), "--prefix", str(prefix),
                 "--config-root", str(root / "config")],
                check=True, text=True, stdout=subprocess.PIPE, stderr=subprocess.PIPE, timeout=10,
            )
            receipt = json.loads(result.stdout)
            self.assertFalse(receipt["service_enabled_or_started"])
            self.assertFalse(receipt["existing_shell_changed"])
            self.assertEqual((prefix / "bin/realand-shell").resolve(),
                             (prefix / "bin/clarion-shell").resolve())
            self.assertEqual((prefix / "bin/realand-shell-msg").resolve(),
                             (prefix / "bin/clarion-shell-msg").resolve())
            self.assertFalse((root / "config/systemd/user/graphical-session.target.wants").exists())

    def test_panel_socket_selection_never_retries_an_authoritative_canonical_socket(self):
        environment = {"XDG_RUNTIME_DIR": "/run/user/fixture"}
        canonical = Path("/run/user/fixture/clarion-shell.sock")
        legacy = Path("/run/user/fixture/realand-shell.sock")
        self.assertEqual(shell_msg.socket_path(environment, present=lambda _: True), canonical)
        self.assertEqual(shell_msg.socket_path(environment, present=lambda p: p == legacy), legacy)
        self.assertEqual(shell_msg.socket_path(environment, present=lambda _: False), canonical)
        self.assertEqual(shell_msg.socket_path({"CLARION_SHELL_SOCKET": "/new", "REALAND_SHELL_SOCKET": "/old"}), Path("/new"))
        self.assertEqual(shell_msg.socket_path({"REALAND_SHELL_SOCKET": "/old"}), Path("/old"))
        with self.assertRaises(ValueError):
            shell_msg.socket_path({"XDG_RUNTIME_DIR": "relative"})

    def test_panel_client_sends_one_literal_command_without_starting_a_shell(self):
        with tempfile.TemporaryDirectory(prefix="clarion-msg-") as temporary:
            path = Path(temporary) / "panel.sock"
            with socket.socket(socket.AF_UNIX, socket.SOCK_STREAM) as server:
                server.bind(str(path))
                server.listen(1)
                server.settimeout(1)
                shell_msg.send_command(path, "spotlight")
                client, _ = server.accept()
                with client:
                    client.settimeout(1)
                    self.assertEqual(client.recv(1024), b"spotlight\n")
                    self.assertEqual(client.recv(1), b"")


if __name__ == "__main__":
    unittest.main()
