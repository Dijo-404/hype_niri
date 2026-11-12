#!/usr/bin/env bash
set -euo pipefail

source "${BASH_SOURCE[0]%/*}/runtime-dir.sh"
state_file="$RUNTIME_DIR/hype-copilot-displays.state"
exec 9>"$RUNTIME_DIR/hype-copilot-displays.lock"
flock -x 9

mode="${1:-toggle}"
case "$mode" in
    toggle|resume) ;;
    *) echo "Usage: $0 [toggle|resume]" >&2; exit 2 ;;
esac

if [[ "$mode" == resume ]]; then
    (
        flock -x 8
        rm -f -- "$RUNTIME_DIR/hype-copilot.state"
    ) 8>"$RUNTIME_DIR/hype-copilot.lock"
elif [[ -n "${HYPE_COPILOT_TOKEN:-}" ]]; then
    (
        flock -x 8
        read -r token phase 2>/dev/null < "$RUNTIME_DIR/hype-copilot.state" || exit 1
        [[ "$token" == "$HYPE_COPILOT_TOKEN" && "$phase" == double ]]
    ) 8>"$RUNTIME_DIR/hype-copilot.lock" || exit 0
fi

notify_failure() {
    printf '%s\n' "$1" >&2
    command -v notify-send >/dev/null 2>&1 || return 0
    notify-send "Copilot display shortcut" "$1" 2>/dev/null || true
}

locked_hint() {
    [[ -n "${XDG_SESSION_ID:-}" ]] || return 1
    loginctl show-session "$XDG_SESSION_ID" -p LockedHint --value
}

ensure_locked() {
    local hint attempt
    hint="$(locked_hint)" || return 1
    [[ "$hint" == yes || "$hint" == no ]] || return 1
    if [[ "$hint" == yes ]] && pgrep -u "$UID" -x hyprlock >/dev/null 2>&1; then
        return 0
    fi

    "$HOME/.config/scripts/lock.sh" sleep 9>&- >/dev/null 2>&1 &
    for ((attempt = 0; attempt < 50; attempt++)); do
        hint="$(locked_hint)" || return 1
        [[ "$hint" == yes || "$hint" == no ]] || return 1
        if [[ "$hint" == yes ]] && pgrep -u "$UID" -x hyprlock >/dev/null 2>&1; then
            return 0
        fi
        sleep 0.1
    done
    return 1
}

wait_outputs() {
    local desired="$1" attempt current
    for ((attempt = 0; attempt < 50; attempt++)); do
        current="$(niri msg --json outputs)" || return 1
        if jq -e --rawfile names "$state_file" --arg desired "$desired" '
            . as $outputs
            | ($names | split("\n") | map(select(length > 0))) as $saved
            | ($saved | length) > 0
              and all($saved[]; . as $name
                | $outputs[$name] == null
                  or (if $desired == "on" then $outputs[$name].current_mode != null
                      else $outputs[$name].current_mode == null end))
              and (if $desired == "on" then any($outputs[]; .current_mode != null) else true end)
        ' <<< "$current" >/dev/null; then
            return 0
        fi
        sleep 0.1
    done
    return 1
}

restore_outputs() {
    local output connected failed=0
    connected="$(niri msg --json outputs)" || return 1
    while IFS= read -r output; do
        [[ -n "$output" ]] || continue
        jq -e --arg name "$output" 'has($name)' <<< "$connected" >/dev/null || continue
        niri msg output "$output" on || failed=1
    done < "$state_file"
    niri msg action power-on-monitors || failed=1
    wait_outputs on || failed=1
    (( failed == 0 ))
}

if [[ -f "$state_file" || "$mode" == resume ]]; then
    if ! ensure_locked; then
        notify_failure "Could not confirm the session lock; displays remain off."
        exit 1
    fi
    if [[ ! -f "$state_file" ]]; then
        niri msg action power-on-monitors
    elif restore_outputs; then
        rm -f -- "$state_file"
    else
        notify_failure "Could not restore all displays. Double-press Copilot with Super to retry."
        exit 1
    fi
    exit 0
fi

if ! hint="$(locked_hint)" || [[ "$hint" != yes && "$hint" != no ]]; then
    notify_failure "Could not read the session state; displays were left on."
    exit 1
fi

outputs_json="$(niri msg --json outputs)"
if ! outputs="$(jq -er '[.[] | select(.current_mode != null) | .name] | if length == 0 then error("No enabled displays") else .[] end' <<< "$outputs_json")"; then
    notify_failure "No enabled displays could be saved."
    exit 1
fi

tmp_file="$(mktemp "$RUNTIME_DIR/.hype-copilot-displays.XXXXXX")"
trap 'rm -f -- "$tmp_file"' EXIT
printf '%s\n' "$outputs" > "$tmp_file"
mv -f -- "$tmp_file" "$state_file"

failed=0
while IFS= read -r output; do
    if ! niri msg output "$output" off; then
        failed=1
        break
    fi
done <<< "$outputs"
if (( failed )) || ! wait_outputs off; then
    if ensure_locked && restore_outputs; then
        rm -f -- "$state_file"
    fi
    notify_failure "Could not turn off every display. Double-press Copilot with Super to retry."
    exit 1
fi

sleep 1
if ! ensure_locked; then
    notify_failure "Could not confirm the session lock; displays remain off. Double-press Copilot with Super to retry."
    exit 1
fi
