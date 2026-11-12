#!/usr/bin/env bash

if [[ -d /run/stealth ]] && systemctl is-active --quiet tor.service &&
    { [[ -f /run/stealth/active ]] ||
        [[ $(/usr/local/bin/stealth version 2>/dev/null) == stealth-transparent-v1 ]]; }; then
    printf '%s\n%s\n' "$HOME/.config/waybar/icons/stealth.svg" 'Stealth mode active · Tor routing'
else
    printf '\n\n'
fi
