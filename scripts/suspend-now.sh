#!/usr/bin/env bash
set -euo pipefail

"$HOME/.config/scripts/caffeine-control.sh" stop >/dev/null 2>&1 || true

"$HOME/.config/scripts/prepare-sleep.sh"

exec systemctl suspend
