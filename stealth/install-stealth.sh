#!/usr/bin/env bash
set -Eeuo pipefail
umask 077

((EUID == 0)) || {
    printf 'Run with sudo.\n' >&2
    exit 1
}
SOURCE=$(dirname "$(readlink -f "$0")")
SCRIPT=/usr/local/bin/stealth
UNIT=/etc/systemd/system/stealth.service
RULES=/etc/stealth.nft
TORRC=/etc/tor/torrc
TOR_DROPIN=/etc/tor/torrc.d/stealth.conf
INCLUDE_LINE='%include /etc/tor/torrc.d/stealth.conf'
SUDOERS_DROPIN=/etc/sudoers.d/stealth
LOG=/var/log/stealth.log

OLD_SCRIPT=/usr/local/bin/privacy-shield.sh
OLD_UNIT=/etc/systemd/system/privacy-shield.service
OLD_RULES=/etc/privacy-shield.nft
OLD_TOR_DROPIN=/etc/tor/torrc.d/privacy-shield.conf
OLD_INCLUDE_LINE='%include /etc/tor/torrc.d/privacy-shield.conf'
OLD_LOG=/var/log/privacy-shield.log
OLD_LOCK=/run/privacy-shield.lock

for command in nft tor systemd-analyze systemctl visudo ip nmcli ss curl jq flock uuidgen od; do
    command -v "$command" >/dev/null || {
        printf 'Missing command: %s\n' "$command" >&2
        exit 1
    }
done
exec 9>/run/stealth.lock
flock -n 9 || {
    printf 'Another Stealth operation is in progress; retry after it finishes.\n' >&2
    exit 1
}
if [[ -e $OLD_LOCK ]]; then
    exec 8<"$OLD_LOCK"
    flock -n 8 || {
        printf 'The previous backend is changing state; retry after it finishes.\n' >&2
        exit 1
    }
fi

[[ -f $TORRC ]] || {
    printf 'Missing Tor configuration: %s\n' "$TORRC" >&2
    exit 1
}
for service in stealth.service privacy-shield.service tor.service; do
    if systemctl is-active --quiet "$service"; then
        printf 'Stop %s before installing Stealth.\n' "$service" >&2
        exit 1
    fi
done
for table in stealth privacy_shield; do
    if nft list table inet "$table" >/dev/null 2>&1; then
        printf 'The %s firewall is active; stop it before installing.\n' "$table" >&2
        exit 1
    fi
done
for state in /run/stealth/state /run/privacy-shield/state; do
    if [[ -e $state ]]; then
        printf 'Saved state remains at %s; recover the service before installing.\n' "$state" >&2
        exit 1
    fi
done
TMP=$(mktemp -d)
trap 'rm -rf "$TMP"' EXIT
cp "$SOURCE/stealth.sh" "$TMP/stealth"
cp "$SOURCE/stealth.service" "$TMP/stealth.service"
cp "$SOURCE/stealth.nft" "$TMP/stealth.nft"
cp "$SOURCE/torrc.conf" "$TMP/torrc.conf"
cp "$SOURCE/stealth.sudoers" "$TMP/stealth.sudoers"
awk -v new="$INCLUDE_LINE" -v old="$OLD_INCLUDE_LINE" \
    '$0 != new && $0 != old' "$TORRC" >"$TMP/torrc.preview"
cat "$TMP/torrc.conf" >>"$TMP/torrc.preview"
bash -n "$TMP/stealth"
tor --verify-config -f "$TMP/torrc.preview" >/dev/null
nft -c -D 'uplink="wlan0"' -f "$TMP/stealth.nft"
visudo -cqf "$TMP/stealth.sudoers"

