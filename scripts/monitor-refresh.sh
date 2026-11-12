#!/usr/bin/env bash

set -euo pipefail

NOTIFY_ID=2006
source "${BASH_SOURCE[0]%/*}/runtime-dir.sh"
source "${BASH_SOURCE[0]%/*}/niri-outputs.sh"
LOCK_FILE="$RUNTIME_DIR/hype-monitor-refresh.lock"

if command -v flock >/dev/null 2>&1; then
    exec 9>"$LOCK_FILE"
    flock -n 9 || exit 0
fi

notify() {
    local title="$1"
    local body="${2:-}"

    command -v notify-send >/dev/null 2>&1 || return 0
    notify-send -r "$NOTIFY_ID" "$title" "$body" 2>/dev/null || true
}

missing_commands() {
    local missing=()
    local cmd

    for cmd in niri jq fuzzel awk mktemp; do
        command -v "$cmd" >/dev/null 2>&1 || missing+=("$cmd")
    done

    if [ "${#missing[@]}" -gt 0 ]; then
        notify "Monitor refresh unavailable" "Missing required command(s): ${missing[*]}"
        exit 1
    fi
}

trim() {
    local value="${1:-}"

    value="${value#"${value%%[![:space:]]*}"}"
    value="${value%"${value##*[![:space:]]}"}"
    printf '%s' "$value"
}

format_rate() {
    awk -v mhz="$1" 'BEGIN {
        hz = mhz / 1000
        value = sprintf("%.3f", hz)
        sub(/0+$/, "", value)
        sub(/\.$/, "", value)
        print value
    }'
}

display_rate() {
    awk -v mhz="$1" 'BEGIN {
        hz = mhz / 1000
        nearest = int(hz + 0.5)
        diff = hz - nearest
        if (diff < 0) {
            diff = -diff
        }

        if (diff < 0.01) {
            print nearest
        } else {
            value = sprintf("%.2f", hz)
            sub(/0+$/, "", value)
            sub(/\.$/, "", value)
            print value
        }
    }'
}

scale_for_resolution() {
    local width="$1"
    local height="$2"
    local short

    [[ "$width" =~ ^[0-9]+$ ]] || {
        printf '1.0\n'
        return
    }
    [[ "$height" =~ ^[0-9]+$ ]] || {
        printf '1.0\n'
        return
    }

    if [ "$width" -lt "$height" ]; then
        short="$width"
    else
        short="$height"
    fi

    if [ "$short" -ge 1800 ]; then
        printf '2.0\n'
    elif [ "$short" -ge 1600 ]; then
        printf '1.5\n'
    else
        printf '1.0\n'
    fi
}

get_outputs_json() {
    local output

    if ! output="$(niri msg --json outputs 2>&1)"; then
        notify "Could not read monitors" "$output"
        exit 1
    fi

    printf '%s' "$output"
}

