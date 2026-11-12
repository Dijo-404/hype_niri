#!/usr/bin/env python3
"""Installer regression checks: python3 tests/test-install.py.

Runs sourced installer functions with mocked privileged commands. A temporary
source copy redirects home paths without changing the real HOME environment.
"""

import os
import fcntl
from pathlib import Path
import re
import shlex
import subprocess
import tempfile
import unittest


REPO = Path(__file__).resolve().parents[1]


class InstallerTests(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory(prefix="hype-niri-install-test-")
        self.addCleanup(self.temporary.cleanup)
        self.root = Path(self.temporary.name)
        self.user_home = self.root / "home"
        self.user_home.mkdir()
        self.runtime = self.root / "runtime"
        self.runtime.mkdir(mode=0o700)
        self.log = self.root / "commands"
        self.source = self.root / "install.sh"
        self.source.write_text(
            (REPO / "install.sh").read_text().replace("$HOME", "$INSTALL_TEST_HOME")
        )

    def run_installer(self, code, *, input_text="", succeeds=True):
        environment = os.environ.copy()
        environment.update(
            INSTALL_TEST_HOME=str(self.user_home),
            INSTALL_TEST_LOG=str(self.log),
            XDG_RUNTIME_DIR=str(self.runtime),
        )
        preamble = f"""
source {shlex.quote(str(self.source))}
SCRIPT_DIR={shlex.quote(str(REPO))}
print_header() {{ :; }}
print_step() {{ :; }}
print_warn() {{ :; }}
print_done() {{ :; }}
confirm() {{ return 0; }}
sleep() {{ :; }}
curl() {{ return 99; }}
sudo() {{ printf '%s\\n' "$*" >> "$INSTALL_TEST_LOG"; return 1; }}
systemctl() {{ return 1; }}
stdbuf() {{ shift 2; "$@"; }}
xdg-user-dir() {{ printf '%s/\\n' "$INSTALL_TEST_HOME"; }}
xdg-user-dirs-update() {{ printf 'xdg-user-dirs-update %s\\n' "$*" >> "$INSTALL_TEST_LOG"; }}
"""
        result = subprocess.run(
            ["bash", "-c", preamble + code],
            input=input_text,
            capture_output=True,
            text=True,
            env=environment,
            timeout=30,
        )
        if succeeds:
            self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        else:
            self.assertNotEqual(result.returncode, 0, result.stdout + result.stderr)
        return result

    def commands(self):
        return self.log.read_text().splitlines() if self.log.exists() else []

    def package_fixture(self, packages, *, repository=(), installed=(), provided=()):
        """Model package availability and dependency satisfaction without pacman."""
        fixture = self.root / "packages"
        fixture.mkdir()
        for name, values in (
            ("pkglist.txt", packages),
            ("repository", repository),
            ("installed", installed),
            ("satisfied", (*installed, *provided)),
        ):
            (fixture / name).write_text("".join(f"{value}\n" for value in values))
        return f"SCRIPT_DIR={shlex.quote(str(fixture))}\n" + r"""
test() {
    # Package mocks simulate a terminal without changing other shell tests.
    if [[ $# == 2 && $1 == -t && $2 == 0 ]]; then
        [[ ${MOCK_HAS_TTY:-1} == 1 ]]
    else
        builtin test "$@"
    fi
}
mock_contains() {
    local line
    while IFS= read -r line; do
        [[ $line != "$2" ]] || return 0
    done < "$SCRIPT_DIR/$1"
    return 1
}
mock_install_targets() {
    local target
    for target in "$@"; do
        case "$target" in
            -*) ;;
            *)
                printf '%s\n' "${target#aur/}" >> "$SCRIPT_DIR/installed"
                printf '%s\n' "${target#aur/}" >> "$SCRIPT_DIR/satisfied"
                ;;
        esac
    done
}
pacman() {
    printf 'pacman %s\n' "$*" >> "$INSTALL_TEST_LOG"
    case "$1" in
        -Si) mock_contains repository "$2" ;;
        -Q|-Qi|-Qq) mock_contains installed "$2" ;;
        -T)
            shift
            local target status=0
            for target in "$@"; do
                if ! mock_contains satisfied "$target"; then
                    printf '%s\n' "$target"
                    status=127
                fi
            done
            return "$status"
            ;;
        *) printf 'Unexpected pacman call: %s\n' "$*" >&2; return 99 ;;
    esac
}
sudo() {
    printf '%s\n' "$*" >> "$INSTALL_TEST_LOG"
    [[ $1 == pacman && $2 == -S ]] || return 99
    [[ -z ${MOCK_PACMAN_OUTPUT:-} ]] || printf '%s\n' "$MOCK_PACMAN_OUTPUT"
    shift 2
    mock_install_targets "$@"
}
ensure_yay() {
    printf 'ensure_yay\n' >> "$INSTALL_TEST_LOG"
    return "${MOCK_ENSURE_YAY_STATUS:-0}"
}
yay() {
    printf 'yay %s\n' "$*" >> "$INSTALL_TEST_LOG"
    case "$1" in
        -Si)
            [[ -z ${MOCK_YAY_QUERY_OUTPUT:-} ]] || printf '%s\n' "$MOCK_YAY_QUERY_OUTPUT"
            return "${MOCK_YAY_QUERY_STATUS:-0}"
            ;;
        -S) ;;
        *) return 99 ;;
    esac
    [[ -z ${MOCK_YAY_OUTPUT:-} ]] || printf '%s\n' "$MOCK_YAY_OUTPUT"
    [[ ${MOCK_YAY_STATUS:-0} == 0 ]] || return "$MOCK_YAY_STATUS"
    if [[ ${MOCK_YAY_INSTALLS:-1} == 1 ]]; then
        shift
        mock_install_targets "$@"
    fi
    return 0
}
mktemp() {
    # Failure logs are deliberately preserved; keep them inside this fixture.
    if [[ ${1:-} == /tmp/hype-niri-install-* ]]; then
        command mktemp "$INSTALL_TEST_HOME/hype-niri-install-XXXXXXXX.log"
    else
        command mktemp "$INSTALL_TEST_HOME/tmp-XXXXXXXX"
    fi
}
"""

    def validation_fixture(self):
        """Provide valid files while varying only the service state under test."""
        source = self.source.read_text()
        critical = re.search(r"local critical_files=\((.*?)\n    \)", source, re.S)
        self.assertIsNotNone(critical)
        for path in re.findall(r'"([^"]+)"', critical.group(1)):
            fixture = Path(path.replace("$INSTALL_TEST_HOME", str(self.user_home)))
            self.assertTrue(fixture.is_relative_to(self.user_home))
            fixture.parent.mkdir(parents=True, exist_ok=True)
            fixture.write_text("")
            fixture.chmod(0o755)

        agent = self.root / "polkit-agent"
        agent.touch()
        agent.chmod(0o755)
        policy = self.root / "installed-oom-policy"
        policy.write_text((REPO / "systemd/user@.service.d/60-hype-niri-oomd.conf").read_text())
        self.source.write_text(
            source.replace("/usr/lib/polkit-gnome/polkit-gnome-authentication-agent-1", str(agent))
            .replace("/etc/systemd/system/user@.service.d/60-hype-niri-oomd.conf", str(policy))
        )
        return r"""
command() {
    [[ $1 == -v ]] && return 0
    builtin command "$@"
}
niri() { [[ $1 == validate ]]; }
fc-match() { printf '%s\n' "$3"; }
systemctl() {
    printf 'systemctl %s\n' "$*" >> "$INSTALL_TEST_LOG"
    case "$*" in
        '--user show-environment') return 1 ;;
        'is-active --quiet user@'*) [[ ${ACTIVE_MANAGER:-0} == 1 ]] ;;
        'list-unit-files NetworkManager.service --no-legend')
            [[ ${MISSING_NETWORK:-0} == 1 ]] || printf 'NetworkManager.service enabled\n'
            ;;
        'list-unit-files '*) printf 'available.service enabled\n' ;;
        'is-enabled --quiet '*|'is-active --quiet '*) return 0 ;;
        'show user@'*)
            printf '%s\n' MemoryAccounting=yes ManagedOOMMemoryPressure=kill ManagedOOMSwap=kill \
                "ManagedOOMMemoryPressureLimit=${PRESSURE_LIMIT:-1717986918}" \
                "ManagedOOMMemoryPressureDurationUSec=${PRESSURE_DURATION:-10s}"
            ;;
        *) printf 'Unexpected systemctl call: %s\n' "$*" >&2; return 99 ;;
    esac
}
"""

    def test_validation_succeeds_before_user_manager_starts(self):
        self.run_installer(self.validation_fixture() + "validate")
        self.assertFalse(any(command.startswith("systemctl show user@") for command in self.commands()))

    def test_validation_rejects_missing_required_network_service(self):
        result = self.run_installer(
            self.validation_fixture() + "MISSING_NETWORK=1 validate", succeeds=False
        )
        self.assertIn("Required system service unit unavailable: NetworkManager.service", result.stdout)

    def test_validation_accepts_effective_oom_policy(self):
        self.run_installer(self.validation_fixture() + "ACTIVE_MANAGER=1 validate")

    def test_validation_rejects_pressure_limit_override(self):
        result = self.run_installer(
            self.validation_fixture() + "ACTIVE_MANAGER=1 PRESSURE_LIMIT=3221225471 validate",
            succeeds=False,
        )
        self.assertIn("required 40% for 10 seconds", result.stdout)

    def test_validation_rejects_pressure_duration_override(self):
        result = self.run_installer(
            self.validation_fixture() + "ACTIVE_MANAGER=1 PRESSURE_DURATION=30s validate",
            succeeds=False,
        )
        self.assertIn("required 40% for 10 seconds", result.stdout)

    def test_backups_include_generated_settings_and_preserve_symlinks(self):
        settings = self.user_home / ".config/gtk-3.0/settings.ini"
        settings.parent.mkdir(parents=True)
        settings.write_text("custom settings\n")
        (self.user_home / ".zshrc").symlink_to("missing-zsh-target")
        result = self.run_installer('backup_configs; printf "%s\\n" "$BACKUP_DIR"')
        backup = Path(result.stdout.strip())
        self.assertEqual((backup / ".config/gtk-3.0/settings.ini").read_text(), "custom settings\n")
        self.assertTrue((backup / ".zshrc").is_symlink())
        self.assertEqual(os.readlink(backup / ".zshrc"), "missing-zsh-target")

    def test_declined_backup_stops_before_overwrite(self):
        settings = self.user_home / ".zshrc"
        settings.write_text("keep me")
        self.run_installer("confirm() { return 1; }; backup_configs", succeeds=False)
        self.assertEqual(settings.read_text(), "keep me")

    def test_failed_directory_publish_restores_previous_config(self):
        destination = self.user_home / ".config/niri"
        destination.mkdir(parents=True)
        (destination / "config.kdl").write_text("previous config")
        staged = self.root / "staged"
        staged.mkdir()
        (staged / "config.kdl").write_text("new config")
        self.run_installer(f"""
mv() {{
    if [[ $* == *'.previous'* || "${{!#}}" != {shlex.quote(str(destination))} ]]; then
        command mv "$@"
    else
        return 1
    fi
}}
replace_directory {shlex.quote(str(staged))} {shlex.quote(str(destination))}
""", succeeds=False)
        self.assertEqual((destination / "config.kdl").read_text(), "previous config")

    def test_mirror_refresh_does_not_sync_databases_without_upgrade(self):
        self.run_installer("""
reflector() { printf 'Server = https://example.invalid/mirror\n' > "${!#}"; }
sudo() { printf '%s\n' "$*" >> "$INSTALL_TEST_LOG"; return 0; }
refresh_mirrors
""")
        self.assertFalse(any(command.startswith("pacman ") for command in self.commands()))

    def test_declined_upgrade_does_not_refresh_package_databases(self):
        self.run_installer(
            "confirm() { return 1; }; update_system_packages", succeeds=False
        )
        self.assertEqual(self.commands(), [])

    def test_package_list_ignores_indented_comments_and_duplicates(self):
        fixture = self.root / "packages"
        fixture.mkdir()
        (fixture / "pkglist.txt").write_text("  # comment\n\n niri # desktop\nniri\n waybar\n")
        self.run_installer(f"""
SCRIPT_DIR={shlex.quote(str(fixture))}
pacman() {{ return 0; }}
sudo() {{ printf '%s\\n' "$*" >> "$INSTALL_TEST_LOG"; return 0; }}
install_packages
""")
        self.assertEqual(self.commands(), ["pacman -S --needed --noconfirm niri waybar"])

    def test_installed_foreign_packages_are_retained_without_aur_lookup(self):
        packages = ("bemoji", "wlogout", "cloudflare-warp-bin")
        self.run_installer(
            self.package_fixture(packages, installed=packages)
            + "MOCK_HAS_TTY=0; MOCK_ENSURE_YAY_STATUS=99; install_packages"
        )
        commands = self.commands()
        self.assertNotIn("ensure_yay", commands)
        self.assertFalse(any(command.startswith("yay ") for command in commands))
        self.assertEqual(commands[-1], "pacman -T bemoji wlogout cloudflare-warp-bin")

    def test_installed_foreign_provider_satisfies_requested_package(self):
        self.run_installer(
            self.package_fixture(
                ("wlogout",), installed=("wlogout-git",), provided=("wlogout",)
            )
            + "MOCK_ENSURE_YAY_STATUS=99; install_packages"
        )
        self.assertNotIn("ensure_yay", self.commands())
        self.assertFalse(any(command.startswith("yay ") for command in self.commands()))
        self.assertEqual(self.commands()[-1], "pacman -T wlogout")

    def test_installed_repository_packages_remain_upgrade_eligible(self):
        self.run_installer(
            self.package_fixture(
                ("niri", "waybar"), repository=("niri", "waybar"), installed=("niri", "waybar")
            )
            + "install_packages"
        )
        self.assertIn("pacman -S --needed --noconfirm niri waybar", self.commands())
        self.assertNotIn("ensure_yay", self.commands())
        self.assertEqual(self.commands()[-1], "pacman -T niri waybar")

    def test_only_missing_foreign_packages_are_exact_aur_targets(self):
        self.run_installer(
            self.package_fixture(
                ("niri", "bemoji", "wlogout", "cloudflare-warp-bin", "fzf-tab", "brave-bin"),
                repository=("niri",),
                installed=("niri", "bemoji", "wlogout", "cloudflare-warp-bin"),
            )
            + "install_packages"
        )
        commands = self.commands()
        self.assertIn("pacman -S --needed --noconfirm niri", commands)
        self.assertEqual(
            [command for command in commands if command.startswith("yay ")],
            [
                "yay -Si --aur aur/fzf-tab aur/brave-bin",
                "yay -S --aur --needed --noconfirm=false --confirm aur/fzf-tab aur/brave-bin",
            ],
        )
        self.assertEqual(
            commands[-1], "pacman -T niri bemoji wlogout cloudflare-warp-bin fzf-tab brave-bin"
        )

    def test_failed_aur_preflight_stops_before_repository_installation(self):
        result = self.run_installer(
            self.package_fixture(("niri", "bemoji"), repository=("niri",))
            + "MOCK_YAY_QUERY_STATUS=1; install_packages; printf 'unexpected next phase\\n'",
            succeeds=False,
        )
        commands = self.commands()
        self.assertIn("yay -Si --aur aur/bemoji", commands)
        self.assertFalse(any(command.startswith("pacman -S ") for command in commands))
        self.assertFalse(any(command.startswith("yay -S ") for command in commands))
        self.assertNotIn("unexpected next phase", result.stdout)

    def test_missing_aur_without_terminal_stops_before_any_installation(self):
        result = self.run_installer(
            self.package_fixture(("niri", "bemoji"), repository=("niri",))
            + "MOCK_HAS_TTY=0; install_packages; printf 'unexpected next phase\\n'",
            succeeds=False,
        )
        commands = self.commands()
        self.assertIn("Missing AUR packages require interactive input", result.stdout)
        self.assertFalse(any(command.startswith("pacman -S ") for command in commands))
        self.assertFalse(any(command.startswith("yay ") for command in commands))
        self.assertNotIn("unexpected next phase", result.stdout)

    def test_aur_preflight_eof_stops_before_repository_installation(self):
        result = self.run_installer(
            self.package_fixture(("niri", "wlogout"), repository=("niri",))
            + r"""
MOCK_YAY_QUERY_OUTPUT='request failed: Get https://aur.archlinux.org/rpc: EOF'
MOCK_YAY_QUERY_STATUS=1
install_packages
printf 'unexpected next phase\n'
""",
            succeeds=False,
        )
        commands = self.commands()
        self.assertIn("yay -Si --aur aur/wlogout", commands)
        self.assertFalse(any(command.startswith("pacman -S ") for command in commands))
        self.assertFalse(any(command.startswith("yay -S ") for command in commands))
        self.assertIn("request failed: Get https://aur.archlinux.org/rpc: EOF", result.stdout)
        self.assertNotIn("unexpected next phase", result.stdout)

    def test_failed_aur_helper_stops_installation(self):
        result = self.run_installer(
            self.package_fixture(("bemoji",))
            + "MOCK_YAY_STATUS=42; install_packages; printf 'unexpected next phase\\n'",
            succeeds=False,
        )
        self.assertNotIn("unexpected next phase", result.stdout)
        self.assertNotEqual(self.commands()[-1], "pacman -T bemoji")

    def test_successful_helper_with_unsatisfied_targets_stops_installation(self):
        result = self.run_installer(
            self.package_fixture(("bemoji",))
            + "MOCK_YAY_INSTALLS=0; install_packages; printf 'unexpected next phase\\n'",
            succeeds=False,
        )
        self.assertIn("Required packages remain unsatisfied", result.stdout)
        self.assertNotIn("unexpected next phase", result.stdout)
        self.assertEqual(self.commands()[-1], "pacman -T bemoji")

    def test_aur_failure_does_not_reuse_official_package_mirror_advice(self):
        result = self.run_installer(
            self.package_fixture(
                ("xwayland-satellite", "bemoji"), repository=("xwayland-satellite",)
            )
            + r"""
print_warn() { printf '%s\n' "$*"; }
MOCK_PACMAN_OUTPUT='warning: xwayland-satellite is up to date -- skipping'
MOCK_YAY_OUTPUT='error: AUR helper failed'
MOCK_YAY_STATUS=1
install_packages
""",
            succeeds=False,
        )
        self.assertIn("AUR helper failed", result.stdout)
        self.assertNotIn("refresh manually", result.stdout)
        self.assertNotIn("official Arch extra package", result.stdout)
        self.assertNotIn("sudo reflector", result.stdout)
        self.assertNotIn("sudo pacman -Syu", result.stdout)

    def test_official_download_failure_keeps_mirror_recovery_advice(self):
        result = self.run_installer(
            self.package_fixture(("xwayland-satellite",), repository=("xwayland-satellite",))
            + r"""
print_warn() { printf '%s\n' "$*"; }
sudo() {
    printf '%s\n' "$*" >> "$INSTALL_TEST_LOG"
    printf 'error: failed retrieving file xwayland-satellite.pkg.tar.zst: 404 Not Found\n'
    return 1
}
install_packages
""",
            succeeds=False,
        )
        self.assertIn("official Arch extra package", result.stdout)
        self.assertIn("sudo reflector", result.stdout)
        self.assertIn("sudo pacman -Syu", result.stdout)

    def test_wallpaper_selection_handles_large_collection_and_preserves_files(self):
        fixture = self.root / "configs"
        for directory in ("niri", "waybar", "scripts", "alacritty", "fuzzel", "mako", "fastfetch", "wlogout", "hypr", "wallpapers"):
            (fixture / directory).mkdir(parents=True)
        (fixture / "scripts/dummy.sh").write_text("#!/bin/bash\nexit 0\n")
        (fixture / "wallpapers/00000 wallpaper.jpg").write_text("repository wallpaper")
        wallpapers = self.user_home / "pictures/wallpapers"
        wallpapers.mkdir(parents=True)
        for index in range(5000):
            (wallpapers / f"{index:05d} wallpaper.jpg").touch()
        first = wallpapers / "00000 wallpaper.jpg"
        first.write_text("user wallpaper")
        self.run_installer(f"SCRIPT_DIR={shlex.quote(str(fixture))}; copy_configs")
        self.assertEqual(first.read_text(), "user wallpaper")
        self.assertEqual((self.user_home / ".local/state/niri/current_wallpaper").resolve(), first)
        self.assertTrue((self.user_home / ".config/niri/outputs.kdl").exists())

    def test_reinstall_keeps_saved_monitor_settings_and_moves_drive_links(self):
        fixture = self.root / "configs"
        for directory in ("niri", "waybar", "scripts", "alacritty", "fuzzel", "mako", "fastfetch", "wlogout", "hypr"):
            (fixture / directory).mkdir(parents=True)
        (fixture / "scripts/dummy.sh").write_text("#!/bin/bash\nexit 0\n")
        (fixture / "niri/config.kdl").write_text("repository config\n")
        saved = 'output "Dell Inc. DELL U2723QE ABC123" {\n    mode "2560x1440@165"\n}\n'
        niri = self.user_home / ".config/niri"
        niri.mkdir(parents=True)
        (niri / "outputs.kdl").write_text(saved)
        (niri / "config.kdl").write_text("old config\n")
        legacy_drives = self.user_home / "Drives"
        legacy_drives.mkdir()
        (legacy_drives / "Windows-SSD").symlink_to("/run/media/user/Windows-SSD")
        self.run_installer(f"SCRIPT_DIR={shlex.quote(str(fixture))}; copy_configs")
        self.assertEqual((niri / "outputs.kdl").read_text(), saved)
        self.assertEqual((niri / "config.kdl").read_text(), "repository config\n")
        self.assertFalse(legacy_drives.exists())
        self.assertTrue((self.user_home / "drives").is_dir())

    def test_generated_theme_does_not_modify_symlink_targets(self):
        original = self.root / "original-theme"
        original.mkdir()
        (original / "settings.ini").write_text("original settings")
        (self.user_home / ".config").mkdir()
        (self.user_home / ".config/gtk-3.0").symlink_to(original)
        self.run_installer("papirus-folders() { return 0; }; dconf() { return 0; }; setup_gtk")
        self.assertEqual((original / "settings.ini").read_text(), "original settings")
        self.assertFalse((self.user_home / ".config/gtk-3.0").is_symlink())
        self.assertIn("Adwaita-dark", (self.user_home / ".config/gtk-3.0/settings.ini").read_text())

    def test_firewall_setup_retains_existing_rules(self):
        self.run_installer("""
ufw() { :; }
sudo() { printf '%s\n' "$*" >> "$INSTALL_TEST_LOG"; return 0; }
setup_firewall
""")
        self.assertFalse(any("reset" in command for command in self.commands()))
        self.assertIn("ufw --force enable", self.commands())

    def test_warp_mode_failure_prevents_connection(self):
        self.run_installer("""
systemctl() { return 0; }
sudo() { printf '%s\n' "$*" >> "$INSTALL_TEST_LOG"; return 0; }
warp-cli() {
    printf 'warp-cli %s\n' "$*" >> "$INSTALL_TEST_LOG"
    [[ $* != '--accept-tos mode doh' ]]
}
setup_cloudflare
""", input_text="1\n")
        self.assertNotIn("warp-cli --accept-tos connect", self.commands())
        self.assertTrue(any("enable --now warp-svc" in command for command in self.commands()))

    def test_warp_registration_failure_continues_installation_with_diagnostic(self):
        result = self.run_installer(r"""
print_warn() { printf '%s\n' "$*"; }
sudo() { printf '%s\n' "$*" >> "$INSTALL_TEST_LOG"; return 0; }
warp-cli() {
    printf 'warp-cli %s\n' "$*" >> "$INSTALL_TEST_LOG"
    case "$*" in
        '--accept-tos registration show') return 1 ;;
        '--accept-tos registration new')
            printf '%s\n' 'Registration request failed: network unavailable' >&2
            return 7 ;;
    esac
    return 0
}
following_phase() { printf '%s\n' 'following phase reached'; }
run_phase setup_cloudflare
run_phase following_phase
""")
        self.assertIn("Registration request failed: network unavailable", result.stdout)
        self.assertIn("continuing installation", result.stdout)
        self.assertIn("following phase reached", result.stdout)
        self.assertNotIn("Installation failed", result.stdout)
        self.assertIn("warp-cli --accept-tos registration new", self.commands())
        self.assertFalse(any(
            " mode " in command or command.endswith((" connect", " disconnect"))
            for command in self.commands()
        ))

    def test_warp_waits_for_daemon_readiness_before_registration(self):
        self.run_installer(r"""
sudo() { printf '%s\n' "$*" >> "$INSTALL_TEST_LOG"; return 0; }
warp-cli() {
    printf 'warp-cli %s\n' "$*" >> "$INSTALL_TEST_LOG"
    case "$*" in
        '--accept-tos status')
            local attempts=0
            if [ -f "$INSTALL_TEST_LOG.status-count" ]; then
                read -r attempts < "$INSTALL_TEST_LOG.status-count"
            fi
            attempts=$((attempts + 1))
            printf '%s\n' "$attempts" > "$INSTALL_TEST_LOG.status-count"
            if [ "$attempts" -lt 3 ]; then
                printf '%s\n' 'Daemon socket is not ready' >&2
                return 1
            fi
            ;;
        '--accept-tos registration show') return 1 ;;
    esac
    return 0
}
setup_cloudflare
""", input_text="3\n")
        commands = self.commands()
        self.assertEqual(commands.count("warp-cli --accept-tos status"), 3)
        self.assertEqual(commands[-2:], [
            "warp-cli --accept-tos registration show",
            "warp-cli --accept-tos registration new",
        ])
        self.assertFalse(any(command.endswith((" connect", " disconnect")) for command in commands))

    def test_warp_unready_daemon_continues_without_registration_or_connection(self):
        result = self.run_installer(r"""
print_warn() { printf '%s\n' "$*"; }
sudo() { printf '%s\n' "$*" >> "$INSTALL_TEST_LOG"; return 0; }
warp-cli() {
    printf 'warp-cli %s\n' "$*" >> "$INSTALL_TEST_LOG"
    printf '%s\n' 'Daemon socket is unavailable' >&2
    return 1
}
following_phase() { printf '%s\n' 'following phase reached'; }
run_phase setup_cloudflare
run_phase following_phase
""")
        self.assertIn("Daemon socket is unavailable", result.stdout)
        self.assertIn("continuing installation", result.stdout)
        self.assertIn("following phase reached", result.stdout)
        commands = self.commands()
        self.assertEqual(commands.count("warp-cli --accept-tos status"), 10)
        self.assertFalse(any(
            " registration " in command or " mode " in command
            or command.endswith((" connect", " disconnect"))
            for command in commands
        ))

    def test_warp_service_failure_continues_without_cli_requests(self):
        result = self.run_installer(r"""
print_warn() { printf '%s\n' "$*"; }
sudo() {
    printf '%s\n' "$*" >> "$INSTALL_TEST_LOG"
    printf '%s\n' 'warp-svc could not be started' >&2
    return 1
}
warp-cli() { printf 'warp-cli %s\n' "$*" >> "$INSTALL_TEST_LOG"; return 0; }
following_phase() { printf '%s\n' 'following phase reached'; }
run_phase setup_cloudflare
run_phase following_phase
""")
        self.assertIn("warp-svc could not be started", result.stdout)
        self.assertIn("continuing installation", result.stdout)
        self.assertIn("following phase reached", result.stdout)
        self.assertEqual(self.commands(), ["systemctl enable --now warp-svc"])

    def test_warp_connection_failure_continues_installation_with_diagnostic(self):
        result = self.run_installer(r"""
print_warn() { printf '%s\n' "$*"; }
sudo() { printf '%s\n' "$*" >> "$INSTALL_TEST_LOG"; return 0; }
warp-cli() {
    printf 'warp-cli %s\n' "$*" >> "$INSTALL_TEST_LOG"
    if [[ $* == '--accept-tos connect' ]]; then
        printf '%s\n' 'Connection failed: tunnel unavailable' >&2
        return 2
    fi
    return 0
}
following_phase() { printf '%s\n' 'following phase reached'; }
run_phase setup_cloudflare
run_phase following_phase
""", input_text="1\n")
        self.assertIn("Connection failed: tunnel unavailable", result.stdout)
        self.assertIn("continuing installation", result.stdout)
        self.assertIn("following phase reached", result.stdout)
        self.assertIn("warp-cli --accept-tos mode doh", self.commands())
        self.assertIn("warp-cli --accept-tos connect", self.commands())
        self.assertNotIn("warp-cli --accept-tos disconnect", self.commands())

    def test_warp_skip_keeps_current_connection(self):
        self.run_installer("""
systemctl() { return 0; }
sudo() { printf '%s\n' "$*" >> "$INSTALL_TEST_LOG"; return 0; }
warp-cli() { printf 'warp-cli %s\n' "$*" >> "$INSTALL_TEST_LOG"; return 0; }
setup_cloudflare
""", input_text="3\n")
        self.assertFalse(any(command.endswith((" connect", " disconnect")) for command in self.commands()))

    def test_warp_registers_when_status_succeeds_but_registration_is_missing(self):
        self.run_installer("""
sudo() { printf '%s\n' "$*" >> "$INSTALL_TEST_LOG"; return 0; }
warp-cli() {
    printf 'warp-cli %s\n' "$*" >> "$INSTALL_TEST_LOG"
    [[ $* != '--accept-tos registration show' ]]
}
setup_cloudflare
""", input_text="1\n")
        commands = self.commands()
        self.assertIn("warp-cli --accept-tos registration new", commands)
        self.assertLess(
            commands.index("warp-cli --accept-tos registration new"),
            commands.index("warp-cli --accept-tos connect"),
        )

    def test_warp_retains_existing_registration(self):
        self.run_installer("""
sudo() { printf '%s\n' "$*" >> "$INSTALL_TEST_LOG"; return 0; }
warp-cli() { printf 'warp-cli %s\n' "$*" >> "$INSTALL_TEST_LOG"; return 0; }
setup_cloudflare
""", input_text="1\n")
        self.assertIn("warp-cli --accept-tos registration show", self.commands())
        self.assertNotIn("warp-cli --accept-tos registration new", self.commands())

    def test_failed_required_service_propagates_failure(self):
        self.run_installer("""
systemctl() { printf 'NetworkManager.service disabled\n'; }
enable_system_service_now NetworkManager.service
""", succeeds=False)

    def test_stealth_stages_only_runtime_payload_and_replaces_stale_files(self):
        destination = self.user_home / ".local/share/stealth"
        destination.mkdir(parents=True)
        (destination / "stale-test.sh").write_text("stale")
        self.run_installer("""
sudo() { printf '%s\n' "$*" >> "$INSTALL_TEST_LOG"; return 0; }
setup_stealth
""")
        self.assertEqual(
            {entry.name for entry in destination.iterdir()},
            {"stealth.zsh", "install-stealth.sh", "stealth.sh", "stealth.service", "stealth.nft", "stealth.sudoers", "torrc.conf"},
        )

    def test_failed_stealth_install_retains_previous_staging(self):
        destination = self.user_home / ".local/share/stealth"
        destination.mkdir(parents=True)
        (destination / "existing").write_text("keep me")
        self.run_installer("""
sudo() { [[ $1 != bash ]]; }
setup_stealth
""", succeeds=False)
        self.assertEqual((destination / "existing").read_text(), "keep me")

    def test_main_stops_at_failed_phase(self):
        result = self.run_installer("""
preflight() { return 1; }
refresh_mirrors() { printf 'unexpected phase\n'; }
main
""", succeeds=False)
        self.assertEqual(self.commands(), [])
        self.assertNotIn("unexpected phase", result.stdout)

    def test_help_and_invalid_arguments_do_not_start_installation(self):
        self.run_installer("main --help")
        self.run_installer("main --unknown", succeeds=False)
        self.assertEqual(self.commands(), [])

    def test_concurrent_installer_stops_before_mutation(self):
        with (self.runtime / "hype-niri-install.lock").open("w") as lock:
            fcntl.flock(lock, fcntl.LOCK_EX)
            result = self.run_installer("""
preflight() { printf 'unexpected mutation\n'; }
main
""", succeeds=False)
        self.assertIn("Another Hype Niri installation is already running", result.stdout)
        self.assertNotIn("unexpected mutation", result.stdout)
        self.assertEqual(self.commands(), [])

    def test_stale_audio_service_is_removed_without_touching_its_source(self):
        external = self.root / "legacy.service"
        external.write_text("[Service]\nExecStart=%h/.config/waybar/scripts/audio-jack-switch.sh\n")
        unit = self.user_home / ".config/systemd/user/audio-jack-switch.service"
        unit.parent.mkdir(parents=True)
        unit.symlink_to(external)
        self.run_installer("""
systemctl() { printf '%s\n' "$*" >> "$INSTALL_TEST_LOG"; return 0; }
remove_stale_audio_service
""")
        self.assertFalse(unit.is_symlink())
        self.assertTrue(external.exists())
        self.assertIn("--user disable --now audio-jack-switch.service", self.commands())

    def test_working_audio_service_is_retained(self):
        unit = self.user_home / ".config/systemd/user/audio-jack-switch.service"
        unit.parent.mkdir(parents=True)
        unit.write_text("ExecStart=%h/.config/waybar/scripts/audio-jack-switch.sh\n")
        script = self.user_home / ".config/waybar/scripts/audio-jack-switch.sh"
        script.parent.mkdir(parents=True)
        script.write_text("#!/bin/sh\n")
        script.chmod(0o755)
        self.run_installer("remove_stale_audio_service")
        self.assertTrue(unit.exists())
        self.assertEqual(self.commands(), [])

    def test_existing_nonexecutable_audio_script_is_not_removed(self):
        unit = self.user_home / ".config/systemd/user/audio-jack-switch.service"
        unit.parent.mkdir(parents=True)
        unit.write_text("ExecStart=%h/.config/waybar/scripts/audio-jack-switch.sh\n")
        script = self.user_home / ".config/waybar/scripts/audio-jack-switch.sh"
        script.parent.mkdir(parents=True)
        script.write_text("#!/bin/sh\n")
        self.run_installer("remove_stale_audio_service")
        self.assertTrue(unit.exists())
        self.assertEqual(self.commands(), [])

    def test_audio_service_with_another_command_is_retained(self):
        unit = self.user_home / ".config/systemd/user/audio-jack-switch.service"
        unit.parent.mkdir(parents=True)
        unit.write_text("ExecStart=%h/xconfig/waybar/scripts/audio-jack-switchXsh\n")
        self.run_installer("remove_stale_audio_service")
        self.assertTrue(unit.exists())
        self.assertEqual(self.commands(), [])

    def test_desktop_integrations_preserve_external_symlink_targets(self):
        external = self.root / "external-systemd"
        (external / "user").mkdir(parents=True)
        existing = external / "user/hype-auto-power-profile.service"
        existing.write_text("original service\n")
        config = self.user_home / ".config"
        config.mkdir()
        (config / "systemd").symlink_to(external, target_is_directory=True)
        self.run_installer("""
configure_file_indexing() { :; }
systemctl() { printf '%s\n' "$*" >> "$INSTALL_TEST_LOG"; return 0; }
setup_desktop_integrations
""")
        self.assertEqual(existing.read_text(), "original service\n")
        self.assertFalse((config / "systemd").is_symlink())
        self.assertEqual(
            (config / "systemd/user/hype-auto-power-profile.service").read_bytes(),
            (REPO / "systemd/user/hype-auto-power-profile.service").read_bytes(),
        )
        self.assertEqual(
            (config / "wireplumber/wireplumber.conf.d/60-hype-niri-audio.conf").read_bytes(),
            (REPO / "wireplumber/wireplumber.conf.d/60-hype-niri-audio.conf").read_bytes(),
        )
        self.assertIn("--user daemon-reload", self.commands())

    def test_power_service_is_enabled_without_a_running_user_manager(self):
        self.run_installer("""
configure_file_indexing() { :; }
setup_desktop_integrations
""")
        link = self.user_home / ".config/systemd/user/graphical-session.target.wants/hype-auto-power-profile.service"
        self.assertTrue(link.is_symlink())
        self.assertEqual(link.resolve().read_bytes(), (REPO / "systemd/user/hype-auto-power-profile.service").read_bytes())
        self.assertEqual(self.commands(), [])

    def test_brave_becomes_default_browser_without_dropping_other_defaults(self):
        config = self.user_home / ".config"
        config.mkdir()
        (config / "mimeapps.list").write_text(
            "[Default Applications]\n"
            "x-scheme-handler/claude-cli=claude-code-url-handler.desktop\n"
            "text/html=firefox.desktop\n"
            "\n"
            "[Added Associations]\n"
            "image/png=feh.desktop;\n"
        )
        self.run_installer("set_default_browser\nset_default_browser")
        lines = (config / "mimeapps.list").read_text().splitlines()
        self.assertIn("x-scheme-handler/claude-cli=claude-code-url-handler.desktop", lines)
        self.assertIn("image/png=feh.desktop;", lines)
        self.assertNotIn("text/html=firefox.desktop", lines)
        for mime in ("text/html", "x-scheme-handler/http", "x-scheme-handler/https"):
            self.assertEqual(lines.count(f"{mime}=brave-browser.desktop;"), 1, mime)
        self.assertLess(
            lines.index("x-scheme-handler/https=brave-browser.desktop;"),
            lines.index("[Added Associations]"),
        )

    def test_default_browser_creates_mimeapps_list(self):
        self.run_installer("set_default_browser")
        lines = (self.user_home / ".config/mimeapps.list").read_text().splitlines()
        self.assertEqual(lines[0], "[Default Applications]")
        self.assertIn("x-scheme-handler/https=brave-browser.desktop;", lines)

    def test_user_dirs_are_lowercase_and_adopt_legacy_folders(self):
        legacy = self.user_home / "Pictures/Wallpapers"
        legacy.mkdir(parents=True)
        (legacy / "kept.jpg").write_text("wallpaper")
        (self.user_home / "Music").mkdir()
        (self.user_home / "Videos").mkdir()
        (self.user_home / "Videos/clip.mp4").write_text("clip")
        (self.user_home / "videos").mkdir()
        self.run_installer("setup_user_dirs")
        self.assertEqual((self.user_home / "pictures/Wallpapers/kept.jpg").read_text(), "wallpaper")
        self.assertFalse((self.user_home / "Pictures").exists())
        self.assertFalse((self.user_home / "Music").exists())
        self.assertTrue((self.user_home / "Videos/clip.mp4").exists())
        for directory in ("desktop", "documents", "downloads", "music", "projects", "public", "templates"):
            self.assertTrue((self.user_home / directory).is_dir(), directory)
        self.assertIn(f"xdg-user-dirs-update --set PICTURES {self.user_home}/pictures", self.commands())
        self.assertIn(f"xdg-user-dirs-update --set DOWNLOAD {self.user_home}/downloads", self.commands())

    def test_user_dirs_never_move_external_configured_paths(self):
        external = self.root / "external-pictures"
        external.mkdir()
        self.run_installer(f"""
xdg-user-dir() {{ printf '%s\\n' {shlex.quote(str(external))}; }}
setup_user_dirs
""")
        self.assertTrue(external.is_dir())
        self.assertTrue((self.user_home / "pictures").is_dir())

    def test_blueman_launches_use_escape_wrapper(self):
        desktop = self.root / "blueman-manager.desktop"
        desktop.write_text("[Desktop Entry]\nName=Bluetooth Manager\nExec=blueman-manager\nIcon=blueman\n")
        self.source.write_text(
            self.source.read_text().replace("/usr/share/applications/blueman-manager.desktop", str(desktop))
        )
        self.run_installer("""
configure_file_indexing() { :; }
setup_desktop_integrations
""")
        entry = (self.user_home / ".local/share/applications/blueman-manager.desktop").read_text()
        self.assertIn(f'Exec=/usr/bin/python3 "{self.user_home}/.config/scripts/blueman-manager.py"', entry)
        self.assertIn("Icon=blueman", entry)
        self.assertEqual(
            (self.user_home / ".config/systemd/user/blueman-manager.service.d/hype-niri.conf").read_bytes(),
            (REPO / "systemd/user/blueman-manager.service.d/hype-niri.conf").read_bytes(),
        )

    def docker_fixture(self, containers="", *, query_status=0):
        return f"""
enable_system_service_now() {{ printf 'enable %s\\n' "$*" >> "$INSTALL_TEST_LOG"; }}
systemctl() {{ return 0; }}
docker() {{
    printf 'docker %s\\n' "$*" >> "$INSTALL_TEST_LOG"
    [[ -z ${{DOCKER_HOST:-}} && -z ${{DOCKER_CONTEXT:-}} ]] || return 99
    printf '%s\\n' {shlex.quote(containers)}
    return {query_status}
}}
sudo() {{ printf '%s\\n' "$*" >> "$INSTALL_TEST_LOG"; return 0; }}
DOCKER_HOST=tcp://remote.invalid:2375
DOCKER_CONTEXT=remote
configure_docker_socket
"""

    def test_idle_docker_switches_to_socket_activation(self):
        self.run_installer(self.docker_fixture())
        self.assertEqual(self.commands(), ["enable docker.socket", "docker --host unix:///run/docker.sock ps --quiet", "systemctl disable --now docker.service"])

    def test_running_docker_containers_are_preserved(self):
        self.run_installer(self.docker_fixture("running-container"))
        self.assertEqual(self.commands(), ["enable docker.socket", "docker --host unix:///run/docker.sock ps --quiet"])

    def test_failed_docker_query_preserves_existing_service(self):
        self.run_installer(self.docker_fixture(query_status=1))
        self.assertEqual(self.commands(), ["enable docker.socket", "docker --host unix:///run/docker.sock ps --quiet"])

    def test_ly_enable_failure_preserves_previous_display_manager(self):
        self.run_installer("""
systemctl() { printf 'ly@.service disabled\nsddm.service enabled\n'; }
sudo() {
    printf '%s\n' "$*" >> "$INSTALL_TEST_LOG"
    [[ $* != 'systemctl enable --force ly@tty2.service' ]]
}
setup_system
""", succeeds=False)
        self.assertIn("systemctl enable --force ly@tty2.service", self.commands())
        self.assertFalse(any("systemctl disable" in command for command in self.commands()))

    def test_ly_template_is_enabled_before_previous_manager_is_disabled(self):
        self.run_installer("""
systemctl() {
    case "$*" in
        'list-unit-files --type=service --no-legend') printf 'ly@.service disabled\nsddm.service enabled\n' ;;
        'list-unit-files '*) printf '%s enabled\n' "$2" ;;
        '--user show-environment') return 1 ;;
        *) return 0 ;;
    esac
}
sudo() { printf '%s\n' "$*" >> "$INSTALL_TEST_LOG"; return 0; }
bash() { return 0; }
docker() { return 0; }
setup_system
""")
        commands = self.commands()
        self.assertLess(
            commands.index("systemctl enable --force ly@tty2.service"),
            commands.index("systemctl disable sddm.service"),
        )
        self.assertIn("systemctl disable getty@tty2.service", commands)
        self.assertFalse(any("start ly" in command for command in commands))


if __name__ == "__main__":
    unittest.main(verbosity=2)
