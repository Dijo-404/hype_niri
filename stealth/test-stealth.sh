#!/usr/bin/env bash
set -euo pipefail

SOURCE=$(dirname "$(realpath "$0")")/stealth.sh
TEST_ROOT=$(mktemp -d)
trap 'rm -rf "$TEST_ROOT"' EXIT
export STEALTH_TEST_ROOT=$TEST_ROOT
mkdir -p "$TEST_ROOT"/{bin,run,sys/class/net/wlan0/wireless,sys/class/net/enp0s13f0u3}
printf '%s\n' '98:bd:80:57:0e:ae' >"$TEST_ROOT/sys/class/net/wlan0/address"
printf '%s\n' '60:6d:3c:e0:f1:6a' >"$TEST_ROOT/sys/class/net/enp0s13f0u3/address"
touch "$TEST_ROOT/rules"
printf '\n' >"$TEST_ROOT/original_policy"
cp "$TEST_ROOT/original_policy" "$TEST_ROOT/original_policy_disk"

sed \
    -e "s|/var/log/stealth.log|$TEST_ROOT/log|g" \
    -e "s|/run/stealth|$TEST_ROOT/run/stealth|g" \
    -e "s|/etc/stealth.nft|$TEST_ROOT/rules|g" \
    -e "s|/sys/class/net|$TEST_ROOT/sys/class/net|g" \
    "$SOURCE" >"$TEST_ROOT/script"
chmod +x "$TEST_ROOT/script"
INDICATOR_SOURCE=$(dirname "$SOURCE")/../scripts/stealth-indicator.sh
sed \
    -e "s|/run/stealth|$TEST_ROOT/run/stealth|g" \
    -e "s|/usr/local/bin/stealth|$TEST_ROOT/script|g" \
    "$INDICATOR_SOURCE" >"$TEST_ROOT/indicator"
sed 's|stealth-transparent-v2|stealth-transparent-v1|g' "$TEST_ROOT/script" >"$TEST_ROOT/script-v1"
chmod +x "$TEST_ROOT/script-v1"
sed \
    -e "s|/run/stealth|$TEST_ROOT/run/stealth|g" \
    -e "s|/usr/local/bin/stealth|$TEST_ROOT/script-v1|g" \
    "$INDICATOR_SOURCE" >"$TEST_ROOT/indicator-v1"

