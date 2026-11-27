#!/usr/bin/env bash
# One version names the crate: bumps it, judges a release pull request and plans what main publishes
# Usage:
#   scripts/release.sh bump <X.Y.Z>     write the version into the manifest and the lockfile
#   scripts/release.sh check <branch>   a release pull request names its version and breaks no API that version may not break
#   scripts/release.sh plan             version=<X.Y.Z> when HEAD carries a version no tag names, else version=
set -euo pipefail

root="$(git rev-parse --show-toplevel)"
readonly root

# the version the workspace carries
current() {
    sed -nE '/^\[workspace\.package\]/,/^\[/ s/^version = "([^"]+)"/\1/p' "$root/Cargo.toml"
}

# the newest release tag, or nothing before the first release
previous() {
    git -C "$root" tag --list 'v[0-9]*' --sort=-version:refname | head -n 1
}

# fails unless the version is X.Y.Z and above every release
fresh() {
    local version="$1" last newest
    [[ "$version" =~ ^[0-9]+\.[0-9]+\.[0-9]+$ ]] || { echo "'$version' is not X.Y.Z" >&2; exit 2; }
    last="$(previous)"
    [ -n "$last" ] || return 0
    newest="$(printf '%s\n' "${last#v}" "$version" | sort -V | tail -n 1)"
    [ "$newest" = "$version" ] && [ "v$version" != "$last" ] || { echo "$version does not follow $last" >&2; exit 1; }
}

bump() {
    local version="$1" old
    fresh "$version"
    old="$(current)"
    old="${old//./\\.}"
    sed -i -E "/^\[workspace\.package\]/,/^\[/ s/^version = \"$old\"/version = \"$version\"/" "$root/Cargo.toml"
    sed -i -E "s/^(etar = \{ path = \"\.\", version = )\"$old\"/\1\"$version\"/" "$root/Cargo.toml"
    # only the workspace's own entries move, so no foreign crate re-resolves
    cargo update --manifest-path "$root/Cargo.toml" --workspace --offline
}

check() {
    local branch="$1" version last
    version="$(current)"
    [ "$branch" = "release/v$version" ] || { echo "$branch releases $version: name it release/v$version" >&2; exit 1; }
    fresh "$version"
    last="$(previous)"
    [ -n "$last" ] || { echo "v$version is the first release: no API to compare"; return 0; }
    cargo semver-checks --manifest-path "$root/Cargo.toml" -p etar --baseline-rev "$last"
}

plan() {
    local version
    version="$(current)"
    if git -C "$root" rev-parse -q --verify "refs/tags/v$version" >/dev/null; then
        echo "version="
    else
        fresh "$version"
        echo "version=$version"
    fi
}

case "${1:-}" in
    bump) [ $# -eq 2 ] || exit 2; bump "$2" ;;
    check) [ $# -eq 2 ] || exit 2; check "$2" ;;
    plan) plan ;;
    *) sed -n '2,6p' "${BASH_SOURCE[0]}" >&2; exit 2 ;;
esac
