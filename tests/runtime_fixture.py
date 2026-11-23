"""Isolate native desktop launcher tests from the user's processes and services."""

import json
import os
import shutil
import socketserver
import subprocess
import tempfile
import threading
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
SYSTEMD = "org.freedesktop.systemd1"
LOGIN = "org.freedesktop.login1"
SYSTEMD_PATH = "/org/freedesktop/systemd1"
LOGIN_PATH = "/org/freedesktop/login1"
SESSION_PATH = LOGIN_PATH + "/session/test"
UNIT_NAMES = {
    "warp-svc.service": SYSTEMD_PATH + "/unit/warp_2dsvc_2eservice",
    "tor.service": SYSTEMD_PATH + "/unit/tor_2eservice",
}


class NiriFixture(socketserver.StreamRequestHandler):
    def handle(self):
        request = self.rfile.readline(65536)
        with self.server.log.open("ab") as log:
            log.write(request)
        self.wfile.write(b'{"Ok":"Handled"}\n')


class PrivateBus:
    def __init__(self, root, add_cleanup):
        self.root = root
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
        add_cleanup(self.stop_daemon)
        self.address = self.daemon.stdout.readline().strip()
        if not self.address:
            raise RuntimeError(self.daemon.stderr.read())
        self.ready = threading.Event()
        self.error = None
        self.thread = threading.Thread(target=self.serve, daemon=True)
        self.thread.start()
        add_cleanup(self.close)
        if not self.ready.wait(5):
            raise RuntimeError("Private D-Bus fixture did not start")
        if self.error:
            raise self.error

    def serve(self):
        context = GLib.MainContext.new()
        context.push_thread_default()
        try:
            conn = Gio.DBusConnection.new_for_address_sync(
                self.address,
                Gio.DBusConnectionFlags.AUTHENTICATION_CLIENT
                | Gio.DBusConnectionFlags.MESSAGE_BUS_CONNECTION,
                None,
                None,
            )
            for name in (SYSTEMD, LOGIN):
                response = conn.call_sync(
                    "org.freedesktop.DBus",
                    "/org/freedesktop/DBus",
                    "org.freedesktop.DBus",
                    "RequestName",
                    GLib.Variant("(su)", (name, 0)),
                    None,
                    Gio.DBusCallFlags.NONE,
                    2000,
                    None,
                )
                assert response.unpack() == (1,), response
            self.register(
                conn,
                SYSTEMD_PATH,
                SYSTEMD + ".Manager",
                """
                <method name="GetUnit">
                    <arg type="s" direction="in"/>
                    <arg type="o" direction="out"/>
                </method>
                """,
            )
            for path in UNIT_NAMES.values():
                self.register(
                    conn,
                    path,
                    SYSTEMD + ".Unit",
                    '<property name="ActiveState" type="s" access="read"/>',
                )
            self.register(
                conn,
                LOGIN_PATH,
                LOGIN + ".Manager",
                """
                <method name="GetSession">
                    <arg type="s" direction="in"/>
                    <arg type="o" direction="out"/>
                </method>
                <method name="Suspend"><arg type="b" direction="in"/></method>
                <method name="Reboot"><arg type="b" direction="in"/></method>
                <method name="PowerOff"><arg type="b" direction="in"/></method>
                """,
            )
            self.register(
                conn,
                SESSION_PATH,
                LOGIN + ".Session",
                '<property name="LockedHint" type="b" access="read"/>',
            )
            self.loop = GLib.MainLoop.new(context, False)
            self.ready.set()
            self.loop.run()
            conn.close_sync()
        except Exception as error:  # noqa: BLE001 - relay fixture thread failures to unittest.
            self.error = error
            self.ready.set()
        finally:
            context.pop_thread_default()

    def register(self, conn, path, name, contents):
        info = Gio.DBusNodeInfo.new_for_xml(
            f'<node><interface name="{name}">{contents}</interface></node>'
        )
        conn.register_object(path, info.interfaces[0], self.method, self.property)

    def method(self, _conn, _sender, _path, interface, method, args, invocation):
        try:
            if interface == SYSTEMD + ".Manager" and method == "GetUnit":
                (name,) = args.unpack()
                path = UNIT_NAMES.get(name)
                if path is None or not (self.root / name).exists():
                    invocation.return_dbus_error(
                        SYSTEMD + ".NoSuchUnit", f"{name} is not loaded"
                    )
                    return
                invocation.return_value(GLib.Variant("(o)", (path,)))
            elif method == "GetSession":
                assert args.unpack() == ("test-session",), args
                invocation.return_value(GLib.Variant("(o)", (SESSION_PATH,)))
            else:
                assert method in ("Suspend", "Reboot", "PowerOff"), method
                assert not (self.root / "runtime/caffeine_state").exists()
                if method == "Suspend":
                    assert (self.root / "locked").exists()
                with (self.root / "login-log").open("a") as log:
                    log.write(json.dumps([method, args.unpack()]) + "\n")
                invocation.return_value(None)
        except Exception as error:  # noqa: BLE001 - relay D-Bus callback failures to unittest.
            self.error = error
            invocation.return_dbus_error("org.example.Fixture.Error", str(error))

    def property(self, _conn, _sender, path, interface, name):
        if (interface, name) == (LOGIN + ".Session", "LockedHint"):
            return GLib.Variant("b", (self.root / "locked").exists())
        assert (interface, name) == (SYSTEMD + ".Unit", "ActiveState")
        unit = next(unit for unit, key in UNIT_NAMES.items() if key == path)
        return GLib.Variant("s", (self.root / unit).read_text())

    def close(self):
        if hasattr(self, "loop"):
            self.loop.quit()
        self.thread.join(timeout=3)

    def stop_daemon(self):
        self.daemon.terminate()
        self.daemon.communicate(timeout=3)


