#!/usr/bin/env python3
"""Exercise hype-control against private typed D-Bus services and temporary sysfs.

Run with /usr/bin/python3 after cargo build -p hype-desktop. No desktop control
commands are mocked: bubblewrap gives each real client a private PID namespace,
network namespace, /proc, /run and /sys so Waybar signals cannot escape tests.
"""

import json
import os
import shutil
import signal
import subprocess
import tempfile
import threading
import time
import unittest
from pathlib import Path

try:
    from gi.repository import Gio, GLib
except ImportError:
    Gio = GLib = None

REPO = Path(__file__).resolve().parents[1]
BINARY = Path(
    os.environ.get("HYPE_CONTROL_BINARY", REPO / "target/debug/hype-control")
).resolve()
NM = "org.freedesktop.NetworkManager"
NM_ROOT = "/org/freedesktop/NetworkManager"
SETTINGS = NM_ROOT + "/Settings"
DEVICE = NM_ROOT + "/Devices/1"
CONNECTION = NM + ".Settings.Connection"
UP = "org.freedesktop.UPower"
UP_ROOT = "/org/freedesktop/UPower"
BATTERY = UP_ROOT + "/devices/DisplayDevice"
PP = UP + ".PowerProfiles"
PP_ROOT = UP_ROOT + "/PowerProfiles"
PROPERTIES = "org.freedesktop.DBus.Properties"


def interface(name, methods=(), properties=()):
    body = ""
    for method, inputs, outputs in methods:
        body += f'<method name="{method}">'
        body += "".join(f'<arg type="{t}" direction="in"/>' for t in inputs)
        body += "".join(f'<arg type="{t}" direction="out"/>' for t in outputs)
        body += "</method>"
    body += "".join(
        f'<property name="{n}" type="{t}" access="{a}"/>' for n, t, a in properties
    )
    return Gio.DBusNodeInfo.new_for_xml(
        f'<node><interface name="{name}">{body}</interface></node>'
    ).interfaces[0]


def settings_variant(value):
    """Keep NM's nested variant types intact, especially u32 channel and ay SSID."""
    result = {}
    for index in range(value.n_children()):
        section = value.get_child_value(index)
        name = section.get_child_value(0).get_string()
        result[name] = {}
        properties = section.get_child_value(1)
        for index in range(properties.n_children()):
            entry = properties.get_child_value(index)
            key = entry.get_child_value(0).get_string()
            result[name][key] = entry.get_child_value(1).get_variant()
    return result


