# Hype Niri -- Installation Guide

## Quick Install (Recommended)

The automated installer can refresh mirrors, performs a required Arch keyring and full system upgrade, installs packages, backs up and copies configurations, sets up Zsh and GTK theming, enables desktop services and memory pressure protection with `systemd-oomd`, and offers optional firewall, Cloudflare WARP, and Stealth configuration.

```bash
git clone https://github.com/Dijo-404/Hype_Niri.git
cd Hype_Niri
chmod +x install.sh
./install.sh
```

Run the installer as your regular user with sudo access. It asks before refreshing mirrors, upgrading the system, replacing existing configurations, switching the display manager, setting lid-switch behavior, configuring networking, or changing your login shell. Declining the required system upgrade or a required backup stops the installation.

> [!TIP]
> The Powerlevel10k prompt theme is pre-configured. Run `p10k configure` if you want to customize it.

> [!NOTE]
> The installer uses `pacman` for official repository packages and `yay` only for AUR packages. Install `yay` first with your preferred method; this installer does not clone AUR repos to bootstrap it.

---

## Manual Install

If you prefer to selectively apply configurations, follow these manual steps. Back up existing files before replacing them.

### 1. Install Required Packages

`pkglist.txt` contains the base desktop packages, including ufw and Cloudflare WARP. Their configuration is optional. Stealth packages are installed separately when you choose that option.

For smoother downloads, refresh mirrors before installing packages. If `reflector` is already installed, use it:

```bash
sudo cp /etc/pacman.d/mirrorlist /etc/pacman.d/mirrorlist.hype-niri.bak
sudo reflector --protocol https --latest 30 --sort rate --save /etc/pacman.d/mirrorlist
```

If `reflector` is not installed yet, the automated installer can download a fresh HTTPS mirrorlist directly from Arch's mirror status service before package installation.

Then update the keyring and complete the full system upgrade before installing new packages:

```bash
sudo pacman -Sy --needed archlinux-keyring
sudo pacman -Syu
```

```bash
bash -c 'source ./install.sh; install_packages'
```

### 2. Back Up Existing Configs

Back up the desktop, scripts, theme settings, shell files, and any existing Stealth staging before overwriting them. This preserves files and symlinks:

```bash
BACKUP_DIR="$(mktemp -d "$HOME/.config-backup-$(date +%Y%m%d-%H%M%S).XXXXXX")"
for target in \
    .config/niri .config/waybar .config/scripts .config/alacritty \
    .config/fuzzel .config/mako .config/fastfetch .config/wlogout .config/hypr \
    .config/gtk-3.0 .config/gtk-4.0 .config/autostart .config/fontconfig \
    .config/mimeapps.list \
    .config/systemd/user .config/wireplumber \
    .local/share/icons/Papirus-Dark \
    .zshrc .p10k.zsh .local/share/stealth .local/share/privacy-shield; do
    if [ -e "$HOME/$target" ] || [ -L "$HOME/$target" ]; then
        mkdir -p "$BACKUP_DIR/$(dirname "$target")"
        cp -a "$HOME/$target" "$BACKUP_DIR/$target"
    fi
done
```

### 3. Copy Configurations

Move the dotfiles to their respective locations in your home directory.

```bash
mkdir -p ~/.config ~/.cache/cliphist
bash -c 'source ./install.sh; setup_user_dirs'
mkdir -p ~/pictures/screenshots ~/pictures/wallpapers

cp -r niri waybar scripts alacritty fuzzel mako fastfetch wlogout hypr ~/.config/
touch ~/.config/niri/outputs.kdl

[ -d wallpapers ] && cp -an wallpapers/. ~/pictures/wallpapers/

cp zsh/.zshrc ~/
cp zsh/.p10k.zsh ~/

chmod +x ~/.config/scripts/*.sh
bash -c 'source ./install.sh; setup_desktop_integrations'
```

