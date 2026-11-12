#!/usr/bin/env bash
set -euo pipefail

CONFIG_FILE="${WAYBAR_CONFIG:-$HOME/.config/waybar/config.jsonc}"
source "${BASH_SOURCE[0]%/*}/runtime-dir.sh"
exec 9>"$RUNTIME_DIR/hype-waybar.lock"
flock -w 3 9 || exit 0
pgrep -u "$UID" -x waybar >/dev/null 2>&1 && exit 0

exec env GTK_THEME="${GTK_THEME:-Adwaita:dark}" waybar --config "$CONFIG_FILE"
