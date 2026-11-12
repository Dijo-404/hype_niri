#!/usr/bin/env bash

set -euo pipefail

source "${BASH_SOURCE[0]%/*}/runtime-dir.sh"
STATE_FILE="$RUNTIME_DIR/caffeine_state"
PID_FILE="$RUNTIME_DIR/caffeine_pid"
LOCK_FILE="$RUNTIME_DIR/caffeine.lock"
SCRIPT_FILE="$(cd -- "${BASH_SOURCE[0]%/*}" && pwd -P)/${BASH_SOURCE[0]##*/}"
ID=2002

process_starttime() {
    local stat
    local -a fields=()
    IFS= read -r stat < "/proc/$1/stat" 2>/dev/null || return 1
    read -r -a fields <<< "${stat##*) }"
    printf '%s' "${fields[19]:-}"
}

if [ "${1:-}" == hold ]; then
    # systemd-inhibit starts this child only after acquiring the idle inhibitor.
    printf '%s %s\n' "$$" "$(process_starttime "$$")" > "$2"
    exec sleep infinity
fi

if command -v flock >/dev/null 2>&1; then
    exec 9>"$LOCK_FILE"
    flock -w 2 9 || exit 0
fi

notify() {
    command -v notify-send >/dev/null 2>&1 || return 0
    notify-send -r "$ID" "$@" 2>/dev/null || true
}

read_pid_file() {
    local pid=""
    if [ -f "$PID_FILE" ]; then
        read -r pid < "$PID_FILE" 2>/dev/null || pid=""
        [[ "$pid" =~ ^[0-9]+$ ]] || pid=""
    fi
    printf '%s' "$pid"
}

