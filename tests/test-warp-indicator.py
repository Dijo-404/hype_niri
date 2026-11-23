#!/usr/bin/env python3
"""Check WARP visibility against private process and system D-Bus fixtures."""

import json
import time
import unittest

from runtime_fixture import RuntimeFixture


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

    def test_stopped_daemon_hides_indicator_without_contacting_cli(self):
        self.set_unit("warp-svc.service", "inactive")
        self.assertEqual(self.indicator(), {"text": ""})
        self.assertFalse(self.log.exists())

    def test_connected_daemon_shows_connected_status(self):
        result = self.indicator()
        self.assertTrue(result["text"])
        self.assertEqual(result["class"], "connected")
        self.assertEqual(result["tooltip"], "Cloudflare WARP · Connected")

    def test_native_taskbar_avoids_duplicate_icon(self):
        self.mock(
            "warp-taskbar",
            'touch "$FIXTURE_ROOT/tray-ready"\nwhile :; do sleep 0.05; done',
        )
        result = self.isolated(
            "warp-taskbar >/dev/null 2>&1 &\n"
            'wait_for "$FIXTURE_ROOT/tray-ready"\n'
            "control warp-indicator"
        )
        self.assertEqual(json.loads(result.stdout), {"text": ""})
        self.assertFalse(self.log.exists())

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
