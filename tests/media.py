#!/usr/bin/env python3
"""Run native media control against a private MPRIS bus, without playerctl."""

import os
import subprocess
import tempfile
import threading
import unittest
from pathlib import Path

from gi.repository import Gio, GLib

REPO = Path(__file__).resolve().parents[1]
BINARY = Path(os.environ.get("HYPE_CONTROL_BINARY", REPO / "target/debug/hype-control"))
PLAYER = "org.mpris.MediaPlayer2.Player"
PATH = "/org/mpris/MediaPlayer2"


class Players:
    def __init__(self, root, players):
        self.players = players
        self.events = []
        self.ready = threading.Event()
        self.error = None
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
            raise RuntimeError("Private bus did not start")
        self.thread = threading.Thread(target=self.serve, daemon=True)
        self.thread.start()
        if not self.ready.wait(5):
            raise RuntimeError("MPRIS fixture did not start")
        if self.error:
            raise self.error

    def serve(self):
        context = GLib.MainContext.new()
        context.push_thread_default()
        self.connections = []
        try:
            info = Gio.DBusNodeInfo.new_for_xml(
                '<node><interface name="org.mpris.MediaPlayer2.Player">'
                '<method name="PlayPause"/><method name="Next"/><method name="Previous"/>'
                '<property name="PlaybackStatus" type="s" access="read"/>'
                + "".join(
                    f'<property name="{key}" type="b" access="read"/>'
                    for key in [
                        "CanControl",
                        "CanPlay",
                        "CanPause",
                        "CanGoNext",
                        "CanGoPrevious",
                    ]
                )
                + "</interface></node>"
            ).interfaces[0]
            for name, status, capable in self.players:
                conn = Gio.DBusConnection.new_for_address_sync(
                    self.address,
                    Gio.DBusConnectionFlags.AUTHENTICATION_CLIENT
                    | Gio.DBusConnectionFlags.MESSAGE_BUS_CONNECTION,
                    None,
                    None,
                )
                conn.call_sync(
                    "org.freedesktop.DBus",
                    "/org/freedesktop/DBus",
                    "org.freedesktop.DBus",
                    "RequestName",
                    GLib.Variant("(su)", (name, 0)),
                    None,
                    Gio.DBusCallFlags.NONE,
                    3000,
                    None,
                )

                def method(
                    _conn,
                    _sender,
                    _path,
                    interface,
                    action,
                    args,
                    invocation,
                    name=name,
                ):
                    if name.endswith(".gone"):
                        invocation.return_dbus_error(
                            "org.freedesktop.DBus.Error.NameHasNoOwner", "player quit"
                        )
                        return
                    self.events.append([name, interface, action, args.unpack()])
                    invocation.return_value(None)

                def prop(
                    _conn,
                    _sender,
                    _path,
                    _interface,
                    key,
                    status=status,
                    capable=capable,
                ):
                    return (
                        GLib.Variant("s", status)
                        if key == "PlaybackStatus"
                        else GLib.Variant("b", capable)
                    )

                conn.register_object(PATH, info, method, prop)
                self.connections.append(conn)
            self.loop = GLib.MainLoop.new(context, False)
            self.ready.set()
            self.loop.run()
            for conn in self.connections:
                conn.close_sync()
        except Exception as error:  # noqa: BLE001 - relay private fixture thread failures.
            self.error = error
            self.ready.set()
        finally:
            context.pop_thread_default()

    def close(self):
        if hasattr(self, "loop"):
            self.loop.quit()
        self.thread.join(3)
        self.daemon.terminate()
        self.daemon.communicate(timeout=3)


class MediaTests(unittest.TestCase):
    def fixture(self, players):
        temporary = tempfile.TemporaryDirectory(prefix="hype-media-test-")
        self.addCleanup(temporary.cleanup)
        service = Players(Path(temporary.name), players)
        self.addCleanup(service.close)
        return service

    def run_media(self, service, *args):
        return subprocess.run(
            [str(BINARY), "media", *args],
            env=os.environ
            | {"DBUS_SESSION_BUS_ADDRESS": service.address, "PATH": "/nonexistent"},
            text=True,
            capture_output=True,
            check=False,
            timeout=8,
        )

    def test_no_player_is_a_successful_noop_without_cli(self):
        service = self.fixture([])
        result = self.run_media(service, "play-pause")
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(service.events, [])

    def test_playing_player_wins_and_all_media_actions_use_mpris(self):
        service = self.fixture(
            [
                ("org.mpris.MediaPlayer2.alpha", "Paused", True),
                ("org.mpris.MediaPlayer2.zeta", "Playing", True),
            ]
        )
        for action, method in [
            ("play-pause", "PlayPause"),
            ("next", "Next"),
            ("previous", "Previous"),
        ]:
            result = self.run_media(service, action)
            self.assertEqual(result.returncode, 0, result.stderr)
            self.assertEqual(
                service.events[-1], ["org.mpris.MediaPlayer2.zeta", PLAYER, method, ()]
            )

    def test_player_disappearance_falls_back_to_another_capable_player(self):
        service = self.fixture(
            [
                ("org.mpris.MediaPlayer2.gone", "Playing", True),
                ("org.mpris.MediaPlayer2.remaining", "Paused", True),
            ]
        )
        result = self.run_media(service, "play-pause")
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(
            service.events,
            [["org.mpris.MediaPlayer2.remaining", PLAYER, "PlayPause", ()]],
        )

    def test_incapable_player_is_skipped_and_invalid_action_does_not_control_players(
        self,
    ):
        service = self.fixture(
            [
                ("org.mpris.MediaPlayer2.alpha", "Playing", False),
                ("org.mpris.MediaPlayer2.beta", "Paused", True),
            ]
        )
        result = self.run_media(service, "next")
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(
            service.events, [["org.mpris.MediaPlayer2.beta", PLAYER, "Next", ()]]
        )
        for args in [(), ("stop",), ("next", "extra")]:
            result = self.run_media(service, *args)
            self.assertNotEqual(result.returncode, 0)
        self.assertEqual(len(service.events), 1)


if __name__ == "__main__":
    unittest.main()
