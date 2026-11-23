#!/usr/bin/env bash
set -euo pipefail

REPO=$(dirname "$(dirname "$(realpath "$0")")")
CARGO=${CARGO:-cargo}
if ! command -v "$CARGO" >/dev/null 2>&1 && [[ -x $HOME/.cargo/bin/cargo ]]; then
    CARGO=$HOME/.cargo/bin/cargo
fi
exec "$CARGO" test --manifest-path "$REPO/Cargo.toml" -p hype-desktop --lib stealth::tests
