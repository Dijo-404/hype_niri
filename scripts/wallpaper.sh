#!/usr/bin/env bash

set -euo pipefail

source "${BASH_SOURCE[0]%/*}/runtime-dir.sh"
exec 9>"$RUNTIME_DIR/hype-wallpaper.lock"
flock -w 2 9 || exit 0

WALLPAPER_DIR="${WALLPAPER_DIR:-$HOME/pictures/wallpapers}"

STATE_DIR="$HOME/.local/state/niri"
CURRENT_LINK="$STATE_DIR/current_wallpaper"
mkdir -p "$STATE_DIR"

TRANSITION_TYPE="center"
TRANSITION_STEP=90
TRANSITION_FPS=60
TRANSITION_DURATION=0.16
TRANSITION_BEZIER=".333333,.666667,.666667,1"

find_wallpaper() {
    [[ -d "$WALLPAPER_DIR" ]] || return 1
    local img
    IFS= read -r -d '' img < <(find "$WALLPAPER_DIR" -type f \( -iname '*.png' -o -iname '*.jpg' -o -iname '*.jpeg' -o -iname '*.webp' \) -print0 | sort -z) || return 1
    printf '%s' "$img"
}

update_current_link() {
    local img
    img="$(realpath -- "$1")"
    tmp_link="$(mktemp "$STATE_DIR/.current-wallpaper.XXXXXX")"
    trap 'rm -f -- "${tmp_link:-}"' EXIT
    ln -sfT -- "$img" "$tmp_link"
    mv -Tf -- "$tmp_link" "$CURRENT_LINK"
}

ensure_current_wallpaper() {
    [[ -f "$CURRENT_LINK" ]] && return 0

    local fallback
    fallback="$(find_wallpaper)" || true
    [[ -f "$fallback" ]] || return 1
    update_current_link "$fallback"
}

start_daemon() {
    command -v awww >/dev/null 2>&1 || return 1

    if ! pgrep -u "$UID" -x awww-daemon >/dev/null; then
        local daemon_pid
        awww-daemon --format xrgb >/dev/null 2>&1 9>&- &
        daemon_pid=$!
        sleep 0.3
        if ! kill -0 "$daemon_pid" 2>/dev/null; then
            wait "$daemon_pid" 2>/dev/null || true
        fi
    fi

    pgrep -u "$UID" -x awww-daemon >/dev/null || return 1
}

apply_wallpaper() {
    local img="$1"
    local transition="${2:-$TRANSITION_TYPE}"

    start_daemon || return 0
    awww img "$img" \
        --transition-type "$transition" \
        --transition-step "$TRANSITION_STEP" \
        --transition-fps "$TRANSITION_FPS" \
        --transition-duration "$TRANSITION_DURATION" \
        --transition-bezier "$TRANSITION_BEZIER"
}

set_wallpaper() {
    local img="$1"
    if [[ -f "$img" ]]; then
        update_current_link "$img"
        apply_wallpaper "$img"
    fi
}

wallpaper_thumbnail() {
    local img="$1" key thumbnail tmp
    key="$(printf '%s\0%s' "$img" "$(stat -c '%y:%s' -- "$img")" | sha256sum)"
    thumbnail="$THUMBNAIL_DIR/${key%% *}.png"
    if [[ ! -s "$thumbnail" ]]; then
        tmp="$(mktemp "$THUMBNAIL_DIR/.thumbnail.XXXXXX.png")"
        if magick "${img}[0]" -auto-orient -thumbnail 256x256 \
            -strip "PNG:$tmp" 2>/dev/null; then
            mv -f -- "$tmp" "$thumbnail"
        else
            rm -f -- "$tmp"
            return 1
        fi
    fi
    printf '%s' "$thumbnail"
}

case "${1:-}" in
    ensure)
        ensure_current_wallpaper || exit 0
        ;;
    init)
        ensure_current_wallpaper || exit 0
        apply_wallpaper "$CURRENT_LINK" none
        ;;
    current | restore | sync)
        ensure_current_wallpaper || exit 0
        apply_wallpaper "$CURRENT_LINK" none
        ;;
    random)
        [[ -d "$WALLPAPER_DIR" ]] || exit 0
        IFS= read -r -d '' img < <(find "$WALLPAPER_DIR" -type f \( -iname '*.png' -o -iname '*.jpg' -o -iname '*.jpeg' -o -iname '*.webp' \) -print0 | shuf -z -n1) || true
        set_wallpaper "$img"
        ;;
    select)
        [[ -d "$WALLPAPER_DIR" ]] || exit 0
        mapfile -d '' -t images < <(find "$WALLPAPER_DIR" -type f \( -iname '*.png' -o -iname '*.jpg' -o -iname '*.jpeg' -o -iname '*.webp' \) -print0 | sort -z)
        ((${#images[@]})) || exit 0
        THUMBNAIL_DIR="${XDG_CACHE_HOME:-$HOME/.cache}/hype-niri/wallpapers"
        mkdir -p "$THUMBNAIL_DIR"
        flock -u 9
        choice="$(
            for index in "${!images[@]}"; do
                label="${images[index]//$'\n'/\\n}"
                label="${label//$'\r'/\\r}"
                label="${label//$'\t'/\\t}"
                if thumbnail="$(wallpaper_thumbnail "${images[index]}")"; then
                    printf 'img:%s:text:%s\n' "$thumbnail" "$label"
                else
                    printf '%s\n' "$label"
                fi
            done | wofi --dmenu -p "Wallpaper: " --no-custom-entry --allow-images \
                --height 640 --width 640 --define image_size=96 --define dmenu-print_line_num=true
        )" || exit 0
        flock -w 2 9 || exit 0
        [[ "$choice" =~ ^[0-9]+$ ]] && ((choice < ${#images[@]})) || exit 0
        set_wallpaper "${images[choice]}"
        ;;
    *)
        if [[ -f "${1:-}" ]]; then
            set_wallpaper "$1"
        else
            echo "Usage: wallpaper.sh {init|current|random|select|<path>}"
            exit 1
        fi
        ;;
esac