When editing `~/.config/niri/outputs.kdl`, keep each output block's opening
line as `output "name" {`, with each setting and the closing brace on separate
lines. Display helpers reject compact blocks before changing monitor settings.

### 4. Apply Dark Theme (GTK + dconf)

Set the dark theme for GTK apps. Apply via `dconf` so GNOME apps such as Nautilus pick it up immediately.

```bash
mkdir -p ~/.config/gtk-3.0 ~/.config/gtk-4.0

cat > ~/.config/gtk-3.0/settings.ini << 'EOF'
[Settings]
gtk-theme-name=Adwaita-dark
gtk-icon-theme-name=Papirus-Dark
gtk-cursor-theme-name=Bibata-Modern-Ice
gtk-cursor-theme-size=24
gtk-font-name=JetBrains Mono 10
gtk-application-prefer-dark-theme=true
EOF

cp ~/.config/gtk-3.0/settings.ini ~/.config/gtk-4.0/settings.ini

dconf write /org/gnome/desktop/interface/color-scheme   "'prefer-dark'"
dconf write /org/gnome/desktop/interface/gtk-theme      "'Adwaita-dark'"
dconf write /org/gnome/desktop/interface/icon-theme     "'Papirus-Dark'"
dconf write /org/gnome/desktop/interface/cursor-theme   "'Bibata-Modern-Ice'"
dconf write /org/gnome/desktop/interface/cursor-size    "24"
dconf write /org/gnome/desktop/interface/font-name      "'JetBrains Mono 10'"
dconf write /org/gnome/desktop/interface/monospace-font-name "'JetBrains Mono 10'"
dconf write /org/gnome/desktop/interface/document-font-name  "'JetBrains Mono 10'"

mkdir -p ~/.config/fontconfig/conf.d
cp fontconfig/60-hype-niri-fonts.conf ~/.config/fontconfig/conf.d/

papirus-folders -C grey --theme Papirus-Dark

# Apply matching Wi-Fi and Bluetooth outline icons in the user theme.
bash -c 'source ./install.sh; setup_tray_icons'
```

### 5. System-Wide Setup

These steps require elevated privileges (`sudo`).

```bash
sudo sed -i 's/^#Color$/Color/' /etc/pacman.conf
sudo sed -i 's/^#ParallelDownloads.*/ParallelDownloads = 6/' /etc/pacman.conf

sudo cp polkit/*.rules /etc/polkit-1/rules.d/

sudo systemctl enable --now NetworkManager bluetooth power-profiles-daemon
bash -c 'source ./install.sh; configure_docker_socket'
sudo usermod -aG docker "$USER"   # log out/in before using docker without sudo

systemctl --user enable --now pipewire.socket pipewire-pulse.socket wireplumber.service gnome-keyring-daemon.socket
systemctl --user enable --now hype-auto-power-profile.service
systemctl --user enable hypridle 2>/dev/null || true

sudo mkdir -p /etc/systemd/logind.conf.d
sudo tee /etc/systemd/logind.conf.d/10-hype-niri-lid.conf >/dev/null << 'EOF'
[Login]
HandleLidSwitch=suspend
HandleLidSwitchExternalPower=suspend
# Docked (external monitor): don't suspend; niri blanks the built-in panel.
HandleLidSwitchDocked=ignore
LidSwitchIgnoreInhibited=yes
HoldoffTimeoutSec=0s
InhibitDelayMaxSec=5
EOF
```

Niri starts `prepare-sleep.sh --watch`, which uses `swayidle -w` to hold a delay inhibitor until lock readiness is confirmed. Hypridle continues to handle idle timeouts and display restoration after resume. Explicit suspend requests stop if locking fails. External sleep requests can still proceed when `InhibitDelayMaxSec` expires, so this setting bounds the available lock preparation time.

To switch to Ly at the next boot, review your current display manager first. Arch ships `ly@tty2.service`. Enable it successfully before disabling an existing display manager, then reserve its TTY:

