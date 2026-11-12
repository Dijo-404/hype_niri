#!/usr/bin/env bash
set -Eeuo pipefail
umask 077

LOG=/var/log/stealth.log
STATE_DIR=/run/stealth
STATE_FILE=$STATE_DIR/state
ACTIVE_FILE=$STATE_DIR/active
RULES=/etc/stealth.nft
CHECK_URL=https://check.torproject.org/api/ip

log() {
    printf '[%s] %s\n' "$(date '+%Y-%m-%d %H:%M:%S')" "$*" | tee -a "$LOG"
}

fail() {
    log "ERROR: $*" >&2
    exit 1
}

require_root() {
    ((EUID == 0)) || {
        printf 'Run this command with sudo.\n' >&2
        exit 1
    }
    touch "$LOG"
    chmod 600 "$LOG"
}

lock_changes() {
    exec 9>/run/stealth.lock
    flock -n 9 || fail 'Another stealth change is in progress.'
}

valid_mac() {
    [[ $1 =~ ^([[:xdigit:]]{2}:){5}[[:xdigit:]]{2}$ ]]
}

valid_interface_name() {
    [[ $1 =~ ^[[:alnum:]_.:-]+$ ]]
}

valid_wifi_interface() {
    valid_interface_name "$1" && [[ -d /sys/class/net/$1/wireless ]]
}

valid_connection_uuid() {
    [[ $1 =~ ^[[:xdigit:]]{8}-[[:xdigit:]]{4}-[[:xdigit:]]{4}-[[:xdigit:]]{4}-[[:xdigit:]]{12}$ ]]
}

valid_clone_name() {
    [[ $1 =~ ^stealth-[[:xdigit:]]{8}-[[:xdigit:]]{4}-[[:xdigit:]]{4}-[[:xdigit:]]{4}-[[:xdigit:]]{12}$ ]]
}

