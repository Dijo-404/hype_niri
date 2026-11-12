#!/usr/bin/env bash

set -euo pipefail

if ! command -v powerprofilesctl >/dev/null 2>&1; then
    [[ "${1:-}" != "menu" ]] && echo '{"text": "", "tooltip": "power-profiles-daemon not installed", "class": "missing"}'
    exit 0
fi

ID=2004

notify() {
    command -v notify-send >/dev/null 2>&1 || return 0
    notify-send -r "$ID" "$@" 2>/dev/null || true
}

get_current_profile() {
    powerprofilesctl get 2>/dev/null || true
}

get_icon() {
    case $1 in
        "performance") echo "󰓅" ;;
        "balanced") echo "󰾅" ;;
        "power-saver") echo "󰾆" ;;
        *) echo "󰾅" ;;
    esac
}

get_name() {
    case $1 in
        "performance") echo "Performance" ;;
        "balanced") echo "Balanced" ;;
        "power-saver") echo "Power Saver" ;;
        *) echo "$1" ;;
    esac
}

send_notification() {
    profile=$1
    icon=$(get_icon "$profile")
    name=$(get_name "$profile")
    notify "$icon  Power Mode: $name"
}

set_profile() {
    local profile="$1"
    local output

    if output="$(powerprofilesctl set "$profile" 2>&1)"; then
        send_notification "$profile"
    else
        notify "Could not set Power Mode" "${output:-$(get_name "$profile") is unavailable on this system}"
        return 1
    fi
}

if [[ "${1:-}" == "menu" ]]; then
    command -v fuzzel >/dev/null 2>&1 || exit 0
    options="󰓅 Performance\n󰾅 Balanced\n󰾆 Power Saver"
    choice=$(echo -e "$options" | fuzzel --dmenu -p "Power Profile") || true

    case "$choice" in
        *"Performance"*)
            set_profile "performance" || true
            ;;
        *"Balanced"*)
            set_profile "balanced" || true
            ;;
        *"Power Saver"*)
            set_profile "power-saver" || true
            ;;
    esac
    pkill -RTMIN+16 waybar || true
else
    current=$(get_current_profile)
    if [ -z "$current" ]; then
        echo '{"text": "", "tooltip": "power profile unavailable", "class": "missing", "percentage": 50}'
        exit 0
    fi

    case "$current" in
        "performance") pct=100 ;;
        "balanced") pct=50 ;;
        "power-saver") pct=0 ;;
        *) pct=50 ;;
    esac
    echo "{\"text\": \"\", \"tooltip\": \"Power Profile: $current\", \"class\": \"$current\", \"percentage\": $pct}"
fi
