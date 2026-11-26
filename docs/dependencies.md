# Dependencies and services

Hype Niri installs the core Wayland desktop and all extra groups by default. Answer `n` at the extras prompt for core only or `custom` for a subset.

The core requests 49 packages. With all extras selected, the installer requests 121 unique packages and builds Hype Flow from source. Pacman may install additional dependencies.

## Native replacements

| Former runtime dependency                            | Replacement                                                            |
| ---------------------------------------------------- | ---------------------------------------------------------------------- |
| Wofi launcher, power menu, password prompts          | Existing Rust GTK4 dialogs; GIO desktop discovery and launch           |
| playerctl                                            | Native MPRIS D-Bus media commands                                      |
| wl-paste plus cliphist watcher pipelines             | One native Wayland clipboard watcher                                   |
| cliphist picker plus wl-copy                         | Native private history, GTK picker and retained clipboard owner        |
| bemoji plus Wofi plus wtype                          | Unicode emoji metadata, GTK picker and native Wayland virtual keyboard |
| loginctl/niri CLI in idle actions                    | Native session locking and Niri socket commands                        |
| pkill for stopping Waybar                            | User-owned process matching and pidfd signaling                        |
| ImageMagick, lm_sensors, notify-send control helpers | Existing image library, sysfs sensors and notification D-Bus           |

The core runtime no longer requires those utility executables. The optional desktop-tools group retains wl-clipboard for hyprpicker’s own autocopy operation. Native code still uses established libraries, including GTK/GIO/GLib, libpulse and libnftables. A native binding does not remove the underlying library dependency.

Clipboard history stores up to 750 entries, 8 MiB per entry and 32 MiB total. It preserves supported text/image MIME types and bytes, skips password-manager hints and rejects malformed or unsafe state. Stored history is private under `$XDG_STATE_HOME/hype-niri-clipboard`, or `~/.local/state/hype-niri-clipboard`. A clipboard owner remains alive until another application replaces the selected content. If `cliphist` and its old database are still present, the watcher imports the history once and keeps the source database intact. Use `hype-control clipboard import-cliphist [database]` to import explicitly. Clearing native history prevents another automatic import.

The emoji picker uses bemoji's history location and ranks frequently used and recent emoji first. It reads custom lists from the bemoji data directory or `BEMOJI_CUSTOM_LIST`, and honors bemoji's history location, recent limit, and private mode settings.

## Required infrastructure

| Component                                       | Why retained                                                              |
| ----------------------------------------------- | ------------------------------------------------------------------------- |
| Niri, GTK4, layer-shell, Waybar                 | Compositor, native UI, notch and status bar                               |
| NetworkManager, network applet/editor           | Network state, Wi-Fi authentication and connection management             |
| PipeWire, pipewire-pulse, WirePlumber, libpulse | Audio/video services, routing and native audio control                    |
| logind/systemd, Hyprlock, Hypridle              | Session lifecycle, password locking, idle detection and inhibitor support |
| power-profiles-daemon, UPower                   | Power policy and battery information                                      |
| systemd-oomd                                    | Existing cgroup memory-pressure protection                                |
| awww                                            | Wallpaper rendering through its native protocol                           |
| Mako, Polkit agent, portals, keyring            | Notifications, authorization, screen sharing and credentials              |
| UDisks2, GVfs, Nautilus                         | Native mount/unlock operations and file browsing                          |
| Rust, Cargo, base-devel, Git                    | Locally build the installer and desktop binaries                          |

The installer enables essential network, audio, power and OOM services. Desktop startup launches the bar, notch, notification daemon, authorization agent, network applet and wallpaper backend. Systemd user services supervise the clipboard and sleep watchers and restart them if they exit. D-Bus activation and socket activation remain owned by their upstream services. Hypridle handles idle detection and calls Rust for lock and monitor actions.

## Optional packages

The exact group list is [packages/extras.txt](../packages/extras.txt). At the custom prompt, enter group names separated by spaces, for example:

```text
browser-firefox editor-neovim bluetooth
```

