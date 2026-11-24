# Hype Niri

> A clean, professional, highly functional Wayland setup based on the Niri scrollable-tiling compositor and a minimalist Monochrome theme.

![Niri](https://img.shields.io/badge/Niri-Wayland-b0b0b0?style=for-the-badge&logo=wayland&logoColor=white)
![Arch Linux](https://img.shields.io/badge/Arch_Linux-888888?style=for-the-badge&logo=archlinux&logoColor=white)
![Monochrome Theme](https://img.shields.io/badge/Theme-Monochrome-141414?style=for-the-badge)

<br/>

## Overview

Personal dotfiles for a minimal Arch Linux setup using the **Niri** scrollable tiling Wayland compositor. Focused on delivering a premium, smooth, and full-featured desktop experience using native Wayland tools.

## Tech Stack

| Component             | Tool / Link                                                        |
| :-------------------- | :----------------------------------------------------------------- |
| **Compositor**        | [Niri](https://github.com/YaLTeR/niri)                             |
| **Bar**               | [Waybar](https://github.com/Alexays/Waybar)                        |
| **Terminal**          | [Alacritty](https://github.com/alacritty/alacritty)                |
| **Shell**             | [Zsh](https://www.zsh.org/) (Powerlevel10k optional)               |
| **Text Font**         | [JetBrains Mono](https://github.com/JetBrains/JetBrainsMono)       |
| **Waybar Font**       | Roboto with Material Design Icons                                  |
| **Launcher**          | Native Rust GTK4 application picker                                |
| **Notifications**     | [Mako](https://github.com/emersion/mako)                           |
| **Lock Screen**       | [Hyprlock](https://github.com/hyprwm/hyprlock)                     |
| **Idle Daemon**       | [Hypridle](https://github.com/hyprwm/hypridle)                     |
| **Memory Protection** | [systemd-oomd](INSTALL.md#memory-pressure-protection-systemd-oomd) |
| **Wallpaper**         | [Awww](https://codeberg.org/LGFae/awww)                            |
| **Editor**            | Optional; system default editor                                    |
| **Recording**         | Optional [OBS Studio](https://obsproject.com/)                     |
| **Login Manager**     | Existing manager; [Ly](https://github.com/fairyglade/ly) optional  |
| **Fetch App**         | Optional [Fastfetch](https://github.com/fastfetch-cli/fastfetch)   |

<br/>

## Installation

For an interactive installation, clone the repository and run the setup script:

```bash
curl -fsSL https://raw.githubusercontent.com/Dijo-404/hype_niri/main/install.sh | bash
```

The one-command installer becomes available when this branch reaches `main`. It downloads source into a temporary directory, builds the GTK-independent Rust installer locally, then runs it with terminal input. Local builds take longer on the first run. If Git, Rust or compilation tools are missing, the bootstrap asks before a full Arch upgrade installs them. Run as your regular user with sudo access. yay is needed only for missing selected AUR packages. Existing settings are backed up before replacement.

Answer `n` to **Install optional extras?** for the core desktop. Answer `y` or `yes` and enter group names such as `browser-firefox editor-neovim bluetooth` to install and set up those extras. Docker, VPNs, Bluetooth, browsers, editors, recording tools, extra themes, compatibility tools and hardware-specific drivers are not installed by default. See [the dependency and service ledger](DEPENDENCIES.md) and [optional-packages.txt](optional-packages.txt). Existing packages are never automatically uninstalled.

Select `hype-flow` for offline dictation on x86-64. The installer builds Hype Flow, downloads and verifies the English Parakeet model, and configures Niri shortcuts. The app runs in the background from login. The speech worker loads on first dictation and unloads after 10 idle minutes by default.

To install from a checked-out branch:

```bash
git clone https://github.com/Dijo-404/hype_niri.git
cd hype_niri
./install.sh
```

> [!NOTE]
> See [INSTALL.md](INSTALL.md) for manual setup instructions and post-installation steps.

<br/>

## Updating

To pull the latest config changes and apply them:

```bash
cd Hype_Niri
git pull
./install.sh
```

The installer optionally refreshes Arch mirrors, requires a full system upgrade before installing packages, and backs up existing user configurations before replacing them. It uses `pacman` for official repository packages and `yay` for missing AUR packages, retaining installed AUR packages and providers. It checks exact missing AUR package names before installing; a failed lookup stops the package phase. Missing AUR packages require a terminal so you can review provider and conflict choices. Run it as your regular user with sudo access. Install `yay` with your preferred method only if selected missing packages require the AUR.

To update only packages without re-running the full script:

```bash
sudo pacman -Syu
yay -Syu --aur
```

Power profiles switch automatically: Performance on AC, Balanced on battery at 30% or above, and Power Saver below 30%. Manual selections last until the next charger or threshold change. The controller uses D-Bus events and stops at logout; hardware without Performance support uses Balanced.

Selecting Docker enables socket activation; it is absent from the core install. Idle audio devices suspend after five seconds, and file indexing skips dependency/cache directories. Native controls use locks and atomic writes to prevent duplicate processes and partial configurations.

`Super+Shift+W` opens a wallpaper picker with cached image previews. Niri animations use `ease-out-quad` over 180 ms without overshoot. New windows slide down from their top edge; closing windows slide back into that edge without fading. Resizing shows new contents immediately. Gesture motion follows your fingers, with the same settling duration after release. Hyprlock fades over 180 ms; the lock before sleep stays instant. Wallpaper changes grow from the centre over 180 ms. Native GTK pickers open as floating windows under Niri. See [Niri animations](https://niri-wm.github.io/niri/Configuration%3A-Animations.html).

Click the hotspot icon beside the tray to start or stop sharing. Right-click it, or press `Super+Shift+H`, for credentials, **Change password**, and connection settings. Password entry is masked and accepts 8–63 ASCII characters or 64 hexadecimal digits (raw keys support WPA2 only). Changing an active hotspot's password restarts that hotspot; connected devices must reconnect. The first start creates a hotspot named `Hype-Niri` with a random password. It prefers 5 GHz on channel 36, uses 2.4 GHz on adapters without 5 GHz, and retries on 2.4 GHz if 5 GHz activation fails. The saved profile keeps the working band for later starts. Each activation attempt allows 35 seconds for NetworkManager to finish; busy actions show a notification. Security uses WPA2 Personal with RSN, AES/CCMP, and optional protected management frames. Starting an older saved hotspot upgrades these settings without changing its password and restarts it if needed. Starting it may disconnect Wi-Fi on the same adapter. Internet sharing requires an upstream connection, such as Ethernet, USB tethering, or another Wi-Fi adapter. The installer includes `dnsmasq` and configures DHCP/DNS access on hotspot-capable Wi-Fi interfaces when enabling UFW.

The application launcher, power menu, clipboard history, emoji picker, password prompts, display controls and other utility dialogs use native Rust GTK4. Press Enter to select, Escape to cancel, or activate a menu shortcut again to close it without choosing an action. The launcher discovers visible installed desktop applications through GIO. Browser/editor shortcuts use system defaults; missing optional applications produce a notification.

One native clipboard watcher captures supported text and image selections. History is private and bounded to 64 entries, 8 MiB per entry and 32 MiB total. Password-manager hints are excluded. `Super+V` opens history, and `hype-control clipboard clear` clears stored history. Previous cliphist databases remain untouched. Emoji selection types the complete Unicode sequence through Wayland's virtual keyboard protocol. Media keys call MPRIS directly.

Bluetooth and Cloudflare tray applets start only when their corresponding optional groups are selected. The core launches the NetworkManager applet. Optional color-picker support retains its own clipboard tool dependency; see the dependency ledger. WARP and Stealth indicators are added to Waybar only for their selected integrations.

Waybar volume scrolling filters small touchpad movements with a `1.0` smooth-scrolling threshold. Volume and brightness keys use 1% steps (rounded to hardware units for brightness), and brightness ramps between values. Volume changes never queue behind an active change, and notifications do not hold either control's input lock. Brightness stops at 1%; decreasing an already lower value leaves it unchanged.

The `media` group includes OBS Studio; `editor-neovim` installs Neovim and the [LazyVim starter](https://www.lazyvim.org/installation). Existing LazyVim settings are retained; other Neovim configurations are backed up before replacement. Open `nvim` to download plugins on first launch, then run `:LazyHealth`.

Selecting `bluetooth` configures Blueman to use its [built-in dialogs](https://github.com/blueman-project/blueman/wiki/Integrations#desktop-notifications) for Bluetooth notifications, including pairing PIN codes, passkeys and confirmation buttons. To pair a keyboard, open **Bluetooth Devices** from the tray, put the keyboard in pairing mode, search for it, select it and choose **Pair** (the key button). Type the displayed code on the Bluetooth keyboard and press **Enter**. Devices that request a code instead show an entry dialog; devices that use numeric comparison show confirmation buttons.

The native `hype-control prepare-sleep --watch` process holds a logind sleep delay inhibitor while it confirms the password locker and session lock are ready. Explicit suspend requests abort if that confirmation fails. Lid-close and other external requests remain subject to logind's five-second delay limit.

<br/>

## Repository Structure

```text
.
├── alacritty/       # Fast, GPU-accelerated terminal configuration
├── fastfetch/       # System information fetch utility with custom logo
├── hypr/            # Hypridle + Hyprlock configuration
├── mako/            # Notification daemon configuration
├── niri/            # Core Niri compositor settings and rules
├── polkit/          # Polkit rules for NetworkManager and drive access
├── wallpapers/      # Default curated wallpapers
├── native/          # Rust desktop controls and privileged Stealth backend
├── notch/           # Rust GTK4 notch companion
├── scripts/         # Stylesheet and installed native binary destination
├── systemd/         # Automatic power service and OOM protection
├── stealth/         # Optional transparent Tor service and installer
├── tests/           # Installer, runtime and power-policy regression checks
├── waybar/          # Comprehensive status bar configuration
├── wireplumber/     # Audio routing and idle suspension
├── zsh/             # Shell config, Stealth commands, and Powerlevel10k theme
├── install.sh       # Small source-build bootstrap
├── installer/       # Native Rust installer, configuration and system setup
├── pkglist.txt      # Core desktop/build package requirements
├── optional-packages.txt # Explicitly selected extras
└── *.md             # Documentation files
```

<br/>

## Documentation

- **[Keybindings Reference](keybindings.md)** - Learn how to navigate and manage windows
- **[Zsh Aliases](alias.md)** - Shell aliases
- **[Stealth Tor Routing](INSTALL.md#stealth-tor-routing)** - Optional Tor routing commands
- **[Installation Guide](INSTALL.md)** - Detailed setup instructions
- **[Dependencies and Services](DEPENDENCIES.md)** - Core requirements, optional groups and native replacements

## Native Desktop Controls

The installer builds `hype-control`, `hype-notch` and `hype-status` before replacing configurations. It builds the privileged `stealth` binary when installing the optional Stealth integration. Cargo artifacts are reused from `~/.cache/hype-niri/cargo-target`. Niri, Waybar, Hypridle and the power-profile service call `~/.config/scripts/hype-control` directly. Waybar also uses `hype-status` to stream temperature and hotspot status. Application backends such as Hyprlock, Hyprpicker, Nautilus and `awww-daemon` run as separate programs.

Display changes use Niri IPC and its own configuration decoder, pinned to Niri 26.04. Wallpaper rendering uses the native awww 0.12.1 protocol; keep these dependencies aligned when upgrading their installed backends. GTK4 dialogs provide masked password entry and cached wallpaper previews. The selected wallpaper remains an atomic symlink at `~/.local/state/niri/current_wallpaper` (or `$XDG_STATE_HOME/niri/current_wallpaper`).

```bash
cargo build --workspace --bins --release --locked
./target/release/hype-control --check
./target/release/hype-control auto-power-profile --check
```

The first check needs no display or desktop services. The power-profile check reads the system D-Bus state without changing it. The installer keeps temporary compatibility entry points while replacing configurations, then removes them after callers switch to the native binary.

## Niri Notch

A Finotch-inspired companion sits just below Waybar, centered on each display. At rest it is a small pill. Hovering the pill slides out the compact notch with the current song, timer, and temperature; moving the pointer within the compact notch after it opens reveals the full drawer. Leaving closes it after a short delay unless the search bar holds text. Click the header or press `Super+N` to pin it open. Press `Esc`, click the pinned header again, or focus another window to collapse it. Hovering does not take keyboard focus.

The drawer uses Waybar's translucent black surface, frosted pills and light active state, animates over 150 ms, and provides:

- A quick search bar. Typing lists matching installed apps; Enter launches the highlighted one, and the last row searches DuckDuckGo in the default browser. Arrow keys move the highlight. `Super+N` focuses the search bar.
- Current weather and temperature from wttr.in, refreshed every 30 minutes through native HTTPS. The location comes from the public IP address, so WARP or Stealth routing shows the exit location.
- Volume and brightness sliders. They read the current levels each time the drawer opens and apply changes through the native PipeWire PulseAudio API (capped at 100%) and backlight/logind APIs while you drag. Brightness stops at 1%. A slider hides when its service or device is missing. Waybar keeps the microphone and speaker mute buttons, speaker volume scrolling, and right-click access to an installed audio mixer (`desktop-tools` includes `pavucontrol`).
- Live MPRIS music and video titles with previous, play/pause, and next controls. A playing source takes priority over a paused one. While something plays, a small animated wave replaces the music icon in the header and sits beside the track title; it stays still when GTK animations are off.
- A Pomodoro tab with a 25-minute focus timer and five-minute break. Completion sends a notification; start the next phase when ready. Pausing preserves remaining time. Reset returns to a fresh focus session.
- A Stopwatch tab with start, pause, and reset. The header shows whichever timer is in use.
- Optional Hype Flow offline dictation. The default shortcut `Alt+Space` starts or finishes recording, and `Super+Ctrl+Esc` cancels. A voice icon and live waveform replace the resting indicator while dictation is active. Click the capsule to finish or right-click to cancel. The drawer includes start/stop, cancel, copy-last, and settings controls.

The dictation connection uses session D-Bus actions and receives state and microphone levels, never transcript text. The capsule does not take keyboard focus. When Hype Flow stops, the notch restores its resting indicator. The existing weather and web-search features remain separate from offline dictation.

The notch is a Rust GTK4 layer-shell surface. Its surface keeps a fixed size and only the visible part accepts input, so the compositor never resizes it mid-animation. GTK4 asks Niri to blur the visible rounded notch and dictation capsule through `ext-background-effect-v1`, so text stays readable over busy windows. It reserves no additional workspace and uses native symbolic icons. Opening it can cover the top of an application. GTK's animation setting controls reduced motion. Media updates arrive through MPRIS D-Bus events. The timer runs only while this process is alive and does not prevent sleep. It pauses during system suspend.

The notch runs as one native Rust process with an embedded stylesheet. The installer builds it once with Cargo before replacing any configuration, then installs `~/.config/scripts/hype-notch`. Startup and `Super+N` call that binary directly. Weather, audio, brightness, media, dictation, and focused-output selection use native APIs; the notch does not invoke control commands or shell scripts. GTK4 manages each output's scale independently, including mixed-scale displays and monitor hotplug.

Build dependencies (`base-devel` and `rust`) and runtime libraries (`gtk4`, `gtk4-layer-shell`, and `libpulse`) are included in `pkglist.txt`. Niri starts the notch after installing this branch and logging in again. To preview directly from the repository without replacing live configuration:

```bash
cargo build --release --locked
./target/release/hype-notch --check
./target/release/hype-notch
# From another terminal:
./target/release/hype-notch --toggle
./target/release/hype-notch --quit
```

`--check` validates the embedded stylesheet without needing a display. Running the UI requires a Wayland compositor with layer-shell support. GTK4 4.24 or newer and Rust 1.92 or newer are required. Missing optional services do not prevent startup; brightness write failures appear in the drawer.

The GTK4 bindings and layer-shell extension follow the [gtk4-rs API](https://gtk-rs.org/gtk4-rs/stable/latest/docs/gtk4/) and [GTK4 layer-shell API](https://docs.rs/gtk4-layer-shell/0.8.1/gtk4_layer_shell/). Brightness fallback uses logind's [SetBrightness method](https://www.freedesktop.org/software/systemd/man/latest/org.freedesktop.login1.html).

The application uses one instance per session. Preview and installed copies share that instance; quit the preview before starting the installed version.

## Checks

Native regression checks use temporary files, socket fixtures and private D-Bus services. The Python native suite runs the actual `hype-control` binary in Bubblewrap namespaces, with no live network, device or desktop access; it requires `python-gobject`, `dbus` and `bubblewrap`. `HYPE_CONTROL_BINARY` selects a different built binary. Installer tests exercise Rust against temporary home/system roots and controlled external command boundaries. Launcher tests run the real Rust binary in private namespaces. The Rust notch suite covers clocks, weather, Niri replies, backlight discovery and rounded regions. Its optional UI test needs an enabled output and an isolated session bus; it exercises real GTK4 widgets, fake native MPRIS/Flow services, shared timers and view teardown:

```bash
python3 tests/test-bootstrap.py
cargo test --workspace --locked
cargo build --workspace --bins --locked
/usr/bin/python3 tests/test-runtime.py
/usr/bin/python3 tests/test-native-runtime.py
/usr/bin/python3 tests/test-warp-indicator.py
/usr/bin/python3 tests/test-native-media.py
/usr/bin/python3 tests/test-native-session.py
/usr/bin/python3 tests/test-native-gui.py
./target/debug/hype-notch --check
./target/debug/hype-control --check
./target/debug/hype-status --check
./target/debug/stealth version
# Run ignored GTK tests only with an isolated session bus and Wayland compositor:
cargo test --workspace --locked -- --ignored --test-threads=1
shellcheck install.sh
shfmt -l install.sh
cargo fmt --all --check
cargo clippy --workspace --locked --all-targets -- -D warnings
prettier --check . --ignore-unknown
ruff check tests
ruff format --check tests
```

Static checks require ShellCheck, shfmt, Prettier, and Ruff. Shell formatting
uses the four-space indentation in `.editorconfig`; `ruff.toml` also checks
import placement.

`install.sh` is the only tracked `.sh` file. Desktop launchers and menus use `hype-control`, and the temperature and hotspot streams use `hype-status`. Installation phases live in `hype-install`. Infrastructure applications and vendor tools such as Hyprlock, pacman, yay and optional warp-cli remain separate programs. Wofi, playerctl, cliphist, wl-clipboard, bemoji and wtype are optional or test-only tools. The bootstrap supports `--help`, and the Rust installer exposes named `--phase` operations for the manual steps in [INSTALL.md](INSTALL.md).

Installation retains four native compatibility symlinks for an already-running Waybar or a custom bar configuration. The temporary power-profile Bash launcher is removed after its replacement user unit has been published and reloaded.
