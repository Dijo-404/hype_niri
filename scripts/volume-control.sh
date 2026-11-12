#!/usr/bin/env bash

set -euo pipefail

command -v wpctl >/dev/null 2>&1 || exit 0
source "${BASH_SOURCE[0]%/*}/runtime-dir.sh"
exec 9>"$RUNTIME_DIR/hype-volume-control.lock"
flock -w 1 9 || exit 0

ID=2001

case "${1:-}" in
    up)
        wpctl set-volume -l 1 @DEFAULT_AUDIO_SINK@ 1%+ 2>/dev/null || exit 0
        ;;
    down)
        wpctl set-volume @DEFAULT_AUDIO_SINK@ 1%- 2>/dev/null || exit 0
        ;;
    mute)
        wpctl set-mute @DEFAULT_AUDIO_SINK@ toggle 2>/dev/null || exit 0
        ;;
esac

vol_info=$(wpctl get-volume @DEFAULT_AUDIO_SINK@ 2>/dev/null) || exit 0
read -r _ volume _ <<<"$vol_info"
[[ "$volume" =~ ^([0-9]+)([.]([0-9]+))?$ ]] || exit 0
fraction="${BASH_REMATCH[3]:-}00"
vol=$((10#${BASH_REMATCH[1]} * 100 + 10#${fraction:0:2}))

command -v notify-send >/dev/null 2>&1 || exit 0

if [[ "$vol_info" == *MUTED* ]]; then
    notify-send -r "$ID" \
        -h string:x-canonical-private-synchronous:volume \
        "󰝟  Muted" 2>/dev/null || true
else
    if [ "$vol" -lt 30 ]; then
        icon="󰕿"
    elif [ "$vol" -lt 70 ]; then
        icon="󰖀"
    else
        icon="󰕾"
    fi

    notify-send -r "$ID" \
        -h string:x-canonical-private-synchronous:volume \
        -h int:value:"$vol" \
        "$icon  Volume: ${vol}%" 2>/dev/null || true
fi
