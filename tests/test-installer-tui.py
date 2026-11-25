#!/usr/bin/env python3
"""Exercise the real installer UI in a private PTY with stubbed system tools."""

import errno
import fcntl
import os
import pty
import re
import select
import signal
import struct
import subprocess
import tempfile
import termios
import time
import unittest
from pathlib import Path

REPO = Path(__file__).resolve().parents[1]
BINARY = REPO / "target/debug/hype-install"
ROWS, COLUMNS = 27, 100
TERMINAL_TOKEN = re.compile(r"\x1b\[([0-?]*)([ -/]*)([@-~])|([\s\S])")


def screen_text(output):
    """Apply the cursor moves and clears emitted by the installer."""
    screen = [[" "] * COLUMNS for _ in range(ROWS)]
    row = column = 0
    for parameters, _, command, character in TERMINAL_TOKEN.findall(output):
        if command == "H":
            positions = parameters.split(";")
            row = int(positions[0] or 1) - 1
            column = int(positions[1] or 1) - 1 if len(positions) > 1 else 0
        elif (command == "J" and parameters == "2") or (
            command in {"h", "l"} and parameters == "?1049"
        ):
            screen = [[" "] * COLUMNS for _ in range(ROWS)]
            row = column = 0
        elif character == "\r":
            column = 0
        elif character == "\n":
            row = min(row + 1, ROWS - 1)
        elif character and character.isprintable() and row < ROWS and column < COLUMNS:
            screen[row][column] = character
            column += 1
    return "\n".join("".join(line).rstrip() for line in screen)