cat >"$TEST_ROOT/bin/mock-command" <<'MOCK'
#!/usr/bin/env bash
set -euo pipefail
name=${0##*/}
root=$STEALTH_TEST_ROOT
case "$name" in
    ip)
        case "$*" in
            '-o -4 route get '*)
                [[ ! -e $root/route_missing ]] || exit 1
                if [[ -e $root/route_wired ]]; then
                    printf '1.1.1.1 via 192.0.2.1 dev enp0s13f0u3 src 192.0.2.10\n'
                else
                    printf '1.1.1.1 via 192.0.2.1 dev wlan0 src 192.0.2.10\n'
                fi
                ;;
            '-4 route show default dev wlan0')
                [[ ! -e $root/nm_disconnected ]] && printf 'default via 192.0.2.1 dev wlan0\n'
                ;;
            '-4 route show default dev enp0s13f0u3')
                [[ ! -e $root/route_missing ]] && printf 'default via 192.0.2.1 dev enp0s13f0u3\n'
                ;;
            'link set dev wlan0 down'|'link set dev wlan0 up') ;;
            'link set dev wlan0 address '*) printf '%s\n' "$6" > "$root/sys/class/net/wlan0/address" ;;
            'link set dev enp0s13f0u3 '*)
                printf 'Unexpected wired interface change: %s\n' "$*" >&2
                exit 1
                ;;
            *) printf 'Unexpected ip command: %s\n' "$*" >&2; exit 1 ;;
        esac
        ;;
    od) printf ' 00 11 22 33 44 55\n' ;;
    uuidgen) printf '%s\n' 'aaaaaaaa-bbbb-4ccc-8ddd-eeeeeeeeeeee' ;;
    nmcli)
        case "$*" in
            '-g GENERAL.TYPE device show wlan0') echo wifi ;;
            '-g GENERAL.TYPE device show enp0s13f0u3')
                if [[ -f $root/device_type ]]; then cat "$root/device_type"; else echo ethernet; fi
                ;;
            '-g GENERAL.STATE device show wlan0')
                if [[ -e $root/nm_disconnected ]]; then echo '30 (disconnected)'; else echo '100 (connected)'; fi
                ;;
            '-g GENERAL.CON-UUID device show wlan0')
                if [[ ! -e $root/nm_disconnected ]]; then
                    if [[ -e $root/clone_active ]]; then
                        echo '11111111-2222-4333-8444-555555555555'
                    else
                        echo '64595948-f950-4cf0-8a80-d234a3cb4448'
                    fi
                fi
                ;;
            '-g IP4.ADDRESS device show wlan0')
                [[ ! -e $root/nm_disconnected ]] && echo '192.0.2.10/24'
                ;;
            '-g GENERAL.STATE device show enp0s13f0u3') echo '100 (connected)' ;;
            '-g GENERAL.CON-UUID device show enp0s13f0u3')
                echo 'dc74f0c3-1589-3b85-9c84-44463b87cc32'
                ;;
            '-g IP4.ADDRESS device show enp0s13f0u3')
                [[ ! -e $root/route_missing ]] && echo '192.0.2.10/24'
                ;;
            '-w 15 device disconnect wlan0') touch "$root/nm_disconnected" ;;
            'device set wlan0 managed no') touch "$root/nm_unmanaged" ;;
            'connection clone --temporary uuid 64595948-f950-4cf0-8a80-d234a3cb4448 stealth-aaaaaaaa-bbbb-4ccc-8ddd-eeeeeeeeeeee')
                [[ ${MOCK_CLONE_FAIL:-0} == 0 ]] || exit 1
                touch "$root/clone_exists"
                ;;
            '-g connection.uuid connection show id stealth-aaaaaaaa-bbbb-4ccc-8ddd-eeeeeeeeeeee'|'-g connection.uuid connection show uuid 11111111-2222-4333-8444-555555555555')
                [[ -e $root/clone_exists ]] && echo '11111111-2222-4333-8444-555555555555'
                ;;
            'connection modify --temporary uuid 11111111-2222-4333-8444-555555555555 connection.autoconnect no 802-11-wireless.cloned-mac-address '*)
                printf '%s\n' "$9" > "$root/clone_mac"
                ;;
            '--escape no -g 802-11-wireless.cloned-mac-address connection show uuid 64595948-f950-4cf0-8a80-d234a3cb4448')
                cat "$root/original_policy"
                ;;
            'connection modify --temporary uuid 64595948-f950-4cf0-8a80-d234a3cb4448 802-11-wireless.cloned-mac-address '*)
                if [[ ${MOCK_POLICY_RESET_FAIL:-0} == 1 && $7 == "$(cat "$root/original_policy_disk")" ]]; then exit 1; fi
                printf '%s\n' "$7" > "$root/original_policy"
                ;;
            '-w 30 connection up uuid 11111111-2222-4333-8444-555555555555 ifname wlan0')
                if [[ ${MOCK_NM_UP_FAIL:-0} == 1 ]]; then exit 1; fi
                touch "$root/clone_active"
                cp "$root/clone_mac" "$root/sys/class/net/wlan0/address"
                rm -f "$root/nm_disconnected"
                ;;
            '-w 30 connection up uuid 64595948-f950-4cf0-8a80-d234a3cb4448 ifname wlan0')
                if [[ ${MOCK_NM_UP_FAIL:-0} == 1 ]]; then
                    if [[ ${MOCK_NM_SCAN_RANDOM:-0} == 1 ]]; then
                        printf '%s\n' '7a:00:00:00:00:01' > "$root/sys/class/net/wlan0/address"
                    fi
                    exit 1
                fi
                rm -f "$root/clone_active"
                policy=$(cat "$root/original_policy")
                case $policy in
                    random) printf '%s\n' '06:66:77:88:99:aa' > "$root/sys/class/net/wlan0/address" ;;
                    ''|preserve) ;;
                    *) printf '%s\n' "$policy" > "$root/sys/class/net/wlan0/address" ;;
                esac
                rm -f "$root/nm_disconnected"
                if [[ ${MOCK_NM_UP_SIGNAL:-} == TERM ]]; then kill -TERM "$PPID"; fi
                ;;
            'connection delete uuid 11111111-2222-4333-8444-555555555555'|'connection delete id stealth-aaaaaaaa-bbbb-4ccc-8ddd-eeeeeeeeeeee')
                rm -f "$root/clone_exists" "$root/clone_mac" "$root/clone_active"
                ;;
            *enp0s13f0u3*|*dc74f0c3-1589-3b85-9c84-44463b87cc32*)
                printf 'Unexpected wired NetworkManager change: %s\n' "$*" >&2
                exit 1
                ;;
            *) printf 'Unexpected nmcli command: %s\n' "$*" >&2; exit 1 ;;
        esac
        ;;
    systemctl)
        case "$*" in
            'is-active --quiet tor.service') [[ -e $root/tor_active ]] ;;
            'is-active tor.service') if [[ -e $root/tor_active ]]; then echo active; else echo inactive; exit 3; fi ;;
            'is-active stealth.service') if [[ -e $root/stealth_active ]]; then echo active; else echo inactive; exit 3; fi ;;
            'start tor.service') touch "$root/tor_active" ;;
            'stop tor.service') rm -f "$root/tor_active" ;;
            *) printf 'Unexpected systemctl command: %s\n' "$*" >&2; exit 1 ;;
        esac
        ;;
    nft)
        case "$*" in
            'list table inet stealth') [[ -e $root/firewall ]] ;;
            '-D uplink="wlan0" -f '*)
                printf '%s\n' wlan0 > "$root/firewall_uplink"
                touch "$root/firewall"
                if [[ ${MOCK_NFT_SIGNAL:-} == TERM ]]; then kill -TERM "$PPID"; fi
                ;;
            '-D uplink="enp0s13f0u3" -f '*)
                printf '%s\n' enp0s13f0u3 > "$root/firewall_uplink"
                touch "$root/firewall"
                ;;
            'delete table inet stealth') rm -f "$root/firewall" "$root/firewall_uplink" ;;
            *) printf 'Unexpected nft command: %s\n' "$*" >&2; exit 1 ;;
        esac
        ;;
    ss)
        [[ -e $root/tor_active ]] || exit 0
        case "$*" in
            *9040*) printf 'LISTEN 0 4096 127.0.0.1:9040 0.0.0.0:*\n' ;;
            *5353*) printf 'UNCONN 0 0 127.0.0.1:5353 0.0.0.0:*\n' ;;
        esac
        ;;
    curl)
        if [[ ${MOCK_CURL_FAIL:-0} == 1 ]]; then
            printf '%s\n' '{"IsTor":false,"IP":"198.51.100.10"}'
        else
            printf '%s\n' '{"IsTor":true,"IP":"203.0.113.10"}'
        fi
        ;;
    sleep) ;;
    *) printf 'Unexpected mock command: %s\n' "$name" >&2; exit 1 ;;
