#!/usr/bin/env python3
"""Drive auto-power-profile.sh with mocked D-Bus tools."""

import os
import signal
import subprocess
import tempfile
import time
import unittest
from pathlib import Path

REPO = Path(__file__).resolve().parents[1]
SCRIPT = REPO / "scripts/auto-power-profile.sh"

BUSCTL = r"""
state="$POWER_TEST_STATE"
printf '%s\n' "$*" >> "$state/busctl.log"
value() { cat "$state/$1" 2>/dev/null || true; }
case "$*" in
    *"/org/freedesktop/UPower org.freedesktop.UPower OnBattery")
        [[ -n "$(value on_battery)" ]] || exit 1
        printf 'b %s\n' "$(value on_battery)"
        ;;
    *"DisplayDevice org.freedesktop.UPower.Device IsPresent Type Percentage")
        printf 'b %s\nu %s\nd %s\n' "$(value present)" "$(value type)" "$(value percentage)"
        ;;
    *"ActiveProfile Profiles")
        [[ ! -e "$state/profiles_down" ]] || exit 1
        printf 's "%s"\naa{sv} 3' "$(value active)"
        for name in $(value profiles); do
            printf ' 4 "Profile" s "%s" "CpuDriver" s "x" "PlatformDriver" s "y" "Driver" s "z"' "$name"
        done
        printf '\n'
        ;;
    *" call "*" Set ssv "*" ActiveProfile s "*)
        target="${!#}"
        printf '%s\n' "$target" >> "$state/calls"
        failures="$(value fail_set)"
        if [[ "${failures:-0}" -gt 0 ]]; then
            printf '%s' "$((failures - 1))" > "$state/fail_set"
            exit 1
        fi
        printf '%s' "$target" > "$state/active"
        if [[ -e "$state/monitor.ready" ]]; then
            printf '   string "ActiveProfile"\n' > "$state/events"
        fi
        ;;
    *) exit 64 ;;
esac
"""

SLEEP = r"""
state="$POWER_TEST_STATE"
printf '%s\n' "$1" >> "$state/sleep.log"
if [[ -e "$state/next_percentage" ]]; then
    mv "$state/next_percentage" "$state/percentage"
fi
"""


