#!/usr/bin/env bash

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ACTION="${1:-}"

close_wlogout() {
    pkill -x wlogout >/dev/null 2>&1 || true
}

stop_caffeine() {
    "$SCRIPT_DIR/caffeine-control.sh" stop >/dev/null 2>&1 || true
}

case "$ACTION" in
    lock)
        close_wlogout
        exec "$SCRIPT_DIR/lock-screen.sh"
        ;;
    suspend)
        stop_caffeine
        close_wlogout
        exec "$SCRIPT_DIR/suspend-now.sh"
        ;;
    logout)
        stop_caffeine
        close_wlogout
        exec niri msg action quit --skip-confirmation
        ;;
    shutdown)
        stop_caffeine
        close_wlogout
        exec systemctl poweroff
        ;;
    reboot | restart)
        stop_caffeine
        close_wlogout
        exec systemctl reboot
        ;;
    *)
        echo "Usage: $0 {lock|suspend|logout|shutdown|reboot}" >&2
        exit 2
        ;;
esac