esac
MOCK
chmod +x "$TEST_ROOT/bin/mock-command"
for command in ip od uuidgen nmcli systemctl nft ss curl sleep; do
    ln -s mock-command "$TEST_ROOT/bin/$command"
done

run_script() {
    unshare -Urn env PATH="$TEST_ROOT/bin:$PATH" STEALTH_TEST_ROOT="$TEST_ROOT" \
        MOCK_CURL_FAIL="${MOCK_CURL_FAIL:-0}" MOCK_NM_UP_FAIL="${MOCK_NM_UP_FAIL:-0}" \
        MOCK_CLONE_FAIL="${MOCK_CLONE_FAIL:-0}" MOCK_NM_SCAN_RANDOM="${MOCK_NM_SCAN_RANDOM:-0}" \
        MOCK_NFT_SIGNAL="${MOCK_NFT_SIGNAL:-}" \
        MOCK_POLICY_RESET_FAIL="${MOCK_POLICY_RESET_FAIL:-0}" \
        MOCK_NM_UP_SIGNAL="${MOCK_NM_UP_SIGNAL:-}" \
        bash "$TEST_ROOT/script" "$1"
}

run_indicator() {
    local indicator=$TEST_ROOT/indicator
    [[ ${1:-} != v1 ]] || indicator=$TEST_ROOT/indicator-v1
    env PATH="$TEST_ROOT/bin:$PATH" STEALTH_TEST_ROOT="$TEST_ROOT" HOME="$TEST_ROOT/home" \
        bash "$indicator"
}

