#!/usr/bin/env bash
set -euo pipefail

SOURCE=$(dirname "$(realpath "$0")")
TEST_ROOT=$(mktemp -d)
trap 'rm -rf "$TEST_ROOT"' EXIT
export STEALTH_TEST_ROOT=$TEST_ROOT
mkdir -p "$TEST_ROOT"/{bin,stage,live/etc/tor/torrc.d,live/etc/sudoers.d,live/etc/systemd/system,live/usr/local/bin,live/var/log,live/run}
cp "$SOURCE"/{stealth.sh,stealth.nft,stealth.service,stealth.sudoers,torrc.conf} "$TEST_ROOT/stage/"
sed \
    -e "s|/usr/local/bin/stealth|$TEST_ROOT/live/usr/local/bin/stealth|g" \
    -e "s|/usr/local/bin/privacy-shield.sh|$TEST_ROOT/live/usr/local/bin/privacy-shield.sh|g" \
    -e "s|/etc/systemd/system/stealth.service|$TEST_ROOT/live/etc/systemd/system/stealth.service|g" \
    -e "s|/etc/systemd/system/privacy-shield.service|$TEST_ROOT/live/etc/systemd/system/privacy-shield.service|g" \
    -e "s|/etc/stealth.nft|$TEST_ROOT/live/etc/stealth.nft|g" \
    -e "s|/etc/privacy-shield.nft|$TEST_ROOT/live/etc/privacy-shield.nft|g" \
    -e "s|/etc/tor|$TEST_ROOT/live/etc/tor|g" \
    -e "s|/etc/sudoers.d|$TEST_ROOT/live/etc/sudoers.d|g" \
    -e "s|/var/backups/stealth|$TEST_ROOT/live/var/backups/stealth|g" \
    -e "s|/run/stealth|$TEST_ROOT/live/run/stealth|g" \
    -e "s|/run/privacy-shield|$TEST_ROOT/live/run/privacy-shield|g" \
    -e "s|/var/log/stealth.log|$TEST_ROOT/live/var/log/stealth.log|g" \
    -e "s|/var/log/privacy-shield.log|$TEST_ROOT/live/var/log/privacy-shield.log|g" \
    "$SOURCE/install-stealth.sh" >"$TEST_ROOT/stage/install-stealth.sh"
printf '%s\n' 'original script' >"$TEST_ROOT/live/usr/local/bin/privacy-shield.sh"
printf '%s\n' 'original unit' >"$TEST_ROOT/live/etc/systemd/system/privacy-shield.service"
printf '%s\n' 'original rules' >"$TEST_ROOT/live/etc/privacy-shield.nft"
printf '%s\n' 'original Tor listener config' >"$TEST_ROOT/live/etc/tor/torrc.d/privacy-shield.conf"
printf 'User tor\n%%include %s\n' "$TEST_ROOT/live/etc/tor/torrc.d/privacy-shield.conf" >"$TEST_ROOT/live/etc/tor/torrc"
printf '%s\n' 'old log' >"$TEST_ROOT/live/var/log/privacy-shield.log"
chmod 644 "$TEST_ROOT/live/var/log/privacy-shield.log"
touch "$TEST_ROOT/old_enabled"

