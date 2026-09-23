#!/usr/bin/env bash
set -euo pipefail

baseline="${1:-}"
release_type="${2:-minor}"
if [[ "$release_type" != minor && "$release_type" != major ]]; then
    echo "release type must be minor or major" >&2
    exit 2
fi
if [[ -z "$baseline" ]]; then
    baseline="$(git describe --tags --match 'v[0-9]*' --abbrev=0 2>/dev/null || true)"
fi
if [[ -z "$baseline" ]]; then
    echo 'no etar baseline yet; first-release API review is manual'
    exit 0
fi
if ! git show "$baseline:Cargo.toml" 2>/dev/null | rg -q '^name = "etar"$'; then
    echo 'baseline predates the etar package; first-release API review is manual'
    exit 0
fi

cargo semver-checks -p etar --baseline-rev "$baseline" --release-type "$release_type"
