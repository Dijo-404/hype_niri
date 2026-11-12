#!/bin/bash

set -Eeuo pipefail

RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
CYAN='\033[0;36m'
GREY='\033[0;90m'
NC='\033[0m'
BOLD='\033[1m'

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
BACKUP_DIR=""
INSTALL_PHASES=(
    preflight refresh_mirrors update_system_packages install_packages
    backup_configs copy_configs setup_shell setup_gtk setup_desktop_integrations
    setup_system setup_logind setup_firewall setup_cloudflare setup_stealth
)
PHASE_TOTAL=$((${#INSTALL_PHASES[@]} + 1))
PHASE_CURRENT=0
DESKTOP_CONFIG_DIRS=(niri waybar scripts alacritty fuzzel mako fastfetch wlogout hypr)
XDG_USER_DIRS=(
    DESKTOP:desktop DOWNLOAD:downloads TEMPLATES:templates PUBLICSHARE:public
    DOCUMENTS:documents MUSIC:music PICTURES:pictures VIDEOS:videos PROJECTS:projects
)

INDENT="  "
BOX_W=60

_tmp_resources=()
cleanup_tmp() {
    local r
    for r in "${_tmp_resources[@]:-}"; do
        if [ -e "$r" ] || [ -L "$r" ]; then
            rm -rf -- "$r" 2>/dev/null || true
        fi
    done
}
trap cleanup_tmp EXIT
trap 'echo; print_error "Installation interrupted"; exit 130' INT
trap 'echo; print_error "Installation interrupted"; exit 143' TERM
trap 'print_error "Installation failed at line $LINENO (status $?)."' ERR

_repeat() {
    local char="$1" count="$2" out="" i
    for ((i = 0; i < count; i++)); do out+="$char"; done
    printf '%s' "$out"
}

_wrap_text() {
    local text="$1" max="$2" line="" word
    for word in $text; do
        if [ -z "$line" ]; then
            line="$word"
        elif [ "$(( ${#line} + 1 + ${#word} ))" -le "$max" ]; then
            line="$line $word"
        else
            printf '%s\n' "$line"
            line="$word"
        fi
    done
    if [ -n "$line" ]; then printf '%s\n' "$line"; fi
}

_box_top() {
    echo -e "${INDENT}${CYAN}╭$(_repeat '─' "$BOX_W")╮${NC}"
}

_box_top_tag() {
    local tag="$1" fill
    fill=$(( BOX_W - 3 - ${#tag} ))
    if [ "$fill" -lt 0 ]; then fill=0; fi
    echo -e "${INDENT}${CYAN}╭─ ${BOLD}${tag}${NC}${CYAN} $(_repeat '─' "$fill")╮${NC}"
}

_box_bottom() {
    echo -e "${INDENT}${CYAN}╰$(_repeat '─' "$BOX_W")╯${NC}"
}

_box_line() {
    local text="$1" color="${2:-}" padlen
    padlen=$(( BOX_W - 1 - ${#text} ))
    if [ "$padlen" -lt 0 ]; then padlen=0; fi
    echo -e "${INDENT}${CYAN}│${NC} ${color}${text}${NC}$(_repeat ' ' "$padlen")${CYAN}│${NC}"
}

_progress_bar() {
    local current="$1" total="$2" barw pct filled empty
    barw=$(( BOX_W - 6 ))
    pct=$(( current * 100 / total ))
    filled=$(( current * barw / total ))
    empty=$(( barw - filled ))
    echo -e "${INDENT}${GREEN}$(_repeat '█' "$filled")${GREY}$(_repeat '░' "$empty")${NC}  ${BOLD}${pct}%${NC}"
}

print_header() {
    local title="$1" subtitle="${2:-}" line
    local maxtext=$(( BOX_W - 2 ))
    local in_phase=false
    if [ "$PHASE_CURRENT" -gt 0 ] && [ "$PHASE_CURRENT" -le "$PHASE_TOTAL" ]; then
        in_phase=true
    fi

    echo ""
    if $in_phase; then
        _box_top_tag "Phase ${PHASE_CURRENT} / ${PHASE_TOTAL}"
    else
        _box_top
    fi

    while IFS= read -r line; do
        _box_line "$line" "${BOLD}"
    done < <(_wrap_text "$title" "$maxtext")

    if [ -n "$subtitle" ]; then
        while IFS= read -r line; do
            _box_line "$line" "${GREY}"
        done < <(_wrap_text "$subtitle" "$maxtext")
    fi

    _box_bottom
    if $in_phase; then
        _progress_bar "$PHASE_CURRENT" "$PHASE_TOTAL"
    fi
    echo ""
}

print_step() {
    echo -e "${INDENT}${CYAN}▸${NC} $1"
}

print_warn() {
    echo -e "${INDENT}${YELLOW}▲${NC} $1"
}

print_error() {
    echo -e "${INDENT}${RED}✗${NC} $1"
}

print_done() {
    echo -e "${INDENT}${GREEN}✓${NC} $1"
}

run_phase() {
    PHASE_CURRENT=$((PHASE_CURRENT + 1))
    "$@"
}

confirm() {
    local response
    [ -t 0 ] || return 1
    echo ""
    while true; do
        read -rp "$(echo -e "${INDENT}${YELLOW}?${NC} $1 ${GREY}[y/n]${NC}") " response || return 1
        case "$response" in
            [yY][eE][sS]|[yY]) return 0 ;;
            [nN][oO]|[nN]) return 1 ;;
            *) print_warn "Please answer y or n." ;;
        esac
    done
}

save_install_log() {
    local install_log="$1" source="${2:-repo}"
    print_error "Package installation failed. Last 40 log lines:"
    tail -n 40 "$install_log" | sed 's/^/    /'

    if [ "$source" = aur ]; then
        print_warn "AUR installation failed. Check package lookup, provider choices, and conflicts above."
        print_warn "Check the AUR error above and rerun ./install.sh when it is resolved."
    elif [ "$source" = repo ] && grep -Eqi 'failed retrieving file|404 Not Found|Could not resolve host|Connection timed out|SSL certificate problem|invalid or corrupted package' "$install_log"; then
        print_warn "If xwayland-satellite failed, it is an official Arch extra package, not an AUR package."
        print_warn "Rerun this installer and allow the mirror refresh step, or refresh manually:"
        print_warn "  sudo reflector --protocol https --latest 30 --sort rate --save /etc/pacman.d/mirrorlist"
        print_warn "  sudo pacman -Syu"
        print_warn "  sudo pacman -S --needed xwayland-satellite"
    fi

    local persisted
    if persisted="$(mktemp /tmp/hype-niri-install-XXXXXXXX.log)" &&
       cp -- "$install_log" "$persisted"; then
        print_warn "Full install log saved to: $persisted"
    else
        print_warn "Could not preserve the full installation log"
    fi
}

check_internet() {
    if command -v curl &>/dev/null; then
        curl --connect-timeout 5 -fsS https://archlinux.org >/dev/null 2>&1
        return $?
    fi

    if command -v wget &>/dev/null; then
        wget --timeout=5 --spider -q https://archlinux.org >/dev/null 2>&1
        return $?
    fi

    print_warn "curl/wget not found; skipping network preflight"
    print_warn "Package installation will report any network errors"
    return 0
}

refresh_mirrors() {
    print_header "Mirror Refresh" "rank fresh HTTPS mirrors for faster downloads"

    if ! confirm "Refresh Arch mirrors before installing packages?"; then
        print_warn "Skipping mirror refresh"
        print_warn "If downloads fail with 404s or timeouts, rerun and allow this step."
        return 0
    fi

    local mirrorlist="/etc/pacman.d/mirrorlist"
    local backup="/etc/pacman.d/mirrorlist.hype-niri.bak"
    local mirror_tmp

    if [ -f "$mirrorlist" ]; then
        print_step "Backing up current mirrorlist to $backup..."
        if ! sudo cp "$mirrorlist" "$backup"; then
            print_error "Could not back up current mirrorlist; mirror refresh stopped."
            return 1
        fi
    fi

    mirror_tmp="$(mktemp)" || { print_error "Failed to create temp mirrorlist"; exit 1; }
    _tmp_resources+=("$mirror_tmp")

    if command -v reflector &>/dev/null; then
        print_step "Ranking fresh HTTPS mirrors with reflector..."
        if reflector --protocol https --latest 30 --sort rate --save "$mirror_tmp" &&
           grep -q '^Server = https://' "$mirror_tmp"; then
            sudo install -m 644 "$mirror_tmp" "$mirrorlist"
            print_done "Mirrorlist refreshed with reflector"
            return 0
        fi

        print_warn "reflector failed; falling back to Arch's mirrorlist service"
    fi

    print_step "Downloading fresh HTTPS mirrorlist from archlinux.org..."
    if command -v curl &>/dev/null; then
        if ! curl --connect-timeout 10 -fsSL 'https://archlinux.org/mirrorlist/?country=all&protocol=https&ip_version=4&use_mirror_status=on' -o "$mirror_tmp"; then
            print_error "Failed to download a fresh mirrorlist with curl"
            print_warn "Keeping the existing mirrorlist"
            exit 1
        fi
    elif command -v wget &>/dev/null; then
        if ! wget --timeout=10 -qO "$mirror_tmp" 'https://archlinux.org/mirrorlist/?country=all&protocol=https&ip_version=4&use_mirror_status=on'; then
            print_error "Failed to download a fresh mirrorlist with wget"
            print_warn "Keeping the existing mirrorlist"
            exit 1
        fi
    else
        print_error "curl or wget is required to refresh mirrors without reflector"
        print_warn "Install reflector or refresh /etc/pacman.d/mirrorlist manually, then rerun ./install.sh"
        exit 1
    fi

    if ! grep -q '^#Server = https://' "$mirror_tmp"; then
        print_error "Downloaded mirrorlist did not contain HTTPS mirrors"
        print_warn "Keeping the existing mirrorlist"
        exit 1
    fi

    sed -i 's/^#Server = https:/Server = https:/' "$mirror_tmp"
    sudo install -m 644 "$mirror_tmp" "$mirrorlist"
    print_done "Mirrorlist refreshed from Arch mirror status"

}

update_system_packages() {
    print_header "System Package Update" "refresh keyring and upgrade installed packages"

    print_step "A full system upgrade is required before installing new Arch packages."
    if ! confirm "Update Arch keyring and system packages before installing?"; then
        print_error "System upgrade declined; installation stopped before installing packages."
        return 1
    fi

    local update_log
    update_log="$(mktemp)" || { print_error "Failed to create temp log"; exit 1; }
    _tmp_resources+=("$update_log")

    print_step "Updating archlinux-keyring first..."
    if ! stdbuf -oL -eL sudo pacman -Sy --needed archlinux-keyring 2>&1 | tee "$update_log"; then
        save_install_log "$update_log"
        exit 1
    fi

    print_step "Updating system packages..."
    if ! stdbuf -oL -eL sudo pacman -Syu 2>&1 | tee -a "$update_log"; then
        save_install_log "$update_log"
        exit 1
    fi

    print_done "System packages updated"
}

ensure_yay() {
    if command -v yay &>/dev/null; then
        print_done "yay found"
        return 0
    fi

    print_error "yay (AUR helper) is required for AUR packages, but it is not installed."
    print_warn "Install yay with your preferred method, then rerun ./install.sh."
    print_warn "The installer does not clone AUR repos to bootstrap yay."
    exit 1
}

preflight() {
    print_header "Preflight Checks" "verify Arch and network before any changes"

    if ! command -v pacman &>/dev/null; then
        print_error "This script requires Arch Linux (pacman not found)"
        exit 1
    fi
    print_done "Arch Linux detected"

    local config file
    for config in "${DESKTOP_CONFIG_DIRS[@]}"; do
        if [ ! -d "$SCRIPT_DIR/$config" ]; then
            print_error "Required source directory is missing: $SCRIPT_DIR/$config"
            exit 1
        fi
    done
    local required_files=(
        pkglist.txt zsh/.zshrc zsh/.p10k.zsh
        niri/config.kdl niri/opacity.kdl waybar/config.jsonc waybar/style.css
        fontconfig/60-hype-niri-fonts.conf
        waybar/icons/16x16/panel/blueman-tray.svg
        waybar/icons/16x16/panel/blueman-disabled.svg
        waybar/icons/16x16/panel/blueman-active.svg
        waybar/icons/16x16/panel/nm-signal-00.svg
        waybar/icons/16x16/panel/nm-signal-25.svg
        waybar/icons/16x16/panel/nm-signal-50.svg
        waybar/icons/16x16/panel/nm-signal-75.svg
        waybar/icons/16x16/panel/nm-signal-100.svg
        waybar/icons/16x16/panel/nm-no-connection.svg
        scripts/copilot-display.sh scripts/copilot-shortcut.sh scripts/display-scale.sh scripts/niri-outputs.sh scripts/runtime-dir.sh scripts/auto-power-profile.py
        scripts/blueman-manager.py
        systemd/user/hype-auto-power-profile.service
        systemd/user/blueman-manager.service.d/hype-niri.conf
        wireplumber/wireplumber.conf.d/60-hype-niri-audio.conf
        systemd/setup-oomd.sh systemd/user@.service.d/60-hype-niri-oomd.conf
        polkit/49-udisks2.rules polkit/50-network-manager.rules
    )
    for file in "${required_files[@]}"; do
        if [ ! -r "$SCRIPT_DIR/$file" ]; then
            print_error "Required source file is missing or unreadable: $SCRIPT_DIR/$file"
            exit 1
        fi
    done
    print_done "Source files present"
    ensure_yay

    if ! check_internet; then
        print_error "No internet connection"
        exit 1
    fi
    print_done "Network preflight complete"
}

install_packages() {
    print_header "Installing Packages" "official repo and AUR packages from pkglist.txt"

    if [ ! -f "$SCRIPT_DIR/pkglist.txt" ]; then
        print_error "pkglist.txt not found at $SCRIPT_DIR/pkglist.txt"
        exit 1
    fi

    local packages=()
    local pacman_packages=()
    local aur_packages=()
    local retained_packages=()
    local total
    local pkg i
    local install_log

    mapfile -t packages < <(awk 'NF && $1 !~ /^#/ && !seen[$1]++ {print $1}' "$SCRIPT_DIR/pkglist.txt")

    total=${#packages[@]}
    print_step "Installing $total packages..."
    echo ""

    if [ "$total" -eq 0 ]; then
        print_error "No packages found in pkglist.txt"
        exit 1
    fi

    print_step "Package queue:"
    for i in "${!packages[@]}"; do
        printf "    [%3d/%3d] %s\n" "$((i + 1))" "$total" "${packages[$i]}"
    done
    echo ""

    print_step "Classifying packages..."
    for pkg in "${packages[@]}"; do
        if pacman -Si "$pkg" >/dev/null 2>&1; then
            pacman_packages+=("$pkg")
        elif pacman -T "$pkg" >/dev/null 2>&1; then
            # Foreign packages and their installed providers already satisfy the
            # requirement. AUR lookup failures must not select replacements.
            retained_packages+=("$pkg")
            print_done "Retaining installed package/provider: $pkg"
        else
            aur_packages+=("aur/$pkg")
        fi
    done

    print_step "Pacman packages: ${#pacman_packages[@]}"
    print_step "AUR packages to install: ${#aur_packages[@]}"
    print_step "Installed AUR requirements retained: ${#retained_packages[@]}"

    if [ "${#aur_packages[@]}" -gt 0 ]; then
        ensure_yay
    fi

    install_log="$(mktemp)" || { print_error "Failed to create temp log"; exit 1; }
    _tmp_resources+=("$install_log")

    if [ "${#aur_packages[@]}" -gt 0 ]; then
        if ! test -t 0; then
            print_error "Missing AUR packages require interactive input; rerun ./install.sh in a terminal."
            exit 1
        fi
        # yay's install resolver can search alternate providers after an RPC
        # error. Its info operation checks exact names and fails on that error.
        print_step "Checking missing AUR package metadata..."
        if ! yay -Si --aur "${aur_packages[@]}" >> "$install_log" 2>&1; then
            save_install_log "$install_log" aur
            exit 1
        fi
    fi

    if [ "${#pacman_packages[@]}" -gt 0 ]; then
        print_step "Installing official repository packages with pacman..."
        if ! stdbuf -oL -eL sudo pacman -S --needed --noconfirm "${pacman_packages[@]}" 2>&1 | tee -a "$install_log"; then
            save_install_log "$install_log"
            exit 1
        fi
    fi

    if [ "${#aur_packages[@]}" -gt 0 ]; then
        print_step "Installing AUR packages with yay..."
        print_step "If a provider menu appears, select the package name from the queue above."
        if ! stdbuf -oL -eL yay -S --aur --needed --noconfirm=false --confirm "${aur_packages[@]}" 2>&1 | tee -a "$install_log"; then
            save_install_log "$install_log" aur
            exit 1
        fi
    fi

    # Do not report success if a helper skipped an unavailable target.
    if ! pacman -T "${packages[@]}" >> "$install_log" 2>&1; then
        print_error "Required packages remain unsatisfied; installation stopped."
        save_install_log "$install_log" verification
        exit 1
    fi

    print_done "All packages installed"
}

backup_configs() {
    print_header "Backing Up Existing Configs" "saved to a timestamped folder in your home"

    local targets=(
        .zshrc .p10k.zsh
        .config/gtk-3.0 .config/gtk-4.0 .config/autostart .config/fontconfig
        .config/systemd/user .config/wireplumber
        .local/share/applications/blueman-manager.desktop
        .local/share/icons/Papirus-Dark
        .local/share/stealth .local/share/privacy-shield
    )
    local config target
    for config in "${DESKTOP_CONFIG_DIRS[@]}"; do
        targets+=(".config/$config")
    done

    local existing=()
    for target in "${targets[@]}"; do
        if [ -e "$HOME/$target" ] || [ -L "$HOME/$target" ]; then
            existing+=("$target")
        fi
    done

    if [ "${#existing[@]}" -eq 0 ]; then
        print_done "No existing configs to back up"
        return 0
    fi

    print_warn "Existing configs found"
    if ! confirm "Back up existing configs before replacing them?"; then
        print_error "Backup declined. Existing configs will not be overwritten."
        exit 1
    fi

    BACKUP_DIR="$(mktemp -d "$HOME/.config-backup-$(date +%Y%m%d-%H%M%S).XXXXXX")" || {
        print_error "Could not create a config backup directory"
        return 1
    }
    for target in "${existing[@]}"; do
        if ! mkdir -p "$(dirname "$BACKUP_DIR/$target")" ||
           ! cp -a -- "$HOME/$target" "$BACKUP_DIR/$target"; then
            print_error "Could not back up $target; existing configs will not be overwritten."
            return 1
        fi
        print_done "Backed up $target"
    done
    print_done "Backup saved to $BACKUP_DIR"
}

# Publish a staged directory, restoring the previous path if the rename fails.
replace_directory() {
    local staged="$1" destination="$2" previous="$1.previous"
    if [ -e "$destination" ] || [ -L "$destination" ]; then
        mv -T -- "$destination" "$previous" || return 1
    fi
    if ! mv -T -- "$staged" "$destination"; then
        if [ -e "$previous" ] || [ -L "$previous" ]; then
            mv -T -- "$previous" "$destination" || \
                print_error "Previous configuration remains at $previous"
        fi
        print_error "Could not install $destination"
        return 1
    fi
    rm -rf -- "$previous"
}

# Generated files must replace symlinks without changing their external targets.
prepare_generated_config_dir() {
    local dir="$1" tmp
    mkdir -p "$(dirname "$dir")"
    if [ -L "$dir" ]; then
        tmp="$(mktemp -d "${dir}.XXXXXX")" || return 1
        _tmp_resources+=("$tmp")
        if [ -d "$dir" ]; then
            if ! cp -a -- "$dir/." "$tmp/"; then
                print_error "Could not stage existing contents of $dir"
                return 1
            fi
        elif [ -e "$dir" ]; then
            print_error "Expected a configuration directory: $dir"
            return 1
        fi
        replace_directory "$tmp" "$dir" || return 1
    else
        mkdir -p "$dir"
    fi
}

# Rename a legacy directory to its lowercase path without merging into existing content.
adopt_lowercase_dir() {
    local legacy="$1" target="$2"
    if [ "$legacy" != "$target" ] && [ -d "$legacy" ] && [ ! -L "$legacy" ]; then
        if [ ! -e "$target" ] && [ ! -L "$target" ]; then
            if ! mv -T -- "$legacy" "$target"; then
                print_error "Could not rename $legacy to $target"
                return 1
            fi
            print_done "Renamed $legacy -> $target"
        elif rmdir -- "$legacy" 2>/dev/null; then
            print_done "Removed empty $legacy"
        else
            print_warn "Kept $legacy beside $target; merge its files manually"
        fi
    fi
    mkdir -p "$target"
}

setup_user_dirs() {
    local entry name dir target configured legacy
    for entry in "${XDG_USER_DIRS[@]}"; do
        name="${entry%%:*}"
        dir="${entry#*:}"
        target="$HOME/$dir"
        configured=""
        if command -v xdg-user-dir &>/dev/null; then
            configured="$(xdg-user-dir "$name" 2>/dev/null)" || configured=""
            configured="${configured%/}"
        fi
        # Adopt only directories directly under home, never home itself or external paths.
        for legacy in "$configured" "$HOME/${dir^}"; do
            if [ -n "$legacy" ] && [ "$(dirname -- "$legacy")" = "$HOME" ]; then
                adopt_lowercase_dir "$legacy" "$target" || return 1
            fi
        done
        mkdir -p "$target"
        if command -v xdg-user-dirs-update &>/dev/null; then
            xdg-user-dirs-update --set "$name" "$target" || {
                print_error "Could not set XDG $name directory"
                return 1
            }
        fi
    done
    if command -v xdg-user-dirs-update &>/dev/null; then
        print_done "XDG user directories set to lowercase names"
    else
        print_warn "xdg-user-dirs-update not found; created lowercase directories only"
    fi
}

copy_configs() {
    print_header "Copying Configurations" "niri, waybar, terminal, theming and dotfiles"

    mkdir -p "$HOME/.config"

    local config tmp
    for config in "${DESKTOP_CONFIG_DIRS[@]}"; do
        tmp="$(mktemp -d "$HOME/.config/.${config}.XXXXXX")" || {
            print_error "Could not stage $config"
            return 1
        }
        _tmp_resources+=("$tmp")
        if ! cp -a -- "$SCRIPT_DIR/$config/." "$tmp/"; then
            print_error "Could not copy $config; its existing configuration was retained."
            return 1
        fi
        # Saved monitor modes and scales live in outputs.kdl and must survive reinstalls.
        if [ "$config" = niri ] && [ -f "$HOME/.config/niri/outputs.kdl" ]; then
            if ! cp -p -- "$HOME/.config/niri/outputs.kdl" "$tmp/outputs.kdl"; then
                print_error "Could not keep saved monitor settings; niri configuration was retained."
                return 1
            fi
        fi
        replace_directory "$tmp" "$HOME/.config/$config" || return 1
        print_done "Copied $config -> ~/.config/$config"
    done

    if ! chmod +x "$HOME/.config/scripts/"*.sh; then
        print_error "Could not make desktop scripts executable"
        return 1
    fi
    print_done "Made scripts executable"

    # Seed outputs.kdl so niri's include resolves on first launch.
    if [ -x "$HOME/.config/scripts/display-scale.sh" ]; then
        "$HOME/.config/scripts/display-scale.sh" --no-reload >/dev/null 2>&1 || true
    fi
    if [ -d "$HOME/.config/niri" ] && [ ! -e "$HOME/.config/niri/outputs.kdl" ]; then
        : > "$HOME/.config/niri/outputs.kdl"
    fi
    print_done "Seeded niri output-scale config"

    setup_user_dirs || return 1
    # open-drives.sh links mounts under ~/drives; the old ~/Drives held only those links.
    if [ -d "$HOME/Drives" ] && [ ! -L "$HOME/Drives" ]; then
        find "$HOME/Drives" -maxdepth 1 -type l -delete
        adopt_lowercase_dir "$HOME/Drives" "$HOME/drives" || return 1
    fi
    adopt_lowercase_dir "$HOME/pictures/Screenshots" "$HOME/pictures/screenshots" || return 1
    adopt_lowercase_dir "$HOME/pictures/Wallpapers" "$HOME/pictures/wallpapers" || return 1
    if [ -d "$SCRIPT_DIR/wallpapers" ]; then
        cp -an -- "$SCRIPT_DIR/wallpapers/." "$HOME/pictures/wallpapers/"
        print_done "Added wallpapers -> ~/pictures/wallpapers (existing files preserved)"
    else
        print_warn "No wallpapers found in source directory"
    fi

    mkdir -p "$HOME/.local/state/niri"
    if [ ! -e "$HOME/.local/state/niri/current_wallpaper" ]; then
        local wallpapers=() seed_wallpaper
        mapfile -d '' -t wallpapers < <(find "$HOME/pictures/wallpapers" -type f \( -iname '*.png' -o -iname '*.jpg' -o -iname '*.jpeg' -o -iname '*.webp' \) -print0 | sort -z)
        seed_wallpaper="${wallpapers[0]:-}"
        if [ -n "$seed_wallpaper" ] && [ -f "$seed_wallpaper" ]; then
            ln -sfn "$seed_wallpaper" "$HOME/.local/state/niri/current_wallpaper"
            print_done "Seeded wallpaper pointer -> ~/.local/state/niri/current_wallpaper"
        else
            print_warn "No wallpaper available to seed current_wallpaper"
        fi
    fi

    mkdir -p "$HOME/.cache/cliphist"

    prepare_generated_config_dir "$HOME/.config/autostart"
    tmp="$(mktemp "$HOME/.config/autostart/.blueman.XXXXXX")"
    _tmp_resources+=("$tmp")
    cat > "$tmp" << 'EOF'
[Desktop Entry]
Type=Application
Hidden=true
EOF
    chmod 644 "$tmp"
    mv -fT -- "$tmp" "$HOME/.config/autostart/blueman.desktop"
    print_done "Suppressed blueman tray autostart"
}

setup_shell() {
    print_header "Setting Up Zsh" "zsh, powerlevel10k and fzf-tab"

    local current_shell dotfile tmp

    print_step "Checking fzf-tab plugin..."
    if [ -f /usr/share/zsh/plugins/fzf-tab/fzf-tab.plugin.zsh ] || \
       [ -f "$HOME/.zsh/fzf-tab/fzf-tab.plugin.zsh" ]; then
        print_done "fzf-tab available"
    else
        print_warn "fzf-tab not found -- ensure the 'fzf-tab' package installed from pkglist.txt"
    fi

    for dotfile in .zshrc .p10k.zsh; do
        tmp="$(mktemp "$HOME/${dotfile}.XXXXXX")"
        _tmp_resources+=("$tmp")
        cp -p -- "$SCRIPT_DIR/zsh/$dotfile" "$tmp"
        mv -fT -- "$tmp" "$HOME/$dotfile"
        print_done "Copied $dotfile -> ~/$dotfile"
    done

    current_shell=$(basename "${SHELL:-}")
    if [ "$current_shell" != "zsh" ]; then
        if confirm "Change default shell to zsh?"; then
            chsh -s /usr/bin/zsh
            print_done "Default shell changed to zsh"
            print_warn "Log out and back in for this to take effect"
        fi
    else
        print_done "Zsh is already the default shell"
    fi
}

setup_tray_icons() {
    local theme="$HOME/.local/share/icons/Papirus-Dark" size icon tmp
    prepare_generated_config_dir "$theme"
    if [ ! -f "$theme/index.theme" ]; then
        cp -- /usr/share/icons/Papirus-Dark/index.theme "$theme/index.theme"
    fi
    prepare_generated_config_dir "$theme/16x16"
    if [ ! -e "$theme/16x16@2x" ] && [ ! -L "$theme/16x16@2x" ]; then
        ln -s 16x16 "$theme/16x16@2x"
    elif [ "$(readlink "$theme/16x16@2x" || true)" != 16x16 ]; then
        prepare_generated_config_dir "$theme/16x16@2x"
    fi
    for size in 16x16 16x16@2x; do
        prepare_generated_config_dir "$theme/$size/panel"
        for icon in blueman-tray blueman-disabled blueman-active \
            nm-signal-00 nm-signal-25 nm-signal-50 nm-signal-75 nm-signal-100 nm-no-connection; do
            tmp="$(mktemp "$theme/$size/panel/.${icon}.XXXXXX")"
            _tmp_resources+=("$tmp")
            cp -- "$SCRIPT_DIR/waybar/icons/16x16/panel/$icon.svg" "$tmp"
            chmod 644 "$tmp"
            # Replace only the applet alias, preserving shared theme icons.
            mv -fT -- "$tmp" "$theme/$size/panel/$icon.svg"
        done
    done
    if command -v gtk-update-icon-cache &>/dev/null; then
        gtk-update-icon-cache -f -t "$theme" || print_warn "Could not refresh the user icon cache"
    fi
    print_done "Applied matching Wi-Fi and Bluetooth outline icons (Bluetooth remains 14px)"
}

setup_gtk() {
    print_header "GTK Theme Setup" "dark GTK and icon theming"

    local gtk_dir tmp
    for gtk_dir in gtk-3.0 gtk-4.0; do
        prepare_generated_config_dir "$HOME/.config/$gtk_dir"
        tmp="$(mktemp "$HOME/.config/$gtk_dir/.settings.XXXXXX")"
        _tmp_resources+=("$tmp")
        cat > "$tmp" << 'EOF'
[Settings]
gtk-theme-name=Adwaita-dark
gtk-icon-theme-name=Papirus-Dark
gtk-cursor-theme-name=Bibata-Modern-Ice
gtk-cursor-theme-size=24
gtk-font-name=JetBrains Mono 10
gtk-application-prefer-dark-theme=true
EOF
        chmod 644 "$tmp"
        mv -fT -- "$tmp" "$HOME/.config/$gtk_dir/settings.ini"
        print_done "Created $gtk_dir settings"
    done

    prepare_generated_config_dir "$HOME/.config/fontconfig"
    prepare_generated_config_dir "$HOME/.config/fontconfig/conf.d"
    tmp="$(mktemp "$HOME/.config/fontconfig/conf.d/.hype-niri-fonts.XXXXXX")"
    _tmp_resources+=("$tmp")
    cp -- "$SCRIPT_DIR/fontconfig/60-hype-niri-fonts.conf" "$tmp"
    chmod 644 "$tmp"
    mv -fT -- "$tmp" "$HOME/.config/fontconfig/conf.d/60-hype-niri-fonts.conf"
    print_done "Set default desktop and monospace fonts to JetBrains Mono"

    if command -v papirus-folders &>/dev/null; then
        print_step "Setting Papirus-Dark folder color to grey..."
        if papirus-folders -C grey --theme Papirus-Dark; then
            print_done "Set Papirus-Dark folder color to grey"
        else
            print_warn "papirus-folders failed -- folder color unchanged"
        fi
    else
        print_warn "papirus-folders not found -- install 'papirus-folders-catppuccin-git' from AUR to recolor folders"
    fi

    setup_tray_icons

    if command -v dconf &>/dev/null; then
        print_step "Applying dark theme via dconf..."
        if dconf write /org/gnome/desktop/interface/color-scheme "'prefer-dark'" 2>/dev/null &&
           dconf write /org/gnome/desktop/interface/gtk-theme "'Adwaita-dark'" 2>/dev/null &&
           dconf write /org/gnome/desktop/interface/icon-theme "'Papirus-Dark'" 2>/dev/null &&
           dconf write /org/gnome/desktop/interface/cursor-theme "'Bibata-Modern-Ice'" 2>/dev/null &&
           dconf write /org/gnome/desktop/interface/cursor-size "24" 2>/dev/null &&
           dconf write /org/gnome/desktop/interface/font-name "'JetBrains Mono 10'" 2>/dev/null &&
           dconf write /org/gnome/desktop/interface/monospace-font-name "'JetBrains Mono 10'" 2>/dev/null &&
           dconf write /org/gnome/desktop/interface/document-font-name "'JetBrains Mono 10'" 2>/dev/null; then
            print_done "Dark theme applied via dconf"
        else
            print_warn "Could not apply all dconf settings; GTK settings.ini files will still apply"
        fi
    elif command -v gsettings &>/dev/null; then
        print_step "Applying dark theme via gsettings..."
        if gsettings set org.gnome.desktop.interface color-scheme 'prefer-dark' 2>/dev/null &&
           gsettings set org.gnome.desktop.interface gtk-theme 'Adwaita-dark' 2>/dev/null &&
           gsettings set org.gnome.desktop.interface icon-theme 'Papirus-Dark' 2>/dev/null &&
           gsettings set org.gnome.desktop.interface cursor-theme 'Bibata-Modern-Ice' 2>/dev/null &&
           gsettings set org.gnome.desktop.interface cursor-size 24 2>/dev/null &&
           gsettings set org.gnome.desktop.interface font-name 'JetBrains Mono 10' 2>/dev/null &&
           gsettings set org.gnome.desktop.interface monospace-font-name 'JetBrains Mono 10' 2>/dev/null &&
           gsettings set org.gnome.desktop.interface document-font-name 'JetBrains Mono 10' 2>/dev/null; then
            print_done "Dark theme applied via gsettings"
        else
            print_warn "Could not apply all gsettings values; GTK settings.ini files will still apply"
        fi
    else
        print_warn "Neither dconf nor gsettings found; GTK settings.ini files will still apply"
    fi
}

remove_stale_audio_service() {
    local unit="$HOME/.config/systemd/user/audio-jack-switch.service"
    [ -f "$unit" ] || return 0
    grep -Fxq 'ExecStart=%h/.config/waybar/scripts/audio-jack-switch.sh' "$unit" || return 0
    [ ! -e "$HOME/.config/waybar/scripts/audio-jack-switch.sh" ] || return 0

    if systemctl --user show-environment >/dev/null 2>&1; then
        systemctl --user disable --now audio-jack-switch.service || return 1
        systemctl --user reset-failed audio-jack-switch.service 2>/dev/null || true
    else
        prepare_generated_config_dir "$HOME/.config/systemd/user/default.target.wants"
        rm -f -- "$HOME/.config/systemd/user/default.target.wants/audio-jack-switch.service"
    fi
    rm -f -- "$unit"
    print_done "Removed obsolete audio jack service; WirePlumber handles routing"
}

configure_file_indexing() {
    /usr/bin/python3 - <<'PY'
from gi.repository import Gio

source = Gio.SettingsSchemaSource.get_default()
schema = source.lookup("org.freedesktop.Tracker3.Miner.Files", True) if source else None
if schema and schema.has_key("ignored-directories"):
    settings = Gio.Settings.new_full(schema, None, None)
    current = settings.get_strv("ignored-directories")
    excluded = ["node_modules", ".venv", "venv", "miniconda3", "__pycache__", "target",
                ".mypy_cache", ".pytest_cache", ".ruff_cache"]
    updated = list(dict.fromkeys(current + excluded))
    if updated != current and not settings.set_strv("ignored-directories", updated):
        raise SystemExit("Could not update file indexing exclusions")
    Gio.Settings.sync()
PY
}

# Launch Blueman from the app launcher through the wrapper that closes on Escape.
install_blueman_desktop_entry() {
    local source=/usr/share/applications/blueman-manager.desktop
    local destination="$HOME/.local/share/applications/blueman-manager.desktop" tmp
    if [ ! -f "$source" ]; then
        print_warn "Blueman desktop entry not found; skipped launcher override"
        return 0
    fi
    prepare_generated_config_dir "$HOME/.local/share/applications" || return 1
    tmp="$(mktemp "${destination}.XXXXXX")"
    _tmp_resources+=("$tmp")
    sed "s|^Exec=.*|Exec=/usr/bin/python3 \"$HOME/.config/scripts/blueman-manager.py\"|" "$source" > "$tmp"
    chmod 644 "$tmp"
    mv -fT -- "$tmp" "$destination"
}

setup_desktop_integrations() {
    print_header "Desktop Integration Setup" "audio routing, Bluetooth manager and automatic power profiles"

    local dir file tmp destination
    for dir in systemd systemd/user systemd/user/graphical-session.target.wants \
        systemd/user/blueman-manager.service.d wireplumber wireplumber/wireplumber.conf.d; do
        prepare_generated_config_dir "$HOME/.config/$dir"
    done
    for file in systemd/user/hype-auto-power-profile.service \
        systemd/user/blueman-manager.service.d/hype-niri.conf \
        wireplumber/wireplumber.conf.d/60-hype-niri-audio.conf; do
        destination="$HOME/.config/$file"
        tmp="$(mktemp "${destination}.XXXXXX")"
        _tmp_resources+=("$tmp")
        cp -- "$SCRIPT_DIR/$file" "$tmp"
        chmod 644 "$tmp"
        mv -fT -- "$tmp" "$destination"
    done
    destination="$HOME/.config/systemd/user/graphical-session.target.wants/hype-auto-power-profile.service"
    tmp="$(mktemp "${destination}.XXXXXX")"
    _tmp_resources+=("$tmp")
    ln -sfT -- ../hype-auto-power-profile.service "$tmp"
    mv -fT -- "$tmp" "$destination"
    install_blueman_desktop_entry || return 1
    remove_stale_audio_service
    configure_file_indexing || print_warn "Could not configure file indexing exclusions"
    if systemctl --user show-environment >/dev/null 2>&1; then
        systemctl --user daemon-reload
    fi
    print_done "Installed audio policy, Bluetooth manager and automatic power service"
}

enable_system_service_now() {
    local service="$1"
    local label="${service%.service}"
    local unit_state

    unit_state="$(systemctl list-unit-files "$service" --no-legend 2>/dev/null || true)"
    if [ -z "$unit_state" ]; then
        print_warn "System service unit not found: $service"
        return 1
    fi

    if sudo systemctl enable --now "$service" >/dev/null 2>&1; then
        print_done "Enabled + started system service: $label"
        return 0
    fi

    if sudo systemctl enable "$service" >/dev/null 2>&1; then
        print_warn "Enabled system service but could not start now: $label"
    else
        print_warn "Failed to enable system service: $label"
    fi

    return 1
}

enable_user_service() {
    local service="$1"
    local start_now="${2:-later}"
    local label="${service%.service}"
    local unit_state

    unit_state="$(systemctl --user list-unit-files "$service" --no-legend 2>/dev/null || true)"
    if [ -z "$unit_state" ]; then
        print_warn "User service unit not found: $service"
        return 1
    fi

    if [ "$start_now" = "now" ]; then
        if systemctl --user enable --now "$service" >/dev/null 2>&1; then
            print_done "Enabled + started user service: $label"
            return 0
        fi

        if systemctl --user enable "$service" >/dev/null 2>&1; then
            print_warn "Enabled user service but could not start now: $label"
        else
            print_warn "Failed to enable user service: $label"
        fi
    elif systemctl --user enable "$service" >/dev/null 2>&1; then
        print_done "Enabled user service: $label"
        return 0
    else
        print_warn "Failed to enable user service: $label"
    fi

    return 1
}

configure_docker_socket() {
    enable_system_service_now docker.socket || return 1
    if systemctl is-enabled --quiet docker.service; then
        local containers
        if systemctl is-active --quiet docker.service; then
            if ! containers="$(unset DOCKER_HOST DOCKER_CONTEXT; docker --host unix:///run/docker.sock ps --quiet 2>/dev/null)"; then
                print_warn "Could not inspect Docker containers; existing service retained"
                return 0
            fi
            if [ -n "$containers" ]; then
                print_warn "Docker has running containers; existing service retained"
                return 0
            fi
        fi
        sudo systemctl disable --now docker.service || return 1
    fi
    print_done "Docker starts on demand through its socket"
}

setup_system() {
    print_header "System Configuration" "services, display manager and pacman tuning (sudo)"

    if [ -f /etc/pacman.conf ]; then
        print_step "Tuning pacman output..."
        if ! grep -qx '\[options\]' /etc/pacman.conf; then
            print_error "Missing [options] section in /etc/pacman.conf"
            return 1
        fi
        sudo sed -i -e 's/^#Color$/Color/' -e 's/^#VerbosePkgLists$/VerbosePkgLists/' /etc/pacman.conf || return 1
        local directive
        for directive in Color VerbosePkgLists; do
            if ! grep -qx "$directive" /etc/pacman.conf; then
                sudo sed -i "/^\[options\]$/a $directive" /etc/pacman.conf || return 1
            fi
        done
        if grep -q '^#ParallelDownloads' /etc/pacman.conf; then
            sudo sed -i 's/^#ParallelDownloads.*/ParallelDownloads = 6/' /etc/pacman.conf || return 1
        elif grep -q '^ParallelDownloads' /etc/pacman.conf; then
            sudo sed -i 's/^ParallelDownloads.*/ParallelDownloads = 6/' /etc/pacman.conf || return 1
        else
            sudo sed -i '/^\[options\]$/a ParallelDownloads = 6' /etc/pacman.conf || return 1
        fi
        if ! grep -q '^ILoveCandy$' /etc/pacman.conf; then
            sudo sed -i '/^\[options\]$/a ILoveCandy' /etc/pacman.conf || return 1
        fi
        print_done "Pacman output tuned"
    fi

    if confirm "Set up ly as display manager?"; then
        sudo systemctl daemon-reload || return 1
        local installed_units ly_unit
        installed_units=$(systemctl list-unit-files --type=service --no-legend | awk '{print $1}') || return 1

        if grep -qx 'ly@\.service' <<< "$installed_units"; then
            ly_unit=ly@tty2.service
        elif grep -qx 'ly\.service' <<< "$installed_units"; then
            ly_unit=ly.service
        else
            print_error "Ly service unit is missing -- install ly before switching display managers"
            return 1
        fi

        # Configure the next boot without disrupting the current login session.
        if ! sudo systemctl enable --force "$ly_unit"; then
            print_error "Could not enable $ly_unit; the current display manager was retained"
            return 1
        fi
        for dm in sddm gdm lightdm greetd; do
            if grep -qx "${dm}\.service" <<< "$installed_units" && systemctl is-enabled --quiet "$dm.service"; then
                if ! sudo systemctl disable "$dm.service"; then
                    print_error "Could not disable $dm.service; finish the display manager switch before rebooting"
                    return 1
                fi
                print_step "Disabled $dm.service"
            fi
        done
        sudo systemctl disable getty@tty2.service || return 1
        print_done "Enabled $ly_unit for the next boot"
    fi

    print_step "Installing Polkit rules (NetworkManager)..."
    local rule
    for rule in "$SCRIPT_DIR/polkit/"*.rules; do
        [ -f "$rule" ] || { print_error "Polkit rules are missing"; return 1; }
        sudo install -Dm644 "$rule" "/etc/polkit-1/rules.d/${rule##*/}" || return 1
    done
    print_done "Polkit rules applied"

    if [ ! -f /etc/pam.d/hyprlock ]; then
        printf '#%%PAM-1.0\nauth include login\n' | sudo tee /etc/pam.d/hyprlock >/dev/null || return 1
        print_done "Created /etc/pam.d/hyprlock"
    else
        print_done "hyprlock PAM config present"
    fi

    print_step "Enabling system services..."

    print_step "Configuring memory pressure protection..."
    if bash "$SCRIPT_DIR/systemd/setup-oomd.sh"; then
        print_done "systemd-oomd configured for sustained user memory pressure"
    else
        print_error "Could not configure systemd-oomd"
        return 1
    fi

    local required_services=(
        "NetworkManager.service"
        "power-profiles-daemon.service"
    )
    local optional_services=("bluetooth.service")
    local service

    for service in "${required_services[@]}"; do
        if ! enable_system_service_now "$service"; then
            print_error "Required system service could not be configured: $service"
            return 1
        fi
    done
    for service in "${optional_services[@]}"; do
        enable_system_service_now "$service" || true
    done
    configure_docker_socket || print_warn "Could not configure Docker socket activation"

    if command -v docker >/dev/null 2>&1 && getent group docker >/dev/null 2>&1; then
        local current_user
        current_user="$(id -un)"

        if id -nG "$current_user" 2>/dev/null | grep -qw docker; then
            print_done "User $current_user is already in docker group"
        elif confirm "Add $current_user to docker group? This requires logging out and back in."; then
            if sudo usermod -aG docker "$current_user"; then
                print_done "Added $current_user to docker group"
                print_warn "Log out and back in before running Docker without sudo"
            else
                print_warn "Could not add $current_user to docker group"
            fi
        else
            print_warn "Docker group setup skipped -- use sudo docker or add your user later"
        fi
    fi

    if systemctl --user show-environment >/dev/null 2>&1; then
        for service in pipewire.socket pipewire-pulse.socket wireplumber.service gnome-keyring-daemon.socket; do
            if ! enable_user_service "$service" now; then
                print_error "Required audio service could not be configured: $service"
                return 1
            fi
        done
        enable_user_service "hypridle.service" later || \
            print_warn "Niri startup will still try to launch hypridle directly as a fallback"
        local power_start=later
        if systemctl --user is-active --quiet graphical-session.target; then
            power_start=now
        fi
        enable_user_service "hype-auto-power-profile.service" "$power_start" || return 1
    else
        print_warn "User systemd manager unavailable; audio services will use package defaults at login"
        print_warn "Niri startup will launch hypridle with its service or direct fallback"
    fi
    print_done "System services configured"
}

setup_logind() {
    print_header "Lid Switch Behavior" "suspend on lid close, stay awake while docked"

    if ! confirm "Configure systemd-logind to suspend on lid close (but keep running when an external monitor is connected)?"; then
        print_warn "Lid switch setup skipped"
        return 0
    fi

    local conf_dir=/etc/systemd/logind.conf.d
    local conf_file="$conf_dir/10-hype-niri-lid.conf"

    sudo mkdir -p "$conf_dir" || return 1
    sudo tee "$conf_file" >/dev/null << 'EOF' || return 1
[Login]
HandleLidSwitch=suspend
HandleLidSwitchExternalPower=suspend
# Docked (external monitor): don't suspend; niri blanks the built-in panel.
HandleLidSwitchDocked=ignore
LidSwitchIgnoreInhibited=yes
HoldoffTimeoutSec=0s
InhibitDelayMaxSec=5
EOF
    print_done "Wrote $conf_file"

    print_warn "Lid switch changes will apply after reboot"
    print_warn "Skipping systemd-logind restart to avoid blanking the current session"
}

setup_firewall() {
    print_header "Firewall Setup" "ufw with sensible desktop defaults"

    if ! command -v ufw &>/dev/null; then
        print_warn "ufw not installed -- skipping firewall setup"
        return 0
    fi

    if ! confirm "Configure ufw with desktop defaults (deny incoming, allow outgoing)?"; then
        print_warn "Firewall setup skipped (you can run 'sudo ufw enable' later)"
        return 0
    fi

    print_step "Setting default policies..."
    sudo ufw default deny incoming   >/dev/null || return 1
    sudo ufw default allow outgoing  >/dev/null || return 1
    sudo ufw default allow routed    >/dev/null || return 1

    sudo ufw allow in on lo  >/dev/null || return 1
    sudo ufw allow out on lo >/dev/null || return 1
    sudo ufw logging low >/dev/null || return 1
    sudo ufw --force enable >/dev/null || return 1
    sudo systemctl enable ufw.service || return 1

    print_done "ufw enabled with desktop defaults"
    print_step "Current rules:"
    sudo ufw status verbose | sed 's/^/    /'
}

warn_cloudflare_setup() {
    print_warn "$1"
    if [ -n "${2:-}" ]; then
        printf '%s\n' "$2" | sed -n '1,20s/^/    /p'
    fi
    print_warn "Cloudflare WARP setup incomplete; continuing installation"
    print_warn "Inspect WARP logs with: journalctl -u warp-svc"
}

setup_cloudflare() {
    print_header "Cloudflare WARP" "optional DNS-over-HTTPS or full VPN"

    if ! command -v warp-cli &>/dev/null; then
        print_warn "warp-cli not installed -- skipping (install cloudflare-warp-bin if you want it)"
        return 0
    fi

    if ! confirm "Set up Cloudflare WARP (DNS-over-HTTPS by default, full VPN optional)?"; then
        print_warn "Cloudflare WARP setup skipped"
        return 0
    fi

    local warp_output attempt warp_ready=false
    if ! warp_output=$(sudo systemctl enable --now warp-svc 2>&1); then
        warn_cloudflare_setup "Could not enable and start warp-svc" "$warp_output"
        return 0
    fi
    print_done "warp-svc enabled and running"

    # A Type=simple service can be active before its CLI socket is ready.
    # Missing registration still returns a successful status query.
    for attempt in {1..10}; do
        if warp_output=$(warp-cli --accept-tos status 2>&1); then
            warp_ready=true
            break
        fi
        if [ "$attempt" -lt 10 ]; then sleep 1; fi
    done
    if ! $warp_ready; then
        warn_cloudflare_setup "WARP daemon did not become ready for CLI requests" "$warp_output"
        return 0
    fi

    if ! warp-cli --accept-tos registration show >/dev/null 2>&1; then
        if ! warp_output=$(warp-cli --accept-tos registration new 2>&1); then
            warn_cloudflare_setup "WARP registration failed; mode and connection were not changed" "$warp_output"
            print_warn "Manually retry with: warp-cli --accept-tos registration new"
            return 0
        fi
        print_done "Device registered with Cloudflare"
    fi

    echo ""
    echo "  Choose WARP mode:"
    echo "    1) DNS-over-HTTPS only  [default]"
    echo "    2) Full WARP VPN (encrypted tunnel)"
    echo "    3) Skip connection changes (preserve the current connection)"
    local mode_choice
    read -rp "  Mode [1/2/3]: " mode_choice || mode_choice=""
    case "${mode_choice:-1}" in
        2)
            if warp_output=$(warp-cli --accept-tos mode warp 2>&1); then
                print_done "Mode: WARP (VPN)"
            else
                warn_cloudflare_setup "Failed to set WARP mode; connection was not changed" "$warp_output"
                return 0
            fi
            ;;
        3) print_warn "WARP mode and connection preserved"; return 0 ;;
        1|"")
            if warp_output=$(warp-cli --accept-tos mode doh 2>&1); then
                print_done "Mode: DoH"
            else
                warn_cloudflare_setup "Failed to set DoH mode; connection was not changed" "$warp_output"
                return 0
            fi
            ;;
        *) warn_cloudflare_setup "Invalid WARP mode; connection was not changed"; return 0 ;;
    esac

    if warp_output=$(warp-cli --accept-tos connect 2>&1); then
        sleep 1
        local status
        status=$(warp-cli --accept-tos status 2>/dev/null || echo "unknown")
        print_done "WARP: $status"
    else
        warn_cloudflare_setup "WARP connection failed; check 'warp-cli status' manually" "$warp_output"
        return 0
    fi
}

setup_stealth() {
    print_header "Stealth Tor Routing" "optional transparent Tor for host TCP and DNS"

    if ! confirm "Install Stealth routing? It stays off until stealth-start and supports NetworkManager Wi-Fi or Ethernet."; then
        print_warn "Stealth installation skipped"
        return 0
    fi

    if [ ! -f "$SCRIPT_DIR/zsh/stealth.zsh" ] || [ ! -f "$SCRIPT_DIR/stealth/install-stealth.sh" ]; then
        print_error "Stealth files are missing from $SCRIPT_DIR"
        return 1
    fi

    print_step "Installing Stealth packages..."
    sudo pacman -S --needed --noconfirm tor nftables iproute2 curl || return 1

    local staging_dir destination
    destination="$HOME/.local/share/stealth"
    mkdir -p "$HOME/.local/share" || return 1
    staging_dir=$(mktemp -d "$HOME/.local/share/.stealth.XXXXXXXX") || return 1
    _tmp_resources+=("$staging_dir")
    cp "$SCRIPT_DIR/zsh/stealth.zsh" "$staging_dir/stealth.zsh" || return 1
    cp "$SCRIPT_DIR/stealth/"{install-stealth.sh,stealth.sh,stealth.service,stealth.nft,stealth.sudoers,torrc.conf} \
        "$staging_dir/" || return 1

    if ! sudo bash "$staging_dir/install-stealth.sh"; then
        print_error "Stealth installation failed; previous user commands were retained"
        return 1
    fi

    replace_directory "$staging_dir" "$destination" || return 1
    rm -rf -- "$HOME/.local/share/privacy-shield" || return 1
    print_done "Stealth commands and installer staged -> ~/.local/share/stealth/"
    print_done "Stealth service installed but inactive"
    print_warn "Open a new terminal, then use stealth-start, stealth-status, and stealth-stop"
}

validate() {
    print_header "Validating Installation" "check commands, configs, fonts and services"

    local all_ok=true
    local required_commands=(
        "niri"
        "waybar"
        "wlogout"
        "hyprlock"
        "hypridle"
        "mako"
        "fuzzel"
        "wl-paste"
        "cliphist"
        "gnome-keyring-daemon"
        "nm-applet"
        "blueman-applet"
        "notify-send"
        "brightnessctl"
        "powerprofilesctl"
        "loginctl"
        "python3"
    )
    local optional_commands=(
        "pavucontrol"
        "playerctl"
    )
    local cmd

    for cmd in "${required_commands[@]}"; do
        if command -v "$cmd" >/dev/null 2>&1; then
            print_done "Command available: $cmd"
        else
            print_error "Missing command: $cmd"
            all_ok=false
        fi
    done

    for cmd in "${optional_commands[@]}"; do
        if command -v "$cmd" >/dev/null 2>&1; then
            print_done "Command available: $cmd"
        else
            print_warn "Optional command missing: $cmd"
        fi
    done

    if [ -x /usr/lib/polkit-gnome/polkit-gnome-authentication-agent-1 ]; then
        print_done "Polkit agent executable present"
    else
        print_error "Missing polkit agent: /usr/lib/polkit-gnome/polkit-gnome-authentication-agent-1"
        all_ok=false
    fi

    if command -v niri &>/dev/null; then
        if niri validate -c "$HOME/.config/niri/config.kdl"; then
            print_done "Niri config is valid"
        else
            print_warn "Niri config validation failed -- check ~/.config/niri/config.kdl"
            all_ok=false
        fi
    else
        print_warn "niri not found in PATH (may need a reboot)"
    fi

    local critical_files=(
        "$HOME/.config/niri/config.kdl"
        "$HOME/.config/niri/opacity.kdl"
        "$HOME/.config/niri/outputs.kdl"
        "$HOME/.config/waybar/config.jsonc"
        "$HOME/.config/waybar/style.css"
        "$HOME/.config/scripts/brightness-control.sh"
        "$HOME/.config/scripts/caffeine-control.sh"
        "$HOME/.config/scripts/copilot-display.sh"
        "$HOME/.config/scripts/copilot-shortcut.sh"
        "$HOME/.config/scripts/display-scale.sh"
        "$HOME/.config/scripts/niri-outputs.sh"
        "$HOME/.config/scripts/fullscreen-toggle.sh"
        "$HOME/.config/scripts/lock-screen.sh"
        "$HOME/.config/scripts/lock.sh"
        "$HOME/.config/scripts/mic-control.sh"
        "$HOME/.config/scripts/monitor-refresh.sh"
        "$HOME/.config/scripts/open-drives.sh"
        "$HOME/.config/scripts/opacity-toggle.sh"
        "$HOME/.config/scripts/power-menu-action.sh"
        "$HOME/.config/scripts/power-profile.sh"
        "$HOME/.config/scripts/auto-power-profile.py"
        "$HOME/.config/scripts/blueman-manager.py"
        "$HOME/.config/scripts/runtime-dir.sh"
        "$HOME/.config/scripts/prepare-sleep.sh"
        "$HOME/.config/scripts/start-tray-applets.sh"
        "$HOME/.config/scripts/start-waybar.sh"
        "$HOME/.config/scripts/suspend-now.sh"
        "$HOME/.config/scripts/temperature-control.sh"
        "$HOME/.config/scripts/volume-control.sh"
        "$HOME/.config/scripts/wallpaper.sh"
        "$HOME/.config/alacritty/alacritty.toml"
        "$HOME/.config/fuzzel/fuzzel.ini"
        "$HOME/.config/mako/config"
        "$HOME/.config/fastfetch/config.jsonc"
        "$HOME/.config/hypr/hyprlock.conf"
        "$HOME/.config/hypr/hypridle.conf"
        "$HOME/.config/wlogout/layout"
        "$HOME/.config/wlogout/style.css"
        "$HOME/.config/wlogout/icons/lock.png"
        "$HOME/.config/wlogout/icons/logout.png"
        "$HOME/.config/wlogout/icons/reboot.png"
        "$HOME/.config/wlogout/icons/shutdown.png"
        "$HOME/.config/wlogout/icons/suspend.png"
        "$HOME/.config/gtk-3.0/settings.ini"
        "$HOME/.config/gtk-4.0/settings.ini"
        "$HOME/.config/fontconfig/conf.d/60-hype-niri-fonts.conf"
        "$HOME/.config/systemd/user/hype-auto-power-profile.service"
        "$HOME/.config/systemd/user/blueman-manager.service.d/hype-niri.conf"
        "$HOME/.config/wireplumber/wireplumber.conf.d/60-hype-niri-audio.conf"
        "$HOME/.zshrc"
        "$HOME/.p10k.zsh"
    )

    for f in "${critical_files[@]}"; do
        if [ -f "$f" ]; then
            print_done "$(basename "$f")"
        else
            print_error "Missing: $f"
            all_ok=false
        fi
    done

    local script
    for script in "$HOME/.config/scripts/"*.sh; do
        [ -e "$script" ] || continue
        if [ -x "$script" ]; then
            print_done "Script executable: $(basename "$script")"
        else
            print_error "Script not executable: $script"
            all_ok=false
        fi

        if bash -n "$script" 2>/dev/null; then
            print_done "Script syntax: $(basename "$script")"
        else
            print_error "Script syntax failed: $script"
            all_ok=false
        fi
    done

    if /usr/bin/python3 - "$HOME/.config/scripts/auto-power-profile.py" \
        "$HOME/.config/scripts/blueman-manager.py" <<'PY'
import ast
from pathlib import Path
import sys
for path in sys.argv[1:]:
    ast.parse(Path(path).read_text())
PY
    then
        print_done "Python helper syntax valid"
    else
        print_error "Python helper syntax invalid"
        all_ok=false
    fi

    if command -v fc-match &>/dev/null; then
        local font_match
        font_match=$(fc-match -f '%{family}\n' 'JetBrains Mono' 2>/dev/null | head -n 1 || true)
        if [[ "$font_match" == *"JetBrains Mono"* ]]; then
            print_done "Font: JetBrains Mono"
        else
            print_warn "JetBrains Mono font not resolving -- install ttf-jetbrains-mono"
        fi

        font_match=$(fc-match -f '%{family}\n' 'Roboto' 2>/dev/null | head -n 1 || true)
        if [[ "$font_match" == *"Roboto"* ]]; then
            print_done "Waybar font: Roboto"
        else
            print_warn "Roboto font not resolving -- install ttf-roboto"
        fi

        font_match=$(fc-match -f '%{family}\n' 'Material Design Icons' 2>/dev/null | head -n 1 || true)
        if [[ "$font_match" == *"Material Design Icons"* ]]; then
            print_done "Font: Material Design Icons"
        else
            print_warn "Material Design Icons font not resolving -- install ttf-material-design-icons-webfont"
        fi
    else
        print_warn "fontconfig not found -- cannot validate Waybar fonts"
    fi

    local unit required check_active
    local unit_state
    for unit in NetworkManager.service power-profiles-daemon.service bluetooth.service docker.socket; do
        required=false
        case "$unit" in
            NetworkManager.service|power-profiles-daemon.service) required=true ;;
        esac
        unit_state="$(systemctl list-unit-files "$unit" --no-legend 2>/dev/null || true)"
        if [ -n "$unit_state" ]; then
            if systemctl is-enabled --quiet "$unit" 2>/dev/null; then
                print_done "System service enabled: ${unit%.service}"
            else
                if $required; then
                    print_error "Required system service not enabled: ${unit%.service}"
                    all_ok=false
                else
                    print_warn "Optional system service not enabled: ${unit%.service}"
                fi
            fi

            if systemctl is-active --quiet "$unit" 2>/dev/null; then
                print_done "System service running: ${unit%.service}"
            else
                if $required; then
                    print_error "Required system service not running: ${unit%.service}"
                    all_ok=false
                else
                    print_warn "Optional system service not running: ${unit%.service}"
                fi
            fi
        else
            if $required; then
                print_error "Required system service unit unavailable: $unit"
                all_ok=false
            else
                print_warn "Optional system service unit unavailable: $unit"
            fi
        fi
    done

    if systemctl is-enabled --quiet systemd-oomd.service 2>/dev/null && \
        systemctl is-active --quiet systemd-oomd.service 2>/dev/null; then
        print_done "systemd-oomd enabled and running"
    else
        print_error "systemd-oomd must be enabled and running"
        all_ok=false
    fi

    if cmp -s "$SCRIPT_DIR/systemd/user@.service.d/60-hype-niri-oomd.conf" \
        /etc/systemd/system/user@.service.d/60-hype-niri-oomd.conf; then
        print_done "User memory pressure protection policy installed"
    else
        print_error "User memory pressure protection policy is missing or outdated"
        all_ok=false
    fi
    local oom_policy user_manager
    user_manager="user@$(id -u).service"
    if systemctl is-active --quiet "$user_manager"; then
        oom_policy="$(systemctl show "$user_manager" \
            -p MemoryAccounting -p ManagedOOMMemoryPressure -p ManagedOOMSwap \
            -p ManagedOOMMemoryPressureLimit -p ManagedOOMMemoryPressureDurationUSec 2>/dev/null || true)"
        # systemd normalizes 40% to UINT32_MAX * 40 / 100.
        if grep -qx 'MemoryAccounting=yes' <<< "$oom_policy" && \
            grep -qx 'ManagedOOMMemoryPressure=kill' <<< "$oom_policy" && \
            grep -qx 'ManagedOOMSwap=kill' <<< "$oom_policy" && \
            grep -qx 'ManagedOOMMemoryPressureLimit=1717986918' <<< "$oom_policy" && \
            grep -qx 'ManagedOOMMemoryPressureDurationUSec=10s' <<< "$oom_policy"; then
            print_done "User memory pressure and swap monitoring enabled"
        else
            print_error "User OOM monitoring policy differs from the required 40% for 10 seconds; check systemd drop-ins"
            all_ok=false
        fi
    else
        print_warn "User manager inactive; live OOM monitoring will apply at login"
    fi

    if systemctl --user show-environment >/dev/null 2>&1; then
        for unit in pipewire.socket pipewire-pulse.socket wireplumber.service gnome-keyring-daemon.socket hypridle.service hype-auto-power-profile.service; do
            required=true
            check_active=true
            [ "$unit" = hypridle.service ] && required=false
            if [ "$unit" = hype-auto-power-profile.service ] && \
                ! systemctl --user is-active --quiet graphical-session.target; then
                check_active=false
            fi
            unit_state="$(systemctl --user list-unit-files "$unit" --no-legend 2>/dev/null || true)"
            if [ -n "$unit_state" ]; then
                if systemctl --user is-enabled --quiet "$unit" 2>/dev/null; then
                    print_done "User service enabled: ${unit%.service}"
                else
                    if $required; then
                        print_error "Required audio unit not enabled: $unit"
                        all_ok=false
                    else
                        print_warn "hypridle service not enabled; Niri has a direct-launch fallback"
                    fi
                fi
                if $required && $check_active && ! systemctl --user is-active --quiet "$unit"; then
                    print_error "Required user unit not active: $unit"
                    all_ok=false
                fi
            else
                if $required; then
                    print_error "Required audio unit unavailable: $unit"
                    all_ok=false
                else
                    print_warn "hypridle service unavailable; Niri has a direct-launch fallback"
                fi
            fi
        done
    else
        print_warn "User systemd manager unavailable; skipping live user-service validation"
    fi

    if $all_ok; then
        echo ""
        echo -e "${GREEN}${BOLD}  All files in place!${NC}"
        return 0
    fi

    return 1
}

print_summary() {
    echo ""
    _box_top
    _box_line "✓  Installation Complete" "${BOLD}${GREEN}"
    _box_line "your Niri desktop is ready to use" "${GREY}"
    _box_bottom
    echo ""
    echo -e "  ${BOLD}Next steps${NC}"
    echo -e "    ${CYAN}1${NC}  Reboot your system"
    echo -e "    ${CYAN}2${NC}  Select ${BOLD}niri-session${NC} in your display manager"
    echo -e "    ${CYAN}3${NC}  Powerlevel10k is preconfigured (run ${BOLD}p10k configure${NC} to tweak)"
    echo -e "    ${CYAN}4${NC}  ${BOLD}Super+A${NC} app launcher  ${GREY}·${NC}  ${BOLD}Super+T${NC} terminal"
    echo ""
    echo -e "  ${BOLD}Key files${NC}"
    echo -e "    ${GREY}niri  ${NC}  ~/.config/niri/config.kdl"
    echo -e "    ${GREY}waybar${NC}  ~/.config/waybar/"
    echo -e "    ${GREY}scripts${NC}  ~/.config/scripts/"
    echo -e "    ${GREY}zsh   ${NC}  ~/.zshrc"
    echo -e "    ${GREY}keys  ${NC}  $SCRIPT_DIR/keybindings.md"
    echo ""
}

prompt_reboot() {
    [ -t 0 ] || return 0

    if confirm "Restart now?"; then
        print_warn "Restarting now..."
        if ! systemctl reboot; then
            print_error "Could not restart automatically. Please reboot manually."
            return 1
        fi
    else
        print_warn "Restart skipped. Reboot when ready to start using Niri."
    fi
}

main() {
    if [ "$#" -eq 1 ] && [[ "$1" == --help || "$1" == -h ]]; then
        printf 'Usage: ./install.sh\n\nRun interactively as your regular Arch Linux user with sudo access and yay installed.\n'
        return 0
    fi
    if [ "$#" -ne 0 ]; then
        print_error "Unknown arguments. Use ./install.sh --help for usage."
        return 2
    fi
    if [ "$EUID" -eq 0 ]; then
        print_error "Run ./install.sh as your regular user; it uses sudo when required."
        return 1
    fi
    source "$SCRIPT_DIR/scripts/runtime-dir.sh"
    exec 8>"$RUNTIME_DIR/hype-niri-install.lock"
    if ! flock -n 8; then
        print_error "Another Hype Niri installation is already running"
        return 1
    fi
    if [ -t 1 ] && [ -n "${TERM:-}" ]; then
        clear || true
    fi
    echo ""
    echo -e "${CYAN}${BOLD}"
    echo "    ╦ ╦╦ ╦╔═╗╔═╗  ╔╗╔╦╦═╗╦"
    echo "    ╠═╣╚╦╝╠═╝║╣   ║║║║╠╦╝║"
    echo "    ╩ ╩ ╩ ╩  ╚═╝  ╝╚╝╩╩╚═╩"
    echo -e "${NC}"
    echo -e "  ${BOLD}Arch Linux + Niri Wayland Setup${NC}"
    echo -e "  ${GREY}monochrome theme · automated installer${NC}"
    echo ""
    echo -e "${INDENT}${CYAN}$(_repeat '─' "$BOX_W")${NC}"
    echo ""

    if ! confirm "Start installation?"; then
        echo -e "\n  ${YELLOW}Installation cancelled.${NC}\n"
        exit 0
    fi

    local phase
    for phase in "${INSTALL_PHASES[@]}"; do
        run_phase "$phase"
    done
    if run_phase validate; then
        print_summary
        prompt_reboot
    else
        print_error "Validation failed; installation did not complete cleanly"
        exit 1
    fi
}

if [[ "${BASH_SOURCE[0]}" == "$0" ]]; then
    main "$@"
fi