assert_file() { [[ -e $1 ]] || {
    printf 'Expected file: %s\n' "$1" >&2
    exit 1
}; }
assert_no_file() { [[ ! -e $1 ]] || {
    printf 'Unexpected file: %s\n' "$1" >&2
    exit 1
}; }
assert_mac() { [[ $(cat "$TEST_ROOT/sys/class/net/wlan0/address") == "$1" ]] || exit 1; }
assert_state_link_type() {
    local state_file="$TEST_ROOT/run/stealth/state"
    [[ $(wc -l <"$state_file") == 8 ]] || {
        printf 'Expected eight saved state fields.\n' >&2
        exit 1
    }
    [[ $(sed -n '2p' "$state_file") == "$1" ]] || {
        printf 'Expected saved link type %s.\n' "$1" >&2
        exit 1
    }
}

[[ $(run_script version) == stealth-transparent-v2 ]]
run_script start >"$TEST_ROOT/start.out"
assert_file "$TEST_ROOT/firewall"
assert_file "$TEST_ROOT/tor_active"
assert_file "$TEST_ROOT/run/stealth/state"
assert_file "$TEST_ROOT/run/stealth/active"
[[ $(stat -c '%a' "$TEST_ROOT/run/stealth") == 755 ]]
[[ $(stat -c '%a' "$TEST_ROOT/run/stealth/active") == 644 ]]
[[ $(stat -c '%a' "$TEST_ROOT/run/stealth/state") == 600 ]]
assert_file "$TEST_ROOT/clone_exists"
assert_state_link_type wifi
[[ $(cat "$TEST_ROOT/firewall_uplink") == wlan0 ]]
assert_mac '02:11:22:33:44:55'
[[ $(run_indicator) == "$TEST_ROOT/home/.config/waybar/icons/stealth.svg"$'\n''Stealth mode active · Tor routing' ]]
mv "$TEST_ROOT/run/stealth/active" "$TEST_ROOT/run/stealth/active.saved"
[[ $(run_indicator v1) == "$TEST_ROOT/home/.config/waybar/icons/stealth.svg"$'\n''Stealth mode active · Tor routing' ]]
mv "$TEST_ROOT/run/stealth/active.saved" "$TEST_ROOT/run/stealth/active"
run_script status >"$TEST_ROOT/direct-status.out"
rg -q '^Stealth service: inactive$' "$TEST_ROOT/direct-status.out"
rg -q 'Transparent Tor check: verified' "$TEST_ROOT/direct-status.out"
touch "$TEST_ROOT/stealth_active"
run_script status >"$TEST_ROOT/status.out"
rg -q '^Stealth service: active$' "$TEST_ROOT/status.out"
rg -q 'Transparent Tor check: verified' "$TEST_ROOT/status.out"
cp "$TEST_ROOT/run/stealth/state" "$TEST_ROOT/saved-state"
rm "$TEST_ROOT/run/stealth/state"
if run_script status >"$TEST_ROOT/missing-state-status.out"; then
    printf 'Expected status to flag a missing MAC state file.\n' >&2
    exit 1
fi
rg -q 'safe stop is unavailable' "$TEST_ROOT/missing-state-status.out"
mv "$TEST_ROOT/saved-state" "$TEST_ROOT/run/stealth/state"
run_script stop >"$TEST_ROOT/stop.out"
assert_no_file "$TEST_ROOT/firewall"
assert_no_file "$TEST_ROOT/tor_active"
assert_no_file "$TEST_ROOT/run/stealth/active"
assert_no_file "$TEST_ROOT/clone_exists"
assert_mac '98:bd:80:57:0e:ae'
[[ -z $(cat "$TEST_ROOT/original_policy") && -z $(cat "$TEST_ROOT/original_policy_disk") ]]
assert_no_file "$TEST_ROOT/run/stealth/mac-policy"
[[ -z $(run_indicator) ]]
run_script stop >"$TEST_ROOT/stop-again.out"
assert_no_file "$TEST_ROOT/run/stealth/state"
assert_no_file "$TEST_ROOT/clone_exists"

