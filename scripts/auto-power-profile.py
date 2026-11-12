#!/usr/bin/env python3

import argparse
import fcntl
import json
import math
import os
import signal
import stat
import sys
from pathlib import Path

import gi

gi.require_version("Gio", "2.0")
gi.require_version("GLibUnix", "2.0")
from gi.repository import Gio, GLib, GLibUnix  # noqa: E402

UP_NAME = "org.freedesktop.UPower"
UP_PATH = "/org/freedesktop/UPower"
PP_NAME = "org.freedesktop.UPower.PowerProfiles"
PP_PATH = "/org/freedesktop/UPower/PowerProfiles"


def requested_profile(on_battery, percentage):
    if on_battery is False:
        return "performance"
    if on_battery is not True or percentage is None:
        return None
    if not math.isfinite(percentage) or not 0 <= percentage <= 100:
        return None
    return "power-saver" if percentage < 30 else "balanced"


def proxy(name, path, interface):
    return Gio.DBusProxy.new_for_bus_sync(
        Gio.BusType.SYSTEM,
        Gio.DBusProxyFlags.GET_INVALIDATED_PROPERTIES,
        None,
        name,
        path,
        interface,
        None,
    )


def notify_waybar():
    for process in Path("/proc").iterdir():
        if not process.name.isdecimal():
            continue
        descriptor = None
        try:
            if (
                process.stat().st_uid != os.getuid()
                or (process / "comm").read_text().strip() != "waybar"
            ):
                continue
            descriptor = os.pidfd_open(int(process.name))
            if (
                process.stat().st_uid == os.getuid()
                and (process / "comm").read_text().strip() == "waybar"
            ):
                signal.pidfd_send_signal(descriptor, signal.SIGRTMIN + 16)
        except OSError:
            pass
        finally:
            if descriptor is not None:
                os.close(descriptor)


class AutoPowerProfile:
    def __init__(self, upower, battery, profiles):
        self.upower = upower
        self.battery = battery
        self.profiles = profiles
        self.source = 0
        self.retry_source = 0
        self.pending = False
        self.last_state = None
        self.last_profile = None
        self.retry_state = None
        self.failures = 0
        self.cancellable = Gio.Cancellable()
        self.handlers = []
        for remote in (upower, battery, profiles):
            self.handlers.extend(
                (remote, remote.connect(name, callback))
                for name, callback in (
                    ("g-properties-changed", self.properties_changed),
                    ("notify::g-name-owner", self.owner_changed),
                )
            )

    @staticmethod
    def property(remote, name):
        value = remote.get_cached_property(name)
        return value.unpack() if value is not None else None

    def snapshot(self):
        available = sorted(
            entry["Profile"]
            for entry in (self.property(self.profiles, "Profiles") or [])
            if entry.get("Profile") in {"performance", "balanced", "power-saver"}
        )
        on_battery = self.property(self.upower, "OnBattery")
        percentage = self.property(self.battery, "Percentage")
        owners = tuple(
            remote.get_name_owner()
            for remote in (self.upower, self.battery, self.profiles)
        )
        wanted = None
        if (
            all(owners)
            and self.property(self.battery, "IsPresent") is True
            and self.property(self.battery, "Type") == 2
        ):
            wanted = requested_profile(on_battery, percentage)
        target = None
        if wanted is not None:
            target = next(
                (
                    name
                    for name in (wanted, "balanced", "power-saver")
                    if name in available
                ),
                None,
            )
        return {
            "on_battery": on_battery,
            "percentage": percentage,
            "available": available,
            "active": self.property(self.profiles, "ActiveProfile"),
            "policy": wanted,
            "target": target,
            "owners": owners,
        }

    def properties_changed(self, remote, changed, invalidated):
        if remote is self.profiles and (
            "ActiveProfile" in changed.unpack() or "ActiveProfile" in invalidated
        ):
            self.notify_profile()
        relevant = (
            {"OnBattery"}
            if remote is self.upower
            else {"IsPresent", "Percentage", "Type"}
            if remote is self.battery
            else {"Profiles"}
        )
        if relevant.intersection(changed.unpack()) or relevant.intersection(
            invalidated
        ):
            self.queue()

    def notify_profile(self):
        current = (
            self.property(self.profiles, "ActiveProfile")
            if self.profiles.get_name_owner()
            else None
        )
        if current != self.last_profile:
            self.last_profile = current
            if current is not None:
                notify_waybar()

    def owner_changed(self, remote, *_):
        if remote is self.profiles:
            self.notify_profile()
        self.queue()

    def queue(self):
        if self.retry_source:
            if self.state_key(self.snapshot()) == self.retry_state:
                return
            GLib.source_remove(self.retry_source)
            self.retry_source = 0
        if not self.source and not self.pending:
            self.source = GLib.timeout_add(100, self.apply)

    @staticmethod
    def state_key(state):
        if state["target"] is None:
            return None
        return (state["policy"], tuple(state["available"]), state["owners"])

    def apply(self):
        self.source = 0
        self.notify_profile()
        state = self.snapshot()
        key = self.state_key(state)
        if key != self.retry_state:
            self.retry_state = key
            self.failures = 0
        if key is None:
            self.last_state = None
            return GLib.SOURCE_REMOVE
        if key == self.last_state:
            return GLib.SOURCE_REMOVE
        self.last_state = key
        if state["active"] != state["target"]:
            self.pending = True
            self.profiles.call(
                "org.freedesktop.DBus.Properties.Set",
                GLib.Variant(
                    "(ssv)",
                    (PP_NAME, "ActiveProfile", GLib.Variant("s", state["target"])),
                ),
                Gio.DBusCallFlags.NONE,
                5000,
                self.cancellable,
                self.finished,
                (state["target"], key),
            )
        return GLib.SOURCE_REMOVE

    def finished(self, remote, result, attempt):
        target, key = attempt
        failed = False
        try:
            remote.call_finish(result)
            self.failures = 0
            print(f"Power profile: {target}", flush=True)
        except GLib.Error as error:
            failed = True
            print(
                f"Could not set {target}: {error.message}", file=sys.stderr, flush=True
            )
        self.pending = False
        if failed and self.state_key(self.snapshot()) == key:
            self.failures += 1
            if self.failures < 3:
                self.retry_source = GLib.timeout_add(
                    (1000, 3000)[self.failures - 1], self.retry
                )
            return
        self.queue()

    def retry(self):
        self.retry_source = 0
        self.last_state = None
        return self.apply()

    def close(self):
        if self.source:
            GLib.source_remove(self.source)
            self.source = 0
        if self.retry_source:
            GLib.source_remove(self.retry_source)
            self.retry_source = 0
        self.cancellable.cancel()
        for remote, handler in self.handlers:
            remote.disconnect(handler)
        self.handlers.clear()