```bash
systemctl status display-manager.service --no-pager
sudo systemctl enable ly@tty2.service && sudo systemctl disable getty@tty2.service
```

If another display manager is enabled, disable its service only after deciding to replace it with Ly. Select **niri-session** at the next Ly login.

#### Automatic power profiles

The user service follows charger and battery events without polling:

| Power source            | Profile     |
| ----------------------- | ----------- |
| Charger connected       | Performance |
| Battery at 30% or above | Balanced    |
| Battery below 30%       | Power Saver |

Hardware without Performance support falls back to Balanced. Manual Waybar selections last until the next charger or battery-threshold change. The service stops with the graphical session.

```bash
/usr/bin/python3 ~/.config/scripts/auto-power-profile.py --check
systemctl --user status hype-auto-power-profile.service
```

Docker starts on demand through `docker.socket`; an existing service with running containers is preserved. WirePlumber handles headphone routing and suspends idle audio devices after five seconds. The installer removes the old audio-jack unit only when its expected script is missing, and excludes dependency/cache directories from file indexing while retaining existing exclusions.

#### Memory pressure protection (systemd-oomd)

The automated installer enables this by default. To apply only OOM protection on an existing setup, run from the repository:

```bash
bash systemd/setup-oomd.sh
```

The script installs `systemd/user@.service.d/60-hype-niri-oomd.conf` into `/etc/systemd/system/user@.service.d/`, reloads systemd, and runs `sudo systemctl enable --now systemd-oomd.service`. Arch includes the daemon in its `systemd` package; this policy requires systemd 257 or newer, cgroup v2, and PSI. It applies to running user managers without restarting the desktop.

User applications are monitored for memory pressure above **40% for 10 seconds**, plus the default swap exhaustion threshold. The pressure percentage measures time stalled waiting for memory, rather than RAM usage. This reduces the chance that runaway agent tests freeze the desktop; it does not guarantee a kill within ten seconds of allocation starting. Keep swap enabled so the daemon has time to react.

OOMD kills all processes in the selected application group. An agent inside an editor or terminal may cause that entire application to close. Use **niri-session** so applications run in separate systemd groups. For memory-intensive tests, an explicit cap in a separate scope provides additional protection:

```bash
systemd-run --user --scope -p MemoryMax=4G -p MemorySwapMax=1G -- your-test-command
```

Verify monitoring and inspect previous kills:

```bash
systemctl is-enabled systemd-oomd.service
systemctl is-active systemd-oomd.service
oomctl --no-pager
journalctl -u systemd-oomd.service --no-pager -n 30
```

