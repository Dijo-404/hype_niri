#!/usr/bin/env python3
"""Isolated regressions for the remaining shell launchers and menus.

Native desktop behavior is exercised in test-native-runtime.py and Rust tests;
the control fixture here records only the remaining launchers' command boundary.
"""

import os
import shlex
import shutil
import signal
import subprocess
import tempfile
import time
import unittest
from concurrent.futures import ThreadPoolExecutor
from pathlib import Path

REPO = Path(__file__).resolve().parents[1]


class RuntimeTests(unittest.TestCase):
    def setUp(self):
        temporary = tempfile.TemporaryDirectory(prefix="hype-niri-runtime-test-")
        self.addCleanup(temporary.cleanup)
        self.root = Path(temporary.name)
        self.user_home = self.root / "home"
        self.scripts = self.user_home / ".config/scripts"
        self.scripts.mkdir(parents=True)
        shutil.copytree(REPO / "wofi", self.user_home / ".config/wofi")
        for source in (REPO / "scripts").glob("*.sh"):
            (self.scripts / source.name).write_text(
                source.read_text().replace("$HOME", "$RUNTIME_TEST_HOME")
            )
            (self.scripts / source.name).chmod(0o755)
        self.runtime = self.root / "runtime"
        self.runtime.mkdir(mode=0o700)
        self.bin = self.root / "bin"
        self.bin.mkdir()
        self.log = self.root / "commands"
        self.environment = os.environ | {
            "RUNTIME_TEST_HOME": str(self.user_home),
            "XDG_RUNTIME_DIR": str(self.runtime),
            "PATH": f"{self.bin}:{os.environ['PATH']}",
            "RUNTIME_TEST_LOG": str(self.log),
            "XDG_CONFIG_HOME": str(self.user_home / ".config"),
            "XDG_CACHE_HOME": str(self.user_home / ".cache"),
        }
        for command in ("notify-send", "pkill"):
            self.mock(command, "exit 0")
        self.mock("pgrep", "exit 1")
        control = self.scripts / "hype-control"
        control.write_text(
            "#!/usr/bin/env bash\nset -eu\n"
            'if [[ "${1:-}" == runtime-dir ]]; then printf "%s\\n" "$XDG_RUNTIME_DIR"; '
            'else printf "hype-control %s\\n" "$*" >> "$RUNTIME_TEST_LOG"; fi\n'
        )
        control.chmod(0o755)

    def mock(self, command, body):
        path = self.bin / command
        path.write_text("#!/usr/bin/env bash\nset -eu\n" + body + "\n")
        path.chmod(0o755)

    def run_script(self, name, *args, environment=None):
        result = subprocess.run(
            ["bash", str(self.scripts / name), *args],
            env=environment or self.environment,
            capture_output=True,
            text=True,
            timeout=8,
            check=False,
        )
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        return result

    def hold_mock(self, command):
        fifo = self.root / f"{command}-hold"
        os.mkfifo(fifo)
        self.environment["RUNTIME_TEST_HOLD"] = str(fifo)
        self.mock(
            command,
            'printf "%s\\n" "$*" >> "$RUNTIME_TEST_LOG"\nread -r _ < "$RUNTIME_TEST_HOLD"',
        )

    def start_script(self, name, *args):
        process = subprocess.Popen(
            ["bash", str(self.scripts / name), *args],
            env=self.environment,
            stdout=subprocess.DEVNULL,
            stderr=subprocess.PIPE,
            text=True,
        )
        self.addCleanup(self.stop_process, process)
        deadline = time.monotonic() + 3
        while not self.log.exists():
            self.assertIsNone(
                process.poll(),
                process.stderr.read() if process.poll() is not None else "",
            )
            self.assertLess(time.monotonic(), deadline, "mock did not start")
            time.sleep(0.01)
        return process

    @staticmethod
    def stop_process(process):
        if process.poll() is None:
            process.terminate()
        process.communicate(timeout=3)

    @staticmethod
    def stop_process_group(process):
        try:
            os.killpg(process.pid, signal.SIGTERM)
        except ProcessLookupError:
            pass
        try:
            process.communicate(timeout=3)
        except subprocess.TimeoutExpired:
            os.killpg(process.pid, signal.SIGKILL)
            process.communicate(timeout=3)

    def waybar_fixture(self):
        config = self.user_home / ".config/waybar/config.jsonc"
        config.parent.mkdir()
        config.write_text('{"modules-left":["clock"]}')
        self.mock(
            "niri",
            'printf \'%s\\n\' \'{"eDP-1":{"name":"eDP-1","current_mode":0},"DP-1":{"name":"DP-1","current_mode":0}}\'',
        )
        return config

    def test_hypridle_resumes_saved_displays_only_after_system_sleep(self):
        hooks = {"after_sleep_cmd": [], "on-resume": []}
        for line in (REPO / "hypr/hypridle.conf").read_text().splitlines():
            key, separator, value = line.partition("=")
            if separator and key.strip() in hooks:
                hooks[key.strip()].append(value.strip())
        self.assertEqual(
            hooks["after_sleep_cmd"],
            ["~/.config/scripts/hype-control copilot-display resume"],
        )
        self.assertTrue(hooks["on-resume"])
        self.assertTrue(
            all(
                command == "niri msg action power-on-monitors"
                for command in hooks["on-resume"]
            )
        )

    def test_waybar_keeps_single_instance_with_hotplug_capable_config(self):
        config = self.waybar_fixture()
        self.hold_mock("waybar")
        self.start_script("start-waybar.sh")
        self.run_script("start-waybar.sh")
        self.assertEqual(self.log.read_text().splitlines(), [f"--config {config}"])

    def test_fullscreen_toggle_does_not_hide_bars_on_other_outputs(self):
        self.mock("niri", 'printf "niri %s\\n" "$*" >> "$RUNTIME_TEST_LOG"')
        self.mock("pgrep", "exit 0")
        self.mock("pkill", 'printf "pkill %s\\n" "$*" >> "$RUNTIME_TEST_LOG"')
        self.run_script("fullscreen-toggle.sh")
        self.assertEqual(
            self.log.read_text().splitlines(), ["niri msg action fullscreen-window"]
        )

    def test_waybar_passes_jsonc_config_through_without_parsing(self):
        config = self.waybar_fixture()
        config.write_text('// Waybar accepts JSONC.\n{"modules-left": ["clock"],}\n')
        self.mock("waybar", 'printf "%s\\n" "$*" >> "$RUNTIME_TEST_LOG"')
        self.run_script("start-waybar.sh")
        self.assertIn(str(config), self.log.read_text())

    def test_power_menu_runs_only_the_chosen_action(self):
        for command in ("niri", "systemctl"):
            self.mock(command, f'printf "%s\\n" "{command} $*" >> "$RUNTIME_TEST_LOG"')
        self.mock(
            "wofi",
            'cat > "$XDG_RUNTIME_DIR/menu"\n'
            'printf "%s\\n" "$*" > "$XDG_RUNTIME_DIR/menu-args"\n'
            '[[ -n "$MENU_CHOICE" ]] || exit 1\n'
            'printf "%s\\n" "$MENU_CHOICE"',
        )
        stop = "hype-control caffeine stop"
        for choice, expected in (
            ("0", ["hype-control lock-screen"]),
            ("1", ["hype-control suspend-now"]),
            ("2", [stop, "niri msg action quit --skip-confirmation"]),
            ("3", [stop, "systemctl poweroff"]),
            ("4", [stop, "systemctl reboot"]),
            ("", []),
        ):
            with self.subTest(choice=choice):
                self.log.unlink(missing_ok=True)
                self.run_script(
                    "power-menu.sh",
                    environment=self.environment | {"MENU_CHOICE": choice},
                )
                logged = self.log.read_text() if self.log.exists() else ""
                self.assertEqual(
                    [line.strip() for line in logged.splitlines()], expected
                )
        entries = (self.runtime / "menu").read_text().splitlines()
        self.assertEqual(
            [entry.split()[-1] for entry in entries],
            ["Lock", "Sleep", "Logout", "Shutdown", "Reboot"],
        )
        arguments = (self.runtime / "menu-args").read_text()
        self.assertIn("--no-custom-entry", arguments)
        self.assertIn("--cache-file /dev/null", arguments)

    def test_tray_launcher_keeps_single_instance_without_process_discovery(self):
        self.hold_mock("nm-applet")
        pid_files = [self.root / name for name in ("nm-applet", "warp-taskbar")]
        for pid_file in pid_files:
            self.mock(
                pid_file.name,
                f'''printf '%s' "$$" > {shlex.quote(str(pid_file))}
printf '{pid_file.name}\\n' >> "$RUNTIME_TEST_LOG"
read -r _ < "$RUNTIME_TEST_HOLD"''',
            )
        self.mock("blueman-applet", "exit 0")
        self.run_script("start-tray-applets.sh")
        deadline = time.monotonic() + 3
        for pid_file in pid_files:
            while not pid_file.exists():
                self.assertLess(time.monotonic(), deadline, "applet did not start")
                time.sleep(0.01)
            self.addCleanup(self.terminate_pid, int(pid_file.read_text()))
        with ThreadPoolExecutor(max_workers=6) as executor:
            list(
                executor.map(
                    lambda _: self.run_script("start-tray-applets.sh"), range(6)
                )
            )
        self.assertCountEqual(
            self.log.read_text().splitlines(), ["nm-applet", "warp-taskbar"]
        )

    def test_tray_launcher_does_not_restart_existing_warp_client(self):
        self.mock("nm-applet", "exit 0")
        self.mock("blueman-applet", "exit 0")
        self.mock("pgrep", '[[ "$*" == "-u $UID -x warp-taskbar" ]]')
        self.mock("warp-taskbar", 'printf started > "$RUNTIME_TEST_LOG"')
        self.run_script("start-tray-applets.sh")
        self.assertFalse(self.log.exists())

    def test_menu_toggle_closes_existing_menu_and_reopens_after_cancel(self):
        pid_file = self.root / "menu-pid"
        self.environment["RUNTIME_TEST_MENU_PID"] = str(pid_file)
        self.mock(
            "pkill",
            '[[ -s "$RUNTIME_TEST_MENU_PID" ]] && '
            'kill -TERM "$(cat "$RUNTIME_TEST_MENU_PID")" 2>/dev/null',
        )
        self.mock(
            "pgrep",
            '[[ -s "$RUNTIME_TEST_MENU_PID" ]] && '
            'kill -0 "$(cat "$RUNTIME_TEST_MENU_PID")" 2>/dev/null',
        )
        program = """
import os
from pathlib import Path
import signal
import time
with Path(os.environ["RUNTIME_TEST_LOG"]).open("a") as log:
    log.write("launched\\n")
time.sleep(0.2)
Path(os.environ["RUNTIME_TEST_MENU_PID"]).write_text(str(os.getpid()))
signal.pause()
"""
        self.mock("wofi", f"exec python3 -c {shlex.quote(program)}")
        for expected_launches in (1, 2):
            process = subprocess.Popen(
                ["bash", str(self.scripts / "menu-toggle.sh"), "wofi", "--dmenu"],
                env=self.environment,
                stdout=subprocess.PIPE,
                stderr=subprocess.PIPE,
                start_new_session=True,
                text=True,
            )
            self.addCleanup(self.stop_process_group, process)
            deadline = time.monotonic() + 3
            while (
                not self.log.exists()
                or len(self.log.read_text().splitlines()) < expected_launches
            ):
                self.assertIsNone(process.poll())
                self.assertLess(time.monotonic(), deadline, "menu did not launch")
                time.sleep(0.01)
            self.run_script("menu-toggle.sh", "wofi", "--dmenu")
            stdout, stderr = process.communicate(timeout=3)
            self.assertEqual(process.returncode, 143, stderr)
            self.assertEqual(stdout, "")
            self.assertEqual(
                self.log.read_text().splitlines(), ["launched"] * expected_launches
            )

    def test_menu_toggle_preserves_command_arguments_output_and_failure(self):
        self.mock("pkill", "exit 1")
        self.mock("pgrep", "exit 1")
        result = self.run_script("menu-toggle.sh", "printf", "%s", "two words")
        self.assertEqual(result.stdout, "two words")
        result = subprocess.run(
            ["bash", str(self.scripts / "menu-toggle.sh"), "sh", "-c", "exit 7"],
            env=self.environment,
            capture_output=True,
            text=True,
            timeout=3,
            check=False,
        )
        self.assertEqual(result.returncode, 7)

    def test_password_prompt_uses_seekable_style_and_cleans_up(self):
        self.mock(
            "wofi",
            r"""
while (( $# )); do
    if [[ "$1" == --style ]]; then
        [[ -f "$2" && -s "$2" ]] || exit 41
        printf '%s\n' "$2" > "$RUNTIME_TEST_LOG"
        break
    fi
    shift
done
printf 'entered-password\n'
""",
        )
        result = self.run_script("password-prompt.sh", "-p", "Password")
        self.assertEqual(result.stdout, "entered-password\n")
        self.assertFalse(Path(self.log.read_text().strip()).exists())

    @staticmethod
    def terminate_pid(pid):
        try:
            os.kill(pid, signal.SIGTERM)
        except ProcessLookupError:
            pass


if __name__ == "__main__":
    unittest.main(verbosity=2)
