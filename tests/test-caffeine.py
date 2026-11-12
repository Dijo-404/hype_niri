#!/usr/bin/env python3
"""Exercise caffeine acquisition and process cleanup without real inhibitors."""

import importlib.util
import json
import os
from pathlib import Path
import shlex
import shutil
import signal
import subprocess
import time
import unittest


spec = importlib.util.spec_from_file_location("runtime_test_utilities", Path(__file__).with_name("test-runtime.py"))
runtime_utilities = importlib.util.module_from_spec(spec)
spec.loader.exec_module(runtime_utilities)


class CaffeineTests(unittest.TestCase):
    setUp = runtime_utilities.RuntimeTests.setUp
    mock = runtime_utilities.RuntimeTests.mock
    stop_process_group = staticmethod(runtime_utilities.RuntimeTests.stop_process_group)

    def fixture(self, *, fail=False, delay=0):
        self.started = self.root / "inhibitor-started"
        self.acquired = self.root / "inhibitor-acquired"
        self.child_pid = self.root / "holder-pid"
        self.environment.update(
            CAFFEINE_TEST_STARTED=str(self.started),
            CAFFEINE_TEST_ACQUIRED=str(self.acquired),
            CAFFEINE_TEST_CHILD=str(self.child_pid),
            CAFFEINE_TEST_FAIL=str(int(fail)),
            CAFFEINE_TEST_DELAY=str(delay),
        )
        program = '''
import os, pathlib, subprocess, sys, time
args = sys.argv[1:]
assert "--what=idle" in args and "--mode=block" in args
pathlib.Path(os.environ["CAFFEINE_TEST_STARTED"]).write_text(str(os.getpid()))
time.sleep(float(os.environ["CAFFEINE_TEST_DELAY"]))
if os.environ["CAFFEINE_TEST_FAIL"] == "1":
    sys.exit(1)
pathlib.Path(os.environ["CAFFEINE_TEST_ACQUIRED"]).touch()
child = subprocess.Popen(args[args.index("--") + 1:])
pathlib.Path(os.environ["CAFFEINE_TEST_CHILD"]).write_text(str(child.pid))
sys.exit(child.wait())
'''
        self.mock("systemd-inhibit", f"exec -a systemd-inhibit {shlex.quote(shutil.which('python3'))} -c {shlex.quote(program)} \"$@\"")
        self.mock("pkill", 'printf "refresh\\n" >> "$RUNTIME_TEST_LOG"')

    def wait_for(self, predicate, message, timeout=3):
        deadline = time.monotonic() + timeout
        while not predicate():
            self.assertLess(time.monotonic(), deadline, message)
            time.sleep(0.01)

    def start(self, action="toggle", *, relative=False):
        script = self.scripts / "caffeine-control.sh"
        process = subprocess.Popen(
            ["bash", str(script.relative_to(self.user_home) if relative else script), action],
            cwd=self.user_home if relative else None,
            env=self.environment,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            text=True,
            start_new_session=True,
        )
        self.addCleanup(self.stop_process_group, process)
        return process

    def wait_child(self):
        self.wait_for(lambda: self.child_pid.exists() and self.child_pid.read_text().strip(), "holder did not start")
        return int(self.child_pid.read_text())

    def result(self, process):
        stdout, stderr = process.communicate(timeout=5)
        self.assertEqual(process.returncode, 0, stdout + stderr)
        return json.loads(stdout)

    @staticmethod
    def running(pid):
        try:
            stat = Path(f"/proc/{pid}/stat").read_text()
        except FileNotFoundError:
            return False
        return stat.rsplit(") ", 1)[1].split()[0] != "Z"

    def test_delayed_acquisition_failure_is_never_reported_active(self):
        self.fixture(fail=True, delay=0.2)
        process = self.start()
        self.wait_for(self.started.exists, "inhibitor did not start")
        result = self.result(process)
        self.assertEqual(result["class"], "deactivated")
        self.assertFalse((self.runtime / "caffeine_state").exists())
        self.assertFalse((self.runtime / "caffeine_pid").exists())

    def test_on_waits_for_child_after_acquisition(self):
        self.fixture(delay=0.2)
        process = self.start()
        self.wait_for(self.started.exists, "inhibitor did not start")
        result = self.result(process)
        self.assertEqual(result["class"], "activated")
        self.assertTrue(self.acquired.exists(), "On was emitted before acquisition")
        child = self.wait_child()
        self.wait_for(lambda: Path(f"/proc/{child}/comm").read_text().strip() == "sleep", "holder did not become sleep")
        self.assertTrue((self.runtime / "caffeine_state").exists())

    def test_holder_exit_clears_state_refreshes_bar_and_reaps_child(self):
        self.fixture()
        process = self.start()
        child = self.wait_child()
        self.assertEqual(self.result(process)["class"], "activated")
        parent = int(self.started.read_text())
        before = self.log.read_text().count("refresh")
        os.kill(parent, signal.SIGTERM)
        self.wait_for(lambda: not (self.runtime / "caffeine_state").exists(), "dead holder left active state")
        self.assertFalse((self.runtime / "caffeine_pid").exists())
        self.wait_for(lambda: not self.running(child), "holder exit orphaned its sleep child")
        self.wait_for(lambda: self.log.read_text().count("refresh") > before, "holder exit did not refresh Waybar")
        self.assertEqual(self.result(self.start("status"))["class"], "deactivated")

    def test_sleep_exit_clears_active_state(self):
        self.fixture()
        process = self.start()
        child = self.wait_child()
        self.assertEqual(self.result(process)["class"], "activated")
        os.kill(child, signal.SIGTERM)
        self.wait_for(lambda: not (self.runtime / "caffeine_state").exists(), "exited sleep left active state")
        self.assertFalse((self.runtime / "caffeine_pid").exists())

    def test_status_and_stop_release_runtime_lock_and_preserve_holder(self):
        self.fixture()
        self.assertEqual(self.result(self.start())["class"], "activated")
        parent = self.started.read_text()
        child = self.wait_child()
        self.assertEqual(self.result(self.start("status"))["class"], "activated")
        self.assertEqual(self.started.read_text(), parent, "status unnecessarily restarted acquired holder")
        process = self.start("stop")
        stdout, stderr = process.communicate(timeout=3)
        self.assertEqual(process.returncode, 0, stdout + stderr)
        self.wait_for(lambda: not self.running(child), "stop orphaned the holder")
        self.wait_for(lambda: not list(self.runtime.glob("caffeine-ready.*")), "readiness file leaked after stop")

    def test_absolute_stop_recognizes_holder_started_from_relative_path(self):
        self.fixture()
        self.assertEqual(self.result(self.start(relative=True))["class"], "activated")
        child = self.wait_child()
        process = self.start("stop")
        stdout, stderr = process.communicate(timeout=3)
        self.assertEqual(process.returncode, 0, stdout + stderr)
        self.wait_for(lambda: not self.running(child), "absolute stop failed to recognize its relative-path holder")


if __name__ == "__main__":
    unittest.main()
