#!/usr/bin/env bash
hype_bootstrap() {
    set -Eeuo pipefail
    if [[ ${1:-} == --help || ${1:-} == -h ]]; then
        printf 'Usage: install.sh --setup-cli\n       install.sh [install | update] [--plain]\n       install.sh [--plain] --phase NAME\n\n--setup-cli installs the hype command in ~/.local/bin.\nRun as a regular Arch Linux user with sudo. yay is needed only for missing AUR packages.\n'
        return 0
    fi
    local temporary="" source_dir="" source_origin=checkout here="${BASH_SOURCE[0]:-}" build_log="" cargo_host answer code=0
    trap 'code=$?; [[ -z ${temporary:-} ]] || rm -rf -- "$temporary"; exit "$code"' EXIT
    trap 'exit 130' INT
    trap 'exit 143' TERM
    if ((EUID == 0)); then
        printf 'Run as your regular user; the installer uses sudo when required.\n' >&2
        exit 1
    fi
    if [[ $(uname -s) != Linux ]] || ! command -v pacman >/dev/null 2>&1; then
        printf 'This installer requires Arch Linux (pacman).\n' >&2
        exit 1
    fi
    # stdin can contain this script, so all interactive input comes from the terminal.
    if ! { exec 3<>/dev/tty; } 2>/dev/null; then
        printf 'An interactive terminal is required. Run the command from a terminal.\n' >&2
        exit 1
    fi
    if ! command -v cargo >/dev/null 2>&1 && [[ -x $HOME/.cargo/bin/cargo ]]; then
        export PATH="$HOME/.cargo/bin:$PATH"
    fi
    if ! command -v git >/dev/null 2>&1 || ! command -v cargo >/dev/null 2>&1 || ! command -v cc >/dev/null 2>&1; then
        printf 'Git, Rust and build tools are needed. Run a full Arch upgrade and install them? [y/N]: ' >&3
        IFS= read -r answer <&3 || exit 1
        [[ $answer == y || $answer == Y || $answer == yes || $answer == YES ]] || exit 1
        sudo pacman -Syu --needed git rust base-devel <&3 || exit 1
    fi
    if [[ -n $here && -f $here ]]; then
        here="$(cd -- "$(dirname -- "$here")" && pwd -P)"
        if [[ -f $here/Cargo.toml && -f $here/installer/Cargo.toml ]]; then
            source_dir=$here
        fi
    fi
    if [[ -z $source_dir ]]; then
        temporary="$(mktemp -d "${TMPDIR:-/tmp}/hype-niri-source.XXXXXXXX")"
        source_dir="$temporary/source"
        printf 'Hype Niri  /  Fetching main\n' >&2
        git clone --depth 1 --branch main --single-branch -- https://github.com/Dijo-404/hype_niri.git "$source_dir" <&3 || exit 1
        source_origin=main
    fi
    printf 'Hype Niri  /  Building installer\n' >&2
    if ! cargo_host=$(cargo -vV | sed -n 's/^host: //p') || [[ ! $cargo_host =~ ^[A-Za-z0-9_-]+$ ]]; then
        printf 'Could not determine Cargo host target.\n' >&2
        exit 1
    fi
    build_log="$(mktemp "${TMPDIR:-/tmp}/hype-niri-build.XXXXXXXX.log")"
    if ! cargo build --release --locked --package hype-install --target "$cargo_host" --manifest-path "$source_dir/Cargo.toml" --target-dir "$source_dir/target" <&3 >"$build_log" 2>&1; then
        tail -n 40 -- "$build_log" >&2
        printf 'Installer build failed. Full log: %s\n' "$build_log" >&2
        exit 1
    fi
    rm -f -- "$build_log"
    local status=0
    HYPE_NIRI_BOOTSTRAP_SOURCE="$source_origin" "$source_dir/target/$cargo_host/release/hype-install" --source "$source_dir" "$@" <&3 || status=$?
    exit "$status"
}

hype_bootstrap "$@"