printf '%s\n' random >"$TEST_ROOT/original_policy"
cp "$TEST_ROOT/original_policy" "$TEST_ROOT/original_policy_disk"
run_script start >"$TEST_ROOT/random-policy-start.out"
run_script stop >"$TEST_ROOT/random-policy-stop.out"
assert_mac '98:bd:80:57:0e:ae'
assert_no_file "$TEST_ROOT/nm_unmanaged"
assert_no_file "$TEST_ROOT/nm_disconnected"
assert_no_file "$TEST_ROOT/firewall"
assert_no_file "$TEST_ROOT/run/stealth/state"
[[ $(cat "$TEST_ROOT/original_policy") == random ]]
[[ $(cat "$TEST_ROOT/original_policy_disk") == random ]]

run_script start >"$TEST_ROOT/policy-interrupted-start.out"
if MOCK_NM_UP_SIGNAL=TERM run_script stop >"$TEST_ROOT/policy-interrupted-stop.out" 2>&1; then
    printf 'Expected stop to retain recovery state after an interrupted activation.\n' >&2
    exit 1
fi
assert_file "$TEST_ROOT/firewall"
assert_file "$TEST_ROOT/run/stealth/state"
assert_no_file "$TEST_ROOT/run/stealth/mac-policy"
assert_mac '98:bd:80:57:0e:ae'
[[ $(cat "$TEST_ROOT/original_policy") == random ]]
[[ $(cat "$TEST_ROOT/original_policy_disk") == random ]]
run_script stop >"$TEST_ROOT/policy-interrupted-recover.out"
assert_no_file "$TEST_ROOT/firewall"
assert_no_file "$TEST_ROOT/run/stealth/state"

run_script start >"$TEST_ROOT/policy-reset-start.out"
if MOCK_POLICY_RESET_FAIL=1 run_script stop >"$TEST_ROOT/policy-reset-failed.out" 2>&1; then
    printf 'Expected stop to retain its firewall when the original MAC policy cannot be restored.\n' >&2
    exit 1
fi
assert_file "$TEST_ROOT/firewall"
assert_file "$TEST_ROOT/run/stealth/state"
assert_file "$TEST_ROOT/run/stealth/mac-policy"
[[ $(stat -c '%a' "$TEST_ROOT/run/stealth/mac-policy") == 600 ]]
[[ $(cat "$TEST_ROOT/run/stealth/mac-policy") == random ]]
[[ $(cat "$TEST_ROOT/original_policy_disk") == random ]]
run_script stop >"$TEST_ROOT/policy-reset-retry.out"
assert_no_file "$TEST_ROOT/firewall"
assert_no_file "$TEST_ROOT/run/stealth/state"
assert_no_file "$TEST_ROOT/run/stealth/mac-policy"
assert_no_file "$TEST_ROOT/nm_unmanaged"
assert_no_file "$TEST_ROOT/nm_disconnected"
assert_mac '98:bd:80:57:0e:ae'
[[ $(cat "$TEST_ROOT/original_policy") == random ]]

run_script start >"$TEST_ROOT/policy-failed-connect-start.out"
MOCK_NM_UP_FAIL=1 run_script stop >"$TEST_ROOT/policy-failed-connect-stop.out"
assert_no_file "$TEST_ROOT/firewall"
assert_no_file "$TEST_ROOT/run/stealth/state"
assert_no_file "$TEST_ROOT/run/stealth/mac-policy"
assert_mac '98:bd:80:57:0e:ae'
[[ $(cat "$TEST_ROOT/original_policy") == random ]]
[[ $(cat "$TEST_ROOT/original_policy_disk") == random ]]
rm -f "$TEST_ROOT/nm_disconnected"
printf '%s\n' '98:bd:80:57:0e:ae' >"$TEST_ROOT/original_policy"
cp "$TEST_ROOT/original_policy" "$TEST_ROOT/original_policy_disk"
run_script start >"$TEST_ROOT/explicit-policy-start.out"
run_script stop >"$TEST_ROOT/explicit-policy-stop.out"
assert_mac '98:bd:80:57:0e:ae'
assert_no_file "$TEST_ROOT/nm_unmanaged"
[[ $(cat "$TEST_ROOT/original_policy") == '98:bd:80:57:0e:ae' ]]
[[ $(cat "$TEST_ROOT/original_policy_disk") == '98:bd:80:57:0e:ae' ]]
printf '\n' >"$TEST_ROOT/original_policy"
cp "$TEST_ROOT/original_policy" "$TEST_ROOT/original_policy_disk"

