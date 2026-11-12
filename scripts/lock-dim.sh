#!/usr/bin/env bash
set -euo pipefail

source "${BASH_SOURCE[0]%/*}/runtime-dir.sh"
lockfile="$RUNTIME_DIR/lock-dim.lock"

exec 8>"$lockfile"
flock -n 8 || exit 0
locker_pid=""
timer_pid=""

# shellcheck disable=SC2329 # Invoked by the EXIT trap.
cleanup() {
    local pid
    for pid in "$timer_pid" "$locker_pid"; do
        [[ -z "$pid" ]] || kill "$pid" 2>/dev/null || true
    done
    for pid in "$timer_pid" "$locker_pid"; do
        [[ -z "$pid" ]] || wait "$pid" 2>/dev/null || true
    done
}
trap 'cleanup' EXIT
trap 'exit 130' INT
trap 'exit 143' TERM

"$HOME/.config/scripts/lock.sh" 8>&- &
locker_pid=$!
sleep 30 8>&- &
timer_pid=$!

status=0
wait -n -p finished "$locker_pid" "$timer_pid" || status=$?
if [[ "$finished" == "$locker_pid" ]]; then
    locker_pid=""
    exit "$status"
fi
timer_pid=""
if ((status == 0)) && pgrep -u "$UID" -x hyprlock >/dev/null 2>&1; then
    niri msg action power-off-monitors || true
fi
status=0
wait "$locker_pid" || status=$?
locker_pid=""
exit "$status"
