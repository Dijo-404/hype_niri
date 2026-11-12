#!/usr/bin/env bash

set -u

USER_NAME="${USER:-$(id -un 2>/dev/null || printf user)}"
MEDIA_DIR="/run/media/$USER_NAME"
DRIVES_DIR="$HOME/drives"
source "${BASH_SOURCE[0]%/*}/runtime-dir.sh"
LOCK_FILE="$RUNTIME_DIR/hype-open-drives.lock"
keyfile=""
trap '[[ -z "$keyfile" ]] || rm -f -- "$keyfile"' EXIT
trap 'exit 130' INT
trap 'exit 143' TERM

if command -v flock >/dev/null 2>&1; then
    exec 9>"$LOCK_FILE"
    flock -n 9 || exit 0
fi

notify() {
    local title="$1"
    local body="$2"

    if command -v notify-send >/dev/null 2>&1; then
        notify-send "$title" "$body" 2>/dev/null || true
    fi
}

have_storage_tools() {
    local missing=()
    local cmd

    for cmd in findmnt lsblk udisksctl; do
        command -v "$cmd" >/dev/null 2>&1 || missing+=("$cmd")
    done

    if [ "${#missing[@]}" -gt 0 ]; then
        notify "Drive mounting unavailable" "Missing: ${missing[*]} — install with: sudo pacman -S --needed util-linux udisks2"
        return 1
    fi
}

is_mounted() {
    findmnt -rn -S "$1" >/dev/null 2>&1
}

luks_cleartext() {
    lsblk -nrpo NAME "$1" 2>/dev/null | sed -n '2p'
}

luks_devices() {
    lsblk -prno NAME,FSTYPE | awk '$2 == "crypto_LUKS" { print $1 }'
}

settle_devices() {
    if command -v udevadm >/dev/null 2>&1; then
        udevadm settle --timeout=5 >/dev/null 2>&1 || true
    else
        sleep 0.2
    fi
}

mount_with_fallback() {
    local dev="$1"
    local fstype="$2"
    local output

    if is_mounted "$dev"; then
        return 0
    fi

    if output="$(udisksctl mount -b "$dev" 2>&1)"; then
        return 0
    fi

    if [ "$fstype" = "ntfs" ]; then
        if output="$(udisksctl mount -b "$dev" -o ro 2>&1)"; then
            notify "Mounted Windows volume read-only" "Windows left this NTFS volume dirty or hibernated. Run chkdsk and fully shut down Windows for write access."
            return 0
        fi
    fi

    notify "Could not mount $dev" "$output"
    return 1
}

luks_prompt_name() {
    local dev="$1" name
    name="$(lsblk -dno PARTLABEL "$dev" 2>/dev/null | head -n 1)"
    [ -n "${name// /}" ] || name="$(lsblk -dno LABEL "$dev" 2>/dev/null | head -n 1)"
    [ -n "${name// /}" ] || name="$(lsblk -dno SIZE "$dev" 2>/dev/null | head -n 1) drive"
    name="${name#"${name%%[![:space:]]*}"}"
    name="${name%"${name##*[![:space:]]}"}"
    printf '%s\n' "${name:-$(basename "$dev")}"
}

unlock_luks_volumes() {
    local dev name pw

    command -v fuzzel >/dev/null 2>&1 || {
        notify "Cannot unlock encrypted drives" "fuzzel is needed for the passphrase prompt."
        return 0
    }

    while read -r dev; do
        [ -b "$dev" ] || continue

        [ -n "$(luks_cleartext "$dev")" ] && continue

        name="$(luks_prompt_name "$dev")"
        pw="$(fuzzel --dmenu --password --prompt "Unlock $name: " </dev/null 2>/dev/null)" || continue
        [ -n "$pw" ] || continue

        keyfile="$(mktemp "$RUNTIME_DIR/hype-unlock.XXXXXX")" || continue
        chmod 600 "$keyfile"
        printf '%s' "$pw" >"$keyfile"
        unset pw

        udisksctl unlock -b "$dev" --key-file "$keyfile" >/dev/null 2>&1 ||
            notify "Unlock failed" "Wrong passphrase for $name, or device busy."
        rm -f "$keyfile"
        keyfile=""
    done < <(luks_devices)
}

mount_unmounted_filesystems() {
    local dev
    local fstype
    local mountpoints

    while read -r dev fstype mountpoints; do
        [ -b "$dev" ] || continue
        [ -n "$fstype" ] || continue
        [ -z "${mountpoints:-}" ] || continue

        case "$fstype" in
            ntfs | exfat | btrfs | ext2 | ext3 | ext4 | xfs)
                mount_with_fallback "$dev" "$fstype" || true
                ;;
        esac
    done < <(lsblk -prno NAME,FSTYPE,MOUNTPOINTS)
}

