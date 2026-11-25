#!/usr/bin/env python3
"""Check WARP visibility against private process and system D-Bus fixtures."""

import json
import threading
import time
import unittest

from runtime_fixture import Gio, GLib, RuntimeFixture

WATCHER = "org.kde.StatusNotifierWatcher"
ITEM = "org.kde.StatusNotifierItem"
ITEM_PATH = "/org/ayatana/NotificationItem/Generated"


class WarpIndicatorTests(RuntimeFixture):
    def setUp(self):
        super().setUp()
        self.set_unit("warp-svc.service", "active")
        self.mock(
            "warp-cli",
            '[[ "$*" == "--accept-tos --json status" ]]\n'
            'printf called > "$FIXTURE_LOG"\n'
            "printf '%s\n' '{\"status\":\"Connected\"}'",
        )

    def indicator(self):
        return json.loads(self.control("warp-indicator").stdout)

    def register_tray(self, status, *, host=True, title="warp-taskbar"):
        ready = threading.Event()

        def register():
            try:
                conn = self.bus.conn
                result = conn.call_sync(
                    "org.freedesktop.DBus",
                    "/org/freedesktop/DBus",
                    "org.freedesktop.DBus",
                    "RequestName",
                    GLib.Variant("(su)", (WATCHER, 0)),
                    None,
                    Gio.DBusCallFlags.NONE,
                    2000,
                    None,
                )
                assert result.unpack() == (1,), result
                watcher_properties = {
                    "IsStatusNotifierHostRegistered": GLib.Variant("b", host),
                    "RegisteredStatusNotifierItems": GLib.Variant(
                        "as", ["invalid tray item", conn.get_unique_name() + ITEM_PATH]
                    ),
                }
                item_properties = {
                    "Id": GLib.Variant("s", "Generated"),
                    "Category": GLib.Variant("s", "ApplicationStatus"),
                    "Status": GLib.Variant("s", status),
                    "Title": GLib.Variant("s", title),
                }
                for name, path, properties in (
                    (WATCHER, "/StatusNotifierWatcher", watcher_properties),
                    (ITEM, ITEM_PATH, item_properties),
                ):
                    fields = "".join(
                        f'<property name="{key}" type="{value.get_type_string()}" access="read"/>'
                        for key, value in properties.items()
                    )
                    info = Gio.DBusNodeInfo.new_for_xml(
                        f'<node><interface name="{name}">{fields}</interface></node>'
                    )
                    conn.register_object(
                        path,
                        info.interfaces[0],
                        None,
                        lambda *args, values=properties: values[args[-1]],
                    )
            except Exception as cause:  # noqa: BLE001 - report callback errors in the test thread.
                self.bus.error = cause
            finally:
                ready.set()
            return GLib.SOURCE_REMOVE

        self.bus.loop.get_context().invoke_full(GLib.PRIORITY_DEFAULT, register)
        self.assertTrue(ready.wait(3), "Tray fixture failed to register")
        self.assertIsNone(self.bus.error)

    def test_stopped_daemon_hides_indicator_without_contacting_cli(self):
        self.set_unit("warp-svc.service", "inactive")
        self.assertEqual(self.indicator(), {"text": ""})
        self.assertFalse(self.log.exists())

    def test_connected_daemon_shows_connected_status(self):
        result = self.indicator()
        self.assertTrue(result["text"])
        self.assertEqual(result["class"], "connected")
        self.assertEqual(result["tooltip"], "Cloudflare WARP · Connected")

    def test_running_taskbar_without_registered_icon_keeps_fallback(self):
        self.mock(
            "warp-taskbar",
            'touch "$FIXTURE_ROOT/tray-ready"\nwhile :; do sleep 0.05; done',
        )
        result = self.isolated(
            "warp-taskbar >/dev/null 2>&1 &\n"
            'wait_for "$FIXTURE_ROOT/tray-ready"\n'
            "control warp-indicator"
        )
        self.assertEqual(json.loads(result.stdout)["class"], "connected")
        self.assertTrue(self.log.exists())

    def test_registered_visible_taskbar_hides_fallback(self):
        self.register_tray("Active")
        self.assertEqual(self.indicator(), {"text": ""})
        self.assertFalse(self.log.exists())

    def test_passive_taskbar_keeps_fallback(self):
        self.register_tray("Passive")
        self.assertEqual(self.indicator()["class"], "connected")

    def test_watcher_without_host_keeps_fallback(self):
        self.register_tray("Active", host=False)
        self.assertEqual(self.indicator()["class"], "connected")

    def test_unrelated_tray_item_keeps_fallback(self):
        self.register_tray("Active", title="Other tray app")
        self.assertEqual(self.indicator()["class"], "connected")

    def test_disconnected_daemon_stays_visible(self):
        self.mock("warp-cli", "printf '%s\n' '{\"status\":\"Disconnected\"}'")
        result = self.indicator()
        self.assertTrue(result["text"])
        self.assertEqual(result["class"], "running")
        self.assertEqual(result["tooltip"], "Cloudflare WARP · Disconnected")

    def test_status_escapes_json_characters(self):
        self.mock(
            "warp-cli", r"""printf '%s\n' '{"status":"Partial \"DNS\"\\line\nbreak"}'"""
        )
        self.assertEqual(
            self.indicator()["tooltip"], 'Cloudflare WARP · Partial "DNS"\\line\nbreak'
        )

    def test_failed_or_invalid_status_stays_visible_without_claiming_connected(self):
        for body in [
            "exit 1",
            "printf 'not JSON'",
            "printf '{}'",
            'printf \'{"status":""}\'',
        ]:
            with self.subTest(body=body):
                self.mock("warp-cli", body)
                result = self.indicator()
                self.assertTrue(result["text"])
                self.assertEqual(result["class"], "running")
                self.assertEqual(result["tooltip"], "Cloudflare WARP · Unknown")

    def test_unresponsive_cli_has_bounded_wait(self):
        self.mock("warp-cli", 'trap "" TERM\nsleep 10')
        began = time.monotonic()
        result = self.indicator()
        self.assertLess(time.monotonic() - began, 3.5)
        self.assertTrue(result["text"])
        self.assertEqual(result["class"], "running")
        self.assertEqual(result["tooltip"], "Cloudflare WARP · Unknown")


if __name__ == "__main__":
    unittest.main()
