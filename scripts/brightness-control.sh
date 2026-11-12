#!/usr/bin/env bash

set -euo pipefail

command -v brightnessctl >/dev/null 2>&1 || exit 0

ID=2000
source "${BASH_SOURCE[0]%/*}/runtime-dir.sh"
LOCK_FILE="$RUNTIME_DIR/brightness-control.lock"

if command -v flock >/dev/null 2>&1; then
    exec 9>"$LOCK_FILE"
    flock -n 9 || exit 0
fi

smooth_set() {
    local current=$1
    local target=$2
    local diff
    local abs_diff
    local steps=8
    local i
    local next
    local last=""

    if [ "$target" -eq "$current" ]; then
        return
    fi

    diff=$((target - current))
    abs_diff=${diff#-}
    [ "$abs_diff" -lt "$steps" ] && steps=$abs_diff
    [ "$steps" -lt 1 ] && steps=1

    for ((i = 1; i <= steps; i++)); do
        next=$((current + diff * i / steps))
        [ "$next" = "$last" ] && continue
        brightnessctl -q set "$next"
        last="$next"
        if [ "$i" -lt "$steps" ]; then
            sleep 0.005
        fi
    done
}

case "${1:-}" in
    up)
        current=$(brightnessctl get)
        max=$(brightnessctl max)
        step=$((max / 100))
        [ "$step" -lt 1 ] && step=1
        target=$((current + step))
        [ "$target" -gt "$max" ] && target=$max
        smooth_set "$current" "$target"
        ;;
    down)
        current=$(brightnessctl get)
        max=$(brightnessctl max)
        min=$((max / 100))
        [ "$min" -lt 1 ] && min=1
        step=$((max / 100))
        [ "$step" -lt 1 ] && step=1
        if [ "$current" -le "$min" ]; then
            target=$current
        else
            target=$((current - step))
        fi
        [ "$target" -lt "$min" ] && target=$min
        smooth_set "$current" "$target"
        ;;
    *)
        echo "Usage: brightness-control.sh {up|down}" >&2
        exit 1
        ;;
esac

current=$(brightnessctl get)
max=$(brightnessctl max)
if [ "$max" -gt 0 ]; then
    percent=$(((current * 100 + max / 2) / max))
else
    percent=0
fi

if [ "$percent" -lt 30 ]; then
    icon="󰃞"
elif [ "$percent" -lt 70 ]; then
    icon="󰃟"
else
    icon="󰃠"
fi

command -v notify-send >/dev/null 2>&1 || exit 0
notify-send -r "$ID" \
    -h string:x-canonical-private-synchronous:brightness \
    -h int:value:"$percent" \
    "$icon  Brightness: ${percent}%" 2>/dev/null || true
