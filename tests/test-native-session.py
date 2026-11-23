#!/usr/bin/env python3
"""Check native monitor commands against an isolated compositor socket."""

import json
import unittest

from runtime_fixture import RuntimeFixture


class MonitorTests(RuntimeFixture):
    def test_monitor_power_actions_use_native_ipc(self):
        for state, action in [("on", "PowerOnMonitors"), ("off", "PowerOffMonitors")]:
            with self.subTest(state=state):
                self.control("monitors", state)
                requests = [
                    json.loads(line) for line in self.take_lines(self.root / "niri-log")
                ]
                self.assertEqual(requests, [{"Action": {action: {}}}])

    def test_invalid_monitor_actions_do_not_change_outputs(self):
        for arguments in [(), ("toggle",), ("on", "extra")]:
            self.control("monitors", *arguments, status=1)
            self.assertFalse((self.root / "niri-log").exists())


if __name__ == "__main__":
    unittest.main()
