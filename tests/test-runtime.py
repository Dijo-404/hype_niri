#!/usr/bin/env python3
"""Exercise native desktop launchers in a shared private PID namespace."""

import json
import unittest

from runtime_fixture import RuntimeFixture


class RuntimeTests(RuntimeFixture):
    def waybar_fixture(self):
        config = self.home / ".config/waybar/config.jsonc"
        config.parent.mkdir()
        config.write_text('// Waybar accepts JSONC.\n{"modules-left": ["clock"],}\n')
        return config

    def test_hypridle_resumes_saved_displays_only_after_system_sleep(self):
        hooks = {"after_sleep_cmd": [], "on-resume": []}
        for line in (self.repo / "hypr/hypridle.conf").read_text().splitlines():
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
                command == "~/.config/scripts/hype-control monitors on"
                for command in hooks["on-resume"]
            )
        )

    def test_fullscreen_toggle_does_not_hide_bars_on_other_outputs(self):
        self.control("fullscreen-toggle")
        self.assertEqual(self.niri_actions(), [{"FullscreenWindow": {"id": None}}])

    def test_idle_start_exports_environment_before_restart_and_falls_back(self):
        self.mock(
            "dbus-update-activation-environment",
            'printf "env %s\n" "$*" >> "$FIXTURE_LOG"',
        )
        self.mock(
            "systemctl",
            'printf "systemctl %s\n" "$*" >> "$FIXTURE_LOG"\n'
            'exit "${FIXTURE_RESTART_STATUS:-0}"',
        )
        self.mock("hypridle", 'printf "hypridle\n" >> "$FIXTURE_LOG"')
        self.control("start-idle")
        expected = [
            "env --systemd WAYLAND_DISPLAY XDG_CURRENT_DESKTOP XDG_SESSION_TYPE XDG_SESSION_ID NIRI_SOCKET",
            "systemctl --user restart hypridle.service",
        ]
        self.assertEqual(self.log.read_text().splitlines(), expected)
        self.control("start-idle", extra_env={"FIXTURE_RESTART_STATUS": "1"})
        self.assertEqual(
            self.log.read_text().splitlines(), [*expected, *expected, "hypridle"]
        )

    def test_legacy_dispatch_accepts_installer_transition_symlink(self):
        (self.root / "fullscreen-toggle.sh").symlink_to(self.binary)
        self.isolated('exec "$FIXTURE_ROOT/fullscreen-toggle.sh"')
        self.assertEqual(self.niri_actions(), [{"FullscreenWindow": {"id": None}}])

    def test_waybar_keeps_single_instance_with_hotplug_capable_config(self):
        config = self.waybar_fixture()
        self.mock(
            "waybar",
            'printf "%s\n" "$*" >> "$FIXTURE_LOG"\nexec sleep 30',
        )
        self.isolated(
            "control start-waybar & first=$!\n"
            'wait_for "$FIXTURE_LOG"\n'
            "control start-waybar\n"
            'kill "$first"\n'
            'wait "$first" || :'
        )
        self.assertEqual(self.log.read_text().splitlines(), [f"--config {config}"])

    def test_waybar_stop_terminates_owned_bar_without_external_utilities(self):
        self.waybar_fixture()
        self.mock("waybar", 'touch "$FIXTURE_LOG"\nwhile :; do sleep 0.05; done')
        self.isolated(
            "control start-waybar & first=$!\n"
            'wait_for "$FIXTURE_LOG"\n'
            'control stop-waybar || { kill "$first"; wait "$first" || :; exit 41; }\n'
            'status=0; wait "$first" || status=$?\n'
            "[[ $status == 143 ]]"
        )
        self.assertEqual(self.niri_actions(), [])

    def test_waybar_passes_custom_jsonc_config_and_preserves_theme(self):
        config = self.waybar_fixture()
        custom = self.root / "custom bar.jsonc"
        config.rename(custom)
        self.mock("waybar", 'printf "%s\n" "$GTK_THEME" "$1" "$2" >> "$FIXTURE_LOG"')
        self.control(
            "start-waybar",
            extra_env={"WAYBAR_CONFIG": str(custom), "GTK_THEME": "Test"},
        )
        self.assertEqual(
            self.log.read_text().splitlines(), ["Test", "--config", str(custom)]
        )

    def test_waybar_restart_replaces_owned_waybar(self):
        config = self.waybar_fixture()
        self.mock(
            "waybar",
            'printf "%s\n" "$*" >> "$FIXTURE_LOG"\n'
            "if [[ ${FIXTURE_RESTART:-} != true ]]; then\n"
            "    while :; do sleep 0.05; done\n"
            "fi",
        )
        self.isolated(
            "control start-waybar & first=$!\n"
            'wait_for "$FIXTURE_LOG"\n'
            "FIXTURE_RESTART=true control start-waybar --restart\n"
            'wait "$first" || :'
        )
        self.assertEqual(self.log.read_text().splitlines(), [f"--config {config}"] * 2)

    def test_power_menu_cancellation_has_no_side_effects(self):
        caffeine_state = self.runtime / "caffeine_state"
        caffeine_state.write_text("fixture")
        self.gui_control("power-menu", keys=["Escape"], title="Power")
        self.assertEqual(caffeine_state.read_text(), "fixture")
        self.assertEqual(self.niri_actions(), [])
        self.assertEqual(self.bus_actions(), [])

    def test_power_menu_second_invocation_toggles_current_native_dialog(self):
        self.gui_control("power-menu", toggle=True)
        self.assertEqual(self.niri_actions(), [])
        self.assertEqual(self.bus_actions(), [])

    def test_power_menu_lock_uses_session_locker(self):
        self.mock("hyprlock", 'printf "%s\n" "$*" >> "$FIXTURE_LOG"')
        self.gui_control("power-menu", keys=["Return"])
        self.assertEqual(self.log.read_text().splitlines(), ["--grace 0"])
        self.assertEqual(self.bus_actions(), [])

    def test_power_menu_suspends_after_lock_ready_and_caffeine_cleanup(self):
        self.mock(
            "hyprlock",
            'printf "%s\n" "$*" >> "$FIXTURE_LOG"\n'
            "sleep 0.1\n"
            'touch "$FIXTURE_ROOT/locked"\n'
            "while :; do sleep 0.05; done",
        )
        caffeine_state = self.runtime / "caffeine_state"
        caffeine_state.write_text("fixture")
        self.gui_control("power-menu", keys=["Down", "Return"])
        self.assertEqual(
            self.log.read_text().splitlines(),
            ["--grace 0 --immediate-render --no-fade-in"],
        )
        self.assertEqual(self.bus_actions(), [["Suspend", [True]]])
        self.assertFalse(caffeine_state.exists())

    def test_power_menu_refuses_suspend_if_locker_exits_before_ready(self):
        self.mock("hyprlock", "exit 1")
        result = self.gui_control("power-menu", keys=["Down", "Return"], status=1)
        self.assertIn("before session locking was confirmed", result.stderr)
        self.assertEqual(self.bus_actions(), [])

    def test_power_menu_logout_shutdown_and_reboot_stop_caffeine_first(self):
        caffeine_state = self.runtime / "caffeine_state"
        for choice, niri, bus in [
            ("2", [{"Quit": {"skip_confirmation": True}}], []),
            ("3", [], [["PowerOff", [True]]]),
            ("4", [], [["Reboot", [True]]]),
        ]:
            with self.subTest(choice=choice):
                caffeine_state.write_text("fixture")
                self.gui_control("power-menu", keys=["Down"] * int(choice) + ["Return"])
                self.assertFalse(caffeine_state.exists())
                self.assertEqual(self.niri_actions(), niri)
                self.assertEqual(self.bus_actions(), bus)

    def optional_groups(self, groups):
        config = self.home / ".config/hype-niri/optional-groups"
        config.parent.mkdir(parents=True, exist_ok=True)
        config.write_text("\n".join(groups) + "\n")

    def test_tray_launcher_defaults_to_network_only(self):
        for name in ("nm-applet", "blueman-applet", "warp-taskbar"):
            self.mock(name, 'printf "%s\\n" "${0##*/}" >> "$FIXTURE_LOG"')
        self.isolated('control start-tray-applets\nwait_for "$FIXTURE_LOG"\nsleep 0.1')
        self.assertEqual(self.log.read_text().splitlines(), ["nm-applet"])

    def test_tray_launcher_only_enables_selected_bluetooth_integration(self):
        self.optional_groups(["bluetooth"])
        for name in ("nm-applet", "blueman-applet", "warp-taskbar"):
            self.mock(name, 'printf "%s\\n" "${0##*/}" >> "$FIXTURE_LOG"')
        self.isolated('control start-tray-applets\nwait_for "$FIXTURE_LOG"\nsleep 0.1')
        self.assertCountEqual(
            self.log.read_text().splitlines(), ["nm-applet", "blueman-applet"]
        )

    def test_tray_launcher_keeps_background_instances_without_process_discovery(self):
        self.optional_groups(["warp"])
        for applet in ("nm-applet", "warp-taskbar"):
            self.mock(
                applet,
                'printf "%s|%s|%s\n" "${0##*/}" "$GTK_THEME" "$*" >> "$FIXTURE_LOG"\n'
                'printf "%s" "$$" > "$FIXTURE_ROOT/${0##*/}.pid"\n'
                "exec sleep 30",
            )
        self.isolated(
            "control start-tray-applets\n"
            "for name in nm-applet warp-taskbar; do\n"
            '    wait_for "$FIXTURE_ROOT/$name.pid"\n'
            '    pid=$(cat "$FIXTURE_ROOT/$name.pid")\n'
            '    until [[ $(cat "/proc/$pid/comm") == sleep ]]; do sleep 0.01; done\n'
            "done\n"
            "for _ in {1..6}; do control start-tray-applets & done\n"
            "wait\n"
            "for name in nm-applet warp-taskbar; do\n"
            '    pid=$(cat "$FIXTURE_ROOT/$name.pid")\n'
            '    kill "$pid"\n'
            '    until [[ ! -e "/proc/$pid" ]]; do sleep 0.01; done\n'
            '    rm "$FIXTURE_ROOT/$name.pid"\n'
            "done\n"
            "control start-tray-applets\n"
            'wait_for "$FIXTURE_ROOT/nm-applet.pid"\n'
            'wait_for "$FIXTURE_ROOT/warp-taskbar.pid"'
        )
        self.assertCountEqual(
            self.log.read_text().splitlines(),
            ["nm-applet|Adwaita:dark|--indicator", "warp-taskbar|Adwaita:dark|"] * 2,
        )

    def test_tray_launcher_does_not_restart_existing_warp_client(self):
        self.optional_groups(["warp"])
        self.mock(
            "warp-taskbar",
            'printf started >> "$FIXTURE_LOG"\nwhile :; do sleep 0.05; done',
        )
        self.isolated(
            'warp-taskbar &\nwait_for "$FIXTURE_LOG"\ncontrol start-tray-applets'
        )
        self.assertEqual(self.log.read_text(), "started")

    def test_menu_toggle_closes_existing_menu_and_reopens_after_cancel(self):
        self.mock("wofi", "while :; do sleep 0.05; done")
        self.mock(
            "delayed-picker",
            'printf "%s\n" "$*" >> "$FIXTURE_LOG"\nsleep 0.2\nexec wofi "$@"',
        )
        self.isolated(
            "for count in 1 2; do\n"
            '    control menu-toggle delayed-picker --dmenu "two words" & first=$!\n'
            '    until [[ -e "$FIXTURE_LOG" ]] && (( $(wc -l < "$FIXTURE_LOG") >= count )); do sleep 0.01; done\n'
            '    control menu-toggle delayed-picker --dmenu "two words"\n'
            "    status=0\n"
            '    wait "$first" || status=$?\n'
            "    [[ $status == 143 ]]\n"
            "done"
        )
        self.assertEqual(self.log.read_text().splitlines(), ["--dmenu two words"] * 2)

    def test_menu_toggle_preserves_arguments_input_output_and_exit_status(self):
        result = self.control("menu-toggle", "printf", "%s", "two words")
        self.assertEqual(result.stdout, "two words")
        result = self.control("menu-toggle", "cat", input_text="stdin kept\n")
        self.assertEqual(result.stdout, "stdin kept\n")
        self.control("menu-toggle", "sh", "-c", "exit 7", status=7)

    def test_password_prompt_preserves_stdout_and_cancellation_contract(self):
        result = self.gui_control(
            "password-prompt",
            "-p",
            "Fixture password",
            title="Fixture password",
            text="two words & literal $",
            keys=["Return"],
        )
        self.assertEqual(result.stdout, "two words & literal $\n")
        result = self.gui_control(
            "password-prompt",
            "--prompt=Fixture password",
            title="Fixture password",
            keys=["Escape"],
            status=1,
        )
        self.assertEqual(result.stdout, "")

    def test_password_prompt_rejects_invalid_arguments_without_showing_ui(self):
        result = self.control("password-prompt", "--unknown", status=1)
        self.assertIn("Usage:", result.stderr)
        self.assertEqual(result.stdout, "")

    def desktop_entry(self, name, fields):
        applications = self.home / ".local/share/applications"
        applications.mkdir(parents=True, exist_ok=True)
        (applications / name).write_text("[Desktop Entry]\nType=Application\n" + fields)

    def test_native_launcher_searches_visible_desktop_entries_and_launches_literal_args(
        self,
    ):
        self.mock("fixture-app", 'printf "%s\\n" "$@" > "$FIXTURE_LOG"')
        self.desktop_entry(
            "fixture.desktop",
            'Name=Fixture Launch\nExec=fixture-app "two words" "literal & value"\n',
        )
        self.desktop_entry(
            "hidden.desktop",
            "Name=Fixture Hidden\nExec=fixture-app hidden\nNoDisplay=true\n",
        )
        self.gui_control(
            "app-launcher", text="fixture hidden", keys=["Return", "Escape"]
        )
        self.assertFalse(self.log.exists())
        self.gui_control("app-launcher", text="fixture launch", keys=["Return"])
        self.assertEqual(
            self.log.read_text().splitlines(), ["two words", "literal & value"]
        )

    def test_launch_default_color_picker_preserves_copy_argument(self):
        self.mock("hyprpicker", 'printf "%s\\n" "$@" > "$FIXTURE_LOG"')
        self.isolated('control launch-default color-picker\nwait_for "$FIXTURE_LOG"')
        self.assertEqual(self.log.read_text().splitlines(), ["-a"])

    def test_launch_default_uses_configured_browser(self):
        self.mock("fixture-app", 'printf "%s\\n" "$@" > "$FIXTURE_LOG"')
        self.desktop_entry(
            "fixture-browser.desktop",
            "Name=Fixture Browser\nExec=fixture-app browser\nMimeType=x-scheme-handler/https;\n",
        )
        (self.home / ".config/mimeapps.list").write_text(
            "[Default Applications]\nx-scheme-handler/https=fixture-browser.desktop;\n"
        )
        self.isolated('control launch-default browser\nwait_for "$FIXTURE_LOG"')
        self.assertEqual(self.log.read_text().splitlines(), ["browser"])

    def test_copilot_single_press_prefers_vs_code_to_default_editor(self):
        self.mock("fixture-app", 'printf "%s\\n" "$@" > "$FIXTURE_LOG"')
        self.desktop_entry(
            "com.microsoft.VSCode.desktop",
            "Name=Visual Studio Code\nExec=fixture-app vscode\n",
        )
        self.desktop_entry(
            "fixture-editor.desktop",
            "Name=Default editor\nExec=fixture-app editor\nMimeType=text/plain;\n",
        )
        (self.home / ".config/mimeapps.list").write_text(
            "[Default Applications]\ntext/plain=fixture-editor.desktop;\n"
        )
        self.isolated('control copilot-shortcut\nwait_for "$FIXTURE_LOG"')
        self.assertEqual(self.log.read_text().splitlines(), ["vscode"])
        self.log.unlink()
        (self.root / "locked").touch()
        self.control("copilot-shortcut")
        self.assertFalse(self.log.exists())

    def test_stealth_indicator_preserves_markup_and_hides_inactive_service(self):
        (self.root / "stealth").mkdir()
        (self.root / "stealth/active").touch()
        self.set_unit("tor.service", "inactive")
        self.assertEqual(self.control("stealth-indicator").stdout, "\n\n")
        self.set_unit("tor.service", "active")
        self.assertEqual(
            self.control("stealth-indicator").stdout.splitlines(),
            [
                str(self.home / ".config/waybar/icons/stealth.svg"),
                "Stealth mode active · Tor routing",
            ],
        )

    def test_stealth_indicator_detects_legacy_active_installation(self):
        (self.root / "stealth").mkdir()
        self.set_unit("tor.service", "active")
        self.mock("stealth", '[[ "$*" == version ]]\nprintf "stealth-transparent-v1\n"')
        self.assertIn("Tor routing", self.control("stealth-indicator").stdout)
        self.mock("stealth", 'printf "stealth-transparent-v2\n"')
        self.assertEqual(self.control("stealth-indicator").stdout, "\n\n")

    def niri_actions(self):
        return [
            json.loads(line)["Action"]
            for line in self.take_lines(self.root / "niri-log")
        ]

    def bus_actions(self):
        return [json.loads(line) for line in self.take_lines(self.root / "login-log")]


if __name__ == "__main__":
    unittest.main(verbosity=2)
