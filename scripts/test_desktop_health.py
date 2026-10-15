"""Diagnostic checks use fixtures and mocks; never alter a running desktop."""

import importlib.util
import contextlib
import io
import json
import os
from pathlib import Path
import sys
import tempfile
import unittest
from unittest.mock import patch


spec = importlib.util.spec_from_file_location("desktop_health", Path(__file__).with_name("desktop-health.py"))
health = importlib.util.module_from_spec(spec)
sys.modules[spec.name] = health
spec.loader.exec_module(health)


class DesktopHealthTests(unittest.TestCase):
    def executable(self, path, text):
        path.write_text(text)
        path.chmod(0o755)
        return path

    def test_old_path_override_and_symlink_to_current_install(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            installed = self.executable(root / "installed", "new")
            local = self.executable(root / "local", "old")
            self.assertEqual(health.check_launcher("realand", local, installed).status, "warn")
            local.unlink()
            local.symlink_to(installed)
            self.assertEqual(health.check_launcher("realand", local, installed).status, "ok")

    def test_equal_copy_and_missing_launcher(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            installed = self.executable(root / "installed", "same")
            local = self.executable(root / "local", "same")
            self.assertEqual(health.check_launcher("realand", local, installed).status, "ok")
            self.assertEqual(health.check_launcher("realand", None, installed).status, "error")
            local.chmod(0o600)
            self.assertEqual(health.check_launcher("realand", local, installed).status, "error")

    def test_fifo_cannot_block_binary_hash_check(self):
        with tempfile.TemporaryDirectory() as directory:
            fifo = Path(directory) / "fifo"
            os.mkfifo(fifo)
            with self.assertRaises(OSError):
                health.binary_digest(fifo)

    def test_live_executable_version_difference_is_detected(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            installed = self.executable(root / "installed", "new")
            running = self.executable(root / "old", "old")
            process = root / "123"
            process.mkdir()
            (process / "exe").symlink_to(running)
            self.assertEqual(health.check_running_binary(123, installed, root).status, "warn")
            (process / "exe").unlink()
            (process / "exe").symlink_to(installed)
            self.assertEqual(health.check_running_binary(123, installed, root).status, "ok")

    def test_service_data_cannot_provide_arbitrary_process_paths(self):
        for value in ["../123", "-1", "4294967295", "9" * 200, "１２３"]:
            self.assertEqual(health.parse_service("MainPID=" + value)[1], 0)
        self.assertEqual(health.parse_service("ActiveState=active\nMainPID=123\n"), ("active", 123))

    def test_mocked_probe_does_not_mutate_or_disclose_config(self):
        commands = []

        def runner(command):
            commands.append(command)
            if command[-1] == "outputs":
                return 0, '{"DP-1":{"serial":"private-serial"}}'
            if "validate" in command:
                return 1, "private config contents"
            if "show" in command:
                return 0, "ActiveState=inactive\nMainPID=0\n"
            return 0, ""

        with patch.object(health, "check_launcher", return_value=health.Check("mock", "ok", "fixture")):
            checks = health.collect_checks(Path("/tmp/config with spaces; literal.kdl"), runner, lambda name: "/fixture/" + name)
        output = repr(checks)
        self.assertNotIn("private", output)
        self.assertTrue(any(check.name == "配置" and check.status == "error" for check in checks))
        self.assertTrue(any(check.name == "桌面 IPC" and check.status == "ok" for check in checks))
        self.assertIn(["/fixture/realand", "validate", "--config", "/tmp/config with spaces; literal.kdl"], commands)
        for command in commands:
            self.assertFalse(set(command) & {"start", "stop", "restart", "reload", "daemon-reload", "action"})

    def test_invalid_ipc_reply_is_unknown_not_success(self):
        with patch.object(health, "check_launcher", return_value=health.Check("mock", "ok", "fixture")):
            checks = health.collect_checks(runner=lambda args: (0, "not JSON"), which=lambda name: "/fixture/realand" if name == "realand" else None)
        self.assertEqual(next(check for check in checks if check.name == "桌面 IPC").status, "warn")

    def test_cli_warnings_errors_and_json_failure_status(self):
        for status, args, expected in [
            ("ok", ["--json", "--strict"], 0),
            ("warn", ["--json"], 0),
            ("warn", ["--json", "--strict"], 1),
            ("error", ["--json"], 1),
        ]:
            with self.subTest(status=status, args=args):
                output = io.StringIO()
                with patch.object(health, "collect_checks", return_value=[health.Check("fixture", status, "bounded")]), contextlib.redirect_stdout(output):
                    result = health.main(args)
                self.assertEqual(result, expected)
                report = json.loads(output.getvalue())
                self.assertEqual(report["failed"], bool(expected))
                self.assertEqual(report["checks"][0]["status"], status)

    def test_live_version_comparison_uses_install_even_if_path_is_shadowed(self):
        def runner(command):
            if "show" in command:
                return 0, "ActiveState=active\nMainPID=123\n"
            return 0, "{}"

        with patch.object(health, "check_launcher", return_value=health.Check("mock", "warn", "old copy")), patch.object(health.Path, "is_file", return_value=True), patch.object(health, "check_running_binary", return_value=health.Check("运行版本", "warn", "old session")) as compare:
            health.collect_checks(runner=runner, which=lambda name: "/fixture/" + name)
        compare.assert_called_once_with(123, Path("/usr/local/bin/realand"))


if __name__ == "__main__":
    unittest.main()