`oomctl` should list `/user.slice/user-<UID>.slice/user@<UID>.service` under both swap and memory pressure monitoring. Merely enabling the daemon without a monitored-group policy provides no proactive protection. See the [systemd OOMD documentation](https://github.com/systemd/systemd/blob/main/man/systemd-oomd.service.xml) for how groups are selected.

### 6. Shell Setup

`fzf-tab` is installed from `pkglist.txt`; no manual plugin clone is needed.

```bash
chsh -s /usr/bin/zsh
```

### 7. Optional Networking

The base package list includes ufw and Cloudflare WARP; enabling and configuring them is **opt-in**. The automated installer asks before each setup step. Stealth installation is also **opt-in**, and its packages are installed only when accepted.

#### Firewall (ufw)

Standard desktop defaults: deny incoming, allow outgoing, allow loopback.

```bash
sudo ufw default deny incoming
sudo ufw default allow outgoing
sudo ufw default allow routed
sudo ufw allow in on lo
sudo ufw allow out on lo
sudo ufw logging low
sudo ufw --force enable
sudo systemctl enable ufw.service
```

Verify:

```bash
sudo ufw status verbose
```

Disable later with `sudo ufw disable`. The installer never opens SSH or any other port — if you need SSH, add `sudo ufw allow ssh` yourself.

#### Cloudflare WARP

The `cloudflare-warp-bin` AUR package ships a system service (`warp-svc`) and a user CLI (`warp-cli`). DoH (DNS-over-HTTPS) is the safe default; full WARP is a VPN tunnel. The installer waits for the daemon's CLI socket before registering. If optional WARP setup fails, it shows the error and continues installation. A failed registration or mode change stops WARP setup before connecting.

```bash
sudo systemctl enable --now warp-svc

warp-cli --accept-tos registration new

warp-cli --accept-tos mode doh

warp-cli --accept-tos connect
warp-cli --accept-tos status
```

Disconnect anytime with `warp-cli disconnect`. Inspect logs with `journalctl -u warp-svc`.

#### Stealth Tor routing

The installer asks whether to install Stealth. Choosing Yes installs its packages, stages the commands in `~/.local/share/stealth/`, and installs the system service. Choosing No skips those steps. It does not start Tor or change your network until you run `stealth-start`. For a manual install from this repository:

```bash
mkdir -p ~/.local/share/stealth
sudo pacman -S --needed tor nftables iproute2 curl jq util-linux networkmanager
cp zsh/stealth.zsh stealth/install-stealth.sh stealth/stealth.sh \
    stealth/stealth.service stealth/stealth.nft stealth/stealth.sudoers \
    stealth/torrc.conf ~/.local/share/stealth/
sudo bash ~/.local/share/stealth/install-stealth.sh
```

Open a new Zsh terminal after installing. The commands are `stealth-start`, `stealth-status`, and `stealth-stop`. If you use your own `.zshrc`, source `~/.local/share/stealth/stealth.zsh` from it. The direct commands `sudo stealth start`, `sudo stealth status`, and `sudo stealth stop` also work; a direct start leaves the systemd service inactive even while routing is active. The installer adds `/etc/sudoers.d/stealth` to disable sudo's pseudo-terminal for these commands, avoiding reproduced PTY teardown hangs when output cannot drain. A historical report of 100% CPU after terminal closure remains unconfirmed. When upgrading an earlier installation, stop its routing service first; the installer migrates its inactive system files.

While Stealth is active, a green Stealth icon appears beside the Wi-Fi and Bluetooth icons in Waybar's tray pill. It disappears after Stealth stops.

This version requires a NetworkManager-managed Wi-Fi or Ethernet default IPv4 route and a Tor service account. The optional installer step installs `tor`, `nftables`, `iproute2`, and `curl`; the main package list includes `jq` and `util-linux`. While active, host IPv4 TCP and DNS go through Tor; IPv6, other UDP, ICMP, and forwarded/container traffic are blocked. Tor's relay connections and DHCP still use the selected physical connection. Wi-Fi mode temporarily spoofs the MAC address and reconnects; Ethernet mode keeps its existing connection and MAC. If startup fails after the firewall is installed, run `stealth-stop` to recover it. Stealth is not a full IP VPN.

### 8. Reboot Your System

Reboot to initialize all changes and the new login manager:

```bash
reboot
```

> [!NOTE]
> At the Ly login screen, select **niri-session** before logging in.

---

## Updating

To pull the latest changes from the repository and apply them:

```bash
cd Hype_Niri
git pull
./install.sh
```

The installer requires a full system upgrade before package installation and a backup before replacing existing user configurations. Package installation uses `--needed` to avoid reinstalling current versions. Existing wallpapers are preserved.

To update packages without re-running the script:

```bash
sudo pacman -Syu
yay -Syu --aur
```

---

## Post-Install Steps

- **Powerlevel10k Prompt**: The theme is pre-configured out of the box. Run `p10k configure` in your terminal to customize it.
- **Learn the Controls**: Check out `keybindings.md` to learn how to navigate the Niri compositor.
- **User Directories**: The installer sets the XDG user directories to lowercase names (`~/desktop`, `~/documents`, `~/downloads`, `~/music`, `~/pictures`, `~/projects`, `~/public`, `~/templates`, `~/videos`). Existing capitalised folders are renamed when the lowercase folder does not exist yet.
- **Wallpapers**: The wallpaper script looks inside `~/pictures/wallpapers/`. Use `Super+Shift+W` to select one; the selected wallpaper is saved in `~/.local/state/niri/current_wallpaper` and restored after lock, sleep, reboot, and shutdown.
- **Lock Screen**: `Super+L` locks via hyprlock and uses the saved wallpaper pointer from `~/.local/state/niri/current_wallpaper`.
- **Firewall / WARP / Stealth**: If you skipped step 7, use the relevant commands above later. Stealth remains off until `stealth-start`.

## Troubleshooting

### xwayland-satellite fails to download

`xwayland-satellite` is an official Arch `extra` package. If pacman or yay reports a download error for it, the package name is not the problem; refresh mirrors, refresh your system package database, and retry:

```bash
sudo reflector --protocol https --latest 30 --sort rate --save /etc/pacman.d/mirrorlist
sudo pacman -Syu
sudo pacman -S --needed xwayland-satellite
```

If `reflector` is not installed, rerun `./install.sh` and allow the mirror refresh step. Niri starts `xwayland-satellite` automatically when it is installed and available in `PATH`, so it does not need a manual `spawn-at-startup` entry.

### Can't access a Windows partition from Linux (dual-boot)

If your Windows partition doesn't appear in Nautilus, or appears but won't open / mounts read-only:

**1. Install the prerequisites** (already in `pkglist.txt`):

```bash
yay -S --needed ntfs-3g gvfs udisks2 lvm2 libblockdev-lvm
```

- `ntfs-3g` — NTFS driver. Even though the kernel has `ntfs3` built-in, ntfs-3g handles Fast Startup detection cleanly.
- `gvfs` — lets Nautilus mount partitions on click.
- `udisks2` — the mount daemon Nautilus talks to.
- `lvm2` and `libblockdev-lvm` — let encrypted LUKS drives reveal and mount LVM-backed filesystems.

The `Super+U` keybinding runs `~/.config/scripts/open-drives.sh` before opening Nautilus. It mounts ordinary volumes, falls back to read-only for dirty Windows NTFS volumes, asks GVFS to unlock encrypted drives, mounts inner filesystems, and creates friendly links under `~/drives`.

**2. Disable Windows Fast Startup** (this is the cause ~90% of the time).

Fast Startup leaves the Windows NTFS partition hibernated on shutdown, and Linux refuses to mount it writable to protect the filesystem. In Windows:

`Control Panel → Power Options → "Choose what the power buttons do" → "Change settings that are currently unavailable"` → **uncheck "Turn on fast startup"** → Save. Then shut Windows down fully (Shift+Restart → Power off) and boot back into Linux.

**3. Mount it.**

Open Nautilus, click "Other Locations", and click the Windows partition — it should mount and open. Or from the terminal:

```bash
lsblk -f

sudo mkdir -p /mnt/windows
sudo mount -t ntfs3 /dev/nvme0n1pX /mnt/windows
```

If `mount` reports `falling back to read-only`, Fast Startup is still on or Windows was not shut down cleanly — boot back into Windows, fully shut down, retry.

### Want it auto-mounted on every boot?

Add it to `/etc/fstab`. Get the UUID first:

```bash
sudo blkid /dev/nvme0n1pX
```

Then append to `/etc/fstab`:

```
UUID=XXXXXXXX-XXXX  /mnt/windows  ntfs3  defaults,nofail,uid=1000,gid=1000,umask=022  0  0
```

`nofail` is critical — it keeps the system bootable if the Windows partition ever vanishes or is unmountable.

---

## Documentation Links

- [Keybindings Reference](keybindings.md)
- [Zsh Aliases](alias.md)
