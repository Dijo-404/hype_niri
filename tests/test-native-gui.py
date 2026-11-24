#!/usr/bin/env python3
"""Run native GTK/clipboard/keyboard checks on a private headless compositor.

Requires test-only Sway, Cargo and wtype. No connection to the desktop's bus,
clipboard, input devices or application state is passed to child processes.
"""

import os
import shutil
import subprocess
import tempfile
import time
from pathlib import Path

REPO = Path(__file__).resolve().parents[1]


def main():
    prefix = Path.home() / ".cache/hype-niri/notch-build-deps/usr"
    sway = shutil.which("sway") or str(prefix / "bin/sway")
    wtype = shutil.which("wtype")
    cargo = shutil.which("cargo") or str(Path.home() / ".cargo/bin/cargo")
    if not Path(sway).is_file() or not wtype or not Path(cargo).is_file():
        raise SystemExit(
            "Install test-only Sway, wtype and Cargo to run isolated GUI checks"
        )
    with tempfile.TemporaryDirectory(prefix="hype-native-protocol-") as temporary:
        root = Path(temporary)
        runtime = root / "runtime"
        runtime.mkdir(mode=0o700)
        home = root / "home"
        home.mkdir()
        config = root / "sway.conf"
        config.write_text(
            "output HEADLESS-1 resolution 2880x1800 scale 2 position 0 0\n"
            "output HEADLESS-2 resolution 1920x1080 scale 1 position 1440 0\n"
            "input * xkb_layout us\n"
        )
        bus_config = root / "bus.conf"
        bus_config.write_text(
            "<busconfig><type>session</type><listen>unix:tmpdir=/tmp</listen>"
            '<auth>EXTERNAL</auth><policy context="default">'
            '<allow send_destination="*"/><allow receive_sender="*"/><allow own="*"/>'
            "</policy></busconfig>"
        )
        env = os.environ.copy()
        for name in (
            "WAYLAND_SOCKET",
            "WAYLAND_DISPLAY",
            "NIRI_SOCKET",
            "SWAYSOCK",
            "DISPLAY",
        ):
            env.pop(name, None)
        env.update(
            XDG_RUNTIME_DIR=str(runtime),
            XDG_CONFIG_HOME=str(home / ".config"),
            XDG_DATA_HOME=str(home / ".local/share"),
            XDG_STATE_HOME=str(home / ".local/state"),
            XDG_CACHE_HOME=str(home / ".cache"),
            WLR_BACKENDS="headless",
            WLR_HEADLESS_OUTPUTS="2",
            WLR_RENDERER="pixman",
            GTK_A11Y="none",
            GDK_BACKEND="wayland",
            GSK_RENDERER="cairo",
            PULSE_SERVER=f"unix:{root / 'absent-pulse'}",
            LD_LIBRARY_PATH=f"{Path.home() / '.local/lib'}:{prefix / 'lib'}",
            PKG_CONFIG_PATH=f"{Path.home() / '.local/lib/pkgconfig'}:{prefix / 'lib/pkgconfig'}",
            HYPE_NATIVE_PROTOCOL_TESTS="1",
            HYPE_CONTROL_TEST_BINARY=str(REPO / "target/debug/hype-control"),
        )
        # Keep HOME only for Cargo's existing toolchain; all desktop XDG roots are private.
        bus = subprocess.Popen(
            [
                "dbus-daemon",
                "--nofork",
                "--print-address=1",
                f"--config-file={bus_config}",
            ],
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            text=True,
            env=env,
        )
        children = [bus]
        try:
            address = bus.stdout.readline().strip()
            if not address:
                raise RuntimeError(bus.stderr.read())
            env.update(
                DBUS_SESSION_BUS_ADDRESS=address, DBUS_SYSTEM_BUS_ADDRESS=address
            )
            with (root / "sway.log").open("w") as log:
                compositor = subprocess.Popen(
                    [sway, "--unsupported-gpu", "-c", str(config)],
                    env=env,
                    stdout=log,
                    stderr=log,
                )
                children.append(compositor)
                for _ in range(100):
                    sockets = [p for p in runtime.glob("wayland-*") if p.is_socket()]
                    if sockets:
                        break
                    if compositor.poll() is not None:
                        raise RuntimeError((root / "sway.log").read_text())
                    time.sleep(0.05)
                if not sockets:
                    raise RuntimeError("Isolated compositor did not start")
                env["WAYLAND_DISPLAY"] = sockets[0].name
                # Establish an input seat without sending text or accessing real devices.
                keyboard = subprocess.Popen([wtype, "-s", "300000"], env=env)
                children.append(keyboard)
                cases = [
                    (
                        "hype-desktop",
                        "dialog::tests::gtk4_search_password_qr_and_remote_toggle",
                    ),
                    ("hype-notch", "ui::tests::gtk4_parity_and_native_dbus"),
                    (
                        "hype-desktop",
                        "keyboard::protocol_tests::gtk_entry_receives_complete_unicode_sequence",
                    ),
                    (
                        "hype-desktop",
                        "clipboard::protocol_tests::native_watch_copy_image_sensitive_singleton_and_shutdown",
                    ),
                ]
                cases = [
                    (package, case)
                    for package, case in cases
                    if os.environ.get("HYPE_GUI_CASE", "") in case
                ]
                if not cases:
                    raise RuntimeError("No GUI test matches HYPE_GUI_CASE")
                for package, case in cases:
                    result = subprocess.run(
                        [
                            cargo,
                            "test",
                            "--locked",
                            "-p",
                            package,
                            case,
                            "--",
                            "--ignored",
                            "--test-threads=1",
                            "--nocapture",
                        ],
                        cwd=REPO,
                        env=env,
                        capture_output=True,
                        text=True,
                        timeout=300,
                        check=False,
                    )
                    if result.returncode:
                        raise RuntimeError(f"{case}\n{result.stdout}\n{result.stderr}")
                    print(f"PASS {case}", flush=True)
        finally:
            for child in reversed(children):
                if child.poll() is None:
                    child.terminate()
                try:
                    child.wait(timeout=5)
                except subprocess.TimeoutExpired:
                    child.kill()
                    child.wait()
                for pipe in (child.stdout, child.stderr):
                    if pipe:
                        pipe.close()


if __name__ == "__main__":
    main()
