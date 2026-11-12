#!/usr/bin/env python3
"""Run isolated runtime regressions with mocked desktop commands."""

import json
import os
from pathlib import Path
import shlex
import shutil
import signal
import subprocess
import tempfile
import time
import unittest
from concurrent.futures import ThreadPoolExecutor


REPO = Path(__file__).resolve().parents[1]


class RuntimeTests(unittest.TestCase):
    def setUp(self):
        temporary = tempfile.TemporaryDirectory(prefix="hype-niri-runtime-test-")
        self.addCleanup(temporary.cleanup)
        self.root = Path(temporary.name)
        self.user_home = self.root / "home"
        self.scripts = self.user_home / ".config/scripts"
        self.scripts.mkdir(parents=True)
        for source in (REPO / "scripts").glob("*.sh"):
            (self.scripts / source.name).write_text(
                source.read_text().replace("$HOME", "$RUNTIME_TEST_HOME")
            )
            (self.scripts / source.name).chmod(0o755)
        self.runtime = self.root / "runtime"
        self.runtime.mkdir(mode=0o700)
        self.bin = self.root / "bin"
        self.bin.mkdir()
        self.log = self.root / "commands"
        self.environment = os.environ | {
            "RUNTIME_TEST_HOME": str(self.user_home),
            "XDG_RUNTIME_DIR": str(self.runtime),
            "PATH": f"{self.bin}:{os.environ['PATH']}",
            "RUNTIME_TEST_LOG": str(self.log),
        }
        for command in ("notify-send", "pkill"):
            self.mock(command, "exit 0")
        self.mock("pgrep", "exit 1")

    def mock(self, command, body):
        path = self.bin / command
        path.write_text("#!/usr/bin/env bash\nset -eu\n" + body + "\n")
        path.chmod(0o755)

    def run_script(self, name, *args, environment=None):
        result = subprocess.run(
            ["bash", str(self.scripts / name), *args],
            env=environment or self.environment,
            capture_output=True,
            text=True,
            timeout=8,
        )
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        return result

    def hold_mock(self, command):
        fifo = self.root / f"{command}-hold"
        os.mkfifo(fifo)
        self.environment["RUNTIME_TEST_HOLD"] = str(fifo)
        self.mock(command, 'printf "%s\\n" "$*" >> "$RUNTIME_TEST_LOG"\nread -r _ < "$RUNTIME_TEST_HOLD"')

    def start_script(self, name):
        process = subprocess.Popen(
            ["bash", str(self.scripts / name)],
            env=self.environment,
            stdout=subprocess.DEVNULL,
            stderr=subprocess.PIPE,
            text=True,
        )
        self.addCleanup(self.stop_process, process)
        deadline = time.monotonic() + 3
        while not self.log.exists():
            self.assertIsNone(process.poll(), process.stderr.read() if process.poll() is not None else "")
            self.assertLess(time.monotonic(), deadline, "mock did not start")
            time.sleep(0.01)
        return process

    @staticmethod
    def stop_process(process):
        if process.poll() is None:
            process.terminate()
        process.communicate(timeout=3)

    @staticmethod
    def stop_process_group(process):
        try:
            os.killpg(process.pid, signal.SIGTERM)
        except ProcessLookupError:
            pass
        try:
            process.communicate(timeout=3)
        except subprocess.TimeoutExpired:
            os.killpg(process.pid, signal.SIGKILL)
            process.communicate(timeout=3)

    def waybar_fixture(self):
        config = self.user_home / ".config/waybar/config.jsonc"
        config.parent.mkdir()
        config.write_text('{"modules-left":["clock"]}')
        self.mock("niri", "printf '%s\\n' '{\"eDP-1\":{\"name\":\"eDP-1\",\"current_mode\":0},\"DP-1\":{\"name\":\"DP-1\",\"current_mode\":0}}'")
        return config

    def sensor_fixture(self, temperature="45000", label="Package id 0"):
        hwmon = self.root / "hwmon/hwmon0"
        hwmon.mkdir(parents=True)
        (hwmon / "name").write_text("coretemp\n")
        (hwmon / "temp1_input").write_text(temperature + "\n")
        (hwmon / "temp1_label").write_text(label + "\n")
        self.environment.update(HWMON_ROOT=str(hwmon.parent), THERMAL_ROOT=str(self.root / "thermal"))
        return hwmon / "temp1_input"

    def copilot_fixture(self):
        self.environment["XDG_SESSION_ID"] = "mock-copilot"
        self.mock("code", 'printf "code\\n" >> "$RUNTIME_TEST_LOG"')
        self.mock("loginctl", '''
[[ "${RUNTIME_TEST_LOGINCTL_FAIL:-0}" != 1 ]] || exit 1
printf "%s\\n" "${RUNTIME_TEST_LOCKED:-no}"''')
        (self.scripts / "copilot-display.sh").write_text(
            '#!/usr/bin/env bash\nprintf "copilot-display %s\\n" "$*" >> "$RUNTIME_TEST_LOG"\n'
        )
        (self.scripts / "copilot-display.sh").chmod(0o755)
        return self.runtime / "hype-copilot.state"

    def copilot_display_fixture(self):
        self.environment["XDG_SESSION_ID"] = "mock-copilot"
        ready = self.root / "session-locked"
        outputs = self.root / "outputs.json"
        outputs.write_text(json.dumps({
            "eDP-1": {"name": "eDP-1", "current_mode": 0},
            "DP-1": {"name": "DP-1", "current_mode": 1},
            "HDMI-A-1": {"name": "HDMI-A-1", "current_mode": None},
        }))
        self.environment.update(
            RUNTIME_TEST_LOCK_READY=str(ready),
            RUNTIME_TEST_OUTPUTS=str(outputs),
        )
        self.mock("sleep", 'printf "sleep %s\\n" "$*" >> "$RUNTIME_TEST_LOG"')
        self.mock("pgrep", '[[ "${!#}" == hyprlock && -f "$RUNTIME_TEST_LOCK_READY" ]]')
        self.mock("loginctl", '''
[[ "${1:-}" == show-session && "${2:-}" == mock-copilot ]] || exit 90
[[ "${RUNTIME_TEST_LOGINCTL_FAIL:-0}" != 1 ]] || exit 1
if [[ -n "${RUNTIME_TEST_LOGINCTL_VALUE:-}" ]]; then
    value="$RUNTIME_TEST_LOGINCTL_VALUE"
elif [[ -f "$RUNTIME_TEST_LOCK_READY" ]]; then
    value=yes
else
    value=no
fi
printf 'loginctl %s\\n' "$value" >> "$RUNTIME_TEST_LOG"
printf '%s\\n' "$value"''')
        self.mock("niri", '''
printf 'niri %s\\n' "$*" >> "$RUNTIME_TEST_LOG"
case "$*" in
    'msg --json outputs') cat "$RUNTIME_TEST_OUTPUTS" ;;
    'msg output '*' off'|'msg output '*' on')
        if [[ "$4" == off ]]; then
            [[ "${RUNTIME_TEST_OFF_FAIL:-}" != "$3" ]] || exit 1
        else
            [[ "${RUNTIME_TEST_ON_FAIL:-}" != "$3" ]] || exit 1
        fi
        [[ "${RUNTIME_TEST_IGNORE_APPLY:-}" != "$4" ]] || exit 0
        python3 - "$RUNTIME_TEST_OUTPUTS" "$3" "$4" <<'PY'
import json
from pathlib import Path
import sys
path = Path(sys.argv[1])
outputs = json.loads(path.read_text())
if sys.argv[2] in outputs:
    outputs[sys.argv[2]]["current_mode"] = 0 if sys.argv[3] == "on" else None
path.write_text(json.dumps(outputs))
PY
        ;;
    'msg action power-on-monitors')
        [[ "${RUNTIME_TEST_POWER_ON_FAIL:-0}" != 1 ]] ;;
    *) exit 91 ;;
esac''')
        (self.scripts / "lock.sh").write_text('''#!/usr/bin/env bash
set -eu
printf 'lock %s\\n' "$*" >> "$RUNTIME_TEST_LOG"
[[ "${RUNTIME_TEST_LOCK_FAIL:-0}" != 1 ]] || exit 1
if [[ "${RUNTIME_TEST_LOCK_NEVER_READY:-0}" != 1 ]]; then
    touch "$RUNTIME_TEST_LOCK_READY"
fi
''')
        return self.runtime / "hype-copilot-displays.state", ready

    def set_copilot_output_modes(self, enabled):
        path = Path(self.environment["RUNTIME_TEST_OUTPUTS"])
        outputs = json.loads(path.read_text())
        for name in ("eDP-1", "DP-1"):
            if name in outputs:
                outputs[name]["current_mode"] = 0 if enabled else None
        path.write_text(json.dumps(outputs))

    def run_copilot_display(self, succeeds=True, mode="toggle"):
        result = subprocess.run(
            ["bash", str(self.scripts / "copilot-display.sh"), mode],
            env=self.environment,
            capture_output=True,
            text=True,
            timeout=8,
        )
        if succeeds:
            self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        else:
            self.assertNotEqual(result.returncode, 0, "unsafe request unexpectedly succeeded")
        return self.log.read_text().splitlines() if self.log.exists() else []

    def copilot_press(self):
        process = subprocess.Popen(
            ["bash", str(self.scripts / "copilot-shortcut.sh")],
            env=self.environment,
            stdout=subprocess.DEVNULL,
            stderr=subprocess.PIPE,
            text=True,
            start_new_session=True,
        )
        self.addCleanup(self.stop_process_group, process)
        return process

    def wait_copilot_phase(self, phase):
        state = self.runtime / "hype-copilot.state"
        deadline = time.monotonic() + 3
        while True:
            try:
                fields = state.read_text().split()
            except FileNotFoundError:
                fields = []
            if len(fields) == 2 and fields[1] == phase:
                return
            self.assertLess(time.monotonic(), deadline, f"Copilot did not enter {phase} phase")
            time.sleep(0.005)

    def finish_copilot_press(self, process):
        _, stderr = process.communicate(timeout=3)
        self.assertEqual(process.returncode, 0, stderr)

    def test_copilot_single_press_opens_code_after_double_press_window(self):
        self.copilot_fixture()
        process = self.copilot_press()
        self.wait_copilot_phase("single")
        time.sleep(0.08)
        self.assertFalse(self.log.exists(), "single press launched before the double press window closed")
        self.finish_copilot_press(process)
        self.assertEqual(self.log.read_text().splitlines(), ["code"])

    def test_copilot_double_press_toggles_locked_displays_without_opening_code(self):
        self.copilot_fixture()
        first = self.copilot_press()
        self.wait_copilot_phase("single")
        second = self.copilot_press()
        self.wait_copilot_phase("double")
        time.sleep(0.08)
        self.assertFalse(self.log.exists(), "display power command ran before key release could settle")
        self.finish_copilot_press(first)
        self.finish_copilot_press(second)
        self.assertEqual(self.log.read_text().splitlines(), ["copilot-display toggle"])

    def test_copilot_slow_presses_open_code_twice(self):
        self.copilot_fixture()
        self.run_script("copilot-shortcut.sh")
        self.run_script("copilot-shortcut.sh")
        self.assertEqual(self.log.read_text().splitlines(), ["code", "code"])

    def test_copilot_triple_press_toggles_displays_once_without_opening_code(self):
        self.copilot_fixture()
        first = self.copilot_press()
        self.wait_copilot_phase("single")
        second = self.copilot_press()
        self.wait_copilot_phase("double")
        third = self.copilot_press()
        for process in (first, second, third):
            self.finish_copilot_press(process)
        self.assertEqual(self.log.read_text().splitlines(), ["copilot-display toggle"])

    def test_copilot_concurrent_presses_do_not_race_into_code_launches(self):
        self.copilot_fixture()
        with ThreadPoolExecutor(max_workers=2) as executor:
            list(executor.map(lambda _: self.run_script("copilot-shortcut.sh"), range(2)))
        self.assertEqual(self.log.read_text().splitlines(), ["copilot-display toggle"])

    def test_copilot_stale_or_malformed_state_recovers_as_single_press(self):
        state = self.copilot_fixture()
        future = time.time_ns() // 1000 + 60_000_000
        for previous in (
            "0 single\n", "0 double\n", "broken state\n", "12345 unknown\n",
            "000000008 single\n", f"{future} single\n", "\n",
        ):
            with self.subTest(previous=previous):
                state.write_text(previous)
                self.log.unlink(missing_ok=True)
                self.run_script("copilot-shortcut.sh")
                self.assertEqual(self.log.read_text().splitlines(), ["code"])

    def test_copilot_single_press_cannot_open_code_while_locked(self):
        self.copilot_fixture()
        for session_state in ("locked", "no-session", "logind-error", "unknown"):
            with self.subTest(session_state=session_state):
                self.environment["XDG_SESSION_ID"] = "mock-copilot"
                self.environment["RUNTIME_TEST_LOCKED"] = "no"
                self.environment.pop("RUNTIME_TEST_LOGINCTL_FAIL", None)
                if session_state == "locked":
                    self.environment["RUNTIME_TEST_LOCKED"] = "yes"
                elif session_state == "no-session":
                    self.environment.pop("XDG_SESSION_ID")
                elif session_state == "logind-error":
                    self.environment["RUNTIME_TEST_LOGINCTL_FAIL"] = "1"
                else:
                    self.environment["RUNTIME_TEST_LOCKED"] = "unknown"
                self.run_script("copilot-shortcut.sh")
                self.assertFalse(self.log.exists(), "unconfirmed unlocked session launched an application")

    def test_copilot_display_verifies_blackout_then_delays_locker_one_second(self):
        state, _ = self.copilot_display_fixture()
        commands = self.run_copilot_display()
        self.assertEqual(set(state.read_text().splitlines()), {"eDP-1", "DP-1"})
        disabled = [command for command in commands if command.endswith(" off")]
        self.assertEqual(set(disabled), {"niri msg output eDP-1 off", "niri msg output DP-1 off"})
        last_off = max(commands.index(command) for command in disabled)
        delay_index = commands.index("sleep 1")
        self.assertIn("niri msg --json outputs", commands[last_off + 1:delay_index])
        self.assertLess(last_off, delay_index)
        self.assertLess(delay_index, commands.index("lock sleep"))
        self.assertLess(commands.index("lock sleep"), commands.index("loginctl yes"))
        outputs = json.loads(Path(self.environment["RUNTIME_TEST_OUTPUTS"]).read_text())
        self.assertTrue(all(outputs[name]["current_mode"] is None for name in ("eDP-1", "DP-1")))

    def test_copilot_display_restore_leaves_native_password_lock_active(self):
        state, ready = self.copilot_display_fixture()
        ready.touch()
        state.write_text("eDP-1\nDP-1\n")
        self.set_copilot_output_modes(enabled=False)
        commands = self.run_copilot_display()
        restored = [command for command in commands if command.endswith(" on")]
        self.assertEqual(set(restored), {"niri msg output eDP-1 on", "niri msg output DP-1 on"})
        self.assertTrue(all(commands.index("loginctl yes") < commands.index(command) for command in restored))
        power_index = commands.index("niri msg action power-on-monitors")
        self.assertTrue(all(commands.index(command) < power_index for command in restored))
        self.assertFalse(state.exists())
        self.assertTrue(ready.exists(), "restoring displays bypassed the password lock")
        self.assertFalse(any(command.startswith("lock ") for command in commands))

    def test_copilot_display_wake_confirms_password_lock_before_restoring_outputs(self):
        state, ready = self.copilot_display_fixture()
        state.write_text("eDP-1\nDP-1\n")
        self.set_copilot_output_modes(enabled=False)
        commands = self.run_copilot_display()
        locked_index = commands.index("loginctl yes")
        self.assertLess(commands.index("lock sleep"), locked_index)
        restored = [command for command in commands if command.endswith(" on")]
        self.assertEqual(set(restored), {"niri msg output eDP-1 on", "niri msg output DP-1 on"})
        self.assertTrue(all(locked_index < commands.index(command) for command in restored))
        self.assertNotIn("sleep 1", commands)
        self.assertTrue(ready.exists())
        self.assertFalse(state.exists())

    def test_copilot_display_resume_restores_saved_outputs_after_confirming_password_lock(self):
        state, ready = self.copilot_display_fixture()
        state.write_text("eDP-1\nDP-1\n")
        self.set_copilot_output_modes(enabled=False)
        commands = self.run_copilot_display(mode="resume")
        locked_index = commands.index("loginctl yes")
        self.assertLess(commands.index("lock sleep"), locked_index)
        restored = [command for command in commands if command.endswith(" on")]
        self.assertEqual(set(restored), {"niri msg output eDP-1 on", "niri msg output DP-1 on"})
        self.assertTrue(all(locked_index < commands.index(command) for command in restored))
        self.assertIn("niri msg action power-on-monitors", commands)
        self.assertFalse(any(command.endswith(" off") for command in commands))
        self.assertNotIn("sleep 1", commands)
        self.assertTrue(ready.exists())
        self.assertFalse(state.exists())

    def test_copilot_display_resume_without_snapshot_repeatedly_wakes_only_after_lock(self):
        state, ready = self.copilot_display_fixture()
        for resume_number in range(2):
            with self.subTest(resume_number=resume_number):
                self.log.unlink(missing_ok=True)
                commands = self.run_copilot_display(mode="resume")
                power_index = commands.index("niri msg action power-on-monitors")
                self.assertLess(commands.index("loginctl yes"), power_index)
                if resume_number == 0:
                    self.assertLess(commands.index("lock sleep"), power_index)
                self.assertEqual(commands.count("niri msg action power-on-monitors"), 1)
                self.assertFalse(any(command.startswith("niri msg output ") for command in commands))
                self.assertNotIn("sleep 1", commands)
                self.assertTrue(ready.exists())
                self.assertFalse(state.exists(), "resume accidentally entered the display-off toggle mode")

    def test_copilot_display_resume_failures_preserve_recovery_state(self):
        state, ready = self.copilot_display_fixture()
        for failure in ("lock", "apply", "power", "lock-without-snapshot"):
            with self.subTest(failure=failure):
                self.log.unlink(missing_ok=True)
                ready.unlink(missing_ok=True)
                state.unlink(missing_ok=True)
                if failure != "lock-without-snapshot":
                    state.write_text("eDP-1\nDP-1\n")
                self.set_copilot_output_modes(enabled=False)
                self.environment["RUNTIME_TEST_LOCK_NEVER_READY"] = "1" if failure.startswith("lock") else "0"
                self.environment["RUNTIME_TEST_IGNORE_APPLY"] = "on" if failure == "apply" else ""
                self.environment["RUNTIME_TEST_POWER_ON_FAIL"] = "1" if failure == "power" else "0"
                commands = self.run_copilot_display(succeeds=False, mode="resume")
                self.assertFalse(any(command.endswith(" off") for command in commands))
                if failure.startswith("lock"):
                    self.assertFalse(any(command.endswith(" on") for command in commands))
                    self.assertNotIn("niri msg action power-on-monitors", commands)
                else:
                    self.assertIn("niri msg output eDP-1 on", commands)
                if failure == "lock-without-snapshot":
                    self.assertFalse(state.exists())
                else:
                    self.assertEqual(state.read_text(), "eDP-1\nDP-1\n")

    def test_copilot_display_resume_cancels_queued_gesture_and_accepts_fresh_gesture(self):
        state, _ = self.copilot_display_fixture()
        pending = self.runtime / "hype-copilot.state"
        stale_token = str(time.time_ns() // 1000)
        pending.write_text(f"{stale_token} double\n")
        self.run_copilot_display(mode="resume")
        self.assertFalse(pending.exists(), "resume left a queued display-off gesture active")
        self.log.unlink(missing_ok=True)
        self.environment["HYPE_COPILOT_TOKEN"] = stale_token
        commands = self.run_copilot_display()
        self.assertFalse(any(command.endswith(" off") for command in commands))
        self.assertFalse(state.exists())
        fresh_token = str(time.time_ns() // 1000)
        pending.write_text(f"{fresh_token} double\n")
        self.environment["HYPE_COPILOT_TOKEN"] = fresh_token
        commands = self.run_copilot_display()
        self.assertIn("niri msg output eDP-1 off", commands)
        self.assertIn("niri msg output DP-1 off", commands)
        self.assertTrue(state.exists())

    def test_hypridle_resumes_saved_displays_only_after_system_sleep(self):
        hooks = {"after_sleep_cmd": [], "on-resume": []}
        for line in (REPO / "hypr/hypridle.conf").read_text().splitlines():
            key, separator, value = line.partition("=")
            if separator and key.strip() in hooks:
                hooks[key.strip()].append(value.strip())
        self.assertEqual(hooks["after_sleep_cmd"], ["~/.config/scripts/copilot-display.sh resume"])
        self.assertTrue(hooks["on-resume"])
        self.assertTrue(all(command == "niri msg action power-on-monitors" for command in hooks["on-resume"]))

    def test_copilot_display_already_locked_does_not_restart_locker(self):
        state, ready = self.copilot_display_fixture()
        ready.touch()
        commands = self.run_copilot_display()
        self.assertTrue(state.exists())
        self.assertIn("niri msg output eDP-1 off", commands)
        self.assertFalse(any(command.startswith("lock ") for command in commands))

    def test_copilot_display_locked_hint_requires_running_password_locker(self):
        state, ready = self.copilot_display_fixture()
        self.environment["RUNTIME_TEST_LOGINCTL_VALUE"] = "yes"
        for locker_recovers in (True, False):
            with self.subTest(locker_recovers=locker_recovers):
                ready.unlink(missing_ok=True)
                state.unlink(missing_ok=True)
                self.log.unlink(missing_ok=True)
                self.set_copilot_output_modes(enabled=True)
                self.environment["RUNTIME_TEST_LOCK_NEVER_READY"] = "0" if locker_recovers else "1"
                commands = self.run_copilot_display(succeeds=locker_recovers)
                self.assertIn("lock sleep", commands)
                self.assertTrue(any(command.endswith(" off") for command in commands))
                self.assertFalse(any(command.endswith(" on") for command in commands))
                self.assertTrue(state.exists())

    def test_copilot_display_failed_delayed_lock_keeps_displays_off_and_retry_state(self):
        state, _ = self.copilot_display_fixture()
        self.environment["RUNTIME_TEST_LOCK_NEVER_READY"] = "1"
        commands = self.run_copilot_display(succeeds=False)
        self.assertIn("lock sleep", commands)
        self.assertIn("niri msg output eDP-1 off", commands)
        self.assertIn("niri msg output DP-1 off", commands)
        self.assertFalse(any(command.endswith(" on") for command in commands))
        self.assertNotIn("niri msg action power-on-monitors", commands)
        self.assertEqual(set(state.read_text().splitlines()), {"eDP-1", "DP-1"})
        outputs = json.loads(Path(self.environment["RUNTIME_TEST_OUTPUTS"]).read_text())
        self.assertTrue(all(outputs[name]["current_mode"] is None for name in ("eDP-1", "DP-1")))

    def test_copilot_display_partial_blackout_cannot_restore_without_password_lock(self):
        state, _ = self.copilot_display_fixture()
        self.environment["RUNTIME_TEST_OFF_FAIL"] = "DP-1"
        self.environment["RUNTIME_TEST_LOCK_NEVER_READY"] = "1"
        commands = self.run_copilot_display(succeeds=False)
        self.assertIn("lock sleep", commands)
        self.assertIn("niri msg output eDP-1 off", commands)
        self.assertFalse(any(command.endswith(" on") for command in commands))
        self.assertNotIn("niri msg action power-on-monitors", commands)
        self.assertEqual(set(state.read_text().splitlines()), {"eDP-1", "DP-1"})

    def test_copilot_display_stale_snapshot_cannot_restore_without_confirmed_lock(self):
        state, _ = self.copilot_display_fixture()
        state.write_text("eDP-1\nDP-1\n")
        self.environment["RUNTIME_TEST_LOCK_NEVER_READY"] = "1"
        commands = self.run_copilot_display(succeeds=False)
        self.assertFalse(any(command.endswith(" on") for command in commands))
        self.assertNotIn("niri msg action power-on-monitors", commands)
        self.assertEqual(state.read_text(), "eDP-1\nDP-1\n")

    def test_copilot_display_disable_acknowledgement_requires_outputs_to_turn_off(self):
        state, ready = self.copilot_display_fixture()
        ready.touch()
        self.environment["RUNTIME_TEST_IGNORE_APPLY"] = "off"
        commands = self.run_copilot_display(succeeds=False)
        self.assertIn("niri msg output eDP-1 off", commands)
        self.assertIn("niri msg output eDP-1 on", commands)
        self.assertIn("niri msg output DP-1 on", commands)
        self.assertFalse(state.exists(), "successful rollback retained disabled-display state")

    def test_copilot_display_restore_acknowledgement_requires_visible_output(self):
        state, ready = self.copilot_display_fixture()
        ready.touch()
        state.write_text("eDP-1\nDP-1\n")
        self.set_copilot_output_modes(enabled=False)
        self.environment["RUNTIME_TEST_IGNORE_APPLY"] = "on"
        commands = self.run_copilot_display(succeeds=False)
        self.assertIn("niri msg output eDP-1 on", commands)
        self.assertEqual(state.read_text(), "eDP-1\nDP-1\n")

    def test_copilot_display_restore_tolerates_disconnected_saved_output(self):
        state, ready = self.copilot_display_fixture()
        ready.touch()
        path = Path(self.environment["RUNTIME_TEST_OUTPUTS"])
        outputs = json.loads(path.read_text())
        del outputs["DP-1"]
        for all_disconnected in (False, True):
            with self.subTest(all_disconnected=all_disconnected):
                state.write_text("eDP-1\nDP-1\n")
                if all_disconnected:
                    del outputs["eDP-1"]
                else:
                    outputs["eDP-1"]["current_mode"] = None
                path.write_text(json.dumps(outputs))
                self.run_copilot_display(succeeds=not all_disconnected)
                self.assertEqual(state.exists(), all_disconnected)

    def test_copilot_display_rejects_missing_or_untrusted_session_state(self):
        self.copilot_display_fixture()
        for failure in ("missing-session", "loginctl-error", "unknown-lock-state"):
            with self.subTest(failure=failure):
                self.log.unlink(missing_ok=True)
                self.environment["XDG_SESSION_ID"] = "mock-copilot"
                self.environment.pop("RUNTIME_TEST_LOGINCTL_FAIL", None)
                self.environment.pop("RUNTIME_TEST_LOGINCTL_VALUE", None)
                if failure == "missing-session":
                    self.environment.pop("XDG_SESSION_ID")
                elif failure == "loginctl-error":
                    self.environment["RUNTIME_TEST_LOGINCTL_FAIL"] = "1"
                else:
                    self.environment["RUNTIME_TEST_LOGINCTL_VALUE"] = "unknown"
                commands = self.run_copilot_display(succeeds=False)
                self.assertFalse(any(command.endswith(" off") for command in commands))

    def test_copilot_display_restore_failure_preserves_state_for_retry(self):
        state, ready = self.copilot_display_fixture()
        ready.touch()
        for failure in ("output", "power"):
            with self.subTest(failure=failure):
                state.write_text("eDP-1\nDP-1\n")
                self.set_copilot_output_modes(enabled=False)
                self.environment["RUNTIME_TEST_ON_FAIL"] = "eDP-1" if failure == "output" else ""
                self.environment["RUNTIME_TEST_POWER_ON_FAIL"] = "1" if failure == "power" else "0"
                self.run_copilot_display(succeeds=False)
                self.assertEqual(state.read_text(), "eDP-1\nDP-1\n")
                self.assertTrue(ready.exists())

    def test_copilot_display_disable_failure_restores_outputs_and_keeps_retry_state_if_needed(self):
        state, ready = self.copilot_display_fixture()
        ready.touch()
        self.environment["RUNTIME_TEST_OFF_FAIL"] = "DP-1"
        for rollback_fails in (False, True):
            with self.subTest(rollback_fails=rollback_fails):
                self.log.unlink(missing_ok=True)
                self.environment["RUNTIME_TEST_ON_FAIL"] = "eDP-1" if rollback_fails else ""
                commands = self.run_copilot_display(succeeds=False)
                self.assertIn("niri msg output eDP-1 on", commands)
                self.assertIn("niri msg output DP-1 on", commands)
                self.assertEqual(state.exists(), rollback_fails)

    def test_waybar_keeps_single_instance_and_publishes_complete_config(self):
        self.waybar_fixture()
        self.hold_mock("waybar")
        self.start_script("start-waybar.sh")
        self.run_script("start-waybar.sh")
        self.assertEqual(len(self.log.read_text().splitlines()), 1)
        generated = self.runtime / "hype-waybar-multi-output.json"
        self.assertEqual([bar["output"] for bar in json.loads(generated.read_text())], ["eDP-1", "DP-1"])
        self.assertFalse(list(self.runtime.glob(".hype-waybar.*")))

    def test_failed_waybar_generation_cleans_temporary_file(self):
        config = self.waybar_fixture()
        config.write_text("{ invalid JSON }")
        self.mock("waybar", 'printf "%s\\n" "$*" >> "$RUNTIME_TEST_LOG"')
        self.run_script("start-waybar.sh")
        self.assertIn(str(config), self.log.read_text())
        self.assertFalse(list(self.runtime.glob(".hype-waybar.*")))

    def test_concurrent_sensor_scans_publish_complete_cache(self):
        sensor = self.sensor_fixture(label='Package\t"CPU"')
        with ThreadPoolExecutor(max_workers=8) as executor:
            results = list(executor.map(lambda _: self.run_script("temperature-control.sh"), range(8)))
        for result in results:
            self.assertEqual(json.loads(result.stdout)["text"], "45°")
        cache = (self.runtime / "hype-niri-temp-sensor").read_text()
        self.assertEqual(cache.split("\t", 1)[0], str(sensor))
        self.assertFalse(list(self.runtime.glob(".hype-temp-sensor.*")))
        self.mock("mktemp", "exit 99")
        self.assertEqual(json.loads(self.run_script("temperature-control.sh").stdout)["text"], "45°")

    def test_sensor_cache_does_not_escape_configured_sensor_roots(self):
        self.sensor_fixture("075000")
        unrelated = self.root / "unrelated"
        unrelated.write_text("45000\n")
        (self.runtime / "hype-niri-temp-sensor").write_text(f"{unrelated}\tUnrelated\n")
        result = json.loads(self.run_script("temperature-control.sh").stdout)
        self.assertEqual(result["text"], "75°")
        self.assertEqual(result["class"], "warning")

    def test_wallpaper_ensure_handles_newlines_without_launching_daemon(self):
        wallpapers = self.user_home / "pictures/wallpapers"
        wallpapers.mkdir(parents=True)
        image = wallpapers / "one\ntwo.PNG"
        image.touch()
        self.mock("awww", 'printf "unexpected image\\n" >> "$RUNTIME_TEST_LOG"')
        self.mock("awww-daemon", 'printf "unexpected daemon\\n" >> "$RUNTIME_TEST_LOG"')
        with ThreadPoolExecutor(max_workers=6) as executor:
            list(executor.map(lambda _: self.run_script("wallpaper.sh", "ensure"), range(6)))
        current = self.user_home / ".local/state/niri/current_wallpaper"
        self.assertEqual(current.resolve(), image)
        self.assertFalse(self.log.exists())
        self.assertFalse(list(current.parent.glob(".current-wallpaper.*")))

    def test_lock_requests_keep_single_instance(self):
        self.hold_mock("hyprlock")
        self.start_script("lock.sh")
        with ThreadPoolExecutor(max_workers=6) as executor:
            list(executor.map(lambda _: self.run_script("lock.sh"), range(6)))
        self.assertEqual(len(self.log.read_text().splitlines()), 1)

    def test_tray_launcher_keeps_single_instance_without_process_discovery(self):
        self.hold_mock("nm-applet")
        pid_file = self.root / "applet-pid"
        self.environment["RUNTIME_TEST_APPLET_PID"] = str(pid_file)
        self.mock("nm-applet", '''
printf '%s' "$$" > "$RUNTIME_TEST_APPLET_PID"
printf 'nm-applet\\n' >> "$RUNTIME_TEST_LOG"
read -r _ < "$RUNTIME_TEST_HOLD"''')
        self.mock("blueman-applet", "exit 0")
        self.run_script("start-tray-applets.sh")
        deadline = time.monotonic() + 3
        while not pid_file.exists():
            self.assertLess(time.monotonic(), deadline, "applet did not start")
            time.sleep(0.01)
        self.addCleanup(self.terminate_pid, int(pid_file.read_text()))
        with ThreadPoolExecutor(max_workers=6) as executor:
            list(executor.map(lambda _: self.run_script("start-tray-applets.sh"), range(6)))
        self.assertEqual(self.log.read_text().splitlines(), ["nm-applet"])

    def test_volume_parser_preserves_percentage_and_mute_notifications(self):
        self.mock("notify-send", 'printf "%s\\n" "$*" >> "$RUNTIME_TEST_LOG"')
        self.mock("wpctl", "printf 'Volume: 0.65\\n'")
        self.run_script("volume-control.sh")
        self.assertIn("int:value:65", self.log.read_text())
        self.assertIn("Volume: 65%", self.log.read_text())
        self.mock("wpctl", "printf 'Volume: 0.65 [MUTED]\\n'")
        self.run_script("volume-control.sh")
        self.assertIn("Muted", self.log.read_text().splitlines()[-1])

    def test_wallpaper_menu_preserves_newline_filename(self):
        wallpapers = self.user_home / "pictures/wallpapers"
        wallpapers.mkdir(parents=True)
        image = wallpapers / "one\ntwo.jpg"
        image.touch()
        self.mock("fuzzel", "cat >/dev/null\nprintf '0\\n'")
        self.mock("pgrep", '[[ "${!#}" == awww-daemon ]]')
        self.mock("awww", "exit 0")
        self.run_script("wallpaper.sh", "select")
        self.assertEqual((self.user_home / ".local/state/niri/current_wallpaper").resolve(), image)

    def test_caffeine_stop_terminates_owned_sleep_child(self):
        pid_file = self.root / "sleep-pid"
        self.environment["RUNTIME_TEST_CHILD_PID"] = str(pid_file)
        program = (
            "import os,subprocess; "
            "child=subprocess.Popen(['/usr/bin/sleep','infinity']); "
            "open(os.environ['RUNTIME_TEST_CHILD_PID'],'w').write(str(child.pid)); "
            "child.wait()"
        )
        self.mock("systemd-inhibit", f"exec -a systemd-inhibit {shlex.quote(shutil.which('python3'))} -c {shlex.quote(program)} \"$@\"")
        result = json.loads(self.run_script("caffeine-control.sh", "toggle").stdout)
        self.assertEqual(result["class"], "activated")
        child_pid = int(pid_file.read_text())
        parent_pid = int((self.runtime / "caffeine_pid").read_text())
        self.addCleanup(self.terminate_pid, child_pid)
        self.addCleanup(self.terminate_pid, parent_pid)
        self.run_script("caffeine-control.sh", "stop")
        self.assertFalse((self.runtime / "caffeine_pid").exists())
        self.assertFalse((self.runtime / "caffeine_state").exists())
        deadline = time.monotonic() + 3
        while self.pid_running(child_pid):
            self.assertLess(time.monotonic(), deadline, "orphaned sleep process survived")
            time.sleep(0.01)

    @staticmethod
    def pid_running(pid):
        try:
            return Path(f"/proc/{pid}/stat").read_text().split(") ", 1)[1][0] != "Z"
        except FileNotFoundError:
            return False

    @staticmethod
    def terminate_pid(pid):
        try:
            os.kill(pid, signal.SIGTERM)
        except ProcessLookupError:
            pass

    def test_caffeine_does_not_kill_unrelated_recorded_pid(self):
        process = subprocess.Popen(["sleep", "infinity"])
        self.addCleanup(self.stop_process, process)
        (self.runtime / "caffeine_pid").write_text(f"{process.pid}\n")
        (self.runtime / "caffeine_state").touch()
        self.run_script("caffeine-control.sh", "stop")
        self.assertIsNone(process.poll())

    def test_failed_inhibitor_does_not_leave_active_state(self):
        self.mock("systemd-inhibit", "exit 1")
        result = json.loads(self.run_script("caffeine-control.sh", "toggle").stdout)
        self.assertEqual(result["class"], "deactivated")
        self.assertFalse((self.runtime / "caffeine_pid").exists())
        self.assertFalse((self.runtime / "caffeine_state").exists())

    def test_drive_unlock_removes_passphrase_file_when_interrupted(self):
        device = self.root / "mock-device"
        marker = self.root / "key-file"
        device.touch()
        self.environment.update(RUNTIME_TEST_DEVICE=str(device), RUNTIME_TEST_KEY=str(marker))
        self.mock("lsblk", '''
case "$*" in
    '-prno NAME,FSTYPE') printf '%s crypto_LUKS\\n' "$RUNTIME_TEST_DEVICE" ;;
    '-nrpo NAME '*) printf '%s\\n' "$RUNTIME_TEST_DEVICE" ;;
    '-dno PARTLABEL '*) printf 'Drive\\n' ;;
esac''')
        self.mock("findmnt", "exit 1")
        self.mock("fuzzel", "printf 'test-only-passphrase'")
        preamble = '''
function [ {
    if [[ ${1:-} == -b && ${2:-} == "$RUNTIME_TEST_DEVICE" ]]; then return 0; fi
    builtin [ "$@"
}
udisksctl() {
    printf '%s' "$5" > "$RUNTIME_TEST_KEY"
    sleep 30
}
'''
        process = subprocess.Popen(
            ["bash", "-c", preamble + "source " + shlex.quote(str(self.scripts / "open-drives.sh"))],
            env=self.environment,
            stdout=subprocess.DEVNULL,
            stderr=subprocess.PIPE,
            text=True,
            start_new_session=True,
        )
        self.addCleanup(self.stop_process_group, process)
        deadline = time.monotonic() + 3
        while not marker.exists():
            self.assertIsNone(process.poll(), process.stderr.read() if process.poll() is not None else "")
            self.assertLess(time.monotonic(), deadline, "unlock did not start")
            time.sleep(0.01)
        key_file = Path(marker.read_text())
        self.assertEqual(key_file.read_text(), "test-only-passphrase")
        self.assertEqual(key_file.stat().st_mode & 0o777, 0o600)
        os.killpg(process.pid, signal.SIGTERM)
        process.communicate(timeout=3)
        self.assertFalse(key_file.exists())

    def test_runtime_fallback_rejects_symlink(self):
        helper = self.scripts / "runtime-dir.sh"
        fallback = self.root / "fallback"
        helper.write_text(helper.read_text().replace('/tmp/hype-niri-$UID', str(fallback)))
        fallback.symlink_to(self.runtime, target_is_directory=True)
        result = subprocess.run(
            ["bash", str(helper)],
            env=self.environment | {"XDG_RUNTIME_DIR": str(self.root / "missing")},
            capture_output=True,
            timeout=3,
        )
        self.assertNotEqual(result.returncode, 0)

    def test_runtime_fallback_is_private(self):
        helper = self.scripts / "runtime-dir.sh"
        fallback = self.root / "fallback"
        helper.write_text(helper.read_text().replace('/tmp/hype-niri-$UID', str(fallback)))
        self.run_script("runtime-dir.sh", environment=self.environment | {"XDG_RUNTIME_DIR": str(self.root / "missing")})
        self.assertEqual(fallback.stat().st_mode & 0o777, 0o700)


if __name__ == "__main__":
    unittest.main(verbosity=2)
