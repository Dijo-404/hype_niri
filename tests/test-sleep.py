#!/usr/bin/env python3
"""Sleep-lock regressions with mocked desktop commands: python3 tests/test-sleep.py."""

import json
import os
import shlex
import signal
import subprocess
import tempfile
import time
import unittest
from pathlib import Path

REPO = Path(__file__).resolve().parents[1]


class SleepTests(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory()
        self.addCleanup(self.temporary.cleanup)
        self.root = Path(self.temporary.name)
        self.scripts = self.root / ".config/scripts"
        self.scripts.mkdir(parents=True)
        self.bin = self.root / "bin"
        self.bin.mkdir()
        self.runtime = self.root / "runtime"
        self.runtime.mkdir(mode=0o700)
        for name in ("prepare-sleep.sh", "runtime-dir.sh", "suspend-now.sh"):
            source = REPO / "scripts" / name
            destination = self.scripts / name
            destination.write_text(
                source.read_text().replace("$HOME", "$TEST_SLEEP_HOME")
            )
            destination.chmod(source.stat().st_mode)
        self.environment = os.environ | {
            "TEST_SLEEP_HOME": str(self.root),
            "PATH": f"{self.bin}:{os.environ['PATH']}",
            "XDG_RUNTIME_DIR": str(self.runtime),
            "XDG_SESSION_ID": "test-session",
            "TEST_SLEEP_ROOT": str(self.root),
        }
        self.write_command(
            "loginctl",
            """#!/usr/bin/env bash
[[ "$*" == 'show-session test-session -p LockedHint --value' ]] || exit 2
[[ ! -f "$TEST_SLEEP_ROOT/hint-error" ]] || exit 1
if [[ -f "$TEST_SLEEP_ROOT/ready" ]]; then echo yes; else echo no; fi
""",
        )
        self.write_command(
            "pgrep",
            """#!/usr/bin/env bash
[[ "$*" == "-u $UID -x hyprlock" ]] || exit 2
[[ -f "$TEST_SLEEP_ROOT/locker" ]]
""",
        )
        self.write_command(
            "sleep",
            """#!/usr/bin/env python3
import os, sys, time
time.sleep(float(sys.argv[1]) * float(os.environ.get('TEST_SLEEP_SCALE', '1')))
""",
        )
        self.write_command(
            "lock.sh",
            """#!/usr/bin/env bash
[[ "$*" == sleep ]] || exit 2
touch "$TEST_SLEEP_ROOT/started" "$TEST_SLEEP_ROOT/locker"
[[ ! -f "$TEST_SLEEP_ROOT/fail" ]] || exit 7
while [[ ! -f "$TEST_SLEEP_ROOT/lock-release" ]]; do /usr/bin/sleep 0.01; done
touch "$TEST_SLEEP_ROOT/ready"
/usr/bin/sleep 30
""",
            directory=self.scripts,
        )
        self.write_command(
            "caffeine-control.sh",
            "#!/usr/bin/env bash\nexit 0\n",
            directory=self.scripts,
        )
        self.write_command(
            "systemctl",
            """#!/usr/bin/env bash
[[ "$*" == suspend ]] || exit 2
touch "$TEST_SLEEP_ROOT/suspended"
""",
        )

    def write_command(self, name, text, directory=None):
        path = (directory or self.bin) / name
        path.write_text(text)
        path.chmod(0o755)

    def start(self, *arguments, script="prepare-sleep.sh", command=None):
        process = subprocess.Popen(
            ["bash", "-c", command]
            if command is not None
            else [str(self.scripts / script), *arguments],
            env=self.environment,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            text=True,
            start_new_session=True,
        )

        def cleanup():
            try:
                os.killpg(process.pid, signal.SIGKILL)
            except ProcessLookupError:
                pass
            process.communicate(timeout=3)

        self.addCleanup(cleanup)
        return process

    def wait_file(self, name):
        deadline = time.monotonic() + 2
        while not (self.root / name).exists():
            if time.monotonic() >= deadline:
                self.fail(f"Timed out waiting for {name}")
            time.sleep(0.01)

    def test_slow_locker_must_confirm_readiness_before_releasing_waiter(self):
        process = self.start()
        self.wait_file("started")
        time.sleep(0.4)
        self.assertIsNone(
            process.poll(), "Sleep preparation finished before the locker was ready"
        )
        (self.root / "lock-release").touch()
        self.assertEqual(process.wait(timeout=2), 0)
        self.assertTrue((self.root / "ready").exists())

    def test_existing_locker_without_locked_hint_is_not_ready(self):
        (self.root / "locker").touch()
        process = self.start()
        time.sleep(0.4)
        self.assertIsNone(
            process.poll(), "A locker process alone does not confirm session locking"
        )
        (self.root / "ready").touch()
        self.assertEqual(process.wait(timeout=2), 0)

    def test_confirmed_existing_lock_does_not_start_another_locker(self):
        (self.root / "locker").touch()
        (self.root / "ready").touch()
        process = self.start()
        self.assertEqual(process.wait(timeout=1), 0)
        self.assertFalse((self.root / "started").exists())

    def test_stale_locked_hint_still_requires_current_user_locker(self):
        (self.root / "ready").touch()
        process = self.start()
        self.wait_file("started")
        self.assertEqual(process.wait(timeout=2), 0)

    def test_failed_locker_reports_failure_after_bounded_readiness_wait(self):
        (self.root / "fail").touch()
        self.environment["TEST_SLEEP_SCALE"] = "0.01"
        process = self.start()
        self.assertNotEqual(
            process.wait(timeout=3), 0, "A failed locker was reported ready"
        )
        self.assertFalse((self.root / "ready").exists())
        self.assertIn("lock", process.stderr.read().lower())

    def test_unreadable_lock_state_is_never_reported_ready(self):
        (self.root / "hint-error").touch()
        self.environment["TEST_SLEEP_SCALE"] = "0.01"
        process = self.start()
        self.assertNotEqual(process.wait(timeout=3), 0)

    def test_explicit_suspend_waits_for_confirmed_lock(self):
        process = self.start(script="suspend-now.sh")
        self.wait_file("started")
        self.assertFalse(
            (self.root / "suspended").exists(), "Suspended before the locker was ready"
        )
        (self.root / "lock-release").touch()
        self.assertEqual(process.wait(timeout=2), 0)
        self.assertTrue((self.root / "ready").exists())
        self.assertTrue((self.root / "suspended").exists())

    def test_explicit_suspend_aborts_when_locker_fails(self):
        (self.root / "fail").touch()
        self.environment["TEST_SLEEP_SCALE"] = "0.01"
        process = self.start(script="suspend-now.sh")
        self.assertNotEqual(process.wait(timeout=3), 0)
        self.assertFalse((self.root / "suspended").exists())

    def test_watch_uses_native_waiting_inhibitor_and_ignores_user_idle_config(self):
        self.write_command(
            "swayidle",
            """#!/usr/bin/env python3
import json, os, sys, time
from pathlib import Path
with (Path(os.environ['TEST_SLEEP_ROOT']) / 'watch-args').open('a') as output:
    output.write(json.dumps(sys.argv[1:]) + '\\n')
time.sleep(30)
""",
        )
        startup_commands = [
            json.loads(line.removeprefix("spawn-sh-at-startup "))
            for line in (REPO / "niri/config.kdl").read_text().splitlines()
            if line.startswith("spawn-sh-at-startup ")
        ]
        command = next(
            (command for command in startup_commands if "prepare-sleep.sh" in command),
            None,
        )
        self.assertIsNotNone(
            command, "The compositor does not start the sleep inhibitor"
        )
        watcher = self.start(
            command=command.replace("~/.config/scripts", shlex.quote(str(self.scripts)))
        )
        self.wait_file("watch-args")
        duplicate = self.start("--watch")
        self.assertEqual(duplicate.wait(timeout=2), 0)
        self.assertIsNone(watcher.poll())
        calls = [
            json.loads(line)
            for line in (self.root / "watch-args").read_text().splitlines()
        ]
        self.assertEqual(len(calls), 1)
        arguments = calls[0]
        self.assertIn("-w", arguments)
        self.assertEqual(arguments[arguments.index("-C") + 1], "/dev/null")
        command = arguments[arguments.index("before-sleep") + 1]
        (self.root / "ready").touch()
        (self.root / "locker").touch()
        result = subprocess.run(
            command,
            shell=True,
            env=self.environment,
            capture_output=True,
            timeout=2,
            check=False,
        )
        self.assertEqual(result.returncode, 0, result.stderr)


if __name__ == "__main__":
    unittest.main()