@unittest.skipUnless(
    Gio and shutil.which("bwrap") and shutil.which("dbus-daemon"),
    "Install python-gobject, bubblewrap and dbus-daemon",
)
class RuntimeFixture(unittest.TestCase):
    repo = REPO
    binary = BINARY

    def setUp(self):
        self.assertTrue(
            self.binary.is_file(), f"Build hype-control first: {self.binary}"
        )
        temporary = tempfile.TemporaryDirectory(prefix="hype-launcher-test-")
        self.addCleanup(temporary.cleanup)
        self.root = Path(temporary.name)
        self.runtime = self.root / "runtime"
        self.runtime.mkdir(mode=0o700)
        self.home = self.root / "home"
        self.home.mkdir()
        shutil.copytree(self.repo / "wofi", self.home / ".config/wofi")
        self.bin = self.root / "bin"
        self.bin.mkdir()
        for utility in ("cat", "cp", "printf", "rm", "sh", "sleep", "touch", "wc"):
            (self.bin / utility).symlink_to(shutil.which(utility))
        self.log = self.root / "application-log"
        self.bus = PrivateBus(self.root, self.addCleanup)
        niri_socket = self.root / "niri"
        niri = socketserver.UnixStreamServer(str(niri_socket), NiriFixture)
        niri.log = self.root / "niri-log"
        self.addCleanup(niri.server_close)
        niri_thread = threading.Thread(target=niri.serve_forever, daemon=True)
        niri_thread.start()
        self.addCleanup(niri_thread.join, 3)
        self.addCleanup(niri.shutdown)

        self.environment = os.environ | {
            "HOME": str(self.home),
            "PATH": str(self.bin),
            "XDG_CONFIG_HOME": str(self.home / ".config"),
            "XDG_STATE_HOME": str(self.home / ".local/state"),
            "XDG_CACHE_HOME": str(self.home / ".cache"),
            "XDG_RUNTIME_DIR": str(self.runtime),
            "XDG_SESSION_ID": "test-session",
            "DBUS_SYSTEM_BUS_ADDRESS": self.bus.address,
            "DBUS_SESSION_BUS_ADDRESS": self.bus.address,
            "NIRI_SOCKET": str(niri_socket),
            "DISPLAY": "",
            "WAYLAND_DISPLAY": "",
            "FIXTURE_ROOT": str(self.root),
            "TMPDIR": str(self.root),
            "FIXTURE_LOG": str(self.log),
            "HYPE_CONTROL_FIXTURE": str(self.binary),
        }
        self.environment.pop("GTK_THEME", None)
        self.environment.pop("WAYBAR_CONFIG", None)
        self.launch = [
            shutil.which("bwrap"),
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
            "--ro-bind-try",
            str(self.root / "stealth"),
            "/run/stealth",
            "--ro-bind",
            str(self.bin),
            "/usr/local/bin",
            "--",
            "/usr/bin/bash",
            "-euc",
        ]

    def tearDown(self):
        if hasattr(self, "bus"):
            self.assertIsNone(self.bus.error)

    def mock(self, name, body):
        path = self.bin / name
        path.write_text("#!/usr/bin/bash\nset -eu\n" + body + "\n")
        path.chmod(0o755)

    def set_unit(self, name, state):
        (self.root / name).write_text(state)

    def isolated(self, script, *args, extra_env=None, input_text=None, status=0):
        setup = """
control() { "$HYPE_CONTROL_FIXTURE" "$@"; }
wait_for() {
    for ((attempt=0; attempt<300; attempt++)); do
        [[ ! -e "$1" ]] || return 0
        sleep 0.01
    done
    printf 'Fixture did not become ready: %s\n' "$1" >&2
    return 1
}
"""
        result = subprocess.run(
            self.launch + [setup + script, "fixture", *args],
            env=self.environment | (extra_env or {}),
            input=input_text,
            capture_output=True,
            text=True,
            timeout=12,
            check=False,
        )
        self.assertEqual(result.returncode, status, result.stdout + result.stderr)
        return result

    def control(self, *args, **kwargs):
        return self.isolated('control "$@"', *args, **kwargs)

    @staticmethod
    def take_lines(path):
        if not path.exists():
            return []
        lines = path.read_text().splitlines()
        path.unlink()
        return lines