class DesktopBus:
    def __init__(self, root):
        self.daemon = subprocess.Popen(
            [
                "dbus-daemon",
                "--session",
                "--nofork",
                "--nopidfile",
                "--print-address=1",
                f"--address=unix:path={root / 'bus'}",
            ],
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            text=True,
        )
        self.address = self.daemon.stdout.readline().strip()
        if not self.address:
            raise RuntimeError(self.daemon.stderr.read())
        self.ready = threading.Event()
        self.errors = []
        self.thread = threading.Thread(target=self._serve, daemon=True)
        self.thread.start()
        if not self.ready.wait(5):
            raise RuntimeError("Private D-Bus fixture did not start")
        if self.errors:
            raise self.errors[0]

    def _serve(self):
        self.context = GLib.MainContext.new()
        self.context.push_thread_default()
        try:
            self.connection = Gio.DBusConnection.new_for_address_sync(
                self.address,
                Gio.DBusConnectionFlags.AUTHENTICATION_CLIENT
                | Gio.DBusConnectionFlags.MESSAGE_BUS_CONNECTION,
                None,
                None,
            )
            self.profiles = {}
            self.active_connections = {}
            self.events = []
            self.fail_five = False
            self.deactivation_delay_ms = 0
            self.expected_device = DEVICE
            self.power_sets = []
            self.power_reads = 0
            self.props = {
                (NM_ROOT, NM): {"ActiveConnections": GLib.Variant("ao", [])},
                (DEVICE, NM + ".Device"): {
                    "DeviceType": GLib.Variant("u", 2),
                    "Interface": GLib.Variant("s", "wlan-fixture"),
                },
                (DEVICE, NM + ".Device.Wireless"): {
                    "WirelessCapabilities": GLib.Variant("u", 0x440)
                },
                (UP_ROOT, UP): {"OnBattery": GLib.Variant("b", True)},
                (BATTERY, UP + ".Device"): {
                    "IsPresent": GLib.Variant("b", True),
                    "Type": GLib.Variant("u", 2),
                    "Percentage": GLib.Variant("d", 70.0),
                },
                (PP_ROOT, PP): {
                    "ActiveProfile": GLib.Variant("s", "performance"),
                    "Profiles": GLib.Variant(
                        "aa{sv}",
                        [
                            {"Profile": GLib.Variant("s", p)}
                            for p in ["performance", "balanced", "power-saver"]
                        ],
                    ),
                },
            }
            self.registrations = []
            self.register(
                NM_ROOT,
                interface(
                    NM,
                    [
                        ("GetDevices", [], ["ao"]),
                        ("GetDeviceByIpIface", ["s"], ["o"]),
                        ("ActivateConnection", ["o", "o", "o"], ["o"]),
                        ("DeactivateConnection", ["o"], []),
                    ],
                    [("ActiveConnections", "ao", "read")],
                ),
            )
            self.register(
                SETTINGS,
                interface(
                    NM + ".Settings",
                    [
                        ("ListConnections", [], ["ao"]),
                        ("AddConnection", ["a{sa{sv}}"], ["o"]),
                    ],
                ),
            )
            for path, iface in [
                (DEVICE, NM + ".Device"),
                (DEVICE, NM + ".Device.Wireless"),
                (UP_ROOT, UP),
                (BATTERY, UP + ".Device"),
                (PP_ROOT, PP),
            ]:
                self.register(
                    path,
                    interface(
                        iface,
                        properties=[
                            (
                                key,
                                value.get_type_string(),
                                "readwrite" if key == "ActiveProfile" else "read",
                            )
                            for key, value in self.props[path, iface].items()
                        ],
                    ),
                )
            self.register(
                "/org/freedesktop/Notifications",
                interface(
                    "org.freedesktop.Notifications",
                    [("Notify", ["s", "u", "s", "s", "s", "as", "a{sv}", "i"], ["u"])],
                ),
            )
            for name in [NM, UP, PP, "org.freedesktop.Notifications"]:
                self.name("RequestName", name)
            self.loop = GLib.MainLoop.new(self.context, False)
            self.ready.set()
            self.loop.run()
            self.connection.close_sync(None)
        except Exception as error:  # noqa: BLE001 - relay fixture callback failures to unittest.
            self.errors.append(error)
            self.ready.set()
        finally:
            self.context.pop_thread_default()

    def invoke(self, callback):
        done = threading.Event()
        result = []
        errors = []

        def call(_data):
            try:
                result.append(callback())
            except Exception as error:  # noqa: BLE001 - relay fixture callback failures to unittest.
                errors.append(error)
            finally:
                done.set()
            return GLib.SOURCE_REMOVE

        self.context.invoke_full(GLib.PRIORITY_DEFAULT, call, None)
        if not done.wait(5):
            raise RuntimeError("Private D-Bus fixture stopped responding")
        if errors:
            raise errors[0]
        return result[0]

    def name(self, method, name):
        args = (
            GLib.Variant("(su)", (name, 0))
            if method == "RequestName"
            else GLib.Variant("(s)", (name,))
        )
        return self.connection.call_sync(
            "org.freedesktop.DBus",
            "/org/freedesktop/DBus",
            "org.freedesktop.DBus",
            method,
            args,
            None,
            Gio.DBusCallFlags.NONE,
            3000,
            None,
        )

    def register(self, path, info):
        self.registrations.append(
            self.connection.register_object(
                path, info, self.method, self.get_property, self.set_property
            )
        )

    def get_property(self, _connection, _sender, path, iface, key):
        if iface == PP and key == "Profiles":
            self.power_reads += 1
        return self.props[path, iface][key]

    def set_property(self, _connection, _sender, path, iface, key, value):
        self.power_sets.append(value.unpack())
        self.change(path, iface, key, value)
        return True

    def change(self, path, iface, key, value):
        self.props[path, iface][key] = value
        self.connection.emit_signal(
            None,
            path,
            PROPERTIES,
            "PropertiesChanged",
            GLib.Variant("(sa{sv}as)", (iface, {key: value}, [])),
        )
        self.connection.flush_sync(None)

    def add_profile(self, valid=True, password="saved-fixture-password", band="a"):
        path = f"{SETTINGS}/{len(self.profiles) + 1}"
        self.profiles[path] = {
            "connection": {
                "id": GLib.Variant("s", "Hype Hotspot"),
                "uuid": GLib.Variant("s", f"fixture-{len(self.profiles)}"),
                "interface-name": GLib.Variant("s", "wlan-fixture"),
                "type": GLib.Variant("s", "802-11-wireless"),
            },
            "802-11-wireless": {
                "mode": GLib.Variant("s", "ap" if valid else "infrastructure"),
                "ssid": GLib.Variant("ay", b"Hype-Niri"),
                "band": GLib.Variant("s", band),
                "channel": GLib.Variant("u", 36 if band == "a" else 1),
            },
            "802-11-wireless-security": {"psk": GLib.Variant("s", password)},
            "ipv4": {"method": GLib.Variant("s", "shared")},
        }
        self.register_profile(path)
        return path

    def register_profile(self, path):
        self.register(
            path,
            interface(
                CONNECTION,
                [
                    ("GetSettings", [], ["a{sa{sv}}"]),
                    ("GetSecrets", ["s"], ["a{sa{sv}}"]),
                    ("Update", ["a{sa{sv}}"], []),
                ],
            ),
        )

    def method(self, _connection, _sender, path, iface, method, args, invocation):
        try:
            if method == "Notify":
                invocation.return_value(GLib.Variant("(u)", (1,)))
            elif method == "ListConnections":
                invocation.return_value(GLib.Variant("(ao)", (list(self.profiles),)))
            elif method == "GetDevices":
                devices = [
                    p for p, interface in self.props if interface == NM + ".Device"
                ]
                invocation.return_value(GLib.Variant("(ao)", (devices,)))
            elif method == "GetDeviceByIpIface":
                (name,) = args.unpack()
                device = next(
                    path
                    for (path, interface), props in self.props.items()
                    if interface == NM + ".Device"
                    and props["Interface"].unpack() == name
                )
                invocation.return_value(GLib.Variant("(o)", (device,)))
            elif method == "GetSettings":
                settings = {
                    section: dict(values)
                    for section, values in self.profiles[path].items()
                }
                settings.get("802-11-wireless-security", {}).pop("psk", None)
                invocation.return_value(GLib.Variant("(a{sa{sv}})", (settings,)))
            elif method == "GetSecrets":
                invocation.return_value(
                    GLib.Variant(
                        "(a{sa{sv}})",
                        (
                            {
                                "802-11-wireless-security": self.profiles[path].get(
                                    "802-11-wireless-security", {}
                                )
                            },
                        ),
                    )
                )
            elif method == "AddConnection":
                new_path = f"{SETTINGS}/{len(self.profiles) + 1}"
                self.profiles[new_path] = settings_variant(args.get_child_value(0))
                self.profiles[new_path]["connection"]["uuid"] = GLib.Variant(
                    "s", "created-fixture-uuid"
                )
                self.register_profile(new_path)
                self.events.append(("add", new_path))
                invocation.return_value(GLib.Variant("(o)", (new_path,)))
            elif method == "Update":
                settings = settings_variant(args.get_child_value(0))
                # NM merges saved system secrets omitted by Update (GetSettings excludes them).
                secret = (
                    self.profiles[path].get("802-11-wireless-security", {}).get("psk")
                )
                if secret is not None:
                    settings.setdefault("802-11-wireless-security", {}).setdefault(
                        "psk", secret
                    )
                self.profiles[path] = settings
                self.events.append(
                    ("update", path, settings["802-11-wireless"]["band"].unpack())
                )
                invocation.return_value(None)
            elif method == "ActivateConnection":
                profile, device, specific = args.unpack()
                assert device == self.expected_device and specific == "/", (
                    "Activation selected another Wi-Fi adapter"
                )
                assert not any(
                    self.props[p, NM + ".Connection.Active"]["State"].unpack() == 3
                    for p in self.active_connections
                ), "Activation raced an unfinished deactivation"
                settings = self.profiles[profile]
                band = settings["802-11-wireless"]["band"].unpack()
                active = f"{NM_ROOT}/ActiveConnection/{len(self.events) + 1}"
                self.props[active, NM + ".Connection.Active"] = {
                    "Connection": GLib.Variant("o", profile),
                    "State": GLib.Variant(
                        "u", 4 if self.fail_five and band == "a" else 2
                    ),
                }
                self.register(
                    active,
                    interface(
                        NM + ".Connection.Active",
                        properties=[
                            ("Connection", "o", "read"),
                            ("State", "u", "read"),
                        ],
                    ),
                )
                self.active_connections[active] = profile
                self.props[NM_ROOT, NM]["ActiveConnections"] = GLib.Variant(
                    "ao", list(self.active_connections)
                )
                self.events.append(("activate", profile, band))
                invocation.return_value(GLib.Variant("(o)", (active,)))
            elif method == "DeactivateConnection":
                (active,) = args.unpack()
                self.events.append(("deactivate", active))
                self.props[active, NM + ".Connection.Active"]["State"] = GLib.Variant(
                    "u", 3
                )

                def complete(_data=None):
                    self.active_connections.pop(active)
                    self.props[NM_ROOT, NM]["ActiveConnections"] = GLib.Variant(
                        "ao", list(self.active_connections)
                    )
                    self.props[active, NM + ".Connection.Active"]["State"] = (
                        GLib.Variant("u", 4)
                    )
                    return GLib.SOURCE_REMOVE

                if self.deactivation_delay_ms:
                    source = GLib.timeout_source_new(self.deactivation_delay_ms)
                    source.set_callback(complete)
                    source.attach(self.context)
                else:
                    complete()
                invocation.return_value(None)
            else:
                raise AssertionError(f"Unexpected fixture method {iface}.{method}")
        except Exception as error:  # noqa: BLE001 - relay fixture callback failures to unittest.
            self.errors.append(error)
            invocation.return_dbus_error("org.example.Fixture.Error", str(error))

    def snapshot(self):
        return self.invoke(
            lambda: {
                "events": list(self.events),
                "sets": list(self.power_sets),
                "reads": self.power_reads,
                "profiles": {
                    p: {
                        s: {k: v.unpack() for k, v in values.items()}
                        for s, values in sections.items()
                    }
                    for p, sections in self.profiles.items()
                },
            }
        )

    def close(self):
        if hasattr(self, "loop"):
            self.invoke(self.loop.quit)
        self.thread.join(timeout=3)
        self.daemon.terminate()
        self.daemon.communicate(timeout=3)