| Groups                                                        | Purpose                                                                                          |
| ------------------------------------------------------------- | ------------------------------------------------------------------------------------------------ |
| `browser-firefox`, `browser-brave`                            | Both browsers by default. Brave is the default browser when both are selected.                   |
| `editor-neovim`, `editor-kate`, `editor-gnome`, `editor-code` | Optional editors; Neovim selection includes LazyVim setup                                        |
| `chat`, `notes`, `media`                                      | Chat, notes, media playback and recording applications                                           |
| `hype-flow`                                                   | Offline dictation, Parakeet model setup, `wl-clipboard`, `wtype` and Bubblewrap                  |
| `desktop-tools`                                               | Optional audio mixer, color picker and system monitor; wl-clipboard supports hyprpicker autocopy |
| `docker`                                                      | Docker tools, socket activation and optional group membership                                    |
| `bluetooth`                                                   | BlueZ, Bluetooth service, pairing dialogs, Waybar Bluetooth icon, notch connect alerts           |
| `warp`, `proton-vpn`, `stealth`                               | Explicit VPN or Tor integrations                                                                 |
| `firewall`                                                    | UFW package and separately confirmed firewall configuration                                      |
| `login-manager-ly`                                            | Ly package and separately confirmed login-manager switch                                         |
| `shell-extras`, `theme-extras`                                | Additional terminal tools, prompt and theme packages                                             |
| `compatibility`                                               | XWayland, Qt, additional audio/filesystem/archive compatibility tools                            |
| `hardware-intel`                                              | Intel-specific media and Vulkan drivers                                                          |

Selections are recorded in `~/.config/hype-niri/optional-groups`. Missing or empty state means core only. Merely having Docker, Bluetooth or WARP installed does not select their setup during a full install. The tray starts Bluetooth/WARP applets only for their selected groups; Waybar polls WARP/Stealth only for selected integrations.

Changing selections does not uninstall packages or stop previously enabled services. Existing running applications and personal package choices remain under the user's control. Review obsolete packages and services separately before removing them. Manual named phases explicitly request their operation; generic phases follow saved selections.

Hype uses Pacman for Arch packages and Yay for AUR packages. Install and update workflows install `yay-bin` from the AUR if `yay` is missing. Installed providers remain supported.

## Remaining executable boundaries

Package managers, Cargo, Git, sudo/Polkit elevation, authoritative configuration validators and infrastructure applications remain separate programs. The installer still uses tools such as systemctl, gsettings/dconf, nmcli, UFW and optional vendor CLIs during setup. WARP still uses its vendor `warp-cli`; a private daemon protocol was not introduced. The advanced network editor, file manager, locker and wallpaper daemon keep their existing application responsibilities.

The `hype-flow` group builds a pinned upstream revision and uses Hype Flow's setup command to validate and publish its Niri shortcuts and background startup entry. Rust startup commands export the session environment before starting Hypridle. The generic `menu-toggle` command remains for custom and legacy callers. Default menus use native GTK dialogs.

Waybar runs the headless `hype-status` binary for hotspot and temperature. Hotspot follows NetworkManager events. Temperature keeps the existing sensor selection and samples every five seconds, emitting only changes.

## Library maintenance and compatibility

New clipboard access uses [wl-clipboard-rs](https://github.com/YaLTeR/wl-clipboard-rs), which supports the ext-data-control and wlr-data-control protocols. Virtual keyboard bindings come from [wayland-rs](https://github.com/Smithay/wayland-rs). Emoji names, shortcodes, sequences and skin-tone variants come from the [emojis crate](https://docs.rs/emojis/latest/emojis/). These libraries replace executable helpers without adding a database daemon or another service manager.

Versions are resolved in Cargo.lock. Niri configuration validation and awww IPC use pinned upstream code; keep their pins aligned with the installed compositor/wallpaper backend. No dependency's future maintenance can be guaranteed. Prefer supported protocol boundaries and distribution packages, and review upstream compatibility when updating the lockfile.

GUI/protocol regression checks use Sway and wtype on private buses and Wayland sockets. Python/GObject, formatters and linters are development requirements.
