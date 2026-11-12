#!/usr/bin/env bash
# shellcheck disable=SC2034 # Sourced: the scripts that load this file use its variables.
# Shared helpers for per-output settings in niri's outputs.kdl.
# niri applies the first matching output block, so each output keeps exactly one block there.

OUTPUTS_KDL="${XDG_CONFIG_HOME:-$HOME/.config}/niri/outputs.kdl"

# jq definition of niri's stable make/model/serial name, or the connector when the display
# reports none. Prepend it to a jq filter and call output_id on an output object.
# shellcheck disable=SC2016
OUTPUT_ID_JQ='def output_id:
    if (.make == "Unknown" and .model == "Unknown" and .serial == null) then .name
    else "\(.make) \(.model) \(.serial // "Unknown")" end;'

kdl_escape() {
    local value="$1"

    value="${value//\\/\\\\}"
    value="${value//\"/\\\"}"
    printf '%s' "$value"
}

# Succeeds when FILE has a block for the output named ID or CONNECTOR.
has_output_block() {
    local file="$1" id="$2" connector="$3"

    [ -f "$file" ] || return 1
    awk -v id="$(kdl_escape "$id")" -v connector="$(kdl_escape "$connector")" '
        /^[[:space:]]*output[[:space:]]+"/ {
            name = $0
            sub(/^[[:space:]]*output[[:space:]]+"/, "", name)
            sub(/"[[:space:]]*\{[[:space:]]*$/, "", name)
            if (name == id || name == connector) { found = 1; exit }
        }
        END { exit !found }
    ' "$file"
}

# Print FILE with the output's blocks (by ID or legacy CONNECTOR name) merged into one block
# named ID. Each SETTING line replaces the existing setting with the same leading keyword.
upsert_output_block() {
    local file="$1" id="$2" connector="$3"
    shift 3

    awk -v id="$(kdl_escape "$id")" -v connector="$(kdl_escape "$connector")" \
        -v settings="$(printf '%s\n' "$@")" '
        BEGIN {
            count = split(settings, setting, "\n")
            for (i = 1; i <= count; i++) {
                if (setting[i] == "") continue
                split(setting[i], words, /[[:space:]]+/)
                replaced[words[1]] = 1
            }
        }

        !depth && /^[[:space:]]*output[[:space:]]+"/ {
            name = $0
            sub(/^[[:space:]]*output[[:space:]]+"/, "", name)
            sub(/"[[:space:]]*\{[[:space:]]*$/, "", name)
            if (name == id || name == connector) {
                depth = 1
                next
            }
        }

        depth {
            line = $0
            opens = gsub(/\{/, "{", line)
            closes = gsub(/\}/, "}", line)
            key = $1
            if (depth == 1 && closes > opens && key == "}") {
                depth = 0
                next
            }
            if (depth == 1 && !opens && (key in replaced || key in seen)) {
                depth += opens - closes
                next
            }
            if (depth == 1 && !opens) seen[key] = 1
            kept[++kept_count] = $0
            depth += opens - closes
            next
        }

        { print }

        END {
            print "output \"" id "\" {"
            for (i = 1; i <= count; i++) {
                if (setting[i] != "") print "    " setting[i]
            }
            for (i = 1; i <= kept_count; i++) print kept[i]
            print "}"
        }
    ' "$file"
}