install -d -m 700 /var/backups/stealth
BACKUP=$(mktemp -d /var/backups/stealth/install.XXXXXXXX)
chmod 700 "$BACKUP"
save_file() {
    local source=$1 name=$2
    if [[ -e $source || -L $source ]]; then
        cp -a "$source" "$BACKUP/$name"
    else
        touch "$BACKUP/$name.absent"
    fi
}
restore_file() {
    local destination=$1 name=$2
    if [[ -e $BACKUP/$name.absent ]]; then
        rm -f "$destination"
    else
        cp -a "$BACKUP/$name" "$destination"
    fi
}
save_file "$SCRIPT" script
save_file "$UNIT" unit
save_file "$RULES" rules
save_file "$TORRC" torrc
save_file "$TOR_DROPIN" tor-dropin
save_file "$SUDOERS_DROPIN" sudoers
save_file "$LOG" log
save_file "$OLD_SCRIPT" old-script
save_file "$OLD_UNIT" old-unit
save_file "$OLD_RULES" old-rules
save_file "$OLD_TOR_DROPIN" old-tor-dropin
save_file "$OLD_LOG" old-log
old_unit_enabled=0
if [[ -e $OLD_UNIT ]] && systemctl is-enabled --quiet privacy-shield.service; then
    old_unit_enabled=1
fi

rollback() {
    trap - ERR
    set +e
    restore_file "$SCRIPT" script
    restore_file "$UNIT" unit
    restore_file "$RULES" rules
    restore_file "$TORRC" torrc
    restore_file "$TOR_DROPIN" tor-dropin
    restore_file "$SUDOERS_DROPIN" sudoers
    restore_file "$LOG" log
    restore_file "$OLD_SCRIPT" old-script
    restore_file "$OLD_UNIT" old-unit
    restore_file "$OLD_RULES" old-rules
    restore_file "$OLD_TOR_DROPIN" old-tor-dropin
    restore_file "$OLD_LOG" old-log
    systemctl daemon-reload
    if ((old_unit_enabled)); then
        systemctl enable privacy-shield.service
    fi
    printf 'Installation failed; previous files restored from %s\n' "$BACKUP" >&2
    exit 1
}
trap rollback ERR

install -m 755 "$TMP/stealth" "$SCRIPT"
install -m 644 "$TMP/stealth.service" "$UNIT"
install -m 644 "$TMP/stealth.nft" "$RULES"
install -d -m 755 /etc/tor/torrc.d
install -m 644 "$TMP/torrc.conf" "$TOR_DROPIN"
install -d -m 750 /etc/sudoers.d
install -m 440 "$TMP/stealth.sudoers" "$SUDOERS_DROPIN"
sudoers_report=$(visudo -c)
grep -Fq "$SUDOERS_DROPIN:" <<<"$sudoers_report" ||
    printf 'Warning: /etc/sudoers does not include %s; sudo keeps its pty for Stealth.\n' "$SUDOERS_DROPIN" >&2
awk -v new="$INCLUDE_LINE" -v old="$OLD_INCLUDE_LINE" \
    '$0 != new && $0 != old' "$TORRC" >"$TMP/torrc.rewritten"
printf '\n%s\n' "$INCLUDE_LINE" >>"$TMP/torrc.rewritten"
cat "$TMP/torrc.rewritten" >"$TORRC"
tor --verify-config -f "$TORRC" >/dev/null
systemd-analyze verify "$UNIT"
if [[ -e $OLD_UNIT ]] && ((old_unit_enabled)); then
    systemctl disable privacy-shield.service
fi
systemctl daemon-reload

rm -f "$OLD_SCRIPT" "$OLD_UNIT" "$OLD_RULES" "$OLD_TOR_DROPIN"
if [[ -e $OLD_LOG ]]; then
    if [[ ! -e $LOG ]]; then
        mv "$OLD_LOG" "$LOG"
    else
        rm -f "$OLD_LOG"
    fi
fi
if [[ -e $LOG ]]; then
    chmod 600 "$LOG"
fi
if [[ -e $OLD_LOCK ]]; then
    rm -f "$OLD_LOCK"
fi
rmdir /run/privacy-shield 2>/dev/null || true
tor --verify-config -f "$TORRC" >/dev/null
systemctl daemon-reload

trap - ERR
printf 'Installed Stealth backend. Backup: %s\n' "$BACKUP"
printf 'Service was not started. Use stealth-start, stealth-status, and stealth-stop in a new terminal.\n'
printf 'Stealth requires a NetworkManager Wi-Fi or Ethernet default route and a Tor service account named tor.\n'
