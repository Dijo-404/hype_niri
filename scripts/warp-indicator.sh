#!/usr/bin/env bash
set -euo pipefail

if ! systemctl is-active --quiet warp-svc.service ||
    pgrep -u "$UID" -x warp-taskbar >/dev/null 2>&1; then
    printf '{"text":""}\n'
    exit 0
fi

status="$(timeout 2s warp-cli --accept-tos --json status 2>/dev/null)" || status='{}'
state="$(jq -er '.status | strings | select(length > 0)' <<<"$status" 2>/dev/null)" || state=Unknown
class=running
[[ "$state" != Connected ]] || class=connected

jq -cn --arg state "$state" --arg class "$class" \
    '{text: "󰖟", tooltip: ("Cloudflare WARP · " + $state), class: $class}'
