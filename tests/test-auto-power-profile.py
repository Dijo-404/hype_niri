#!/usr/bin/env python3

import contextlib
import importlib.util
import io
import os
import sys
import tempfile
import unittest
from pathlib import Path
from typing import ClassVar
from unittest.mock import Mock, patch

REPO = Path(__file__).resolve().parents[1]
sys.dont_write_bytecode = True
spec = importlib.util.spec_from_file_location(
    "auto_power_profile", REPO / "scripts/auto-power-profile.py"
)
power = importlib.util.module_from_spec(spec)
spec.loader.exec_module(power)


class FakeProxy:
    TYPES: ClassVar[dict[str, str]] = {
        "OnBattery": "b",
        "IsPresent": "b",
        "Type": "u",
        "Percentage": "d",
        "ActiveProfile": "s",
        "Profiles": "aa{sv}",
    }

    def __init__(self, **properties):
        self.properties = properties
        self.owner = ":1.1"
        self.handlers = {}
        self.calls = []
        self.inflight = None

    def get_cached_property(self, name):
        value = self.properties.get(name)
        if value is None:
            return None
        if name == "Profiles":
            value = [{"Profile": power.GLib.Variant("s", profile)} for profile in value]
        return power.GLib.Variant(self.TYPES[name], value)

    def get_name_owner(self):
        return self.owner

    def connect(self, name, callback):
        handler = len(self.handlers) + 1
        self.handlers[handler] = (name, callback)
        return handler

    def disconnect(self, handler):
        del self.handlers[handler]

    def change(self, **properties):
        self.properties.update(properties)
        changed = power.GLib.Variant(
            "a{sv}", {name: self.get_cached_property(name) for name in properties}
        )
        for name, callback in list(self.handlers.values()):
            if name == "g-properties-changed":
                callback(self, changed, [])

    def change_owner(self, owner):
        self.owner = owner
        for name, callback in list(self.handlers.values()):
            if name == "notify::g-name-owner":
                callback(self, None)

    def call(self, method, parameters, flags, timeout, cancellable, callback, attempt):
        target, _ = attempt
        if self.inflight is not None:
            raise AssertionError("Overlapping profile writes")
        if method != "org.freedesktop.DBus.Properties.Set":
            raise AssertionError(method)
        if parameters.unpack() != (power.PP_NAME, "ActiveProfile", target):
            raise AssertionError(parameters)
        if timeout != 5000:
            raise AssertionError("Profile write must have a bounded timeout")
        self.calls.append(target)
        self.inflight = (callback, attempt)

    def finish(self, error=None):
        callback, attempt = self.inflight
        target, _ = attempt
        self.inflight = None
        if error is None:
            self.change(ActiveProfile=target)
        with (
            contextlib.redirect_stdout(io.StringIO()),
            contextlib.redirect_stderr(io.StringIO()),
        ):
            callback(self, error, attempt)

    def call_finish(self, result):
        if result is not None:
            raise result
        return power.GLib.Variant("()", ())


