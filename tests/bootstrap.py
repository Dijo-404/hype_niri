#!/usr/bin/env python3
"""Exercise bootstrap through a private terminal and fake external tools."""

import errno
import fcntl
import os
import pty
import select
import shutil
import subprocess
import tempfile
import termios
import time
import unittest
from pathlib import Path

REPO = Path(__file__).resolve().parents[1]


class BootstrapTests(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory(prefix="hype-bootstrap-test-")
        self.addCleanup(self.temporary.cleanup)
        self.root = Path(self.temporary.name)
        self.source = self.root / "source with spaces"
        (self.source / "crates/installer").mkdir(parents=True)
        shutil.copy(REPO / "install.sh", self.source / "install.sh")
        (self.source / "Cargo.toml").write_text("fixture")
        (self.source / "crates/installer/Cargo.toml").write_text("fixture")
        self.bin = self.root / "bin"
        self.bin.mkdir()
        self.home = self.root / "home with spaces"
        self.home.mkdir()
        self.log = self.root / "calls"
        self.environment = os.environ | {
            "HOME": str(self.home),
            "PATH": f"{self.bin}:/usr/bin:/bin",
            "BOOTSTRAP_LOG": str(self.log),
            "BOOTSTRAP_SOURCE": str(self.source),
            "TMPDIR": str(self.root),
        }
        self.tool("pacman", "exit 0")
        self.tool("uname", "printf 'Linux\\n'")
        self.tool(
            "cargo",
            """if [[ $1 == -vV ]]; then
    printf 'host: x86_64-test-linux-gnu\\n'
    exit 0
fi
printf 'cargo %s\\n' "$*" >> "$BOOTSTRAP_LOG"
target_dir=""
target_arch="${CARGO_BUILD_TARGET:-}"
while (( $# )); do
    case "$1" in
        --target-dir) target_dir=$2; shift ;;
        --target) target_arch=$2; shift ;;
    esac
    shift
done
[[ -n $target_dir && -n $target_arch ]] || exit 71
release_dir="$target_dir/$target_arch/release"
mkdir -p "$release_dir"
cat > "$release_dir/hype-install" <<'BIN'
#!/usr/bin/env bash
printf 'installer' >> "$BOOTSTRAP_LOG"
printf ' <%s>' "$@" >> "$BOOTSTRAP_LOG"
printf '\\n' >> "$BOOTSTRAP_LOG"
printf 'source <%s>\\n' "$HYPE_NIRI_BOOTSTRAP_SOURCE" >> "$BOOTSTRAP_LOG"
read -r answer
printf 'answer <%s>\\n' "$answer" >> "$BOOTSTRAP_LOG"
BIN
chmod +x "$release_dir/hype-install"
""",
        )
        self.tool(
            "git",
            """printf 'git %s\\n' "$*" >> "$BOOTSTRAP_LOG"
[[ $1 != -C ]] || exit 0
destination=${@: -1}
mkdir -p "$destination/.git"
cp -a "$BOOTSTRAP_SOURCE/." "$destination/"
""",
        )

    def tool(self, name, body):
        path = self.bin / name
        path.write_text("#!/usr/bin/env bash\nset -euo pipefail\n" + body + "\n")
        path.chmod(0o755)

    def run_bootstrap(
        self, *, piped=False, arguments=(), terminal=True, answer=b"terminal answer\n"
    ):
        if not terminal:
            return subprocess.run(
                ["bash", str(self.source / "install.sh"), *arguments],
                env=self.environment,
                stdin=subprocess.DEVNULL,
                check=False,
                capture_output=True,
                text=True,
                timeout=10,
            )
        master, slave = pty.openpty()

        def controlling_terminal():
            os.setsid()
            fcntl.ioctl(slave, termios.TIOCSCTTY, 0)

        command = (
            ["bash", "-s", "--", *arguments]
            if piped
            else ["bash", str(self.source / "install.sh"), *arguments]
        )
        process = subprocess.Popen(
            command,
            env=self.environment,
            stdin=subprocess.PIPE if piped else slave,
            # These tests have no threads; the child needs its own controlling terminal.
            stdout=slave,
            stderr=slave,
            preexec_fn=controlling_terminal,  # noqa: PLW1509
            pass_fds=(slave,),
        )
        os.close(slave)
        output = bytearray()
        try:
            if piped:
                process.stdin.write((self.source / "install.sh").read_bytes())
                process.stdin.close()
            if answer is not None:
                os.write(master, answer)
            deadline = time.monotonic() + 15
            while process.poll() is None and time.monotonic() < deadline:
                if select.select([master], [], [], 0.05)[0]:
                    try:
                        output.extend(os.read(master, 65536))
                    except OSError as error:
                        if error.errno != errno.EIO:
                            raise
            if process.poll() is None:
                process.kill()
                self.fail("Bootstrap did not finish")
            process.wait()
            return subprocess.CompletedProcess(
                command, process.returncode, output.decode(), ""
            )
        finally:
            if process.poll() is None:
                process.kill()
                process.wait()
            os.close(master)

    def test_local_checkout_builds_only_installer_and_preserves_arguments(self):
        result = self.run_bootstrap(arguments=("--help",))
        self.assertEqual(result.returncode, 0, result.stdout)
        self.assertFalse(self.log.exists())
        self.environment["CARGO_BUILD_TARGET"] = "wrong-target"
        result = self.run_bootstrap(arguments=("--source-check", "two words"))
        self.assertEqual(result.returncode, 0, result.stdout)
        calls = self.log.read_text()
        self.assertIn("--package hype-install", calls)
        self.assertIn("--target x86_64-test-linux-gnu", calls)
        self.assertIn("<--source-check> <two words>", calls)
        self.assertIn("source <checkout>", calls)
        self.assertIn("answer <terminal answer>", calls)
        self.assertNotIn("git clone", calls)

    def test_piped_source_gets_answers_from_terminal(self):
        result = self.run_bootstrap(piped=True)
        self.assertEqual(result.returncode, 0, result.stdout)
        calls = self.log.read_text()
        self.assertIn("git clone", calls)
        self.assertIn(f"<--source> <{self.home}/hype-niri/source>", calls)
        self.assertIn("source <main>", calls)
        self.assertIn("answer <terminal answer>", calls)
        self.assertTrue((self.home / "hype-niri/source/.git").is_dir())
        result = self.run_bootstrap(piped=True)
        self.assertEqual(result.returncode, 0, result.stdout)
        calls = self.log.read_text()
        self.assertEqual(calls.count("git clone"), 1)
        self.assertIn(f"git -C {self.home}/hype-niri/source pull --ff-only", calls)

    def test_missing_terminal_fails_before_any_install(self):
        result = self.run_bootstrap(terminal=False)
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("terminal", result.stderr.lower())
        self.assertFalse(self.log.exists())

    def test_missing_compiler_requires_confirmation_before_packages(self):
        (self.bin / "bash").symlink_to("/usr/bin/bash")
        self.environment["PATH"] = str(self.bin)
        result = self.run_bootstrap(answer=b"n\n")
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("full Arch upgrade", result.stdout)
        self.assertFalse(self.log.exists())

    def test_missing_compiler_defaults_to_yes_after_five_seconds(self):
        (self.bin / "bash").symlink_to("/usr/bin/bash")
        self.environment["PATH"] = str(self.bin)
        self.tool("sudo", 'printf "sudo %s\\n" "$*" >> "$BOOTSTRAP_LOG"\nexit 78')
        start = time.monotonic()
        result = self.run_bootstrap(answer=None)
        self.assertEqual(result.returncode, 1, result.stdout)
        self.assertGreaterEqual(time.monotonic() - start, 4.9)
        self.assertIn("Yes in 5s", result.stdout)
        self.assertIn(
            "sudo pacman -Syu --needed git rust base-devel", self.log.read_text()
        )

    def test_failed_source_download_never_builds_or_installs(self):
        self.tool("git", "exit 7")
        result = self.run_bootstrap(piped=True)
        self.assertNotEqual(result.returncode, 0)
        self.assertFalse(self.log.exists())
        self.assertTrue((self.source / "Cargo.toml").exists())
        self.assertEqual(list((self.home / "hype-niri").iterdir()), [])

    def test_failed_build_never_runs_installer(self):
        self.tool(
            "cargo",
            "if [[ $1 == -vV ]]; then printf 'host: x86_64-test-linux-gnu\\n'; exit; fi\nexit 8",
        )
        result = self.run_bootstrap()
        self.assertNotEqual(result.returncode, 0)
        self.assertFalse(self.log.exists())
        self.assertTrue((self.source / "Cargo.toml").exists())

    def test_unsupported_platform_fails_before_build(self):
        self.tool("uname", "printf 'Darwin\\n'")
        result = self.run_bootstrap()
        self.assertNotEqual(result.returncode, 0)
        self.assertFalse(self.log.exists())


if __name__ == "__main__":
    unittest.main(verbosity=2)