build_monitor_options() {
    local json="$1"

    jq -r '
        to_entries[]
        | .value as $output
        | (
            if $output.current_mode == null then
                null
            else
                ($output.modes[$output.current_mode] // null)
            end
        ) as $mode
        | [
            $output.name,
            ($output.make // ""),
            ($output.model // ""),
            (if $mode == null then "" else ($mode.width | tostring) end),
            (if $mode == null then "" else ($mode.height | tostring) end),
            (if $mode == null then "" else ($mode.refresh_rate | tostring) end)
        ]
        | @tsv
    ' <<<"$json" | while IFS=$'\t' read -r name make model width height refresh; do
        local description
        local current
        local label

        description="$(trim "$make $model")"
        [ -n "$description" ] || description="Unknown display"

        if [ -n "$width" ] && [ -n "$height" ] && [ -n "$refresh" ]; then
            current="${width}x${height} @ $(display_rate "$refresh") Hz"
        else
            current="current mode unknown"
        fi

        label="$(printf '%-7s %-24s %s' "$name" "$description" "$current")"
        printf '%s\t%s\n' "$name" "$label"
    done
}

build_mode_options() {
    local json="$1"
    local output_name="$2"

    jq -r --arg output "$output_name" '
        .[$output] as $selected
        | if $selected == null then
            empty
          else
            ($selected.current_mode // -1) as $current
            | $selected.modes
            | to_entries
            | map({
                width: .value.width,
                height: .value.height,
                refresh: .value.refresh_rate,
                current: (.key == $current),
                preferred: (.value.is_preferred // false)
            })
            | sort_by([.width, .height, .refresh])
            | group_by([.width, .height, .refresh])
            | map({
                width: .[0].width,
                height: .[0].height,
                refresh: .[0].refresh,
                current: any(.[]; .current),
                preferred: any(.[]; .preferred)
            })
            | sort_by([(-.width), (-.height), (-.refresh)])
            | .[]
            | [
                (.width | tostring),
                (.height | tostring),
                (.refresh | tostring),
                (if .current then "current" else "" end),
                (if .preferred then "preferred" else "" end)
            ]
            | @tsv
          end
    ' <<<"$json" | while IFS=$'\t' read -r width height refresh is_current is_preferred; do
        local mode_rate
        local label_rate
        local mode
        local label
        local status=""
        local resolution

        mode_rate="$(format_rate "$refresh")"
        label_rate="$(display_rate "$refresh")"
        mode="${width}x${height}@${mode_rate}"
        resolution="${width}x${height}"

        [ "$is_current" = "current" ] && status="current"
        if [ "$is_preferred" = "preferred" ]; then
            [ -n "$status" ] && status="$status, "
            status="${status}preferred"
        fi
        [ -n "$status" ] && status="  [$status]"

        label="$(printf '%-11s %8s Hz%s' "$resolution" "$label_rate" "$status")"
        printf '%s\t%s\n' "$mode" "$label"
    done
}

choose_line() {
    local prompt="$1"
    local options="$2"
    local choice

    choice="$(
        printf '%s\n' "$options" |
            fuzzel --dmenu \
                --prompt "$prompt: " \
                --width 58 \
                --lines 14 \
                --no-sort \
                --only-match \
                --with-nth 2 \
                --accept-nth 1 \
                --match-nth 2 ||
            true
    )"
    [ -n "$choice" ] || exit 0

    printf '%s' "$choice"
}

apply_mode() {
    local output_name="$1"
    local mode="$2"
    local output

    if ! output="$(niri msg output "$output_name" mode "$mode" 2>&1)"; then
        notify "Could not change refresh rate" "$output"
        exit 1
    fi
}

apply_scale() {
    local output_name="$1"
    local scale="$2"
    local output

    if ! output="$(niri msg output "$output_name" scale "$scale" 2>&1)"; then
        notify "Could not set display scale" "$output"
        exit 1
    fi
}

save_output_mode() {
    local id="$1"
    local connector="$2"
    local mode="$3"
    local scale="$4"
    local tmp_file

    mkdir -p "$(dirname "$OUTPUTS_KDL")"
    [ -e "$OUTPUTS_KDL" ] || : >"$OUTPUTS_KDL"
    tmp_file="$(mktemp "$(dirname "$OUTPUTS_KDL")/.outputs.XXXXXX")"
    trap 'rm -f -- "${tmp_file:-}"' EXIT

    upsert_output_block "$OUTPUTS_KDL" "$id" "$connector" \
        "mode \"$(kdl_escape "$mode")\"" "scale $scale" >"$tmp_file"
    if ! niri validate -c "$tmp_file" >/dev/null 2>&1; then
        notify "Could not save default refresh rate" "Edited config did not validate: $OUTPUTS_KDL"
        exit 1
    fi

    mv -f -- "$tmp_file" "$OUTPUTS_KDL"
}

main() {
    local outputs_json
    local monitor_options
    local output_name
    local mode_options
    local mode
    local mode_width
    local mode_height
    local mode_rest
    local scale
    local output_id

    missing_commands

    outputs_json="$(get_outputs_json)"
    monitor_options="$(build_monitor_options "$outputs_json")"
    if [ -z "$monitor_options" ]; then
        notify "No monitors found" "Niri did not report any connected outputs."
        exit 1
    fi

    output_name="$(choose_line "Monitor" "$monitor_options")"

    mode_options="$(build_mode_options "$outputs_json" "$output_name")"
    if [ -z "$mode_options" ]; then
        notify "No refresh rates found" "$output_name does not report selectable modes."
        exit 1
    fi

    mode="$(choose_line "$output_name refresh" "$mode_options")"
    mode_width="${mode%%x*}"
    mode_rest="${mode#*x}"
    mode_height="${mode_rest%%@*}"
    scale="$(scale_for_resolution "$mode_width" "$mode_height")"
    output_id="$(jq -r --arg output "$output_name" "$OUTPUT_ID_JQ"'.[$output] | output_id' <<<"$outputs_json")"

    exec 8>"$RUNTIME_DIR/hype-niri-outputs.lock"
    flock -w 5 8 || exit 0
    if ! check_output_block_format "$OUTPUTS_KDL" "$output_id" "$output_name"; then
        notify "Could not save default refresh rate" "Expand the matching output block to one setting per line: $OUTPUTS_KDL"
        exit 1
    fi
    apply_mode "$output_name" "$mode"
    apply_scale "$output_name" "$scale"
    save_output_mode "$output_id" "$output_name" "$mode" "$scale"
    notify "Monitor refresh saved" "$output_name is now $mode at ${scale}x scale"
}

main "$@"