@unittest.skipUnless(
    Gio and shutil.which("dbus-daemon") and shutil.which("bwrap") and BINARY.is_file(),
    "Build hype-control and install python-gobject, dbus-daemon and bubblewrap",
)
class NativeRuntimeTests(unittest.TestCase):
    def setUp(self):
        temporary = tempfile.TemporaryDirectory(prefix="hype-native-runtime-")
        self.addCleanup(temporary.cleanup)
        self.root = Path(temporary.name)
        self.runtime = self.root / "runtime"
        self.runtime.mkdir(mode=0o700)
        self.home = self.root / "home"
        self.home.mkdir()
        self.bus = DesktopBus(self.root)
        self.addCleanup(self.bus.close)
        self.environment = os.environ | {
            "HOME": str(self.home),
            "XDG_RUNTIME_DIR": str(self.runtime),
            "XDG_CONFIG_HOME": str(self.home / ".config"),
            "XDG_STATE_HOME": str(self.home / ".local/state"),
            "DBUS_SYSTEM_BUS_ADDRESS": self.bus.address,
            "DBUS_SESSION_BUS_ADDRESS": self.bus.address,
            "HWMON_ROOT": str(self.root / "hwmon"),
            "THERMAL_ROOT": str(self.root / "thermal"),
            "HYPE_NIRI_TEMP_WARNING": "70",
            "HYPE_NIRI_TEMP_CRITICAL": "80",
            "DISPLAY": "",
            "WAYLAND_DISPLAY": "",
        }
        self.launch = [
            "bwrap",
            "--die-with-parent",
            "--unshare-pid",
            "--unshare-net",
            "--ro-bind",
            "/",
            "/",
            "--bind",
            str(self.root),
            str(self.root),
            "--proc",
            "/proc",
            "--dev",
            "/dev",
            "--tmpfs",
            "/run",
            "--tmpfs",
            "/sys",
            "--",
            str(BINARY),
        ]

    def tearDown(self):
        self.assertEqual(
            self.bus.errors,
            [],
            "The typed D-Bus fixture encountered an unexpected request",
        )

    def run_control(self, *args, success=True):
        result = subprocess.run(
            self.launch + list(args),
            env=self.environment,
            capture_output=True,
            text=True,
            timeout=8,
            check=False,
        )
        self.assertEqual(result.returncode == 0, success, result.stdout + result.stderr)
        return result

    def start_policy(self):
        process = subprocess.Popen(
            self.launch + ["auto-power-profile"],
            env=self.environment,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            text=True,
            start_new_session=True,
        )
        self.addCleanup(self.stop_policy, process)
        return process

    @staticmethod
    def stop_policy(process):
        if process.poll() is None:
            os.killpg(process.pid, signal.SIGTERM)
        try:
            process.communicate(timeout=3)
        except subprocess.TimeoutExpired:
            os.killpg(process.pid, signal.SIGKILL)
            process.communicate(timeout=3)

    def wait_for(self, predicate):
        deadline = time.monotonic() + 5
        while time.monotonic() < deadline:
            snapshot = self.bus.snapshot()
            if predicate(snapshot):
                return snapshot
            time.sleep(0.02)
        self.fail(
            f"Native client did not reach expected D-Bus state: {self.bus.snapshot()}"
        )

    def change(self, path, iface, key, kind, value):
        before = self.bus.snapshot()["reads"]
        self.bus.invoke(
            lambda: self.bus.change(path, iface, key, GLib.Variant(kind, value))
        )
        self.wait_for(lambda state: state["reads"] > before)
        time.sleep(0.15)

    def test_hotspot_status_and_name_collisions_are_read_only(self):
        self.assertEqual(
            json.loads(self.run_control("hotspot", "status").stdout)["class"],
            "inactive",
        )
        self.bus.invoke(lambda: self.bus.add_profile(valid=False))
        result = self.run_control("hotspot", "start", success=False)
        self.assertIn("another connection", result.stderr)
        self.assertEqual(self.bus.snapshot()["events"], [])
        self.assertEqual(
            json.loads(self.run_control("hotspot", "status").stdout)["class"],
            "unavailable",
        )

        def duplicate():
            first = next(iter(self.bus.profiles.values()))
            first["802-11-wireless"]["mode"] = GLib.Variant("s", "ap")
            self.bus.add_profile()

        self.bus.invoke(duplicate)
        self.assertIn(
            "More than one", self.run_control("hotspot", "start", success=False).stderr
        )
        self.assertEqual(self.bus.snapshot()["events"], [])

    def test_hotspot_fallback_and_saved_password_survive_restart(self):
        profile = self.bus.invoke(self.bus.add_profile)
        self.bus.invoke(lambda: setattr(self.bus, "fail_five", True))
        self.bus.invoke(lambda: setattr(self.bus, "deactivation_delay_ms", 75))
        self.run_control("hotspot", "start")
        state = self.bus.snapshot()
        self.assertEqual(
            [event[2] for event in state["events"] if event[0] == "activate"],
            ["a", "bg"],
        )
        self.assertEqual(sum(event[0] == "deactivate" for event in state["events"]), 1)
        self.assertEqual(
            state["profiles"][profile]["802-11-wireless-security"]["psk"],
            "saved-fixture-password",
        )
        self.assertEqual(
            state["profiles"][profile]["802-11-wireless-security"]["proto"], ["rsn"]
        )
        self.assertEqual(
            json.loads(self.run_control("hotspot", "status").stdout)["class"], "active"
        )
        self.run_control("hotspot", "stop")
        self.run_control("hotspot", "start")
        state = self.bus.snapshot()
        self.assertFalse(any(event[0] == "add" for event in state["events"]))
        self.assertEqual(
            [event[2] for event in state["events"] if event[0] == "activate"],
            ["a", "bg", "bg"],
        )
        self.assertEqual(
            state["profiles"][profile]["802-11-wireless-security"]["psk"],
            "saved-fixture-password",
        )

    def test_hotspot_reuses_saved_adapter_instead_of_first_wifi_device(self):
        def second_adapter():
            self.bus.add_profile()
            self.bus.props[DEVICE, NM + ".Device"]["Interface"] = GLib.Variant(
                "s", "wlan-other"
            )
            device = NM_ROOT + "/Devices/2"
            self.bus.expected_device = device
            for iface, values in [
                (
                    NM + ".Device",
                    {
                        "DeviceType": GLib.Variant("u", 2),
                        "Interface": GLib.Variant("s", "wlan-fixture"),
                    },
                ),
                (
                    NM + ".Device.Wireless",
                    {"WirelessCapabilities": GLib.Variant("u", 0x40)},
                ),
            ]:
                self.bus.props[device, iface] = values
                self.bus.register(
                    device,
                    interface(
                        iface,
                        properties=[
                            (key, value.get_type_string(), "read")
                            for key, value in values.items()
                        ],
                    ),
                )

        self.bus.invoke(second_adapter)
        self.run_control("hotspot", "start")
        self.assertEqual(
            [
                event[2]
                for event in self.bus.snapshot()["events"]
                if event[0] == "activate"
            ],
            ["bg"],
        )

    def test_hotspot_creation_saves_one_secure_profile(self):
        self.run_control("hotspot", "start")
        self.run_control("hotspot", "start")
        state = self.bus.snapshot()
        self.assertEqual(sum(event[0] == "add" for event in state["events"]), 1)
        self.assertEqual(sum(event[0] == "activate" for event in state["events"]), 1)
        (profile,) = state["profiles"].values()
        self.assertEqual(profile["ipv4"]["method"], "shared")
        self.assertEqual(profile["ipv6"]["method"], "disabled")
        self.assertEqual(len(profile["802-11-wireless-security"]["psk"]), 24)
        self.assertEqual(profile["802-11-wireless-security"]["pairwise"], ["ccmp"])

    def test_power_check_reports_boundaries_without_writing(self):
        for battery, percentage, policy in [
            (True, 29.999, "power-saver"),
            (True, 30.0, "balanced"),
            (False, 10.0, "performance"),
            (True, -1.0, ""),
            (True, float("nan"), ""),
        ]:
            with self.subTest(battery=battery, percentage=percentage):
                self.bus.invoke(
                    lambda battery=battery: self.bus.props[UP_ROOT, UP].update(
                        OnBattery=GLib.Variant("b", battery)
                    )
                )
                self.bus.invoke(
                    lambda percentage=percentage: self.bus.props[
                        BATTERY, UP + ".Device"
                    ].update(Percentage=GLib.Variant("d", percentage))
                )
                output = self.run_control("auto-power-profile", "--check").stdout
                fields = dict(line.split("=", 1) for line in output.splitlines())
                self.assertEqual(fields["policy"], policy)
                self.assertEqual(fields["target"], policy)
                self.assertEqual(
                    fields["available"], "power-saver balanced performance"
                )

        def limited_profiles():
            self.bus.props[UP_ROOT, UP]["OnBattery"] = GLib.Variant("b", False)
            self.bus.props[PP_ROOT, PP]["Profiles"] = GLib.Variant(
                "aa{sv}", [{"Profile": GLib.Variant("s", "balanced")}]
            )

        self.bus.invoke(limited_profiles)
        fields = dict(
            line.split("=", 1)
            for line in self.run_control(
                "auto-power-profile", "--check"
            ).stdout.splitlines()
        )
        self.assertEqual(
            (fields["policy"], fields["target"]), ("performance", "balanced")
        )
        self.assertEqual(self.bus.snapshot()["sets"], [])

    def test_power_events_preserve_manual_choice_until_transition_and_restart(self):
        process = self.start_policy()
        self.wait_for(lambda state: state["sets"] == ["balanced"])
        self.change(PP_ROOT, PP, "ActiveProfile", "s", "performance")
        self.change(BATTERY, UP + ".Device", "Percentage", "d", 50.0)
        self.assertEqual(self.bus.snapshot()["sets"], ["balanced"])
        self.change(BATTERY, UP + ".Device", "Percentage", "d", 29.0)
        self.wait_for(lambda state: state["sets"] == ["balanced", "power-saver"])
        self.change(UP_ROOT, UP, "OnBattery", "b", False)
        self.wait_for(
            lambda state: state["sets"] == ["balanced", "power-saver", "performance"]
        )

        def restart():
            self.bus.name("ReleaseName", PP)
            self.bus.props[PP_ROOT, PP]["ActiveProfile"] = GLib.Variant("s", "balanced")
            self.bus.name("RequestName", PP)

        self.bus.invoke(restart)
        self.wait_for(
            lambda state: (
                state["sets"]
                == ["balanced", "power-saver", "performance", "performance"]
            )
        )
        self.assertIsNone(process.poll())

    def test_temperature_selects_cpu_and_recovers_invalid_cached_sensor(self):
        self.assertEqual(
            json.loads(self.run_control("temperature").stdout)["class"], "missing"
        )

        def sensor(index, name, label, raw):
            directory = self.root / f"hwmon/hwmon{index}"
            directory.mkdir(parents=True)
            for key, value in [
                ("name", name),
                ("temp1_label", label),
                ("temp1_input", raw),
            ]:
                (directory / key).write_text(value + "\n")
            return directory / "temp1_input"

        sensor(0, "amdgpu", "edge", "100000")
        cpu = sensor(1, "coretemp", 'Package "CPU"', "74500")
        reading = json.loads(self.run_control("temperature").stdout)
        self.assertEqual((reading["text"], reading["class"]), ("75°", "warning"))
        self.assertIn('Package "CPU"', reading["tooltip"])
        sensor(2, "coretemp", "Core 0", "85000")
        cpu.write_text("invalid\n")
        reading = json.loads(self.run_control("temperature").stdout)
        self.assertEqual((reading["text"], reading["class"]), ("85°", "critical"))
        cache = self.runtime / "hype-niri-temp-sensor"
        self.assertEqual(
            Path(json.loads(cache.read_text())["path"]).name, "temp1_input"
        )


if __name__ == "__main__":
    unittest.main(verbosity=2)
