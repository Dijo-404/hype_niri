#!/usr/bin/env bash

set -euo pipefail

CONFIG_FILE="${NIRI_CONFIG:-${XDG_CONFIG_HOME:-$HOME/.config}/niri/config.kdl}"
CONFIG_DIR="$(dirname -- "$CONFIG_FILE")"
OPACITY_FILE="$CONFIG_DIR/opacity.kdl"
source "${BASH_SOURCE[0]%/*}/runtime-dir.sh"
NOTIFY_ID=2007

notify() {
    command -v notify-send >/dev/null 2>&1 || return 0
    notify-send -r "$NOTIFY_ID" "$1" "${2:-}" 2>/dev/null || true
}

if command -v flock >/dev/null 2>&1; then
    exec 9>"$RUNTIME_DIR/hype-opacity-toggle.lock"
    flock -n 9 || exit 0
fi

if [ ! -f "$CONFIG_FILE" ]; then
    notify "Opacity toggle unavailable" "Niri config not found: $CONFIG_FILE"
    exit 1
fi

mkdir -p "$CONFIG_DIR"
tmp_file="$(mktemp "$CONFIG_DIR/.opacity.kdl.XXXXXX")"
backup_file="$(mktemp "$CONFIG_DIR/.opacity-backup.kdl.XXXXXX")"
trap 'rm -f "$tmp_file" "$backup_file"' EXIT

if [ -f "$OPACITY_FILE" ]; then
    cp -- "$OPACITY_FILE" "$backup_file"
else
    : >"$backup_file"
fi

if grep -Eq '^[[:space:]]*opacity[[:space:]]+1([.]0+)?[[:space:]]*$' "$OPACITY_FILE" 2>/dev/null; then
    state="enabled"
    active_opacity="0.95"
    inactive_opacity="0.90"
else
    state="disabled"
    active_opacity="1.0"
    inactive_opacity="1.0"
fi

printf '%s\n' \
    'window-rule {' \
    '    match is-active=true' \
    '    draw-border-with-background false' \
    "    opacity $active_opacity" \
    '}' \
    '' \
    'window-rule {' \
    '    match is-active=false' \
    '    draw-border-with-background false' \
    "    opacity $inactive_opacity" \
    '}' >"$tmp_file"

mv -- "$tmp_file" "$OPACITY_FILE"

if ! niri validate -c "$CONFIG_FILE" >/dev/null 2>&1; then
    mv -- "$backup_file" "$OPACITY_FILE"
    notify "Opacity toggle failed" "The generated Niri configuration was invalid."
    exit 1
fi

if ! niri msg action load-config-file >/dev/null 2>&1; then
    mv -- "$backup_file" "$OPACITY_FILE"
    niri msg action load-config-file >/dev/null 2>&1 || true
    notify "Opacity toggle failed" "Niri could not reload its configuration."
    exit 1
fi

if [ "$state" = "disabled" ]; then
    notify "Window opacity disabled" "Windows are now fully opaque."
else
    notify "Window opacity enabled" "Active: 95% · Inactive: 90%"
fi
