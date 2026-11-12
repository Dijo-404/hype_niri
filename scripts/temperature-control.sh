#!/usr/bin/env bash

set -euo pipefail

HWMON_ROOT="${HWMON_ROOT:-/sys/class/hwmon}"
THERMAL_ROOT="${THERMAL_ROOT:-/sys/class/thermal}"
WARNING_TEMP="${HYPE_NIRI_TEMP_WARNING:-70}"
CRITICAL_TEMP="${HYPE_NIRI_TEMP_CRITICAL:-80}"

[[ "$WARNING_TEMP" =~ ^[0-9]{1,3}$ ]] || WARNING_TEMP=70
[[ "$CRITICAL_TEMP" =~ ^[0-9]{1,3}$ ]] || CRITICAL_TEMP=80
WARNING_TEMP=$((10#$WARNING_TEMP))
CRITICAL_TEMP=$((10#$CRITICAL_TEMP))

source "${BASH_SOURCE[0]%/*}/runtime-dir.sh"
CACHE_FILE="$RUNTIME_DIR/hype-niri-temp-sensor"

best_score=-9999
best_temp=""
best_source=""
best_input=""

read_first_line() {
    local file="$1"
    local value=""

    [ -r "$file" ] || return 1
    IFS= read -r value <"$file" || return 1
    printf '%s' "$value"
}

sensor_identity() {
    local input="$1" directory name label resolved
    directory="${input%/*}"
    resolved="$(realpath -e -- "$input")" || return 1
    if [[ "$input" == "$HWMON_ROOT"/hwmon*/temp*_input ]]; then
        name="$(read_first_line "$directory/name" 2>/dev/null || printf 'hwmon')"
        label="$(read_first_line "${input%_input}_label" 2>/dev/null || true)"
    else
        name="$(read_first_line "$directory/type" 2>/dev/null || printf 'thermal')"
        label="$name"
    fi
    printf '%q %q %q' "$resolved" "$name" "$label"
}

emit() {
    local value
    local -a escaped=()
    for value in "$@"; do
        value=${value//\\/\\\\}
        value=${value//\"/\\\"}
        value=${value//$'\n'/ }
        value=${value//$'\r'/ }
        value=${value//$'\t'/ }
        escaped+=("$value")
    done
    printf '{"text":"%s","tooltip":"%s","class":"%s"}\n' "${escaped[@]}"
}

to_celsius() {
    local raw="$1"

    raw=${raw//[[:space:]]/}
    [[ "$raw" =~ ^-?[0-9]+$ ]] || return 1
    ((${#raw} <= 10)) || return 1
    if [[ "$raw" == -* ]]; then
        raw=$((-10#${raw#-}))
    else
        raw=$((10#$raw))
    fi

    if ((raw > 1000 || raw < -1000)); then
        if ((raw < 0)); then
            temperature_celsius=$(((raw - 500) / 1000))
        else
            temperature_celsius=$(((raw + 500) / 1000))
        fi
    else
        temperature_celsius="$raw"
    fi
}

score_sensor() {
    local name
    local label
    local path
    local haystack
    local score=0

    name="${1,,}"
    label="${2,,}"
    path="${3,,}"
    haystack="$name $label $path"

    if [[ "$haystack" =~ (nvme|iwlwifi|wifi|wireless|bat|battery|ucsi|usb|charger|adapter|amdgpu|radeon|nouveau|nvidia|gpu|drm) ]]; then
        score=$((score - 300))
    fi

    if [[ "$name" =~ (coretemp|k10temp|zenpower|cpu_thermal|x86_pkg_temp|fam15h_power) ]]; then
        score=$((score + 120))
    elif [[ "$name" =~ (acpitz|thermal|soc) ]]; then
        score=$((score + 20))
    fi

    if [[ "$label" =~ (package|tdie) ]]; then
        score=$((score + 90))
    elif [[ "$label" =~ (tctl|cpu) ]]; then
        score=$((score + 80))
    elif [[ "$label" =~ core ]]; then
        score=$((score + 40))
    elif [ -z "$label" ]; then
        score=$((score + 5))
    fi

    printf '%d\n' "$score"
}

consider_sensor() {
    local name="$1"
    local label="$2"
    local input_file="$3"
    local raw
    local temp
    local score
    local source

    raw="$(read_first_line "$input_file" 2>/dev/null || true)"
    to_celsius "$raw" || return 0
    temp="$temperature_celsius"

    if ((temp < -40 || temp > 150)); then
        return 0
    fi

    score="$(score_sensor "$name" "$label" "$input_file")"
    source="$name"
    [ -n "$label" ] && source="$source / $label"

    if ((score > best_score)) || { ((score == best_score)) && { [ -z "$best_temp" ] || ((temp > best_temp)); }; }; then
        best_score="$score"
        best_temp="$temp"
        best_source="$source"
        best_input="$input_file"
    fi
}

scan_hwmon() {
    local hwmon
    local input
    local name
    local label
    local base

    for hwmon in "$HWMON_ROOT"/hwmon*; do
        [ -d "$hwmon" ] || continue
        name="$(read_first_line "$hwmon/name" 2>/dev/null || printf 'hwmon')"

        for input in "$hwmon"/temp*_input; do
            [ -e "$input" ] || continue
            base="${input%_input}"
            label="$(read_first_line "${base}_label" 2>/dev/null || true)"
            consider_sensor "$name" "$label" "$input"
        done
    done
}

scan_thermal_zones() {
    local zone
    local name

    for zone in "$THERMAL_ROOT"/thermal_zone*; do
        [ -d "$zone" ] || continue
        [ -r "$zone/temp" ] || continue
        name="$(read_first_line "$zone/type" 2>/dev/null || printf 'thermal')"
        consider_sensor "$name" "$name" "$zone/temp"
    done
}

emit_temp() {
    local temp="$1"
    local source="$2"
    local temp_class

    if ((temp >= CRITICAL_TEMP)); then
        temp_class="critical"
    elif ((temp >= WARNING_TEMP)); then
        temp_class="warning"
    else
        temp_class="normal"
    fi

    emit "${temp}°" "CPU temperature: ${temp}°C (${source})" "$temp_class"
}

if [ -r "$CACHE_FILE" ]; then
    IFS=$'\t' read -r cached_path cached_identity cached_source <"$CACHE_FILE" || true
    if [[ "${cached_path:-}" == "$HWMON_ROOT"/hwmon*/temp*_input || "${cached_path:-}" == "$THERMAL_ROOT"/thermal_zone*/temp ]] && [ -r "$cached_path" ] &&
        [[ -n "${cached_identity:-}" && "$cached_identity" == "$(sensor_identity "$cached_path")" ]]; then
        IFS= read -r cached_raw <"$cached_path" || cached_raw=""
        if to_celsius "$cached_raw" && ((temperature_celsius >= -40 && temperature_celsius <= 150)); then
            emit_temp "$temperature_celsius" "${cached_source:-cached}"
            exit 0
        fi
    fi
fi

scan_hwmon
scan_thermal_zones

if [ -z "$best_temp" ] || ((best_score <= 0)); then
    emit "--°" "Temperature unavailable" "missing"
    exit 0
fi

if [ -n "$best_input" ] && identity="$(sensor_identity "$best_input")"; then
    tmp_cache="$(mktemp "$RUNTIME_DIR/.hype-temp-sensor.XXXXXX")"
    trap 'rm -f -- "$tmp_cache"' EXIT
    printf '%s\t%s\t%s\n' "$best_input" "$identity" "$best_source" >"$tmp_cache"
    mv -f -- "$tmp_cache" "$CACHE_FILE"
fi

emit_temp "$best_temp" "$best_source"
