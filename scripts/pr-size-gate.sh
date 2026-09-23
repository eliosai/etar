#!/usr/bin/env bash
set -euo pipefail

if [[ " ${LABELS:-} " == *" size-exempt "* ]]; then
    echo 'size-exempt label accepts this diff'
    exit 0
fi

limit=1000
if [[ " ${LABELS:-} " == *" mechanical "* ]]; then
    limit=3000
fi

added="$(git diff --numstat "${BASE_SHA:?}" "${HEAD_SHA:?}" -- | awk '$1 ~ /^[0-9]+$/ { sum += $1 } END { print sum + 0 }')"
printf 'inserted lines: %s; limit: %s\n' "$added" "$limit"
if (( added > limit )); then
    echo 'split the change or request a reviewed size exemption' >&2
    exit 1
fi
