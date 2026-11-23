#!/usr/bin/env bash

set -euo pipefail
REPO="$(cd -- "${BASH_SOURCE[0]%/*}/.." && pwd -P)"
ui_args=()
if [[ ${HYPE_NOTCH_UI_TESTS:-0} == 1 ]]; then
    ui_args+=(--include-ignored)
fi
cargo test --workspace --locked --manifest-path "$REPO/Cargo.toml" -- "${ui_args[@]}" --test-threads=1
cargo build --workspace --bins --locked --manifest-path "$REPO/Cargo.toml"
binary="${CARGO_TARGET_DIR:-$REPO/target}/debug/hype-notch"
"$binary" --check
if "$binary" --invalid-option >/dev/null 2>&1; then
    echo "Invalid notch option unexpectedly succeeded" >&2
    exit 1
fi
echo "Rust workspace model, native services, style and CLI checks passed"
