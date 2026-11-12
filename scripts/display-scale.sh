#!/usr/bin/env bash
set -euo pipefail

source "${BASH_SOURCE[0]%/*}/runtime-dir.sh"
exec 9>"$RUNTIME_DIR/hype-niri-outputs.lock"
flock -w 5 9 || exit 0

source "${BASH_SOURCE[0]%/*}/niri-outputs.sh"
reload=1
[ "${1:-}" = "--no-reload" ] && reload=0

command -v niri >/dev/null 2>&1 || exit 0
command -v jq >/dev/null 2>&1 || exit 0

mkdir -p "$(dirname "$OUTPUTS_KDL")"
[ -e "$OUTPUTS_KDL" ] || : >"$OUTPUTS_KDL"

scale_for_resolution() {
    local width="$1" height="$2" short

    [[ "$width" =~ ^[0-9]+$ ]] && [[ "$height" =~ ^[0-9]+$ ]] || {
        printf '1.0'
        return
    }
    if [ "$width" -lt "$height" ]; then short="$width"; else short="$height"; fi

    if [ "$short" -ge 1800 ]; then
        printf '2.0'
    elif [ "$short" -ge 1600 ]; then
        printf '1.5'
    else
        printf '1.0'
    fi
}

outputs_json="$(niri msg --json outputs 2>/dev/null)" || exit 0

tmp="$(mktemp "$(dirname "$OUTPUTS_KDL")/.outputs.XXXXXX")"
next="$tmp.next"
trap 'rm -f -- "$tmp" "$next"' EXIT
cp -- "$OUTPUTS_KDL" "$tmp"

while IFS=$'\t' read -r id connector width height; do
    [ -n "$id" ] || continue
    check_output_block_format "$tmp" "$id" "$connector"
    if ! has_output_block "$tmp" "$id" "$connector"; then
        upsert_output_block "$tmp" "$id" "$connector" "scale $(scale_for_resolution "$width" "$height")" >"$next"
    elif [ "$id" != "$connector" ] && has_output_block "$tmp" "$connector" "$connector"; then
        upsert_output_block "$tmp" "$id" "$connector" >"$next"
    else
        continue
    fi
    mv -f -- "$next" "$tmp"
done < <(
    jq -r "$OUTPUT_ID_JQ"'
        to_entries[]
        | .value as $o
        | (if $o.current_mode == null then null else ($o.modes[$o.current_mode] // null) end) as $m
        | select($m != null)
        | [($o | output_id), $o.name, ($m.width | tostring), ($m.height | tostring)]
        | @tsv
    ' <<<"$outputs_json" 2>/dev/null | sort
)

if ! cmp -s "$tmp" "$OUTPUTS_KDL" && niri validate -c "$tmp" >/dev/null 2>&1; then
    mv -f -- "$tmp" "$OUTPUTS_KDL"
    [ "$reload" = 1 ] && niri msg action load-config-file >/dev/null 2>&1 || true
fi