@unittest.skipIf(os.geteuid() == 0, "the installer must run as a regular user")
class InstallerTuiTests(unittest.TestCase):
    def setUp(self):
        if not BINARY.is_file():
            self.fail(
                "Build the installer first with cargo build --locked -p hype-install"
            )
        temporary = tempfile.TemporaryDirectory(prefix="hype-niri-tui-test-")
        self.addCleanup(temporary.cleanup)
        self.root = Path(temporary.name)
        self.home = self.root / "home"
        self.calls = self.root / "calls"
        self.bin = self.root / "bin"
        for directory in [self.home, self.bin, self.root / "runtime"]:
            directory.mkdir()
        self.tool("pacman", "exit 0")
        self.tool("curl", 'printf "preflight\\n" >> "$HYPE_TEST_CALLS"')
        self.tool(
            "git",
            """printf 'git %s\\n' "$*" >> "$HYPE_TEST_CALLS"
/usr/bin/ln -s -- "$HYPE_TEST_SOURCE" "${@: -1}"
""",
        )
        self.tool(
            "sudo",
            """printf 'Native package confirmation: '
IFS= read -r answer
printf 'sudo %s [%s]\\n' "$*" "$answer" >> "$HYPE_TEST_CALLS"
exit 78
""",
        )
        self.output = bytearray()

    def tool(self, name, body):
        path = self.bin / name
        path.write_text("#!/usr/bin/bash\nset -euo pipefail\n" + body + "\n")
        path.chmod(0o700)

    def register_cli(self):
        result = subprocess.run(
            [BINARY, "--setup-cli"],
            env=os.environ
            | {"HOME": str(self.home), "XDG_RUNTIME_DIR": str(self.root / "runtime")},
            stdin=subprocess.DEVNULL,
            capture_output=True,
            text=True,
            check=False,
            timeout=5,
        )
        return result, self.home / ".local/bin/hype"

    def start(self, *arguments, executable=BINARY, source=REPO):
        self.master, slave = pty.openpty()
        self.addCleanup(self.close_master)
        self.addCleanup(os.close, slave)
        fcntl.ioctl(slave, termios.TIOCSWINSZ, struct.pack("HHHH", ROWS, COLUMNS, 0, 0))
        original = termios.tcgetattr(slave)

        def attach():
            os.setsid()
            fcntl.ioctl(slave, termios.TIOCSCTTY, 0)

        environment = os.environ | {
            "HOME": str(self.home),
            "PATH": str(self.bin),
            "TERM": "xterm-256color",
            "NO_COLOR": "1",
            "XDG_RUNTIME_DIR": str(self.root / "runtime"),
            "TMPDIR": str(self.root),
            "HYPE_NIRI_BOOTSTRAP_SOURCE": "checkout",
            "HYPE_TEST_CALLS": str(self.calls),
            "HYPE_TEST_SOURCE": str(REPO),
        }
        process = subprocess.Popen(
            [executable, *(["--source", source] if source else []), *arguments],
            cwd=self.root,
            env=environment,
            stdin=slave,
            stdout=slave,
            stderr=slave,
            preexec_fn=attach,  # noqa: PLW1509
            pass_fds=(slave,),
        )

        def cleanup():
            if process.poll() is None:
                process.kill()
                process.wait()

        self.addCleanup(cleanup)
        return process, slave, original

    def close_master(self):
        if self.master is not None:
            os.close(self.master)
            self.master = None

    def send(self, keys):
        os.write(self.master, keys)

    def expect(self, text):
        deadline = time.monotonic() + 12
        screen = screen_text(self.output.decode(errors="ignore"))
        while text not in screen and time.monotonic() < deadline:
            if select.select([self.master], [], [], 0.1)[0]:
                try:
                    self.output.extend(os.read(self.master, 65536))
                except OSError as cause:
                    if cause.errno != errno.EIO:
                        raise
                    break
                screen = screen_text(self.output.decode(errors="ignore"))
        self.assertIn(text, screen)

    def test_update_fetches_source_and_restores_saved_extras(self):
        saved = self.home / ".config/hype-niri/optional-groups"
        saved.parent.mkdir(parents=True)
        saved.write_text("browser-firefox\ndocker\nretired-group\n")
        result, launcher = self.register_cli()
        self.assertEqual(result.returncode, 0, result.stderr)
        process, slave, original = self.start(
            "update", executable=launcher, source=None
        )
        self.expect("Update packages and Hype Niri")
        self.send(b"\x1b[B\r")
        self.expect("Extras 2/")
        self.expect("[x] Firefox")
        self.expect("[x] Docker")
        self.send(b"\r")
        self.expect("Review update")
        self.send(b"\x1b")
        self.expect("Space  Toggle")
        self.send(b"q")
        self.expect("Update system packages")
        self.send(b"q")
        self.assertEqual(process.wait(timeout=5), 0)
        self.assertEqual(termios.tcgetattr(slave), original)
        self.assertIn("git clone --depth 1 --branch main", self.calls.read_text())
        self.assertNotIn("preflight", self.calls.read_text())
        self.assertFalse(list(self.root.glob("hype-niri-update-*")))

    def test_install_command_starts_the_fresh_install_checklist(self):
        result, launcher = self.register_cli()
        self.assertEqual(result.returncode, 0, result.stderr)
        result, _ = self.register_cli()
        self.assertEqual(result.returncode, 0, result.stderr)
        process, _, _ = self.start("install", executable=launcher, source=None)
        self.expect("[x] Firefox")
        self.send(b"\r")
        self.expect("Review install")
        self.send(b"\x1b")
        self.expect("Space  Toggle")
        self.send(b"q")
        self.expect("Install desktop")
        self.send(b"q")
        self.assertEqual(process.wait(timeout=5), 0)
        self.assertIn("git clone --depth 1 --branch main", self.calls.read_text())

    def test_registration_preserves_an_existing_hype_command(self):
        launcher = self.home / ".local/bin/hype"
        launcher.parent.mkdir(parents=True)
        launcher.write_text("another command\n")
        result, _ = self.register_cli()
        self.assertEqual(result.returncode, 1)
        self.assertIn("Another command already uses", result.stderr)
        self.assertEqual(launcher.read_text(), "another command\n")

    def test_sigterm_restores_terminal_mode(self):
        process, slave, original = self.start()
        self.expect("Install desktop")
        process.send_signal(signal.SIGTERM)
        self.assertEqual(process.wait(timeout=5), 1)
        self.assertEqual(termios.tcgetattr(slave), original)

    def test_terminal_disconnect_exits(self):
        process, _, _ = self.start()
        self.expect("Install desktop")
        self.close_master()
        self.assertEqual(process.wait(timeout=5), 1)

    def test_apply_requires_confirmation_and_restores_tty_for_package_prompt(self):
        process, slave, original = self.start()
        self.expect("Install desktop")
        self.send(b"\r")
        self.expect("Space  Toggle")
        self.send(b"n\r")
        self.expect("Review install")
        self.assertFalse(self.calls.exists())
        self.send(b"\r")
        self.expect("Refresh Arch mirrors")
        self.send(b"\r")
        self.expect("Native package confirmation")
        self.assertEqual(termios.tcgetattr(slave), original)
        self.send(b"terminal answer\r")
        self.expect("Install failed")
        self.send(b"q")
        self.assertEqual(process.wait(timeout=5), 1)
        self.assertEqual(termios.tcgetattr(slave), original)
        self.assertIn(
            "sudo pacman -Sy --needed archlinux-keyring [terminal answer]",
            self.calls.read_text(),
        )

    def test_system_update_runs_pacman_and_yay_without_desktop_checkout(self):
        self.tool("sudo", 'printf "sudo %s\\n" "$*" >> "$HYPE_TEST_CALLS"')
        self.tool("yay", 'printf "yay %s\\n" "$*" >> "$HYPE_TEST_CALLS"')
        process, _, _ = self.start("update", source=None)
        self.expect("Update system packages")
        self.send(b"\r")
        self.expect("Review package update")
        self.assertFalse(self.calls.exists())
        self.send(b"\r")
        self.expect("Package update complete")
        self.send(b"q")
        self.assertEqual(process.wait(timeout=5), 0)
        self.assertEqual(
            self.calls.read_text().splitlines(),
            [
                "sudo pacman -Sy --needed archlinux-keyring",
                "sudo pacman -Syu",
                "yay -Syu --aur --noconfirm=false --confirm",
            ],
        )

    def test_first_install_bootstraps_missing_yay_as_regular_user(self):
        self.tool("sudo", 'printf "sudo %s\\n" "$*" >> "$HYPE_TEST_CALLS"')
        self.tool(
            "git",
            """printf 'git %s\\n' "$*" >> "$HYPE_TEST_CALLS"
/usr/bin/mkdir -- "${@: -1}"
""",
        )
        self.tool(
            "env",
            """[[ $1 == --chdir && -d $2 ]]
shift 2
printf '%s (uid %s)\\n' "$*" "$EUID" >> "$HYPE_TEST_CALLS"
/usr/bin/cp "$PATH/yay-stub" "$PATH/yay"
""",
        )
        self.tool("yay-stub", 'printf "yay %s\\n" "$*" >> "$HYPE_TEST_CALLS"\nexit 15')
        process, _, _ = self.start("install")
        self.expect("Continue to review")
        self.send(b"n\r")
        self.expect("Start installation")
        self.send(b"\r")
        self.expect("Refresh Arch mirrors")
        self.send(b"\r")
        self.expect("Install failed")
        self.send(b"q")
        self.assertEqual(process.wait(timeout=5), 1)
        calls = self.calls.read_text().splitlines()
        self.assertEqual(calls[-5], "sudo pacman -Syu")
        self.assertEqual(calls[-4], "sudo pacman -S --needed git base-devel")
        self.assertIn("https://aur.archlinux.org/yay-bin.git", calls[-3])
        self.assertEqual(calls[-2], f"makepkg -si --needed (uid {os.geteuid()})")
        self.assertEqual(calls[-1], "yay -Syu --aur --noconfirm=false --confirm")
        self.assertFalse(list(self.root.glob("hype-niri-yay-*")))


if __name__ == "__main__":
    unittest.main(verbosity=2)
