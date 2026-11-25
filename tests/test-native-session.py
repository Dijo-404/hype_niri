#!/usr/bin/env python3
"""Check native monitor and sleep commands against isolated desktop services."""

import concurrent.futures
import json
import select
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

    def test_sleep_watcher_preserves_caffeine_and_blanks_before_releasing_delay(self):
        self.mock(
            "hyprlock",
            'touch "$FIXTURE_ROOT/locker-started"\n'
            'until [[ -e "$FIXTURE_ROOT/locked" ]]; do sleep 0.01; done\n'
            "while :; do sleep 0.05; done",
        )

        def inhibitor(kind):
            what, reader = self.bus.inhibitors.get(timeout=3)
            self.addCleanup(reader.close)
            self.assertEqual(what, kind)
            reader.settimeout(3)
            return reader

        with concurrent.futures.ThreadPoolExecutor(max_workers=1) as executor:
            running = executor.submit(
                self.isolated,
                'state="$XDG_RUNTIME_DIR/caffeine_state"\n'
                "control caffeine toggle\n"
                'initial=$(cat "$state")\n'
                "control prepare-sleep --watch & watcher=$!\n"
                'wait_for "$FIXTURE_ROOT/locker-started"\n'
                '[[ ! -e "$FIXTURE_ROOT/niri-log" ]]\n'
                'touch "$FIXTURE_ROOT/locked"\n'
                'wait_for "$FIXTURE_ROOT/resumed"\n'
                '[[ $(cat "$state") == "$initial" ]]\n'
                "control caffeine status\n"
                'kill "$watcher"\n'
                'wait "$watcher" || :',
            )
            idle = inhibitor("idle")
            delay = inhibitor("sleep")
            self.bus.prepare_for_sleep(True)
            self.assertEqual(delay.recv(1), b"")
            self.assertTrue((self.root / "locked").exists())
            self.assertEqual(select.select([idle], [], [], 0)[0], [])
            self.assertEqual(
                self.niri_requests.get_nowait(), {"Action": {"PowerOffMonitors": {}}}
            )

            self.bus.prepare_for_sleep(False)
            inhibitor("sleep")
            # Let the watcher finish resume before terminating its PID namespace.
            self.assertEqual(
                self.niri_requests.get(timeout=3), {"Action": {"PowerOnMonitors": {}}}
            )
            (self.root / "resumed").touch()
            result = running.result(timeout=5)

        self.assertEqual(
            [json.loads(line)["class"] for line in result.stdout.splitlines()],
            ["activated", "activated"],
        )


if __name__ == "__main__":
    unittest.main()
