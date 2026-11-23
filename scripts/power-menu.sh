#!/usr/bin/env bash

set -euo pipefail

SCRIPT_DIR="${BASH_SOURCE[0]%/*}"

stop_caffeine() {
    "$SCRIPT_DIR/hype-control" caffeine stop >/dev/null 2>&1 || true
}

choice="$(printf "<span font_family='JetBrainsMono Nerd Font'>%s</span>  %s\n" \
    $'\U000f033e' Lock $'\U000f04b2' Sleep $'\U000f0343' Logout \
    $'\U000f0425' Shutdown $'\U000f0709' Reboot |
    wofi --dmenu --no-custom-entry --cache-file /dev/null --allow-markup \
        --define dmenu-print_line_num=true -p 'Power')" || exit 0

case "$choice" in
    0) exec "$SCRIPT_DIR/hype-control" lock-screen ;;
    1) exec "$SCRIPT_DIR/hype-control" suspend-now ;;
    2)
        stop_caffeine
        exec niri msg action quit --skip-confirmation
        ;;
    3)
        stop_caffeine
        exec systemctl poweroff
        ;;
    4)
        stop_caffeine
        exec systemctl reboot
        ;;
esac