class PowerProfileTests(unittest.TestCase):
    def setUp(self):
        self.timers = {}
        self.next_timer = 0
        self.enterContext(patch.object(power.GLib, "timeout_add", self.schedule))
        self.enterContext(patch.object(power.GLib, "source_remove", self.timers.pop))
        self.original_notifier = power.notify_waybar
        self.notify_waybar = self.enterContext(patch.object(power, "notify_waybar"))
        self.upower = FakeProxy(OnBattery=True)
        self.battery = FakeProxy(IsPresent=True, Type=2, Percentage=70.0)
        self.profiles = FakeProxy(
            ActiveProfile="performance",
            Profiles=["power-saver", "balanced", "performance"],
        )
        self.app = power.AutoPowerProfile(self.upower, self.battery, self.profiles)
        self.addCleanup(self.app.close)

    def schedule(self, milliseconds, callback):
        self.assertIn(milliseconds, (100, 1000, 3000))
        self.next_timer += 1
        self.timers[self.next_timer] = callback
        self.assertLessEqual(len(self.timers), 1)
        return self.next_timer

    def drain(self):
        while self.timers:
            identifier = next(iter(self.timers))
            callback = self.timers.pop(identifier)
            self.assertFalse(callback())

    def settle(self):
        self.drain()
        if self.profiles.inflight is not None:
            self.profiles.finish()
            self.drain()

    def test_policy_boundary_and_invalid_values(self):
        for on_battery, percentage, expected in (
            (False, 0, "performance"),
            (False, None, "performance"),
            (True, 0, "power-saver"),
            (True, 29.999, "power-saver"),
            (True, 30, "balanced"),
            (True, 100, "balanced"),
            (True, -1, None),
            (True, 101, None),
            (True, float("nan"), None),
            (True, float("inf"), None),
            (True, None, None),
            (None, 50, None),
        ):
            with self.subTest(on_battery=on_battery, percentage=percentage):
                self.assertEqual(
                    power.requested_profile(on_battery, percentage), expected
                )

    def test_initial_policy_and_plug_unplug(self):
        self.app.queue()
        self.settle()
        self.assertEqual(self.profiles.calls, ["balanced"])
        self.upower.change(OnBattery=False)
        self.settle()
        self.upower.change(OnBattery=True)
        self.settle()
        self.assertEqual(self.profiles.calls, ["balanced", "performance", "balanced"])

    def test_ac_takes_precedence_over_low_battery(self):
        self.upower.change(OnBattery=False)
        self.battery.change(Percentage=10.0)
        self.profiles.change(ActiveProfile="power-saver")
        self.settle()
        self.assertEqual(self.profiles.calls, ["performance"])
        self.upower.change(OnBattery=True)
        self.settle()
        self.assertEqual(self.profiles.calls, ["performance", "power-saver"])

    def test_threshold_crossing_without_duplicate_writes(self):
        self.profiles.change(ActiveProfile="balanced")
        self.battery.change(Percentage=30.0)
        self.settle()
        self.assertEqual(self.profiles.calls, [])
        for percentage in (29.99, 29, 20, 0):
            self.battery.change(Percentage=float(percentage))
            self.settle()
        self.assertEqual(self.profiles.calls, ["power-saver"])
        self.battery.change(Percentage=30.0)
        self.settle()
        self.assertEqual(self.profiles.calls, ["power-saver", "balanced"])

    def test_manual_choice_until_next_policy_transition(self):
        self.app.queue()
        self.settle()
        self.profiles.change(ActiveProfile="performance")
        self.assertEqual(self.timers, {})
        self.battery.change(Percentage=50.0)
        self.settle()
        self.assertEqual(self.profiles.calls, ["balanced"])
        self.battery.change(Percentage=29.0)
        self.settle()
        self.assertEqual(self.profiles.calls, ["balanced", "power-saver"])

    def test_unsupported_performance_falls_back_and_recovers(self):
        self.profiles.change(
            ActiveProfile="power-saver", Profiles=["balanced", "power-saver"]
        )
        self.upower.change(OnBattery=False)
        self.settle()
        self.assertEqual(self.profiles.calls, ["balanced"])
        self.profiles.change(Profiles=["power-saver", "balanced"])
        self.settle()
        self.assertEqual(self.profiles.calls, ["balanced"])
        self.profiles.change(Profiles=["balanced", "power-saver", "performance"])
        self.settle()
        self.assertEqual(self.profiles.calls, ["balanced", "performance"])

    def test_daemon_restart_reapplies_policy(self):
        self.app.queue()
        self.settle()
        self.profiles.change_owner(None)
        self.settle()
        self.assertEqual(self.profiles.calls, ["balanced"])
        self.profiles.change(ActiveProfile="power-saver")
        self.profiles.change_owner(":1.2")
        self.settle()
        self.assertEqual(self.profiles.calls, ["balanced", "balanced"])

    def test_upower_restart_and_battery_hotplug(self):
        self.app.queue()
        self.settle()
        self.upower.change_owner(None)
        self.battery.change_owner(None)
        self.settle()
        self.profiles.change(ActiveProfile="performance")
        self.upower.change_owner(":1.3")
        self.battery.change_owner(":1.3")
        self.settle()
        self.battery.change(IsPresent=False)
        self.settle()
        self.profiles.change(ActiveProfile="performance")
        self.battery.change(IsPresent=True, Percentage=20.0)
        self.settle()
        self.assertEqual(self.profiles.calls, ["balanced", "balanced", "power-saver"])

    def test_desktop_without_battery_does_nothing(self):
        self.upower.change(OnBattery=False)
        self.battery.change(IsPresent=False)
        self.settle()
        self.assertEqual(self.profiles.calls, [])

    def test_event_burst_has_one_timer_and_one_write(self):
        for percentage in range(100, 0, -1):
            self.battery.change(Percentage=float(percentage))
        self.assertEqual(len(self.timers), 1)
        self.settle()
        self.assertEqual(self.profiles.calls, ["power-saver"])

    def test_events_during_write_are_serialized_to_latest_state(self):
        self.upower.change(OnBattery=False)
        self.profiles.change(ActiveProfile="balanced")
        self.drain()
        self.assertEqual(self.profiles.calls, ["performance"])
        for percentage in range(100, 0, -1):
            self.upower.change(OnBattery=bool(percentage % 2))
            self.battery.change(Percentage=float(percentage))
        self.assertEqual(self.timers, {})
        self.assertEqual(self.profiles.calls, ["performance"])
        self.profiles.finish()
        self.drain()
        self.assertEqual(self.profiles.calls, ["performance", "power-saver"])
        self.profiles.finish()
        self.drain()
        self.assertEqual(self.profiles.calls, ["performance", "power-saver"])

    def test_transient_events_back_to_same_state_do_not_repeat_write(self):
        self.profiles.change(ActiveProfile="balanced")
        self.upower.change(OnBattery=False)
        self.drain()
        self.upower.change(OnBattery=True)
        self.battery.change(Percentage=20.0)
        self.upower.change(OnBattery=False)
        self.profiles.finish()
        self.drain()
        self.assertEqual(self.profiles.calls, ["performance"])

    def test_failed_set_retries_then_succeeds(self):
        self.app.queue()
        self.drain()
        self.profiles.finish(power.GLib.Error("denied"))
        self.assertNotEqual(self.app.retry_source, 0)
        self.drain()
        self.profiles.finish()
        self.drain()
        self.assertEqual(self.profiles.calls, ["balanced", "balanced"])
        self.assertEqual(self.app.failures, 0)

    def test_failed_set_exhausts_three_attempts_and_resets_on_transition(self):
        self.app.queue()
        for _ in range(3):
            self.drain()
            self.profiles.finish(power.GLib.Error("denied"))
        self.assertEqual(self.timers, {})
        self.assertEqual(self.app.failures, 3)
        self.battery.change(Percentage=60.0)
        self.settle()
        self.assertEqual(self.profiles.calls, ["balanced"] * 3)
        self.battery.change(Percentage=20.0)
        self.settle()
        self.assertEqual(self.profiles.calls, ["balanced"] * 3 + ["power-saver"])

    def test_transition_cancels_pending_retry_and_uses_latest_state(self):
        self.app.queue()
        self.drain()
        self.profiles.finish(power.GLib.Error("denied"))
        previous = self.app.retry_source
        self.battery.change(Percentage=60.0)
        self.assertEqual(self.app.retry_source, previous)
        self.battery.change(Percentage=20.0)
        self.assertEqual(self.app.retry_source, 0)
        self.assertNotIn(previous, self.timers)
        self.settle()
        self.assertEqual(self.profiles.calls, ["balanced", "power-saver"])

    def test_close_cancels_pending_retry(self):
        self.app.queue()
        self.drain()
        self.profiles.finish(power.GLib.Error("denied"))
        self.app.close()
        self.assertEqual(self.timers, {})
        self.assertEqual(self.app.retry_source, 0)

    def test_waybar_updates_for_initial_and_actual_profile_changes(self):
        self.profiles.properties["ActiveProfile"] = "balanced"
        self.app.queue()
        self.settle()
        self.assertEqual(self.notify_waybar.call_count, 1)
        self.battery.change(Percentage=60.0)
        self.settle()
        self.profiles.change(ActiveProfile="balanced")
        self.assertEqual(self.notify_waybar.call_count, 1)
        self.profiles.change(ActiveProfile="performance")
        self.assertEqual(self.notify_waybar.call_count, 2)
        self.battery.change(Percentage=20.0)
        self.settle()
        self.assertEqual(self.notify_waybar.call_count, 3)
        self.profiles.change_owner(None)
        self.settle()
        self.profiles.change_owner(":1.2")
        self.settle()
        self.assertEqual(self.notify_waybar.call_count, 4)

    def test_waybar_notification_only_signals_owned_process_and_closes_pidfd(self):
        own = Mock(name="own_waybar")
        own.name = "101"
        own.stat.return_value.st_uid = os.getuid()
        own.__truediv__ = Mock(
            return_value=Mock(read_text=Mock(return_value="waybar\n"))
        )
        foreign = Mock(name="foreign_waybar")
        foreign.name = "102"
        foreign.stat.return_value.st_uid = os.getuid() + 1
        unrelated = Mock(name="unrelated")
        unrelated.name = "103"
        unrelated.stat.return_value.st_uid = os.getuid()
        unrelated.__truediv__ = Mock(
            return_value=Mock(read_text=Mock(return_value="python\n"))
        )
        root = Mock(iterdir=Mock(return_value=[own, foreign, unrelated]))
        with (
            patch.object(power, "Path", return_value=root),
            patch.object(power.os, "pidfd_open", return_value=42) as opened,
            patch.object(
                power.signal, "pidfd_send_signal", side_effect=ProcessLookupError
            ) as sent,
            patch.object(power.os, "close") as closed,
        ):
            self.original_notifier()
        opened.assert_called_once_with(101)
        sent.assert_called_once_with(42, power.signal.SIGRTMIN + 16)
        closed.assert_called_once_with(42)

    def test_shutdown_cancels_timer_and_releases_handlers(self):
        self.app.queue()
        self.app.close()
        self.assertEqual(self.timers, {})
        self.assertTrue(self.app.cancellable.is_cancelled())
        self.assertEqual(self.app.handlers, [])
        self.assertEqual(self.profiles.handlers, {})

    def test_instance_lock_is_released_on_close(self):
        with (
            tempfile.TemporaryDirectory() as runtime,
            patch.dict(os.environ, {"XDG_RUNTIME_DIR": runtime}),
        ):
            first = power.lock_instance()
            self.assertIsNotNone(first)
            try:
                self.assertIsNone(power.lock_instance())
            finally:
                os.close(first)
            second = power.lock_instance()
            self.assertIsNotNone(second)
            os.close(second)

    def test_instance_lock_rejects_symlinks_and_shared_runtime(self):
        with tempfile.TemporaryDirectory() as runtime:
            link = Path(runtime) / "link"
            link.symlink_to(runtime, target_is_directory=True)
            with (
                patch.dict(os.environ, {"XDG_RUNTIME_DIR": str(link)}),
                self.assertRaises(OSError),
            ):
                power.lock_instance()
            with patch.dict(os.environ, {"XDG_RUNTIME_DIR": runtime}):
                os.chmod(runtime, 0o755)
                with self.assertRaises(OSError):
                    power.lock_instance()
                os.chmod(runtime, 0o700)
                lock = Path(runtime) / "hype-auto-power-profile.lock"
                lock.symlink_to(Path(runtime) / "other")
                with self.assertRaises(OSError):
                    power.lock_instance()
                lock.unlink()
                lock.mkdir()
                with self.assertRaises(OSError):
                    power.lock_instance()


if __name__ == "__main__":
    unittest.main()
