_stealth_installed() {
    local version
    version=$(/usr/local/bin/stealth version 2>/dev/null)
    [[ $version == stealth-transparent-v1 || $version == stealth-transparent-v2 ]]
}

stealth-start() {
    local version status_output current_mac expected_mac state_present=0 firewall_present=0
    version=$(/usr/local/bin/stealth version 2>/dev/null)
    if [[ $version != stealth-transparent-v1 && $version != stealth-transparent-v2 ]]; then
        print -u2 -- "Stealth is not installed. Run: sudo bash $HOME/.local/share/stealth/install-stealth.sh"
        return 1
    fi

    if [[ $version == stealth-transparent-v2 ]] && sudo test -f /run/stealth/active; then
        sudo /usr/local/bin/stealth status || return
        print -- '✓ Stealth enabled'
        return 0
    fi

    sudo test -f /run/stealth/state && state_present=1
    sudo nft list table inet stealth >/dev/null 2>&1 && firewall_present=1
    if (( state_present || firewall_present )); then
        if [[ $version == stealth-transparent-v1 && $state_present == 1 && $firewall_present == 1 ]]; then
            if status_output=$(sudo /usr/local/bin/stealth status); then
                print -r -- "$status_output"
                print -- '✓ Stealth enabled'
                return 0
            fi
            current_mac=$(print -r -- "$status_output" | awk -F ': ' '/^Current MAC: / { print $2; exit }')
            expected_mac=$(print -r -- "$status_output" | awk -F ': ' '/^Expected spoofed MAC: / { print $2; exit }')
            [[ -n $expected_mac ]] || expected_mac=$(print -r -- "$status_output" | awk -F ': ' '/^Saved pre-start MAC: / { print $2; exit }')
            if ! systemctl is-active --quiet stealth.service &&
                [[ $status_output == *'Tor service: active'* &&
                   $status_output == *'Fail-closed firewall: active'* &&
                   $status_output == *'Transparent Tor check: verified ('* &&
                   -n $expected_mac && $current_mac == $expected_mac ]]; then
                print -r -- "$status_output" | awk '$0 != "Warning: service is not active; firewall is blocking traffic after a failed start."'
                print -- '✓ Stealth enabled'
                return 0
            fi
            print -r -- "$status_output"
            return 1
        fi
        sudo /usr/local/bin/stealth status
        return 1
    fi

    if systemctl is-active --quiet stealth.service; then
        sudo systemctl stop stealth.service || return
    fi
    sudo systemctl start stealth.service || return
    sudo /usr/local/bin/stealth status || return
    print -- '✓ Stealth enabled'
}

stealth-status() {
    if ! _stealth_installed; then
        print -u2 -- "Stealth is not installed. Run: sudo bash $HOME/.local/share/stealth/install-stealth.sh"
        print -- "Stealth service: $(systemctl is-active stealth.service 2>/dev/null)"
        print -- "Tor service: $(systemctl is-active tor.service 2>/dev/null)"
        return 1
    fi
    sudo /usr/local/bin/stealth status
}

stealth-stop() {
    if ! _stealth_installed; then
        if [[ $(systemctl is-active stealth.service 2>/dev/null) == active ]]; then
            sudo systemctl stop stealth.service || return
            print -- '✓ Stealth disabled'
        else
            print -u2 -- "Stealth is not installed. Run: sudo bash $HOME/.local/share/stealth/install-stealth.sh"
            return 1
        fi
        return 0
    fi
    sudo /usr/local/bin/stealth stop || return
    sudo systemctl stop stealth.service || return
    if systemctl is-failed --quiet stealth.service; then
        sudo systemctl reset-failed stealth.service || return
    fi
    print -- '✓ Stealth disabled'
}
