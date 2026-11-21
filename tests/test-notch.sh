#!/usr/bin/env bash

set -euo pipefail
REPO="$(cd -- "${BASH_SOURCE[0]%/*}/.." && pwd -P)"
TEMP_DIR="$(mktemp -d)"
trap 'rm -rf -- "$TEMP_DIR"' EXIT
compile_flags="$(pkg-config --cflags --libs gtk+-3.0 gtk-layer-shell-0 playerctl wayland-client)"
read -r -a flags <<<"$compile_flags"
protocol="$(pkg-config --variable=pkgdatadir wayland-protocols)/staging/ext-background-effect/ext-background-effect-v1.xml"
wayland-scanner client-header "$protocol" "$TEMP_DIR/ext-background-effect-v1-client-protocol.h"
wayland-scanner private-code "$protocol" "$TEMP_DIR/ext-background-effect-v1-protocol.c"
cc -std=c11 -O2 -Wall -Wextra -Werror -Wno-deprecated-declarations -I"$TEMP_DIR" \
    "$REPO/tests/test-notch.c" "$TEMP_DIR/ext-background-effect-v1-protocol.c" \
    -o "$TEMP_DIR/test-notch" "${flags[@]}"
mkdir "$TEMP_DIR/bin"
printf '#!/bin/sh\nprintf %%s "Partly cloudy|+21°C|+20°C|40%%|Testville"\n' >"$TEMP_DIR/bin/curl"
chmod +x "$TEMP_DIR/bin/curl"
cat >"$TEMP_DIR/bin/wpctl" <<'FAKE'
#!/bin/sh
[ "$1" = get-volume ] && echo "Volume: 0.40" && exit 0
echo "wpctl $*" >>"$NOTCH_LEVEL_LOG"
FAKE
cat >"$TEMP_DIR/bin/brightnessctl" <<'FAKE'
#!/bin/sh
[ "$1" = -m ] && echo "intel_backlight,backlight,208,52%,400" && exit 0
echo "brightnessctl $*" >>"$NOTCH_LEVEL_LOG"
FAKE
chmod +x "$TEMP_DIR/bin/wpctl" "$TEMP_DIR/bin/brightnessctl"
export NOTCH_LEVEL_LOG="$TEMP_DIR/levels.log"
PATH="$TEMP_DIR/bin:$PATH" GIO_USE_VFS=local GSETTINGS_BACKEND=memory GTK_USE_PORTAL=0 NO_AT_BRIDGE=1 \
    "$TEMP_DIR/test-notch" "$REPO/scripts"
XDG_CACHE_HOME="$TEMP_DIR/cache" bash "$REPO/scripts/notch.sh" --check
binary="$(find "$TEMP_DIR/cache" -type f -name 'notch-*' -print -quit)"
[[ -x "$binary" ]]
before="$(stat -c '%y' "$binary")"
XDG_CACHE_HOME="$TEMP_DIR/cache" bash "$REPO/scripts/notch.sh" --check
[[ "$(stat -c '%y' "$binary")" == "$before" ]]
if XDG_CACHE_HOME="$TEMP_DIR/cache" bash "$REPO/scripts/notch.sh" --invalid-option >/dev/null 2>&1; then
    echo "Invalid notch option unexpectedly succeeded" >&2
    exit 1
fi
echo "Notch build cache and invalid-option checks passed"