is_inhibitor_pid() {
    local pid="$1"
    local -a cmdline=()
    local arg found_who=0 found_holder=0
    [[ "$pid" =~ ^[0-9]+$ ]] || return 1
    [ -O "/proc/$pid" ] || return 1
    [ -r "/proc/$pid/cmdline" ] || return 1
    mapfile -d '' -t cmdline < "/proc/$pid/cmdline" 2>/dev/null || return 1
    (( ${#cmdline[@]} >= 2 )) || return 1
    [[ "${cmdline[0]##*/}" == systemd-inhibit ]] || return 1
    INHIBITOR_WHAT=""
    INHIBITOR_READY_FILE=""
    for arg in "${cmdline[@]}"; do
        [[ "$arg" != '--who=Caffeine Mode' ]] || found_who=1
        [[ "$arg" != --what=* ]] || INHIBITOR_WHAT="${arg#--what=}"
    done
    if (( ${#cmdline[@]} >= 2 )); then
        [[ "${cmdline[-2]}" != sleep || "${cmdline[-1]}" != infinity ]] || found_holder=1
    fi
    if (( ${#cmdline[@]} >= 5 )) && [[ "${cmdline[-5]}" == -- && "${cmdline[-4]}" == bash && "${cmdline[-3]}" == "$SCRIPT_FILE" && "${cmdline[-2]}" == hold && "${cmdline[-1]}" == "$RUNTIME_DIR"/caffeine-ready.* ]]; then
        INHIBITOR_READY_FILE="${cmdline[-1]}"
        found_holder=1
    fi
    (( found_who && found_holder ))
}

is_current_inhibitor_pid() {
    local pid="$1"
    is_inhibitor_pid "$pid" || return 1
    [[ "$INHIBITOR_WHAT" == idle && -n "$INHIBITOR_READY_FILE" && -s "$INHIBITOR_READY_FILE" ]]
}

stop_ready_holder() {
    local ready_file="$1" child="" starttime=""
    local -a cmdline=()
    [ -f "$ready_file" ] || return 0
    read -r child starttime < "$ready_file" || return 0
    [[ "$child" =~ ^[0-9]+$ && "$starttime" =~ ^[0-9]+$ ]] || return 0
    [ -O "/proc/$child" ] && [ "$(process_starttime "$child")" == "$starttime" ] || return 0
    mapfile -d '' -t cmdline < "/proc/$child/cmdline" 2>/dev/null || return 0
    (( ${#cmdline[@]} >= 2 )) || return 0
    if [[ "${cmdline[0]##*/}" == sleep && "${cmdline[1]:-}" == infinity ]] ||
        [[ "${cmdline[0]##*/}" == bash && "${cmdline[1]:-}" == "$SCRIPT_FILE" && "${cmdline[2]:-}" == hold && "${cmdline[3]:-}" == "$ready_file" ]]; then
        kill "$child" 2>/dev/null || true
    fi
}

start_inhibitor() {
    stop_inhibitor
    command -v systemd-inhibit >/dev/null 2>&1 || return 1

    local ready_file pid attempt
    ready_file=$(mktemp "$RUNTIME_DIR/caffeine-ready.XXXXXX") || return 1
    (
        exec 9>&-
        systemd-inhibit --what=idle --who="Caffeine Mode" --why="User requested stay awake" --mode=block -- bash "$SCRIPT_FILE" hold "$ready_file" >/dev/null 2>&1 &
        pid=$!
        printf '%s\n' "$pid" > "$PID_FILE"
        wait "$pid" || true
        stop_ready_holder "$ready_file"
        if command -v flock >/dev/null 2>&1; then
            exec 9>"$LOCK_FILE"
            flock 9
        fi
        if [ "$(read_pid_file)" == "$pid" ]; then
            rm -f "$PID_FILE" "$STATE_FILE"
        fi
        rm -f "$ready_file"
        pkill -RTMIN+15 waybar || true
    ) </dev/null >/dev/null 2>&1 &
    for (( attempt=0; attempt<100; attempt++ )); do
        pid=$(read_pid_file)
        if [ -n "$pid" ]; then
            kill -0 "$pid" 2>/dev/null || break
            [ ! -s "$ready_file" ] || return 0
        fi
        sleep 0.02
    done
    stop_inhibitor
    rm -f "$STATE_FILE" "$ready_file"
    return 1
}

stop_inhibitor() {
    local existing_pid child ready_file=""
    local -a child_cmd=() children=()
    existing_pid=$(read_pid_file)
    if [ -n "$existing_pid" ] && is_inhibitor_pid "$existing_pid" && kill -0 "$existing_pid" 2>/dev/null; then
        ready_file="$INHIBITOR_READY_FILE"
        read -r -a children < "/proc/$existing_pid/task/$existing_pid/children" 2>/dev/null || true
        for child in "${children[@]}"; do
            [ -O "/proc/$child" ] && [ -r "/proc/$child/cmdline" ] || continue
            mapfile -d '' -t child_cmd < "/proc/$child/cmdline" 2>/dev/null || continue
            (( ${#child_cmd[@]} >= 2 )) || continue
            if [[ "${child_cmd[0]##*/}" == sleep && "${child_cmd[1]:-}" == infinity ]] ||
                [[ "${child_cmd[0]##*/}" == bash && "${child_cmd[1]:-}" == "$SCRIPT_FILE" && "${child_cmd[2]:-}" == hold && "${child_cmd[3]:-}" == "$ready_file" ]]; then
                kill "$child" 2>/dev/null || true
            fi
        done
        kill "$existing_pid" 2>/dev/null || true
    fi
    rm -f "$PID_FILE"
}

action="${1:-status}"

if [ "$action" == "stop" ]; then
    rm -f "$STATE_FILE"
    stop_inhibitor
    pkill -RTMIN+15 waybar || true
elif [ "$action" == "toggle" ]; then
    if [ -f "$STATE_FILE" ]; then
        rm -f "$STATE_FILE"
        stop_inhibitor
        notify "󰾪  Caffeine Mode Deactivated" "Idle lock and display sleep restored"
        echo '{"text": "󰾪", "tooltip": "Caffeine: Off", "class": "deactivated"}'
    else
        if start_inhibitor; then
            : > "$STATE_FILE"
            notify "󰅶  Caffeine Mode Active" "Idle lock and display sleep paused"
            echo '{"text": "󰅶", "tooltip": "Caffeine: On", "class": "activated"}'
        else
            notify "Caffeine unavailable" "Could not acquire the idle inhibitor"
            echo '{"text": "󰾪", "tooltip": "Caffeine: Off", "class": "deactivated"}'
        fi
    fi
    pkill -RTMIN+15 waybar || true
else
    if [ -f "$STATE_FILE" ]; then
        existing_pid=$(read_pid_file)
        if [ -z "$existing_pid" ] || ! is_inhibitor_pid "$existing_pid" || ! kill -0 "$existing_pid" 2>/dev/null; then
            rm -f "$STATE_FILE"
            stop_inhibitor
            echo '{"text": "󰾪", "tooltip": "Caffeine: Off", "class": "deactivated"}'
            exit 0
        elif ! is_current_inhibitor_pid "$existing_pid"; then
            if ! start_inhibitor; then
                rm -f "$STATE_FILE"
                echo '{"text": "󰾪", "tooltip": "Caffeine: Off", "class": "deactivated"}'
                exit 0
            fi
        fi
        echo '{"text": "󰅶", "tooltip": "Caffeine: On", "class": "activated"}'
    else
        echo '{"text": "󰾪", "tooltip": "Caffeine: Off", "class": "deactivated"}'
    fi
fi
