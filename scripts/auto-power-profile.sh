#!/usr/bin/env bash

set -euo pipefail

UPOWER=org.freedesktop.UPower
PROFILES=org.freedesktop.UPower.PowerProfiles
PROFILES_PATH=/org/freedesktop/UPower/PowerProfiles
CHANGED="type='signal',interface='org.freedesktop.DBus.Properties',member='PropertiesChanged'"
OWNER="type='signal',sender='org.freedesktop.DBus',member='NameOwnerChanged'"

on_battery="" present="" kind="" percentage="" active="" available="" policy="" target=""
last_active="" last_key=""

property() {
    busctl --system --timeout=5 get-property "$@" 2>/dev/null
}

requested_profile() {
    local whole
    if [[ "$1" == false ]]; then
        printf performance
    elif [[ "$1" == true && "$2" =~ ^[0-9]{1,3}([.][0-9]+)?$ ]]; then
        whole=$((10#${2%%.*}))
        ((whole <= 100)) || return 0
        if ((whole < 30)); then printf power-saver; else printf balanced; fi
    fi
}

read_state() {
    local reply profiles="" name
    on_battery="" present="" kind="" percentage="" active="" available="" policy="" target=""

    reply="$(property "$UPOWER" /org/freedesktop/UPower "$UPOWER" OnBattery)" && on_battery="${reply#b }"
    if reply="$(property "$UPOWER" /org/freedesktop/UPower/devices/DisplayDevice "$UPOWER.Device" IsPresent Type Percentage)"; then
        { read -r _ present && read -r _ kind && read -r _ percentage; } <<<"$reply" || true
    fi
    if reply="$(property "$PROFILES" "$PROFILES_PATH" "$PROFILES" ActiveProfile Profiles)"; then
        { read -r _ active && read -r profiles; } <<<"$reply" || true
        active="${active//\"/}"
        for name in power-saver balanced performance; do
            [[ "$profiles" != *"\"Profile\" s \"$name\""* ]] || available+="$name "
        done
    fi

    if [[ "$present" == true && "$kind" == 2 ]]; then
        policy="$(requested_profile "$on_battery" "$percentage")"
    fi
    [[ -n "$policy" ]] || return 0
    for name in "$policy" balanced power-saver; do
        if [[ " $available" == *" $name "* ]]; then
            target="$name"
            return 0
        fi
    done
}

notify_waybar() {
    [[ "$active" != "$last_active" ]] || return 0
    last_active="$active"
    [[ -z "$active" ]] || pkill -RTMIN+16 -u "$UID" -x waybar || true
}

set_profile() {
    busctl --system --timeout=5 --allow-interactive-authorization=no call \
        "$PROFILES" "$PROFILES_PATH" org.freedesktop.DBus.Properties Set ssv \
        "$PROFILES" ActiveProfile s "$1" >/dev/null
}

apply() {
    local delay key
    for delay in 0 1 3; do
        ((delay == 0)) || sleep "$delay"
        read_state
        notify_waybar
        key="$policy|$available"
        if [[ -z "$target" ]]; then
            last_key=""
            return 0
        fi
        if ((delay == 0)); then
            [[ "$key" != "$last_key" ]] || return 0
            last_key="$key"
        elif [[ "$key" != "$last_key" ]]; then
            apply
            return 0
        fi
        [[ "$active" != "$target" ]] || return 0
        if set_profile "$target"; then
            printf 'Power profile: %s\n' "$target"
            return 0
        fi
        printf 'Could not set %s\n' "$target" >&2
    done
}

if [[ "${1:-}" == --check ]]; then
    read_state
    printf '%s=%s\n' on_battery "$on_battery" percentage "$percentage" \
        available "${available% }" active "$active" policy "$policy" target "$target"
    exit 0
fi

source "${BASH_SOURCE[0]%/*}/runtime-dir.sh"
exec 9>"$RUNTIME_DIR/hype-auto-power-profile.lock"
flock -n 9 || exit 0

# NameAcquired arrives after dbus-monitor installs its signal subscriptions.
while IFS= read -r line; do
    dirty=0
    while :; do
        case "$line" in
            *'"OnBattery"'* | *'"IsPresent"'* | *'"Type"'* | *'"Percentage"'* | *'"ActiveProfile"'* | *'"Profiles"'*)
                dirty=1
                ;;
            *member=NameAcquired* | *member=NameOwnerChanged*)
                dirty=1
                last_key=""
                ;;
        esac
        IFS= read -r -t 0.1 line || break
    done
    ((dirty == 0)) || apply
done < <(
    exec dbus-monitor --system \
        "$CHANGED,path='/org/freedesktop/UPower'" \
        "$CHANGED,path='/org/freedesktop/UPower/devices/DisplayDevice'" \
        "$CHANGED,path='$PROFILES_PATH'" \
        "$OWNER,arg0='$UPOWER'" \
        "$OWNER,arg0='$PROFILES'" 2>/dev/null 9>&-
)
exit 1
