#!/usr/bin/env bash
set -euo pipefail

umask 077
RUNTIME_DIR="$("${BASH_SOURCE[0]%/*}/hype-control" runtime-dir)" || exit 1
exec 9>"$RUNTIME_DIR/hype-tray-applets.lock"
flock -n 9 || exit 0

export GTK_THEME="${GTK_THEME:-Adwaita:dark}"

start_once() {
    local process="$1"
    shift

    command -v "$1" >/dev/null 2>&1 || return 0
    pgrep -u "$UID" -x "$process" >/dev/null 2>&1 && return 0
    exec 8>"$RUNTIME_DIR/hype-tray-$process.lock"
    if ! flock -n 8; then
        exec 8>&-
        return 0
    fi

    "$@" >/dev/null 2>&1 9>&- &
    exec 8>&-
}

start_once "nm-applet" nm-applet --indicator
start_once "blueman-applet" blueman-applet
start_once "warp-taskbar" warp-taskbar