random_mac() {
    local -a bytes
    read -r -a bytes < <(od -An -N6 -tx1 /dev/urandom)
    ((${#bytes[@]} == 6)) || return 1
    printf '%02x:%s:%s:%s:%s:%s\n' "$(((16#${bytes[0]} | 2) & 254))" \
        "${bytes[1]}" "${bytes[2]}" "${bytes[3]}" "${bytes[4]}" "${bytes[5]}"
}

default_interface() {
    ip -o -4 route get 1.1.1.1 2>/dev/null |
        awk '{ for (i=1; i<NF; i++) if ($i == "dev") { print $(i+1); exit } }'
}

current_mac() {
    cat "/sys/class/net/$1/address"
}

firewall_present() {
    nft list table inet stealth >/dev/null 2>&1
}

wait_for_network() {
    local interface=$1 connection=$2 attempt state active_connection address
    for ((attempt = 0; attempt < 15; attempt++)); do
        state=$(nmcli -g GENERAL.STATE device show "$interface" 2>/dev/null || true)
        active_connection=$(nmcli -g GENERAL.CON-UUID device show "$interface" 2>/dev/null || true)
        address=$(nmcli -g IP4.ADDRESS device show "$interface" 2>/dev/null || true)
        if [[ $state == 100* && $active_connection == "$connection" && -n $address ]] &&
            [[ -n $(ip -4 route show default dev "$interface") ]]; then
            return 0
        fi
        sleep 1
    done
    return 1
}

disconnect_nm() {
    local interface=$1 state
    if nmcli -w 15 device disconnect "$interface"; then
        return 0
    fi
    state=$(nmcli -g GENERAL.STATE device show "$interface" 2>/dev/null || true)
    [[ $state == 30* || $state == 20* ]]
}

activate_saved_connection() (
    local interface=$1 original=$2 connection=$3 mac_policy policy_tmp
    local policy_file=$STATE_DIR/mac-policy
    trap - EXIT
    if [[ -f $policy_file ]]; then
        mac_policy=$(cat "$policy_file") || exit 2
    else
        mac_policy=$(nmcli --escape no -g 802-11-wireless.cloned-mac-address connection show uuid "$connection") || exit 2
        policy_tmp=$(mktemp "$STATE_DIR/.mac-policy.XXXXXX") || exit 2
        printf '%s\n' "$mac_policy" >"$policy_tmp" || exit 2
        mv -f "$policy_tmp" "$policy_file" || exit 2
    fi
    trap 'nmcli connection modify --temporary uuid "$connection" 802-11-wireless.cloned-mac-address "$mac_policy" && rm -f "$policy_file" || exit 2' EXIT
    trap 'exit 130' INT
    trap 'exit 143' TERM
    nmcli connection modify --temporary uuid "$connection" 802-11-wireless.cloned-mac-address "$original" || exit 2
    nmcli -w 30 connection up uuid "$connection" ifname "$interface" || exit 1
)

restore_mac() {
    local interface=$1 original=$2 connection=$3 require_network=${4:-1} network_ok=1 activation_result=0
    RESTORE_NETWORK_OK=1
    RESTORE_NM_UNMANAGED=0
    disconnect_nm "$interface"
    ip link set dev "$interface" down
    if ! ip link set dev "$interface" address "$original"; then
        ip link set dev "$interface" up || true
        return 1
    fi
    ip link set dev "$interface" up
    activate_saved_connection "$interface" "$original" "$connection" || activation_result=$?
    if ((activation_result > 1)); then
        return 1
    elif ((activation_result)); then
        network_ok=0
    elif ! wait_for_network "$interface" "$connection"; then
        network_ok=0
    fi
    if [[ $(current_mac "$interface") != "$original" ]] && ((! require_network)); then
        log 'NetworkManager changed the MAC while Wi-Fi was unavailable; disabling its control for recovery.'
        nmcli -w 15 device disconnect "$interface" || true
        nmcli device set "$interface" managed no || return 1
        ip link set dev "$interface" down || return 1
        ip link set dev "$interface" address "$original" || return 1
        ip link set dev "$interface" up || return 1
        network_ok=0
        RESTORE_NM_UNMANAGED=1
    fi
    [[ $(current_mac "$interface") == "$original" ]] || return 1
    if ((! network_ok)); then
        RESTORE_NETWORK_OK=0
        log 'Original MAC restored, but the saved Wi-Fi profile could not reconnect.'
        ((! require_network)) || return 1
    fi
}

delete_clone() {
    local clone_uuid=$1 clone_name=$2
    if [[ -n $clone_uuid ]] && nmcli -g connection.uuid connection show uuid "$clone_uuid" >/dev/null 2>&1; then
        nmcli connection delete uuid "$clone_uuid"
    elif nmcli -g connection.uuid connection show id "$clone_name" >/dev/null 2>&1; then
        nmcli connection delete id "$clone_name"
    fi
}

write_state() {
    local state_tmp
    state_tmp=$(mktemp "$STATE_DIR/.state.XXXXXX")
    printf '%s\n%s\n%s\n%s\n%s\n%s\n%s\n%s\n' \
        "$START_INTERFACE" "$START_LINK_TYPE" "$START_MAC" "$START_TOR_PREEXISTING" \
        "$START_CONNECTION" "$START_CLONE_UUID" "$START_CLONE_NAME" "$START_SPOOF_MAC" >"$state_tmp"
    mv -f "$state_tmp" "$STATE_FILE"
}

read_state() {
    local -a lines
    [[ -f $STATE_FILE ]] || return 1
    mapfile -t lines <"$STATE_FILE"
    ((${#lines[@]} == 8)) || fail 'Saved state is incomplete.'
    SAVED_INTERFACE=${lines[0]}
    SAVED_LINK_TYPE=${lines[1]}
    SAVED_MAC=${lines[2]}
    SAVED_TOR_PREEXISTING=${lines[3]}
    SAVED_CONNECTION=${lines[4]}
    SAVED_CLONE_UUID=${lines[5]}
    SAVED_CLONE_NAME=${lines[6]}
    SAVED_SPOOF_MAC=${lines[7]}
    valid_interface_name "$SAVED_INTERFACE" || fail 'Saved network interface is invalid.'
    [[ $SAVED_LINK_TYPE == wifi || $SAVED_LINK_TYPE == ethernet ]] || fail 'Saved connection type is invalid.'
    valid_mac "$SAVED_MAC" || fail 'Saved MAC address is invalid.'
    [[ $SAVED_TOR_PREEXISTING == 0 || $SAVED_TOR_PREEXISTING == 1 ]] || fail 'Saved Tor state is invalid.'
    valid_connection_uuid "$SAVED_CONNECTION" || fail 'Saved NetworkManager connection is invalid.'
    if [[ $SAVED_LINK_TYPE == wifi ]]; then
        [[ -z $SAVED_CLONE_UUID ]] || valid_connection_uuid "$SAVED_CLONE_UUID" || fail 'Saved temporary connection is invalid.'
        valid_clone_name "$SAVED_CLONE_NAME" || fail 'Saved temporary connection name is invalid.'
        valid_mac "$SAVED_SPOOF_MAC" || fail 'Saved spoofed MAC is invalid.'
    else
        [[ -z $SAVED_CLONE_UUID && -z $SAVED_CLONE_NAME && -z $SAVED_SPOOF_MAC ]] ||
            fail 'Ethernet state unexpectedly contains a temporary Wi-Fi profile.'
    fi
}

tor_listeners_ready() {
    ss -H -ltn '( sport = :9040 )' | grep -Fq '127.0.0.1:9040' &&
        ss -H -lun '( sport = :5353 )' | grep -Fq '127.0.0.1:5353'
}

verify_transparent_tor() {
    local attempts=${1:-3} response ip attempt
    for ((attempt = 1; attempt <= attempts; attempt++)); do
        if response=$(curl --ipv4 --proxy '' --noproxy '*' --fail --silent --show-error \
            --connect-timeout 8 --max-time 15 "$CHECK_URL" 2>>"$LOG") &&
            ip=$(jq -er 'select(.IsTor == true) | .IP | select(type == "string")' <<<"$response"); then
            printf '%s\n' "$ip"
            return 0
        fi
        ((attempt < attempts)) && sleep 2
    done
    return 1
}

start_cleanup() {
    local result=$?
    trap - EXIT
    ((result == 0)) && return 0
    set +e
    rm -f "$ACTIVE_FILE"
    log 'Start failed; attempting to restore the network and Tor state.'
    if ((MAC_ATTEMPTED)); then
        restore_mac "$START_INTERFACE" "$START_MAC" "$START_CONNECTION" || log 'MAC restoration failed; manual recovery is required.'
    fi
    if ((TOR_STARTED)); then
        systemctl stop tor.service || log 'Tor could not be stopped.'
    fi
    if firewall_present; then
        log 'The firewall remains active to block leaks. Run stealth-stop to recover.'
    else
        if ((CLONE_CREATED)); then
            delete_clone "$START_CLONE_UUID" "$START_CLONE_NAME" || log 'Temporary Wi-Fi profile could not be removed.'
        fi
        rm -f "$STATE_FILE"
        rmdir "$STATE_DIR" 2>/dev/null || true
    fi
    return "$result"
}

start_stealth() {
    local tor_preexisting=0 new_mac tor_ip
    require_root
    lock_changes
    [[ -f $RULES ]] || fail "Missing firewall rules: $RULES"
    [[ ! -e $STATE_FILE ]] || fail 'Saved state already exists. Run stealth-stop first.'
    ! firewall_present || fail 'Stealth firewall already exists. Run stealth-stop first.'
    START_INTERFACE=$(default_interface)
    valid_interface_name "$START_INTERFACE" || fail 'No valid default IPv4 network interface was found.'
    [[ -r /sys/class/net/$START_INTERFACE/address ]] || fail 'The default IPv4 interface has no readable MAC address.'
    START_LINK_TYPE=$(nmcli -g GENERAL.TYPE device show "$START_INTERFACE" 2>/dev/null || true)
    case $START_LINK_TYPE in
        wifi) valid_wifi_interface "$START_INTERFACE" || fail 'NetworkManager Wi-Fi interface is unavailable.' ;;
        ethernet) ;;
        *) fail 'The default IPv4 route must use a NetworkManager Wi-Fi or Ethernet connection.' ;;
    esac
    START_MAC=$(current_mac "$START_INTERFACE")
    valid_mac "$START_MAC" || fail 'Could not read the original MAC address.'
    START_CONNECTION=$(nmcli -g GENERAL.CON-UUID device show "$START_INTERFACE")
    valid_connection_uuid "$START_CONNECTION" || fail 'Could not identify the active NetworkManager connection.'
    START_SPOOF_MAC=''
    START_CLONE_NAME=''
    START_CLONE_UUID=''
    if [[ $START_LINK_TYPE == wifi ]]; then
        START_SPOOF_MAC=$(random_mac)
        valid_mac "$START_SPOOF_MAC" || fail 'Could not generate a random MAC address.'
        [[ $START_SPOOF_MAC != "$START_MAC" ]] || fail 'Generated MAC matches the current address.'
        START_CLONE_NAME="stealth-$(uuidgen)"
    fi
    if systemctl is-active --quiet tor.service; then
        tor_preexisting=1
    fi
    mkdir -p "$STATE_DIR"
    chmod 755 "$STATE_DIR"
    START_TOR_PREEXISTING=$tor_preexisting
    MAC_ATTEMPTED=0
    TOR_STARTED=0
    CLONE_CREATED=0
    trap start_cleanup EXIT
    trap 'exit 130' INT
    trap 'exit 143' TERM
    write_state

    log "Installing fail-closed firewall before changing $START_INTERFACE."
    nft -D "uplink=\"$START_INTERFACE\"" -f "$RULES"

    if [[ $START_LINK_TYPE == wifi ]]; then
        nmcli connection clone --temporary uuid "$START_CONNECTION" "$START_CLONE_NAME"
        CLONE_CREATED=1
        START_CLONE_UUID=$(nmcli -g connection.uuid connection show id "$START_CLONE_NAME")
        valid_connection_uuid "$START_CLONE_UUID" || fail 'Temporary Wi-Fi profile has no valid UUID.'
        write_state
        nmcli connection modify --temporary uuid "$START_CLONE_UUID" \
            connection.autoconnect no 802-11-wireless.cloned-mac-address "$START_SPOOF_MAC"

        MAC_ATTEMPTED=1
        disconnect_nm "$START_INTERFACE"
        nmcli -w 30 connection up uuid "$START_CLONE_UUID" ifname "$START_INTERFACE"
        wait_for_network "$START_INTERFACE" "$START_CLONE_UUID" || fail 'Wi-Fi did not obtain a fresh IPv4 activation.'
        new_mac=$(current_mac "$START_INTERFACE")
        [[ $new_mac == "$START_SPOOF_MAC" ]] || fail 'NetworkManager did not apply the spoofed MAC.'
    else
        wait_for_network "$START_INTERFACE" "$START_CONNECTION" || fail 'Ethernet connection lost its IPv4 route.'
        new_mac=$(current_mac "$START_INTERFACE")
        [[ $new_mac == "$START_MAC" ]] || fail 'Ethernet MAC changed unexpectedly.'
    fi
    [[ $(default_interface) == "$START_INTERFACE" ]] || fail 'The default IPv4 route changed during startup.'

    if ((! tor_preexisting)); then
        TOR_STARTED=1
        systemctl start tor.service
    fi
    systemctl is-active --quiet tor.service || fail 'Tor service is inactive.'
    tor_listeners_ready || fail 'Tor transparent TCP and DNS listeners are missing.'
    tor_ip=$(verify_transparent_tor 3) || fail 'A regular TCP request was not verified through Tor.'

    log "Stealth active: $START_LINK_TYPE $START_INTERFACE MAC $new_mac; Tor exit $tor_ip."
    log 'Host IPv4 TCP and DNS use Tor; IPv6, other UDP, ICMP, and forwarded traffic are blocked.'
    : >"$ACTIVE_FILE"
    chmod 644 "$ACTIVE_FILE"
    trap - EXIT INT TERM
}

stop_stealth() {
    local has_firewall=0
    require_root
    lock_changes
    if firewall_present; then
        has_firewall=1
    fi
    if [[ ! -f $STATE_FILE ]] && ((! has_firewall)); then
        rm -f "$ACTIVE_FILE"
        rmdir "$STATE_DIR" 2>/dev/null || true
        log 'Stealth is already stopped.'
        return 0
    fi
    rm -f "$ACTIVE_FILE"
    read_state || fail 'Saved state is missing; keeping the firewall active to avoid an unsafe restore.'
    if [[ $SAVED_LINK_TYPE == wifi ]]; then
        log "Restoring $SAVED_INTERFACE to its saved MAC $SAVED_MAC."
        restore_mac "$SAVED_INTERFACE" "$SAVED_MAC" "$SAVED_CONNECTION" 0 || fail 'MAC restoration failed; firewall remains active.'
        delete_clone "$SAVED_CLONE_UUID" "$SAVED_CLONE_NAME" || fail 'Could not remove temporary Wi-Fi profile; firewall remains active.'
    else
        RESTORE_NETWORK_OK=1
        RESTORE_NM_UNMANAGED=0
        log "Ethernet connection $SAVED_INTERFACE and its MAC were not changed."
    fi
    if [[ $SAVED_TOR_PREEXISTING == 0 ]] && systemctl is-active --quiet tor.service; then
        systemctl stop tor.service || fail 'Could not stop Tor; firewall remains active.'
    fi
    if ((has_firewall)); then
        nft delete table inet stealth || fail 'Could not remove stealth firewall.'
    fi
    rm -f "$STATE_FILE"
    rmdir "$STATE_DIR" 2>/dev/null || true
    if ((RESTORE_NETWORK_OK)); then
        log 'Stealth stopped; ordinary routing restored.'
    else
        log 'Stealth stopped; original MAC restored, but Wi-Fi is unavailable. Reconnect the original profile when the network returns.'
        if ((RESTORE_NM_UNMANAGED)); then
            log "NetworkManager control was disabled for MAC recovery. Run: sudo nmcli device set $SAVED_INTERFACE managed yes; sudo nmcli connection up uuid $SAVED_CONNECTION ifname $SAVED_INTERFACE"
        fi
    fi
}

status_stealth() {
    local interface default_route_interface link_type mac service_state tor_state tor_ip health=0 has_firewall=0 has_state=0
    require_root
    service_state=$(systemctl is-active stealth.service || true)
    tor_state=$(systemctl is-active tor.service || true)
    default_route_interface=$(default_interface || true)
    interface=$default_route_interface
    link_type=$(nmcli -g GENERAL.TYPE device show "$interface" 2>/dev/null || true)
    if [[ -f $STATE_FILE ]]; then
        read_state
        interface=$SAVED_INTERFACE
        link_type=$SAVED_LINK_TYPE
    fi
    if valid_interface_name "$interface" && [[ -r /sys/class/net/$interface/address ]]; then
        mac=$(current_mac "$interface")
    else
        mac=unavailable
    fi
    printf 'Stealth service: %s\nTor service: %s\nInterface: %s\nConnection type: %s\nCurrent MAC: %s\n' \
        "$service_state" "$tor_state" "${interface:-unavailable}" "${link_type:-unavailable}" "$mac"
    printf 'Default IPv4 interface: %s\n' "${default_route_interface:-unavailable}"
    if [[ $service_state != active && $link_type != wifi && $link_type != ethernet ]]; then
        printf 'Start requirement: NetworkManager Wi-Fi or Ethernet default IPv4 route.\n'
    fi
    if [[ -f $STATE_FILE ]]; then
        has_state=1
    fi
    if firewall_present; then
        has_firewall=1
    fi
    if ((has_state)); then
        read_state
        printf 'Saved pre-start MAC: %s\n' "$SAVED_MAC"
        if [[ $SAVED_LINK_TYPE == wifi ]]; then
            printf 'Expected spoofed MAC: %s\n' "$SAVED_SPOOF_MAC"
            if ((has_firewall)) && [[ $mac != "$SAVED_SPOOF_MAC" ]]; then
                printf 'Warning: the expected spoofed MAC is not active.\n'
                health=1
            fi
        else
            printf 'MAC spoofing: off (Ethernet)\n'
            if ((has_firewall)) && [[ $mac != "$SAVED_MAC" ]]; then
                printf 'Warning: the Ethernet MAC changed since Stealth started.\n'
                health=1
            fi
        fi
    elif ((has_firewall)) || [[ $service_state == active ]]; then
        printf 'Warning: saved MAC state is missing; safe stop is unavailable.\n'
        health=1
    fi
    if ((has_firewall)); then
        printf 'Fail-closed firewall: active\n'
        if [[ ! -f $ACTIVE_FILE ]]; then
            printf 'Warning: Stealth routing is not confirmed active.\n'
            health=1
        fi
        if [[ $tor_state == active ]] && tor_listeners_ready; then
            if tor_ip=$(verify_transparent_tor 1); then
                printf 'Transparent Tor check: verified (%s)\n' "$tor_ip"
            else
                printf 'Transparent Tor check: FAILED; traffic remains blocked\n'
                health=1
            fi
        else
            printf 'Transparent Tor check: unavailable; traffic remains blocked\n'
            health=1
        fi
    else
        printf 'Fail-closed firewall: inactive\n'
        if ((has_state)); then
            printf 'Warning: saved state exists without its stealth firewall.\n'
            health=1
        fi
        if [[ $service_state == active ]]; then
            printf 'Warning: service is active without its stealth firewall.\n'
            health=1
        fi
    fi
    printf 'Scope: host IPv4 TCP and DNS through Tor while active; IPv6, other UDP, ICMP, and forwarded traffic blocked.\n'
    return "$health"
}

case ${1:-} in
    version) printf 'stealth-transparent-v2\n' ;;
    start) start_stealth ;;
    stop) stop_stealth ;;
    status) status_stealth ;;
    *)
        printf 'Usage: stealth {start|stop|status}\n' >&2
        exit 2
        ;;
esac