if MOCK_NFT_SIGNAL=TERM run_script start >"$TEST_ROOT/interrupted-start.out" 2>&1; then
    printf 'Expected startup to fail after termination during firewall installation.\n' >&2
    exit 1
fi
assert_file "$TEST_ROOT/firewall"
assert_file "$TEST_ROOT/run/stealth/state"
assert_no_file "$TEST_ROOT/run/stealth/active"
run_script stop >"$TEST_ROOT/interrupted-recover.out"
assert_no_file "$TEST_ROOT/firewall"
assert_no_file "$TEST_ROOT/run/stealth/state"

MOCK_CURL_FAIL=1
export MOCK_CURL_FAIL
if run_script start >"$TEST_ROOT/failed-start.out" 2>&1; then
    printf 'Expected start to fail when Tor verification says false.\n' >&2
    exit 1
fi
assert_file "$TEST_ROOT/firewall"
assert_file "$TEST_ROOT/run/stealth/state"
assert_no_file "$TEST_ROOT/run/stealth/active"
assert_no_file "$TEST_ROOT/tor_active"
assert_mac '98:bd:80:57:0e:ae'
run_script stop >"$TEST_ROOT/recover.out"
assert_no_file "$TEST_ROOT/firewall"
assert_no_file "$TEST_ROOT/clone_exists"
assert_no_file "$TEST_ROOT/run/stealth/state"
unset MOCK_CURL_FAIL

touch "$TEST_ROOT/tor_active"
run_script start >"$TEST_ROOT/preexisting-start.out"
assert_file "$TEST_ROOT/run/stealth/active"
run_script stop >"$TEST_ROOT/preexisting-stop.out"
assert_no_file "$TEST_ROOT/run/stealth/active"
assert_file "$TEST_ROOT/tor_active"
assert_no_file "$TEST_ROOT/firewall"
assert_no_file "$TEST_ROOT/clone_exists"
assert_mac '98:bd:80:57:0e:ae'

MOCK_CURL_FAIL=1
export MOCK_CURL_FAIL
if run_script start >"$TEST_ROOT/preexisting-failed-start.out" 2>&1; then
    printf 'Expected start to fail when Tor verification says false.\n' >&2
    exit 1
fi
assert_file "$TEST_ROOT/firewall"
assert_file "$TEST_ROOT/tor_active"
assert_no_file "$TEST_ROOT/run/stealth/active"
[[ -z $(run_indicator) ]]
if run_script status >"$TEST_ROOT/preexisting-failed-status.out"; then
    printf 'Expected status to flag incomplete Stealth startup.\n' >&2
    exit 1
fi
rg -q 'routing is not confirmed active' "$TEST_ROOT/preexisting-failed-status.out"
run_script stop >"$TEST_ROOT/preexisting-failed-stop.out"
assert_file "$TEST_ROOT/tor_active"
unset MOCK_CURL_FAIL

rm -f "$TEST_ROOT/tor_active"
MOCK_NM_UP_FAIL=1
export MOCK_NM_UP_FAIL
if run_script start >"$TEST_ROOT/nm-failed-start.out" 2>&1; then
    printf 'Expected start to fail when Wi-Fi cannot reactivate.\n' >&2
    exit 1
fi
assert_file "$TEST_ROOT/firewall"
assert_file "$TEST_ROOT/run/stealth/state"
assert_no_file "$TEST_ROOT/tor_active"
unset MOCK_NM_UP_FAIL
run_script stop >"$TEST_ROOT/nm-recover.out"
assert_no_file "$TEST_ROOT/firewall"
assert_no_file "$TEST_ROOT/nm_disconnected"
assert_no_file "$TEST_ROOT/clone_exists"
assert_mac '98:bd:80:57:0e:ae'

MOCK_CLONE_FAIL=1
export MOCK_CLONE_FAIL
if run_script start >"$TEST_ROOT/clone-failed-start.out" 2>&1; then
    printf 'Expected start to fail when the profile clone cannot be created.\n' >&2
    exit 1
fi
assert_file "$TEST_ROOT/firewall"
assert_file "$TEST_ROOT/run/stealth/state"
assert_no_file "$TEST_ROOT/clone_exists"
unset MOCK_CLONE_FAIL
run_script stop >"$TEST_ROOT/clone-recover.out"
assert_no_file "$TEST_ROOT/firewall"
assert_no_file "$TEST_ROOT/run/stealth/state"
assert_mac '98:bd:80:57:0e:ae'

