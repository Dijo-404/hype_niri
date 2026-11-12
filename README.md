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
| **Launcher**          | [Fuzzel](https://codeberg.org/dnkl/fuzzel)                                              |
| **Notifications**     | [Mako](https://github.com/emersion/mako)                                                |
| **Lock Screen**       | [Hyprlock](https://github.com/hyprwm/hyprlock)                                          |
| **Idle Daemon**       | [Hypridle](https://github.com/hyprwm/hypridle)                                          |
| **Memory Protection** | [systemd-oomd](INSTALL.md#memory-pressure-protection-systemd-oomd)                      |
| **Wallpaper**         | [Awww](https://codeberg.org/LGFae/awww)                                                 |
| **Login Manager**     | [Ly](https://github.com/fairyglade/ly)                                                  |
| **Fetch App**         | [Fastfetch](https://github.com/fastfetch-cli/fastfetch)                                 |
| **Logout Menu**       | [Wlogout](https://github.com/ArtsyMacaw/wlogout)                                        |

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

Docker starts on demand, idle audio devices suspend after five seconds, and file indexing skips dependency/cache directories. Startup and control scripts use locks and atomic writes to prevent duplicate processes and partial configurations.

A `swayidle -w` watcher holds a sleep delay inhibitor while the lock helper confirms the session is locked. Explicit suspend requests abort if that confirmation fails. Lid-close and other external requests remain subject to logind's five-second delay limit; see the [swayidle documentation](https://github.com/swaywm/swayidle/blob/master/swayidle.1.scd).

<br/>

## Repository Structure

```text
.
├── alacritty/       # Fast, GPU-accelerated terminal configuration
├── fastfetch/       # System information fetch utility with custom logo
├── fuzzel/          # Application launcher styled for Monochrome
├── hypr/            # Hypridle + Hyprlock configuration
├── mako/            # Notification daemon configuration
├── niri/            # Core Niri compositor settings and rules
├── polkit/          # Polkit rules for NetworkManager and drive access
├── wallpapers/      # Default curated wallpapers
├── scripts/         # Desktop utility and control scripts
├── systemd/         # Automatic power service and OOM protection
├── stealth/         # Optional transparent Tor service and installer
├── tests/           # Installer, runtime and power-policy regression checks
├── waybar/          # Comprehensive status bar configuration
├── wlogout/         # Power menu configuration
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

## Checks

These checks use temporary fixtures and mocked system commands:

```bash
python3 tests/test-install.py
python3 tests/test-runtime.py
python3 tests/test-sleep.py
python3 tests/test-caffeine.py
python3 tests/test-auto-power-profile.py
bash stealth/test-stealth.sh
bash stealth/test-install-stealth.sh
shellcheck install.sh scripts/*.sh stealth/*.sh systemd/*.sh
```
