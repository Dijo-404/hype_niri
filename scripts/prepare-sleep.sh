#!/usr/bin/env bash
set -euo pipefail

case "${1:-}" in
    --watch)
        source "${BASH_SOURCE[0]%/*}/runtime-dir.sh"
        exec 9>"$RUNTIME_DIR/hype-sleep-inhibitor.lock"
        flock -n 9 || exit 0
        # swayidle holds the inhibitor before sleep; HOME expands in its hook's shell.
        # shellcheck disable=SC2016
        exec swayidle -w -C /dev/null before-sleep '"$HOME/.config/scripts/prepare-sleep.sh"'
        ;;
    "") ;;
    *) echo "Usage: $0 [--watch]" >&2; exit 2 ;;
esac

lock_ready() {
    local hint
    [[ -n "${XDG_SESSION_ID:-}" ]] || return 1
    hint="$(loginctl show-session "$XDG_SESSION_ID" -p LockedHint --value 2>/dev/null)" || return 1
    [[ "$hint" == yes ]] && pgrep -u "$UID" -x hyprlock >/dev/null 2>&1
}

lock_ready && exit 0
if ! pgrep -u "$UID" -x hyprlock >/dev/null 2>&1; then
    "$HOME/.config/scripts/lock.sh" sleep 9>&- >/dev/null 2>&1 &
fi

for ((attempt = 0; attempt < 50; attempt++)); do
    lock_ready && exit 0
    sleep 0.1
done

# logind may force sleep at InhibitDelayMaxSec even while this command is waiting.
echo "Could not confirm the session lock before sleep." >&2
exit 1