run_script start >"$TEST_ROOT/ap-start.out"
MOCK_NM_UP_FAIL=1
MOCK_NM_SCAN_RANDOM=1
export MOCK_NM_UP_FAIL MOCK_NM_SCAN_RANDOM
run_script stop >"$TEST_ROOT/ap-stop.out"
assert_no_file "$TEST_ROOT/firewall"
assert_no_file "$TEST_ROOT/clone_exists"
assert_no_file "$TEST_ROOT/run/stealth/state"
assert_file "$TEST_ROOT/nm_unmanaged"
assert_mac '98:bd:80:57:0e:ae'
rg -q 'sudo nmcli device set wlan0 managed yes' "$TEST_ROOT/ap-stop.out"
unset MOCK_NM_UP_FAIL MOCK_NM_SCAN_RANDOM

rm -f "$TEST_ROOT/stealth_active" "$TEST_ROOT/tor_active" \
    "$TEST_ROOT/nm_disconnected" "$TEST_ROOT/nm_unmanaged"
touch "$TEST_ROOT/route_wired"
run_script start >"$TEST_ROOT/wired-start.out"
assert_file "$TEST_ROOT/firewall"
assert_file "$TEST_ROOT/tor_active"
assert_file "$TEST_ROOT/run/stealth/state"
assert_state_link_type ethernet
[[ $(cat "$TEST_ROOT/firewall_uplink") == enp0s13f0u3 ]]
[[ $(cat "$TEST_ROOT/sys/class/net/enp0s13f0u3/address") == '60:6d:3c:e0:f1:6a' ]]
assert_no_file "$TEST_ROOT/clone_exists"
assert_no_file "$TEST_ROOT/nm_disconnected"
touch "$TEST_ROOT/stealth_active"
run_script status >"$TEST_ROOT/wired-status.out"
rg -q '^Interface: enp0s13f0u3$' "$TEST_ROOT/wired-status.out"
rg -q 'Transparent Tor check: verified' "$TEST_ROOT/wired-status.out"
if rg -q 'Expected spoofed MAC:' "$TEST_ROOT/wired-status.out"; then
    printf 'Wired status must not claim that its MAC was spoofed.\n' >&2
    exit 1
fi

touch "$TEST_ROOT/route_missing"
run_script stop >"$TEST_ROOT/wired-stop.out"
assert_no_file "$TEST_ROOT/firewall"
assert_no_file "$TEST_ROOT/tor_active"
assert_no_file "$TEST_ROOT/run/stealth/state"
[[ $(cat "$TEST_ROOT/sys/class/net/enp0s13f0u3/address") == '60:6d:3c:e0:f1:6a' ]]
rm -f "$TEST_ROOT/route_missing" "$TEST_ROOT/stealth_active"

MOCK_CURL_FAIL=1
export MOCK_CURL_FAIL
if run_script start >"$TEST_ROOT/wired-failed-start.out" 2>&1; then
    printf 'Expected wired start to fail when Tor verification says false.\n' >&2
    exit 1
fi
assert_file "$TEST_ROOT/firewall"
assert_file "$TEST_ROOT/run/stealth/state"
assert_state_link_type ethernet
assert_no_file "$TEST_ROOT/tor_active"
assert_no_file "$TEST_ROOT/clone_exists"
run_script stop >"$TEST_ROOT/wired-recover.out"
assert_no_file "$TEST_ROOT/firewall"
assert_no_file "$TEST_ROOT/run/stealth/state"
unset MOCK_CURL_FAIL

printf '%s\n' bridge >"$TEST_ROOT/device_type"
if run_script start >"$TEST_ROOT/unsupported-start.out" 2>&1; then
    printf 'Expected unsupported link type to be rejected.\n' >&2
    exit 1
fi
assert_no_file "$TEST_ROOT/firewall"
assert_no_file "$TEST_ROOT/run/stealth/state"
assert_no_file "$TEST_ROOT/tor_active"

printf 'PASS: Wi-Fi MAC/policy recovery, interrupted firewall recovery, wired routing without link changes, failed-start recovery, and unsupported uplink rejection\n'
