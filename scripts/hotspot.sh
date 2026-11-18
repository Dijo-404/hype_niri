#!/usr/bin/env bash

set -euo pipefail
export LC_ALL=C

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
    local device type interface="" capability password output configured
    local -a saved
    local -a settings=(
        802-11-wireless.band a 802-11-wireless.channel 36
        wifi-sec.key-mgmt wpa-psk wifi-sec.proto rsn wifi-sec.pmf optional
        wifi-sec.pairwise ccmp wifi-sec.group ccmp
    )
    if [[ -z "$uuid" ]]; then
        while IFS=: read -r device type; do
            [[ "$type" == wifi ]] || continue
            capability="$(nmcli -g WIFI-PROPERTIES.AP device show "$device")"
            if [[ "$capability" == yes ]]; then
                capability="$(nmcli -g WIFI-PROPERTIES.5GHZ device show "$device")"
                if [[ "$capability" != yes ]]; then
                    settings[1]="bg"
                    settings[3]=1
                fi
                interface="$device"
                break
            fi
        done < <(nmcli -t -f DEVICE,TYPE device status)
        [[ -n "$interface" ]] || fail "No Wi-Fi adapter supports hotspot mode."
        password="$(od -An -N12 -tx1 /dev/urandom | tr -d ' \n')"
        if ! output="$(nmcli connection add type wifi ifname "$interface" \
            con-name "$PROFILE" autoconnect no ssid "$SSID" \
            802-11-wireless.mode ap ipv4.method shared ipv6.method disabled \
            "${settings[@]}" wifi-sec.psk "$password" 2>&1)"; then
            fail "$output"
        fi
        load_profile
    else
        configured="$(nmcli -g 802-11-wireless.band,802-11-wireless.channel,802-11-wireless-security.key-mgmt,802-11-wireless-security.proto,802-11-wireless-security.pmf,802-11-wireless-security.pairwise,802-11-wireless-security.group connection show uuid "$uuid")"
        mapfile -t saved <<< "$configured"
        if [[ "${saved[0]:-}" == bg ]]; then
            settings[1]="bg"
            settings[3]="${saved[1]:-1}"
        fi
        if [[ "$configured" != "${settings[1]}"$'\n'"${settings[3]}"$'\nwpa-psk\nrsn\n2\nccmp\nccmp' ]]; then
            if ! output="$(nmcli connection modify uuid "$uuid" "${settings[@]}" 2>&1)"; then
                fail "$output"
            fi
            if $active; then
                stop_hotspot
                active=false
            fi
        fi
    fi
    $active && return 0
    # Let the supplicant finish its 25s activation timeout before retrying.
    if ! output="$(nmcli --wait 35 connection up uuid "$uuid" 2>&1)"; then
        [[ "${settings[1]}" == a ]] || fail "$output"
        # Some adapters advertise 5 GHz but cannot start an AP on that band.
        if ! output="$(nmcli connection modify uuid "$uuid" \
            802-11-wireless.band bg 802-11-wireless.channel 1 2>&1)" ||
            ! output="$(nmcli --wait 35 connection up uuid "$uuid" 2>&1)"; then
            fail "$output"
        fi
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

change_password() {
    [[ -n "$uuid" ]] || fail "Start the hotspot once to create its saved profile."
    local password LC_ALL=C
    notify "New hotspot password" "Use 8–63 ASCII characters or 64 hexadecimal digits. Connected devices will need to reconnect."
    # Keep an empty entry so Wofi can submit a prompt without menu results.
    password="$(printf '\n' |
        "${BASH_SOURCE[0]%/*}/password-prompt.sh" -p 'New hotspot password: ')" || return 0
    [[ -n "$password" ]] || return 0
    if ! { [[ ${#password} -ge 8 && ${#password} -le 63 && "$password" != *[![:print:]]* ]] ||
        [[ "$password" =~ ^[[:xdigit:]]{64}$ ]]; }; then
        fail "Use 8–63 ASCII characters or 64 hexadecimal digits."
    fi
    if ! nmcli connection modify uuid "$uuid" wifi-sec.psk "$password" >/dev/null 2>&1; then
        fail "Could not save the hotspot password. Check NetworkManager permissions."
    fi
    unset password
    if $active; then
        if ! nmcli --wait 15 connection down uuid "$uuid" >/dev/null 2>&1 ||
            ! nmcli --wait 35 connection up uuid "$uuid" >/dev/null 2>&1; then
            fail "Password saved, but the hotspot could not restart. Start it again from the hotspot menu."
        fi
    fi
    notify "Hotspot password changed" "Reconnect your devices using the new password."
}

command -v nmcli >/dev/null 2>&1 || fail "NetworkManager is not installed."

if [[ "$action" == menu ]]; then
    action="$(printf '%s\n' 'Start hotspot' 'Stop hotspot' 'Show credentials' 'Change password' 'Edit hotspot' |
        wofi --dmenu --no-custom-entry --cache-file /dev/null -p 'Hotspot')" || exit 0
    case "$action" in
        'Start hotspot') action=start ;;
        'Stop hotspot') action=stop ;;
        'Show credentials') action=credentials ;;
        'Change password') action=password ;;
        'Edit hotspot') action=edit ;;
        *) exit 0 ;;
    esac
fi

case "$action" in
    start | stop | toggle | password)
        source "${BASH_SOURCE[0]%/*}/runtime-dir.sh"
        exec 9>"$RUNTIME_DIR/hype-hotspot.lock"
        flock -n 9 || {
            notify "Hotspot busy" "Another hotspot action is still running."
            exit 0
        }
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
    password) change_password ;;
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
    *)
        printf 'Usage: hotspot.sh {status|menu|toggle|start|stop|credentials|password|edit}\n' >&2
        exit 2
        ;;
esac

if [[ "$action" == start || "$action" == stop || "$action" == toggle ]]; then
    pkill -RTMIN+17 -x waybar 2>/dev/null || true
fi
