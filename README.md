# Hype Niri

> A clean, professional, highly functional Wayland setup based on the Niri scrollable-tiling compositor and a minimalist Monochrome theme.

![Niri](https://img.shields.io/badge/Niri-Wayland-b0b0b0?style=for-the-badge&logo=wayland&logoColor=white)
![Arch Linux](https://img.shields.io/badge/Arch_Linux-888888?style=for-the-badge&logo=archlinux&logoColor=white)
![Monochrome Theme](https://img.shields.io/badge/Theme-Monochrome-141414?style=for-the-badge)

<br/>

## Overview

Personal dotfiles for a minimal Arch Linux setup using the **Niri** scrollable tiling Wayland compositor. Focused on delivering a premium, smooth, and full-featured desktop experience using native Wayland tools.

## Tech Stack

| Component             | Tool / Link                                                                             |
| :-------------------- | :-------------------------------------------------------------------------------------- |
| **Compositor**        | [Niri](https://github.com/YaLTeR/niri)                                                  |
| **Bar**               | [Waybar](https://github.com/Alexays/Waybar)                                             |
| **Terminal**          | [Alacritty](https://github.com/alacritty/alacritty)                                     |
| **Shell**             | [Zsh](https://www.zsh.org/) + [Powerlevel10k](https://github.com/romkatv/powerlevel10k) |
| **Text Font**         | [JetBrains Mono](https://github.com/JetBrains/JetBrainsMono)                            |
| **Waybar Font**       | Roboto with Material Design Icons                                                       |
| **Launcher**          | [Wofi](https://hg.sr.ht/~scoopta/wofi)                                                  |
| **Notifications**     | [Mako](https://github.com/emersion/mako)                                                |
| **Lock Screen**       | [Hyprlock](https://github.com/hyprwm/hyprlock)                                          |
| **Idle Daemon**       | [Hypridle](https://github.com/hyprwm/hypridle)                                          |
| **Memory Protection** | [systemd-oomd](INSTALL.md#memory-pressure-protection-systemd-oomd)                      |
| **Wallpaper**         | [Awww](https://codeberg.org/LGFae/awww)                                                 |
| **Editor**            | [LazyVim](https://www.lazyvim.org/)                                                     |
| **Recording**         | [OBS Studio](https://obsproject.com/)                                                   |
| **Login Manager**     | [Ly](https://github.com/fairyglade/ly)                                                  |
| **Fetch App**         | [Fastfetch](https://github.com/fastfetch-cli/fastfetch)                                 |

<br/>

## Installation

For an interactive installation, clone the repository and run the setup script:

```bash
git clone https://github.com/Dijo-404/Hype_Niri.git
cd Hype_Niri
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

The installer optionally refreshes Arch mirrors, requires a full system upgrade before installing packages, and backs up existing user configurations before replacing them. It uses `pacman` for official repository packages and `yay` for missing AUR packages, retaining installed AUR packages and providers. It checks exact missing AUR package names before installing; a failed lookup stops the package phase. Missing AUR packages require a terminal so you can review provider and conflict choices. Run it as your regular user with sudo access, and install `yay` first with your preferred method.

To update only packages without re-running the full script:

```bash
sudo pacman -Syu
yay -Syu --aur
```

Power profiles switch automatically: Performance on AC, Balanced on battery at 30% or above, and Power Saver below 30%. Manual selections last until the next charger or threshold change. The controller uses D-Bus events and stops at logout; hardware without Performance support uses Balanced.

Docker starts on demand, idle audio devices suspend after five seconds, and file indexing skips dependency/cache directories. Native controls and the remaining startup scripts use locks and atomic writes to prevent duplicate processes and partial configurations.

`Super+Shift+W` opens a wallpaper picker with cached image previews. Niri animations use `ease-out-quad` without overshoot. A new window slides down from its own top edge over 180 ms; closing one (`Super+Q`) slides it back up into that edge over 180 ms, with no opacity change. Other animated Niri transitions run for 180 ms. Maximizing or restoring a column (`Super+F`) and other resizes show the new contents at once instead of crossfading. The same timing covers window movement, view scrolling, workspace switching, overview and compositor dialogs. Niri can only fade the screenshot UI and the recent-windows switcher, so those switch instantly. Three-finger swipes scroll horizontally or switch workspaces vertically; four-finger vertical swipes open and close overview. Gesture motion follows your fingers; the 180 ms timing applies to settling after release. App startup and shutdown delays are separate from animation duration. `Super+A` opens and closes Wofi, and `Esc` also dismisses it, with the same window animations. Search updates every 10 ms; rows use a 180 ms hover transition and one click activates them. The search chevron uses GTK’s symbolic icon renderer. Hyprlock supports only fades, so locking and unlocking fade for 180 ms on a matching quadratic ease-out curve, and the lock screen shows the wallpaper without blur. The lock taken just before sleep stays instant. The power menu (`Super+Shift+Delete`) is a Wofi list, so Niri animates it like the other menus. Wallpaper changes grow from the centre of the screen in 180 ms. Wofi runs as a normal floating window so Niri also animates utility menus, clipboard history, emoji and password prompts. The app launcher keeps the previous 394 × 328 logical-pixel size. Password prompts center text vertically in their compact window. Password prompts and clipboard menus disable Wofi caching. Waybar, Wofi hover effects, and wallpaper transitions use the matching quadratic ease-out curve, `cubic-bezier(0.333333, 0.666667, 0.666667, 1)`, with 180 ms durations. See [Niri animations](https://niri-wm.github.io/niri/Configuration%3A-Animations.html).

Click the hotspot icon beside the tray to start or stop sharing. Right-click it, or press `Super+Shift+H`, for credentials, **Change password**, and connection settings. Password entry is masked and accepts 8–63 ASCII characters or 64 hexadecimal digits (raw keys support WPA2 only). Changing an active hotspot's password restarts that hotspot; connected devices must reconnect. The first start creates a hotspot named `Hype-Niri` with a random password. It prefers 5 GHz on channel 36, uses 2.4 GHz on adapters without 5 GHz, and retries on 2.4 GHz if 5 GHz activation fails. The saved profile keeps the working band for later starts. Each activation attempt allows 35 seconds for NetworkManager to finish; busy actions show a notification. Security uses WPA2 Personal with RSN, AES/CCMP, and optional protected management frames. Starting an older saved hotspot upgrades these settings without changing its password and restarts it if needed. Starting it may disconnect Wi-Fi on the same adapter. Internet sharing requires an upstream connection, such as Ethernet, USB tethering, or another Wi-Fi adapter. The installer includes `dnsmasq` and configures DHCP/DNS access on hotspot-capable Wi-Fi interfaces when enabling UFW.

The app launcher, clipboard, emoji and power menus use Wofi; display, hotspot, power-profile, wallpaper and drive dialogs use native GTK4. If menus stop opening, run `wofi --version` and check `~/.config/wofi/config` and `~/.config/wofi/style.css`. Keep `normal_window=true` so Niri controls opening and closing animations. Menus stay open when they lose focus; pick an entry or press `Esc` to close them. Menu shortcuts and Waybar menu buttons toggle consistently: the first activation opens a menu, and the next closes it without choosing an action. This covers the launcher (`Super+A`), wallpaper picker (`Super+Shift+W`), hotspot controls (`Super+Shift+H`), display settings (`Super+Shift+D`), clipboard (`Super+V`), emoji (`Super+.`), and power menu (`Super+Shift+Delete`).

Cloudflare's native tray client starts with the desktop when installed, showing its cloud status icon. The fallback WARP indicator hides while the native client runs.

Waybar volume scrolling filters small touchpad movements with a `1.0` smooth-scrolling threshold. Volume and brightness keys use 1% steps (rounded to hardware units for brightness), and brightness ramps between values. Volume changes never queue behind an active change, and notifications do not hold either control's input lock. Brightness stops at 1%; decreasing an already lower value leaves it unchanged.

Installation includes OBS Studio and the [LazyVim starter](https://www.lazyvim.org/installation). Existing LazyVim settings are retained; other Neovim configurations are backed up before replacement. Open `nvim` to download plugins on first launch, then run `:LazyHealth`.

Installation configures Blueman to use its [built-in dialogs](https://github.com/blueman-project/blueman/wiki/Integrations#desktop-notifications) for Bluetooth notifications, including pairing PIN codes, passkeys and confirmation buttons. To pair a keyboard, open **Bluetooth Devices** from the tray, put the keyboard in pairing mode, search for it, select it and choose **Pair** (the key button). Type the displayed code on the Bluetooth keyboard and press **Enter**. Devices that request a code instead show an entry dialog; devices that use numeric comparison show confirmation buttons.

The native `hype-control prepare-sleep --watch` process holds a logind sleep delay inhibitor while it confirms the password locker and session lock are ready. Explicit suspend requests abort if that confirmation fails. Lid-close and other external requests remain subject to logind's five-second delay limit.

<br/>

## Repository Structure

```text
.
├── alacritty/       # Fast, GPU-accelerated terminal configuration
├── fastfetch/       # System information fetch utility with custom logo
├── wofi/            # Application launcher styled for Monochrome
├── hypr/            # Hypridle + Hyprlock configuration
├── mako/            # Notification daemon configuration
├── niri/            # Core Niri compositor settings and rules
├── polkit/          # Polkit rules for NetworkManager and drive access
├── wallpapers/      # Default curated wallpapers
├── native/          # Rust desktop controls and privileged Stealth backend
├── notch/           # Rust GTK4 notch companion
├── scripts/         # Remaining app launchers, menus and indicators
├── systemd/         # Automatic power service and OOM protection
├── stealth/         # Optional transparent Tor service and installer
├── tests/           # Installer, runtime and power-policy regression checks
├── waybar/          # Comprehensive status bar configuration
├── wireplumber/     # Audio routing and idle suspension
├── zsh/             # Shell config, Stealth commands, and Powerlevel10k theme
├── install.sh       # Automated installation script
├── pkglist.txt      # Master list of required packages
└── *.md             # Documentation files
```

<br/>

## Documentation

- **[Keybindings Reference](keybindings.md)** - Learn how to navigate and manage windows
- **[Zsh Aliases](alias.md)** - Shell aliases
- **[Stealth Tor Routing](INSTALL.md#stealth-tor-routing)** - Optional Tor routing commands
- **[Installation Guide](INSTALL.md)** - Detailed setup instructions

## Native Desktop Controls

The installer builds `hype-control`, `hype-notch` and the optional privileged `stealth` backend before replacing configurations. Niri, Waybar, Hypridle and the power-profile service call `~/.config/scripts/hype-control` directly. Temperature, audio, brightness, power profiles, caffeine, display settings, hotspot, drives, lock/suspend, Copilot shortcuts and wallpapers use native APIs. Application backends such as Hyprlock, Nautilus and `awww-daemon` still run as separate programs.

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
- Volume and brightness sliders. They read the current levels each time the drawer opens and apply changes through the native PipeWire PulseAudio API (capped at 100%) and backlight/logind APIs while you drag. Brightness stops at 1%. A slider hides when its service or device is missing. Waybar keeps the microphone and speaker mute buttons, speaker volume scrolling, and right-click access to `pavucontrol`.
- Live MPRIS music and video titles with previous, play/pause, and next controls. A playing source takes priority over a paused one. While something plays, a small animated wave replaces the music icon in the header and sits beside the track title; it stays still when GTK animations are off.
- A Pomodoro tab with a 25-minute focus timer and five-minute break. Completion sends a notification; start the next phase when ready. Pausing preserves remaining time. Reset returns to a fresh focus session.
- A Stopwatch tab with start, pause, and reset. The header shows whichever timer is in use.
- Optional Hype Flow offline dictation. When installed, `Super+Ctrl+Space` starts recording and finishes it on the next press; `Super+Ctrl+Esc` cancels. A compact charcoal capsule with a voice icon and live waveform replaces the resting indicator while dictation is active. Click the capsule to finish or right-click to cancel. The drawer includes start/stop, cancel, copy-last, and settings controls. Hype Flow opens from the app launcher and owns its model setup; no terminal is needed for daily use.

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

The application uses one instance per session. Preview and installed copies share that instance; quit the preview before starting the installed version. This first version does not include calendar sync, file sharing, artwork downloads, or Claude Code hooks.

## Checks

Native regression checks use temporary files, socket fixtures and private D-Bus services. The Python native suite runs the actual `hype-control` binary in Bubblewrap namespaces, with no live network, device or desktop access; it requires `python-gobject`, `dbus` and `bubblewrap`. `HYPE_CONTROL_BINARY` selects a different built binary. Remaining shell-launcher tests mock their external application boundaries. The Rust notch suite covers clocks, weather, Niri replies, backlight discovery and rounded regions. Its optional UI test needs an enabled output and an isolated session bus; it exercises real GTK4 widgets, fake native MPRIS/Flow services, shared timers and view teardown:

```bash
python3 tests/test-install.py
python3 tests/test-runtime.py
cargo test --workspace --locked
cargo build --workspace --bins --locked
/usr/bin/python3 tests/test-native-runtime.py
python3 tests/test-warp-indicator.py
bash tests/test-notch.sh
HYPE_NOTCH_UI_TESTS=1 dbus-run-session -- bash tests/test-notch.sh
bash stealth/test-stealth.sh
bash stealth/test-install-stealth.sh
shellcheck install.sh scripts/*.sh tests/test-notch.sh stealth/*.sh systemd/*.sh
shfmt -l install.sh scripts/*.sh tests/test-notch.sh stealth/*.sh systemd/*.sh
cargo fmt --all --check
cargo clippy --workspace --locked --all-targets -- -D warnings
prettier --check . --ignore-unknown
ruff check tests
ruff format --check tests
```

Static checks require ShellCheck, shfmt, Prettier, and Ruff. Shell formatting
uses the four-space indentation in `.editorconfig`; `ruff.toml` also checks
import placement.
