#!/usr/bin/env bash

set -euo pipefail

PROFILE="Hype Hotspot"
SSID="Hype-Niri"
action="${1:-status}"

notify() {
    notify-send -a Hotspot "$@" 2>/dev/null || true
}

fail() {
    if [[ "$action" == status ]]; then
        printf '%s\n' '{"text":"󰀂","class":"unavailable","tooltip":"Hotspot unavailable · Open hotspot menu for details"}'
        printf '%s\n' "$1" >&2
        exit 0
    fi
    notify "Hotspot failed" "$1"
    printf '%s\n' "$1" >&2
    exit 1
}

load_profile() {
    uuid="$(nmcli -g connection.uuid connection show id "$PROFILE" 2>/dev/null)" || uuid=""
    active=false
    local active_uuids mode method
    active_uuids="$(nmcli -g UUID connection show --active)" || fail "NetworkManager is unavailable."
    if [[ -n "$uuid" ]]; then
        mode="$(nmcli -g 802-11-wireless.mode connection show uuid "$uuid")"
        method="$(nmcli -g ipv4.method connection show uuid "$uuid")"
        [[ "$mode" == ap && "$method" == shared ]] || fail "The name '$PROFILE' belongs to another connection. Rename it in Network Connections."
        if [[ $'\n'"$active_uuids"$'\n' == *$'\n'"$uuid"$'\n'* ]]; then
            active=true
        fi
    fi
}

start_hotspot() {
    $active && return 0
    local device type interface="" capability password output
    if [[ -z "$uuid" ]]; then
        while IFS=: read -r device type; do
            [[ "$type" == wifi ]] || continue
            capability="$(nmcli -g WIFI-PROPERTIES.AP device show "$device")"
            if [[ "$capability" == yes ]]; then
                interface="$device"
                break
            fi
        done < <(nmcli -t -f DEVICE,TYPE device status)
        [[ -n "$interface" ]] || fail "No Wi-Fi adapter supports hotspot mode."
        password="$(od -An -N12 -tx1 /dev/urandom | tr -d ' \n')"
        if ! output="$(nmcli connection add type wifi ifname "$interface" \
            con-name "$PROFILE" autoconnect no ssid "$SSID" \
            802-11-wireless.mode ap ipv4.method shared ipv6.method disabled \
            wifi-sec.key-mgmt wpa-psk wifi-sec.proto rsn wifi-sec.psk "$password" 2>&1)"; then
            fail "$output"
        fi
        load_profile
    fi
    if ! output="$(nmcli --wait 20 connection up uuid "$uuid" 2>&1)"; then
        fail "$output"
    fi
    notify "Hotspot started" "Right-click the hotspot button to show credentials or edit the network."
}

stop_hotspot() {
    $active || return 0
    local output
    if ! output="$(nmcli --wait 15 connection down uuid "$uuid" 2>&1)"; then
        fail "$output"
    fi
    notify "Hotspot stopped"
}

command -v nmcli >/dev/null 2>&1 || fail "NetworkManager is not installed."

if [[ "$action" == menu ]]; then
    action="$(printf '%s\n' 'Start hotspot' 'Stop hotspot' 'Show credentials' 'Edit hotspot' |
        fuzzel --dmenu --only-match -p 'Hotspot: ' \
            --mesg 'Starting a hotspot may disconnect Wi-Fi on the same adapter.')" || exit 0
    case "$action" in
        'Start hotspot') action=start ;;
        'Stop hotspot') action=stop ;;
        'Show credentials') action=credentials ;;
        'Edit hotspot') action=edit ;;
        *) exit 0 ;;
    esac
fi

case "$action" in
    start|stop|toggle)
        source "${BASH_SOURCE[0]%/*}/runtime-dir.sh"
        exec 9>"$RUNTIME_DIR/hype-hotspot.lock"
        flock -n 9 || exit 0
        ;;
esac

load_profile
case "$action" in
    status)
        if $active; then
            printf '%s\n' '{"text":"󰀂","class":"active","tooltip":"Hotspot on · Click to stop · Right-click for credentials"}'
        else
            printf '%s\n' '{"text":"󰀂","class":"inactive","tooltip":"Hotspot off · Click to start (may disconnect Wi-Fi) · Right-click for menu"}'
        fi
        ;;
    start) start_hotspot ;;
    stop) stop_hotspot ;;
    toggle)
        if $active; then stop_hotspot; else start_hotspot; fi
        ;;
    credentials)
        $active || fail "Start the hotspot before showing its credentials."
        interface="$(nmcli -g GENERAL.DEVICES connection show uuid "$uuid")"
        exec alacritty --hold -e nmcli device wifi show-password ifname "$interface"
        ;;
    edit)
        [[ -n "$uuid" ]] || fail "Start the hotspot once to create its saved profile."
        exec nm-connection-editor --edit="$uuid"
        ;;
    *) printf 'Usage: hotspot.sh {status|menu|toggle|start|stop|credentials|edit}\n' >&2; exit 2 ;;
esac

if [[ "$action" == start || "$action" == stop || "$action" == toggle ]]; then
    pkill -RTMIN+17 -x waybar 2>/dev/null || true
fi
