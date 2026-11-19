#!/usr/bin/env bash

set -euo pipefail

style="${XDG_CONFIG_HOME:-$HOME/.config}/wofi/style.css"
source "${BASH_SOURCE[0]%/*}/runtime-dir.sh"
prompt_style="$(mktemp "$RUNTIME_DIR/wofi-password.XXXXXX.css")"
trap 'rm -f -- "$prompt_style"' EXIT

cat "$style" >"$prompt_style"
printf '%s\n' \
    '#input { margin: 2px 20px; min-height: 60px; font-feature-settings: "liga=0,calt=0"; }' \
    '#scroll, #inner-box { margin: 0; padding: 0; min-height: 0; }' \
    '#entry { margin: 0; padding: 0; min-height: 0; font-size: 0; }' >>"$prompt_style"
wofi --dmenu --password --height 64 --cache-file /dev/null --style "$prompt_style" "$@"
