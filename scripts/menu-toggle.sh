#!/usr/bin/env bash

set -euo pipefail

source "${BASH_SOURCE[0]%/*}/runtime-dir.sh"
exec 8>"$RUNTIME_DIR/hype-menu-toggle.lock"
flock -x 8

if pkill -u "$UID" -x wofi 2>/dev/null; then
    exit 0
fi

"$@" <&0 8>&- &
child=$!
# Keep a second press queued until Wofi starts or the command exits.
while kill -0 "$child" 2>/dev/null && ! pgrep -u "$UID" -x wofi >/dev/null; do
    sleep 0.02
done
flock -u 8
wait "$child"
