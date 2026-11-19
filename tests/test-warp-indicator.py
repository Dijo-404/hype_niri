#!/usr/bin/env python3
"""Check WARP visibility without changing the real network connection."""

import json
import os
import subprocess
import tempfile
import unittest
from pathlib import Path

SCRIPT = Path(__file__).resolve().parents[1] / "scripts/warp-indicator.sh"


class WarpIndicatorTests(unittest.TestCase):
    def setUp(self):
        temporary = tempfile.TemporaryDirectory(prefix="hype-niri-warp-test-")
        self.addCleanup(temporary.cleanup)
        self.root = Path(temporary.name)
        self.log = self.root / "cli-called"
        self.environment = os.environ | {
            "PATH": f"{self.root}:{os.environ['PATH']}",
            "WARP_TEST_LOG": str(self.log),
        }
        self.mock("systemctl", '[[ "$*" == "is-active --quiet warp-svc.service" ]]')
        self.mock("pgrep", "exit 1")
        self.mock(
            "warp-cli",
            'printf called > "$WARP_TEST_LOG"\nprintf \'%s\\n\' \'{"status":"Connected"}\'',
        )

    def mock(self, name, body):
        path = self.root / name
        path.write_text("#!/usr/bin/env bash\nset -eu\n" + body + "\n")
        path.chmod(0o755)

    def indicator(self):
        result = subprocess.run(
            ["bash", str(SCRIPT)],
            env=self.environment,
            capture_output=True,
            text=True,
            timeout=4,
            check=False,
        )
        self.assertEqual(result.returncode, 0, result.stderr)
        return json.loads(result.stdout)

    def test_stopped_daemon_hides_indicator_without_contacting_cli(self):
        self.mock("systemctl", "exit 3")
        self.assertEqual(self.indicator()["text"], "")
        self.assertFalse(self.log.exists())

    def test_connected_daemon_shows_connected_status(self):
        result = self.indicator()
        self.assertTrue(result["text"])
        self.assertEqual(result["class"], "connected")
        self.assertIn("Connected", result["tooltip"])

    def test_native_taskbar_avoids_duplicate_icon(self):
        self.mock("pgrep", '[[ "$*" == "-u $UID -x warp-taskbar" ]]')
        self.assertEqual(self.indicator()["text"], "")
        self.assertFalse(self.log.exists())

    def test_disconnected_daemon_stays_visible(self):
        self.mock("warp-cli", "printf '%s\\n' '{\"status\":\"Disconnected\"}'")
        result = self.indicator()
        self.assertTrue(result["text"])
        self.assertEqual(result["class"], "running")
        self.assertIn("Disconnected", result["tooltip"])

    def test_failed_or_invalid_status_stays_visible_without_claiming_connected(self):
        for body in ["exit 1", "printf 'not JSON'", "printf '{}'"]:
            with self.subTest(body=body):
                self.mock("warp-cli", body)
                result = self.indicator()
                self.assertTrue(result["text"])
                self.assertEqual(result["class"], "running")
                self.assertIn("Unknown", result["tooltip"])

    def test_unresponsive_cli_has_bounded_wait(self):
        self.mock("warp-cli", "exec sleep 10")
        result = self.indicator()
        self.assertTrue(result["text"])
        self.assertEqual(result["class"], "running")


if __name__ == "__main__":
    unittest.main()
