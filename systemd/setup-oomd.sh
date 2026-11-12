#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"

read -r _ systemd_version _ < <(systemctl --version)
if [[ ! "$systemd_version" =~ ^[0-9]+$ ]] || (( systemd_version < 257 )); then
    printf 'OOM protection requires systemd 257 or newer.\n' >&2
    exit 1
fi

if [[ ! -r /sys/fs/cgroup/cgroup.controllers || ! -r /proc/pressure/memory ]] ||
    ! grep -qw memory /sys/fs/cgroup/cgroup.controllers; then
    printf 'OOM protection requires cgroup v2, the memory controller, and PSI.\n' >&2
    exit 1
fi

privileged=()
if (( EUID != 0 )); then
    privileged=(sudo)
fi

"${privileged[@]}" install -Dm644 \
    "$SCRIPT_DIR/user@.service.d/60-hype-niri-oomd.conf" \
    /etc/systemd/system/user@.service.d/60-hype-niri-oomd.conf
"${privileged[@]}" systemctl daemon-reload
"${privileged[@]}" systemctl enable --now systemd-oomd.service

systemctl is-enabled --quiet systemd-oomd.service
systemctl is-active --quiet systemd-oomd.service

user_manager="user@${SUDO_UID:-$UID}.service"
if systemctl is-active --quiet "$user_manager"; then
    oom_policy="$(systemctl show "$user_manager" \
        -p MemoryAccounting -p ManagedOOMMemoryPressure -p ManagedOOMSwap \
        -p ManagedOOMMemoryPressureLimit -p ManagedOOMMemoryPressureDurationUSec)"
    for setting in MemoryAccounting=yes ManagedOOMMemoryPressure=kill ManagedOOMSwap=kill \
        ManagedOOMMemoryPressureLimit=1717986918 ManagedOOMMemoryPressureDurationUSec=10s; do
        if ! grep -qx "$setting" <<< "$oom_policy"; then
            printf 'OOM policy not active on %s: expected %s. Check other systemd drop-ins.\n' \
                "$user_manager" "$setting" >&2
            exit 1
        fi
    done
fi

printf 'systemd-oomd enabled and running. Monitored groups:\n'
oomctl --no-pager