class PowerProfileTests(unittest.TestCase):
    def setUp(self):
        temporary = tempfile.TemporaryDirectory(prefix="hype-niri-power-test-")
        self.addCleanup(temporary.cleanup)
        self.root = Path(temporary.name)
        self.state = self.root / "state"
        self.state.mkdir()
        self.runtime = self.root / "runtime"
        self.runtime.mkdir(mode=0o700)
        self.bin = self.root / "bin"
        self.bin.mkdir()
        self.mock("busctl", BUSCTL)
        self.mock("sleep", SLEEP)
        self.mock("pkill", 'printf "%s\\n" "$*" >> "$POWER_TEST_STATE/pkill.log"')
        self.mock(
            "dbus-monitor",
            'printf "%s\\n" "$@" > "$POWER_TEST_STATE/monitor.args"\n'
            'touch "$POWER_TEST_STATE/monitor.ready"\n'
            "printf 'signal sender=org.freedesktop.DBus member=NameAcquired\\n'\n"
            'exec cat "$POWER_TEST_STATE/events"',
        )
        os.mkfifo(self.state / "events")
        self.environment = os.environ | {
            "POWER_TEST_STATE": str(self.state),
            "XDG_RUNTIME_DIR": str(self.runtime),
            "PATH": f"{self.bin}:{os.environ['PATH']}",
        }
        self.set(
            on_battery="true",
            present="true",
            type="2",
            percentage="70",
            active="performance",
            profiles="power-saver balanced performance",
        )
        self.events = None
        self.process = None

    def mock(self, command, body):
        path = self.bin / command
        path.write_text("#!/usr/bin/env bash\nset -eu\n" + body + "\n")
        path.chmod(0o755)

    def set(self, **values):
        for name, value in values.items():
            (self.state / name).write_text(value)

    def lines(self, name):
        path = self.state / name
        return path.read_text().splitlines() if path.exists() else []

    def calls(self):
        return self.lines("calls")

    def start(self):
        self.events = os.open(self.state / "events", os.O_RDWR)
        self.process = subprocess.Popen(
            ["bash", str(SCRIPT)],
            env=self.environment,
            stdout=subprocess.DEVNULL,
            stderr=subprocess.DEVNULL,
            start_new_session=True,
        )
        self.addCleanup(self.stop)
        self.settle(0)

    def stop(self):
        if self.process.poll() is None:
            os.killpg(self.process.pid, signal.SIGTERM)
        self.process.wait(timeout=3)
        if self.events is not None:
            os.close(self.events)
            self.events = None

    def settle(self, mark):
        deadline = time.monotonic() + 5
        while len(self.lines("busctl.log")) <= mark:
            self.assertLess(time.monotonic(), deadline, "daemon did not react")
            time.sleep(0.01)
        quiet = (len(self.lines("busctl.log")), time.monotonic())
        while time.monotonic() - quiet[1] < 0.3:
            count = len(self.lines("busctl.log"))
            if count != quiet[0]:
                quiet = (count, time.monotonic())
            time.sleep(0.01)

    def emit(self, *lines):
        os.write(self.events, "".join(f"{line}\n" for line in lines).encode())

    def change(self, **values):
        names = {
            "on_battery": "OnBattery",
            "present": "IsPresent",
            "type": "Type",
            "percentage": "Percentage",
            "active": "ActiveProfile",
            "profiles": "Profiles",
        }
        mark = len(self.lines("busctl.log"))
        self.set(**values)
        self.emit(*(f'   string "{names[name]}"' for name in values))
        self.settle(mark)

    def check(self):
        result = subprocess.run(
            ["bash", str(SCRIPT), "--check"],
            env=self.environment,
            capture_output=True,
            text=True,
            timeout=5,
            check=True,
        )
        return dict(line.split("=", 1) for line in result.stdout.splitlines())

    def test_policy_boundary_and_invalid_values(self):
        for on_battery, percentage, expected in (
            ("false", "0", "performance"),
            ("false", "", "performance"),
            ("true", "0", "power-saver"),
            ("true", "29.999", "power-saver"),
            ("true", "30", "balanced"),
            ("true", "100", "balanced"),
            ("true", "-1", ""),
            ("true", "101", ""),
            ("true", "nan", ""),
            ("true", "inf", ""),
            ("true", "", ""),
            ("", "50", ""),
        ):
            with self.subTest(on_battery=on_battery, percentage=percentage):
                self.set(on_battery=on_battery, percentage=percentage)
                snapshot = self.check()
                self.assertEqual(snapshot["policy"], expected)
                self.assertEqual(snapshot["target"], expected)
        self.assertEqual(self.calls(), [])

    def test_check_reports_state_without_writing(self):
        self.assertEqual(
            self.check(),
            {
                "on_battery": "true",
                "percentage": "70",
                "available": "power-saver balanced performance",
                "active": "performance",
                "policy": "balanced",
                "target": "balanced",
            },
        )
        self.assertEqual(self.calls(), [])
        self.assertEqual(self.lines("pkill.log"), [])

    def test_initial_policy_and_plug_unplug(self):
        self.start()
        self.assertEqual(self.calls(), ["balanced"])
        self.change(on_battery="false")
        self.change(on_battery="true")
        self.assertEqual(self.calls(), ["balanced", "performance", "balanced"])
        writes = [line for line in self.lines("busctl.log") if " call " in line]
        self.assertTrue(
            all(
                "--timeout=5" in line and "--allow-interactive-authorization=no" in line
                for line in writes
            )
        )

    def test_initial_profile_change_refreshes_waybar(self):
        self.start()
        self.assertEqual(self.calls(), ["balanced"])
        self.assertEqual(len(self.lines("pkill.log")), 2)

    def test_charger_change_during_initial_apply_is_not_lost(self):
        self.mock(
            "busctl",
            BUSCTL.replace(
                'printf \'%s\' "$target" > "$state/active"',
                'printf \'%s\' "$target" > "$state/active"\n'
                '        printf false > "$state/on_battery"',
            ),
        )
        self.start()
        self.assertEqual(self.calls(), ["balanced", "performance"])
        self.assertEqual((self.state / "active").read_text(), "performance")

    def test_monitor_watches_power_objects_and_service_restarts(self):
        self.start()
        rules = "\n".join(self.lines("monitor.args"))
        for expected in (
            "path='/org/freedesktop/UPower'",
            "path='/org/freedesktop/UPower/devices/DisplayDevice'",
            "path='/org/freedesktop/UPower/PowerProfiles'",
            "member='NameOwnerChanged',arg0='org.freedesktop.UPower'",
            "member='NameOwnerChanged',arg0='org.freedesktop.UPower.PowerProfiles'",
        ):
            self.assertIn(expected, rules)

    def test_ac_takes_precedence_over_low_battery(self):
        self.set(on_battery="false", percentage="10", active="power-saver")
        self.start()
        self.assertEqual(self.calls(), ["performance"])
        self.change(on_battery="true")
        self.assertEqual(self.calls(), ["performance", "power-saver"])

    def test_threshold_crossing_without_duplicate_writes(self):
        self.set(active="balanced", percentage="30")
        self.start()
        self.assertEqual(self.calls(), [])
        for percentage in ("29.99", "29", "20", "0"):
            self.change(percentage=percentage)
        self.assertEqual(self.calls(), ["power-saver"])
        self.change(percentage="30")
        self.assertEqual(self.calls(), ["power-saver", "balanced"])

    def test_manual_choice_until_next_policy_transition(self):
        self.start()
        self.change(active="performance")
        self.change(percentage="50")
        self.assertEqual(self.calls(), ["balanced"])
        self.change(percentage="29")
        self.assertEqual(self.calls(), ["balanced", "power-saver"])

    def test_unsupported_performance_falls_back_and_recovers(self):
        self.set(
            on_battery="false", active="power-saver", profiles="balanced power-saver"
        )
        self.start()
        self.assertEqual(self.calls(), ["balanced"])
        self.change(profiles="power-saver balanced")
        self.assertEqual(self.calls(), ["balanced"])
        self.change(profiles="balanced power-saver performance")
        self.assertEqual(self.calls(), ["balanced", "performance"])

    def test_daemon_restart_reapplies_policy(self):
        self.start()
        mark = len(self.lines("busctl.log"))
        (self.state / "profiles_down").touch()
        self.emit("signal sender=org.freedesktop.DBus member=NameOwnerChanged")
        self.settle(mark)
        self.assertEqual(self.calls(), ["balanced"])
        mark = len(self.lines("busctl.log"))
        (self.state / "profiles_down").unlink()
        self.set(active="power-saver")
        self.emit("signal sender=org.freedesktop.DBus member=NameOwnerChanged")
        self.settle(mark)
        self.assertEqual(self.calls(), ["balanced", "balanced"])

    def test_battery_hotplug(self):
        self.start()
        self.change(present="false")
        self.set(active="performance")
        self.change(present="true", percentage="20")
        self.assertEqual(self.calls(), ["balanced", "power-saver"])

    def test_desktop_without_battery_does_nothing(self):
        self.set(on_battery="false", present="false")
        self.start()
        self.change(percentage="10")
        self.assertEqual(self.calls(), [])

    def test_event_burst_has_one_write(self):
        self.set(active="balanced")
        self.start()
        mark = len(self.lines("busctl.log"))
        self.set(percentage="1")
        self.emit(*['   string "Percentage"'] * 100)
        self.settle(mark)
        self.assertEqual(self.calls(), ["power-saver"])
        reads = [line for line in self.lines("busctl.log") if "OnBattery" in line]
        self.assertLessEqual(len(reads), 4)

    def test_unrelated_property_changes_are_ignored(self):
        self.start()
        before = len(self.lines("busctl.log"))
        self.emit('   string "EnergyRate"', '   string "TimeToEmpty"')
        time.sleep(0.5)
        self.assertEqual(len(self.lines("busctl.log")), before)

    def test_failed_set_retries_then_succeeds(self):
        self.set(fail_set="1")
        self.start()
        self.assertEqual(self.calls(), ["balanced", "balanced"])
        self.assertEqual(self.lines("sleep.log"), ["1"])

    def test_failed_set_exhausts_three_attempts_and_resets_on_transition(self):
        self.set(fail_set="3")
        self.start()
        self.assertEqual(self.calls(), ["balanced"] * 3)
        self.assertEqual(self.lines("sleep.log"), ["1", "3"])
        self.change(percentage="60")
        self.assertEqual(self.calls(), ["balanced"] * 3)
        self.change(percentage="20")
        self.assertEqual(self.calls(), ["balanced"] * 3 + ["power-saver"])

    def test_transition_during_retry_uses_latest_state(self):
        self.set(fail_set="1", next_percentage="20")
        self.start()
        self.assertEqual(self.calls(), ["balanced", "power-saver"])

    def test_waybar_updates_for_initial_and_actual_profile_changes(self):
        self.set(active="balanced")
        self.start()
        self.assertEqual(len(self.lines("pkill.log")), 1)
        self.change(percentage="60")
        self.change(active="balanced")
        self.assertEqual(len(self.lines("pkill.log")), 1)
        self.change(active="performance")
        self.assertEqual(len(self.lines("pkill.log")), 2)
        self.change(percentage="20")
        self.assertEqual(len(self.lines("pkill.log")), 3)
        self.assertEqual(
            self.lines("pkill.log")[0], f"-RTMIN+16 -u {os.getuid()} -x waybar"
        )

    def test_second_instance_exits_while_first_runs(self):
        self.start()
        before = len(self.lines("busctl.log"))
        result = subprocess.run(
            ["bash", str(SCRIPT)], env=self.environment, timeout=3, check=False
        )
        self.assertEqual(result.returncode, 0)
        self.assertEqual(len(self.lines("busctl.log")), before)

    def test_lost_monitor_exits_with_failure_for_restart(self):
        self.start()
        os.close(self.events)
        self.events = None
        self.assertEqual(self.process.wait(timeout=3), 1)


if __name__ == "__main__":
    unittest.main()