cat >"$TEST_ROOT/bin/mock-command" <<'MOCK'
#!/usr/bin/env bash
set -euo pipefail
root=$STEALTH_TEST_ROOT
printf '%s\n' "$*" >> "$root/mock_calls"
case "${0##*/}" in
    nft)
        [[ $* == '-c -D uplink="wlan0" -f '* ]] && exit 0
        [[ $* == 'list table inet stealth' || $* == 'list table inet privacy_shield' ]] && exit 1
        exit 1
        ;;
    tor)
        count=0
        [[ -f $root/tor_count ]] && count=$(cat "$root/tor_count")
        count=$(( count + 1 ))
        printf '%s\n' "$count" > "$root/tor_count"
        if [[ ${MOCK_TOR_FAIL_AT:-0} == "$count" ]]; then
            exit 1
        fi
        ;;
    systemd-analyze) ;;
    visudo)
        case "$*" in
            '-cqf '*) /usr/bin/visudo "$@" 2> >(grep -v 'sudo.conf is owned' >&2) ;;
            '-c')
                [[ ${MOCK_VISUDO_FAIL:-0} == 1 ]] && exit 1
                for file in "$root"/live/etc/sudoers.d/*; do
                    [[ -e $file ]] && printf '%s: parsed OK\n' "$file"
                done
                ;;
            *) printf 'Unexpected visudo command: %s\n' "$*" >&2; exit 1 ;;
        esac
        ;;
    systemctl)
        case "$*" in
            'daemon-reload') ;;
            'is-active --quiet '*) exit 3 ;;
            'is-enabled --quiet privacy-shield.service') [[ -e $root/old_enabled ]] ;;
            'disable privacy-shield.service') rm "$root/old_enabled" ;;
            'enable privacy-shield.service') touch "$root/old_enabled" ;;
            *) printf 'Unexpected systemctl command: %s\n' "$*" >&2; exit 1 ;;
        esac
        ;;
esac
MOCK
chmod +x "$TEST_ROOT/bin/mock-command"
for command in nft tor systemd-analyze systemctl visudo; do
    ln -s mock-command "$TEST_ROOT/bin/$command"
done

run_install() {
    unshare -Urn env PATH="$TEST_ROOT/bin:$PATH" STEALTH_TEST_ROOT="$TEST_ROOT" \
        MOCK_TOR_FAIL_AT="${MOCK_TOR_FAIL_AT:-0}" MOCK_VISUDO_FAIL="${MOCK_VISUDO_FAIL:-0}" \
        bash "$TEST_ROOT/stage/install-stealth.sh"
}

assert_legacy_restored() {
    [[ $(cat "$TEST_ROOT/live/usr/local/bin/privacy-shield.sh") == 'original script' ]]
    [[ $(cat "$TEST_ROOT/live/etc/systemd/system/privacy-shield.service") == 'original unit' ]]
    [[ $(cat "$TEST_ROOT/live/etc/privacy-shield.nft") == 'original rules' ]]
    [[ $(cat "$TEST_ROOT/live/etc/tor/torrc.d/privacy-shield.conf") == 'original Tor listener config' ]]
    [[ $(cat "$TEST_ROOT/live/var/log/privacy-shield.log") == 'old log' ]]
    [[ -e $TEST_ROOT/old_enabled ]]
    [[ ! -e $TEST_ROOT/live/usr/local/bin/stealth ]]
    [[ ! -e $TEST_ROOT/live/etc/systemd/system/stealth.service ]]
    [[ ! -e $TEST_ROOT/live/etc/stealth.nft ]]
    [[ ! -e $TEST_ROOT/live/etc/tor/torrc.d/stealth.conf ]]
    [[ ! -e $TEST_ROOT/live/etc/sudoers.d/stealth ]]
    [[ ! -e $TEST_ROOT/live/var/log/stealth.log ]]
    [[ $(rg -c '^%include ' "$TEST_ROOT/live/etc/tor/torrc") == 1 ]]
    rg -Fq 'privacy-shield.conf' "$TEST_ROOT/live/etc/tor/torrc"
}

exec 9>"$TEST_ROOT/live/run/stealth.lock"
flock -n 9
if run_install 9>&- >"$TEST_ROOT/locked-install.out" 2>&1; then
    printf 'Expected installer to reject a concurrent Stealth operation.\n' >&2
    exit 1
fi
rg -Fq 'Another Stealth operation is in progress; retry after it finishes.' "$TEST_ROOT/locked-install.out"
assert_legacy_restored
[[ ! -e $TEST_ROOT/mock_calls && ! -e $TEST_ROOT/live/var/backups ]]
flock -u 9
exec 9>&-

exec 8>"$TEST_ROOT/live/run/privacy-shield.lock"
flock -n 8
if run_install 8>&- >"$TEST_ROOT/legacy-locked-install.out" 2>&1; then
    printf 'Expected installer to reject a concurrent legacy operation.\n' >&2
    exit 1
fi
rg -Fq 'The previous backend is changing state; retry after it finishes.' "$TEST_ROOT/legacy-locked-install.out"
assert_legacy_restored
[[ ! -e $TEST_ROOT/mock_calls && ! -e $TEST_ROOT/live/var/backups ]]
flock -n "$TEST_ROOT/live/run/stealth.lock" true
flock -u 8
exec 8>&-

MOCK_TOR_FAIL_AT=3
export MOCK_TOR_FAIL_AT
if run_install >"$TEST_ROOT/failed-install.out" 2>&1; then
    printf 'Expected installer rollback.\n' >&2
    exit 1
fi
assert_legacy_restored
unset MOCK_TOR_FAIL_AT
rm "$TEST_ROOT/tor_count"

if MOCK_VISUDO_FAIL=1 run_install >"$TEST_ROOT/sudoers-failed-install.out" 2>&1; then
    printf 'Expected installer rollback after sudoers validation.\n' >&2
    exit 1
fi
assert_legacy_restored
rm "$TEST_ROOT/tor_count"

run_install >"$TEST_ROOT/successful-install.out" 2>&1
rg -q 'stealth-transparent-v2' "$TEST_ROOT/live/usr/local/bin/stealth"
[[ -f $TEST_ROOT/live/etc/systemd/system/stealth.service ]]
[[ -f $TEST_ROOT/live/etc/stealth.nft ]]
[[ -f $TEST_ROOT/live/etc/tor/torrc.d/stealth.conf ]]
[[ $(stat -c '%a' "$TEST_ROOT/live/etc/sudoers.d/stealth") == 440 ]]
rg -q '^Defaults!STEALTH_CMDS !use_pty$' "$TEST_ROOT/live/etc/sudoers.d/stealth"
[[ $(cat "$TEST_ROOT/successful-install.out") != *Warning* ]]
[[ $(stat -c '%a' "$TEST_ROOT/live/var/log/stealth.log") == 600 ]]
[[ $(cat "$TEST_ROOT/live/var/log/stealth.log") == 'old log' ]]
[[ -d $TEST_ROOT/live/var/backups/stealth ]]
[[ ! -e $TEST_ROOT/old_enabled ]]
[[ ! -e $TEST_ROOT/live/usr/local/bin/privacy-shield.sh ]]
[[ ! -e $TEST_ROOT/live/etc/systemd/system/privacy-shield.service ]]
[[ ! -e $TEST_ROOT/live/etc/privacy-shield.nft ]]
[[ ! -e $TEST_ROOT/live/etc/tor/torrc.d/privacy-shield.conf ]]
[[ ! -e $TEST_ROOT/live/var/log/privacy-shield.log ]]
[[ $(rg -c '^%include ' "$TEST_ROOT/live/etc/tor/torrc") == 1 ]]
rg -Fq 'stealth.conf' "$TEST_ROOT/live/etc/tor/torrc"

run_install >"$TEST_ROOT/reinstall.out"
[[ $(rg -c '^%include ' "$TEST_ROOT/live/etc/tor/torrc") == 1 ]]
[[ $(stat -c '%a' "$TEST_ROOT/live/var/log/stealth.log") == 600 ]]

printf 'PASS: concurrency lock, migration, rollback after legacy cleanup or sudoers failure, backups, Tor include, sudoers drop-in, log permissions, and reinstall\n'