def lock_instance():
    runtime = Path(os.environ.get("XDG_RUNTIME_DIR", f"/run/user/{os.getuid()}"))
    metadata = runtime.stat(follow_symlinks=False)
    if (
        not stat.S_ISDIR(metadata.st_mode)
        or metadata.st_uid != os.getuid()
        or metadata.st_mode & 0o077
    ):
        raise OSError(
            "Power profile runtime directory must be a private directory owned by the user"
        )
    descriptor = os.open(
        runtime / "hype-auto-power-profile.lock",
        os.O_CREAT | os.O_RDWR | os.O_CLOEXEC | os.O_NOFOLLOW,
        0o600,
    )
    metadata = os.fstat(descriptor)
    if (
        not stat.S_ISREG(metadata.st_mode)
        or metadata.st_uid != os.getuid()
        or metadata.st_mode & 0o077
    ):
        os.close(descriptor)
        raise OSError(
            "Power profile lock must be a private regular file owned by the user"
        )
    try:
        fcntl.flock(descriptor, fcntl.LOCK_EX | fcntl.LOCK_NB)
    except BlockingIOError:
        os.close(descriptor)
        return None
    except OSError:
        os.close(descriptor)
        raise
    return descriptor


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument(
        "--check", action="store_true", help="print power policy without changing it"
    )
    args = parser.parse_args()
    descriptor = None if args.check else lock_instance()
    if not args.check and descriptor is None:
        return 0
    app = None
    try:
        app = AutoPowerProfile(
            proxy(UP_NAME, UP_PATH, UP_NAME),
            proxy(UP_NAME, f"{UP_PATH}/devices/DisplayDevice", f"{UP_NAME}.Device"),
            proxy(PP_NAME, PP_PATH, PP_NAME),
        )
        if args.check:
            print(json.dumps(app.snapshot()))
            return 0
        loop = GLib.MainLoop()

        def stop():
            loop.quit()
            return GLib.SOURCE_REMOVE

        for number in (signal.SIGINT, signal.SIGTERM):
            GLibUnix.signal_add(GLib.PRIORITY_DEFAULT, number, stop)
        app.queue()
        loop.run()
    finally:
        if app is not None:
            app.close()
        if descriptor is not None:
            os.close(descriptor)
    return 0


if __name__ == "__main__":
    try:
        sys.exit(main())
    except (GLib.Error, OSError) as error:
        print(str(error), file=sys.stderr)
        sys.exit(1)
