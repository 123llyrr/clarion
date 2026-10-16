"""Session bootstrap regressions using mocks; never starts a desktop session."""

import json
import os
from pathlib import Path
import shlex
import subprocess
import tempfile
import unittest


SESSION = Path(__file__).resolve().parents[1] / "resources" / "clarion-session"


class SessionTests(unittest.TestCase):
    def test_legacy_active_session_is_rejected_before_importing_or_starting_services(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            mock_bin = root / "bin"
            mock_bin.mkdir()
            systemctl = mock_bin / "systemctl"
            systemctl.write_text(
                "#!/usr/bin/python3\n"
                "import json, os, sys\n"
                "with open(os.environ['SESSION_SERVICE_LOG'], 'a') as stream:\n"
                "    stream.write(json.dumps(sys.argv[1:]) + '\\n')\n"
                "sys.exit(0 if sys.argv[-1] == 'realand.service' else 1)\n"
            )
            systemctl.chmod(0o755)
            log = root / "services.jsonl"
            environment = {
                **os.environ, "SHELL": "", "PATH": f"{mock_bin}:/usr/bin:/bin",
                "SESSION_SERVICE_LOG": str(log),
            }
            environment.pop("MANAGERPID", None)
            environment.pop("SYSTEMD_EXEC_PID", None)
            result = subprocess.run([str(SESSION)], env=environment, capture_output=True, timeout=5)
            self.assertEqual(result.returncode, 1, result.stderr.decode())
            self.assertEqual(
                [json.loads(line) for line in log.read_text().splitlines()],
                [["--user", "-q", "is-active", "clarion.service"],
                 ["--user", "-q", "is-active", "realand.service"]],
            )

    def test_login_shell_preserves_paths_and_arguments_as_data(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            launcher = root / "session ' with spaces"
            launcher.write_bytes(SESSION.read_bytes())
            launcher.chmod(0o755)
            mock_bin = root / "bin"
            mock_bin.mkdir()
            # Capture the login command and execute it through a non-login shell.
            # The mocked service manager stops the relaunched script immediately.
            bash = mock_bin / "bash"
            bash.write_text(
                "#!/usr/bin/python3\n"
                "import json, os, sys\n"
                "with open(os.environ['SESSION_ARGV_LOG'], 'w') as stream:\n"
                "    json.dump(sys.argv[1:], stream)\n"
                "os.execv('/bin/sh', ['/bin/sh', '-c', sys.argv[-1]])\n"
            )
            bash.chmod(0o755)
            systemctl = mock_bin / "systemctl"
            systemctl.write_text(
                '#!/bin/sh\nprintf "called\\n" > "$SESSION_SERVICE_LOG"\nexit 0\n'
            )
            systemctl.chmod(0o755)
            marker = root / "injected"
            arguments = [
                "space separated",
                "a'b",
                "",
                "line one\nline two\n",
                f"'; touch '{marker}'; #",
                f"$(touch '{marker}')",
            ]
            argv_log = root / "argv.json"
            service_log = root / "service.log"
            env = {
                **os.environ,
                "SHELL": "/bin/bash",
                "PATH": f"{mock_bin}:/usr/bin:/bin",
                "SESSION_ARGV_LOG": str(argv_log),
                "SESSION_SERVICE_LOG": str(service_log),
            }
            env.pop("MANAGERPID", None)
            env.pop("SYSTEMD_EXEC_PID", None)
            result = subprocess.run(
                [str(launcher), *arguments], env=env, capture_output=True, timeout=5
            )
            self.assertEqual(result.returncode, 1, result.stderr.decode())
            self.assertTrue(service_log.exists(), result.stderr.decode())
            self.assertFalse(marker.exists())
            invocation = json.loads(argv_log.read_text())
            self.assertEqual(invocation[2:5], ["bash", "/bin/bash", "-c"])
            self.assertEqual(
                shlex.split(invocation[-1]), ["exec", str(launcher), "-l", *arguments]
            )


if __name__ == "__main__":
    unittest.main()
