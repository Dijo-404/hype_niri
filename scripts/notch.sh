#!/usr/bin/env bash

set -euo pipefail

SCRIPT_DIR="$(cd -- "${BASH_SOURCE[0]%/*}" && pwd -P)"
source "$SCRIPT_DIR/runtime-dir.sh"
CACHE_DIR="${XDG_CACHE_HOME:-$HOME/.cache}/hype-niri/notch"
mkdir -p -- "$CACHE_DIR"
key="$(printf '%s' "$SCRIPT_DIR" | sha256sum)"
BINARY="$CACHE_DIR/notch-${key%% *}"

exec 9>"$RUNTIME_DIR/hype-notch-build.lock"
flock -w 30 9 || exit 1
if [[ ! -x "$BINARY" || "$SCRIPT_DIR/notch.c" -nt "$BINARY" ]]; then
    for tool in cc pkg-config wayland-scanner; do
        command -v "$tool" >/dev/null 2>&1 || {
            echo "Niri Notch needs base-devel, gtk-layer-shell, playerctl and wayland-protocols." >&2
            exit 1
        }
    done
    compile_flags="$(pkg-config --cflags --libs gtk+-3.0 gtk-layer-shell-0 playerctl wayland-client)"
    read -r -a flags <<<"$compile_flags"
    protocol="$(pkg-config --variable=pkgdatadir wayland-protocols)/staging/ext-background-effect/ext-background-effect-v1.xml"
    tmp="$(mktemp "$CACHE_DIR/.notch.XXXXXX")"
    gen="$(mktemp -d "$CACHE_DIR/.gen.XXXXXX")"
    trap 'rm -rf -- "$tmp" "$gen"' EXIT
    wayland-scanner client-header "$protocol" "$gen/ext-background-effect-v1-client-protocol.h"
    wayland-scanner private-code "$protocol" "$gen/ext-background-effect-v1-protocol.c"
    cc -std=c11 -O2 -Wall -Wextra -Werror -Wno-deprecated-declarations -I"$gen" \
        "$SCRIPT_DIR/notch.c" "$gen/ext-background-effect-v1-protocol.c" -o "$tmp" "${flags[@]}"
    chmod 700 -- "$tmp"
    mv -f -- "$tmp" "$BINARY"
    rm -rf -- "$gen"
    trap - EXIT
fi
exec 9>&-
exec "$BINARY" "$SCRIPT_DIR" "$@"