pending_work() {
    local dev fstype mnt

    while read -r dev; do
        [ -b "$dev" ] || continue
        [ -z "$(luks_cleartext "$dev")" ] && return 0
    done < <(luks_devices)

    while read -r dev fstype mnt; do
        [ -b "$dev" ] || continue
        case "$fstype" in
            ntfs | exfat | btrfs | ext2 | ext3 | ext4 | xfs)
                [ -z "${mnt:-}" ] && return 0
                ;;
        esac
    done < <(lsblk -prno NAME,FSTYPE,MOUNTPOINTS)

    return 1
}

wait_and_mount() {
    local deadline=$((SECONDS + 20))

    while :; do
        settle_devices
        mount_unmounted_filesystems
        create_drive_links

        pending_work || break
        [ "$SECONDS" -ge "$deadline" ] && break
        sleep 1
    done
}

alias_for_mount() {
    local source="$1"
    local target="$2"
    local label

    label="$(lsblk -no LABEL "$source" 2>/dev/null | head -n 1)"
    [ -n "$label" ] || label="$(lsblk -no PARTLABEL "$source" 2>/dev/null | head -n 1)"
    printf '%s\n' "${label:-$(basename "$target")}"
}

sanitize_link_name() {
    local name="$1"

    name="${name//\//-}"
    name="${name//\\/-}"
    name="${name//$'\n'/ }"
    name="${name#"${name%%[![:space:]]*}"}"
    name="${name%"${name##*[![:space:]]}"}"
    [ -n "$name" ] || name="Drive"

    printf '%s\n' "$name"
}

create_drive_links() {
    local source
    local target
    local name
    local base_name
    local suffix
    local link_path
    local -A used_names=()

    mkdir -p "$DRIVES_DIR"
    find "$DRIVES_DIR" -maxdepth 1 -xtype l -delete 2>/dev/null || true

    while read -r source target; do
        printf -v source '%b' "$source"
        printf -v target '%b' "$target"
        case "$target" in
            "$MEDIA_DIR"/*)
                base_name="$(sanitize_link_name "$(alias_for_mount "$source" "$target")")"
                name="$base_name"
                suffix=2

                while [ -n "${used_names[$name]:-}" ] || { [ -e "$DRIVES_DIR/$name" ] && [ ! -L "$DRIVES_DIR/$name" ]; }; do
                    name="$base_name-$suffix"
                    suffix=$((suffix + 1))
                done

                used_names[$name]=1
                link_path="$DRIVES_DIR/$name"
                ln -sfnT "$target" "$link_path" 2>/dev/null || true
                ;;
        esac
    done < <(findmnt -rn -o SOURCE,TARGET)
}

storage_ready=0
have_storage_tools && storage_ready=1

if [ "$storage_ready" = 1 ]; then
    unlock_luks_volumes
    wait_and_mount
fi

mkdir -p "$DRIVES_DIR"
exec nautilus "$DRIVES_DIR" 9>&-
