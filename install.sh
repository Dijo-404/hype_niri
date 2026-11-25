#!/usr/bin/env bash
hype_bootstrap() (
    set -Eeuo pipefail
    if [[ ${1:-} == --help || ${1:-} == -h ]]; then
        printf 'Usage: install.sh --setup-cli\n       install.sh [install | update] [--plain]\n       install.sh [--plain] --phase NAME\n\ninstall and --setup-cli register ~/bin/hype. Downloads go to ~/hype-niri/source.\nRun as a regular Arch Linux user with sudo. The installer sets up yay when it is missing.\n'
        return 0
    fi
    local source_dir="" source_origin=checkout here="${BASH_SOURCE[0]:-}" build_log cargo_host answer code
    bootstrap_stage=""
    trap 'code=$?; [[ -z $bootstrap_stage ]] || rm -rf -- "$bootstrap_stage"; exit "$code"' EXIT
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
    # A piped bootstrap still needs the terminal for prompts.
    if ! { exec 3<>/dev/tty; } 2>/dev/null; then
        printf 'An interactive terminal is required. Run the command from a terminal.\n' >&2
        exit 1
    fi
    if ! command -v cargo >/dev/null 2>&1 && [[ -x $HOME/.cargo/bin/cargo ]]; then
        export PATH="$HOME/.cargo/bin:$PATH"
    fi
    if ! command -v git >/dev/null 2>&1 || ! command -v cargo >/dev/null 2>&1 || ! command -v cc >/dev/null 2>&1; then
        while true; do
            printf 'Git, Rust and build tools are needed. Run a full Arch upgrade and install them? [Y/n, Yes in 5s]: ' >&3
            if IFS= read -r -t 5 answer <&3; then
                case ${answer,,} in
                    '' | y | yes) break ;;
                    n | no) exit 1 ;;
                    *) printf 'Please answer y or n.\n' >&3 ;;
                esac
            else
                code=$?
                ((code > 128)) || exit 1
                printf '\n' >&3
                break
            fi
        done
        sudo pacman -Syu --needed git rust base-devel <&3 || exit 1
    fi
    if [[ -n $here && -f $here ]]; then
        here="$(cd -- "$(dirname -- "$here")" && pwd -P)"
        if [[ -f $here/Cargo.toml && -f $here/installer/Cargo.toml ]]; then
            source_dir=$here
        fi
    fi
    if [[ -z $source_dir ]]; then
        local app_dir="$HOME/hype-niri" repository=https://github.com/Dijo-404/hype_niri.git
        mkdir -p -- "$app_dir"
        if [[ -L $app_dir || ! -O $app_dir ]]; then
            printf 'Expected an owned directory at %s\n' "$app_dir" >&2
            exit 1
        fi
        exec 4<"$app_dir"
        if ! flock -n 4; then
            printf 'Another Hype Niri operation is using %s\n' "$app_dir" >&2
            exit 1
        fi
        source_dir="$app_dir/source"
        printf 'Hype Niri  /  Fetching main into %s\n' "$source_dir" >&2
        if [[ -e $source_dir || -L $source_dir ]]; then
            if [[ -L $source_dir || ! -d $source_dir/.git || -L $source_dir/.git ]]; then
                printf 'Expected a Git checkout at %s\n' "$source_dir" >&2
                exit 1
            fi
            git -C "$source_dir" pull --ff-only -- "$repository" main <&3
        else
            bootstrap_stage="$(mktemp -d "$app_dir/.source.XXXXXXXX")"
            git clone --depth 1 --branch main --single-branch -- "$repository" "$bootstrap_stage/source" <&3
            mv -T --no-clobber -- "$bootstrap_stage/source" "$source_dir"
            if [[ -e $bootstrap_stage/source ]]; then
                printf 'Download destination appeared during setup: %s\n' "$source_dir" >&2
                exit 1
            fi
            rmdir -- "$bootstrap_stage"
            bootstrap_stage=""
        fi
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
    if [[ $source_origin == main ]]; then
        exec 4<&-
    fi
    local status=0
    HYPE_NIRI_BOOTSTRAP_SOURCE="$source_origin" "$source_dir/target/$cargo_host/release/hype-install" --source "$source_dir" "$@" <&3 || status=$?
    exit "$status"
)

hype_bootstrap "$@"
