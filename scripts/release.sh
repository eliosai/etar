#!/usr/bin/env bash
set -euo pipefail

cd "$(dirname "$0")/.."

preview=false
case "${1:-}" in
    --dry-run) preview=true ;;
    '') ;;
    *) echo 'usage: release.sh [--dry-run]' >&2; exit 2 ;;
esac

current="$(cargo metadata --offline --locked --no-deps --format-version 1 |
    python3 -c 'import json, sys; print(next(p["version"] for p in json.load(sys.stdin)["packages"] if p["name"] == "etar"))')"
last_tag="$(git describe --tags --match 'v[0-9]*' --abbrev=0 2>/dev/null || true)"

version_ge() {
    local a_major a_minor a_patch b_major b_minor b_patch
    IFS=. read -r a_major a_minor a_patch <<<"$1"
    IFS=. read -r b_major b_minor b_patch <<<"$2"
    (( a_major > b_major ||
       (a_major == b_major && a_minor > b_minor) ||
       (a_major == b_major && a_minor == b_minor && a_patch >= b_patch) ))
}

next_version() {
    local major minor patch
    IFS=. read -r major minor patch <<<"$1"
    case "$2" in
        major)
            if (( major == 0 )); then
                echo "0.$((minor + 1)).0"
            else
                echo "$((major + 1)).0.0"
            fi
            ;;
        minor) echo "$major.$((minor + 1)).0" ;;
        patch) echo "$major.$minor.$((patch + 1))" ;;
    esac
}

release_kind() {
    local messages
    messages="$(git log --format='%s%n%b' "$last_tag..HEAD")"
    if rg -q 'BREAKING CHANGE:|^[a-z][a-z0-9-]*(\([^)]*\))?!:' <<<"$messages"; then
        echo major
    elif ! bash scripts/semver-check.sh "$last_tag" minor >/dev/null; then
        bash scripts/semver-check.sh "$last_tag" major >/dev/null
        echo major
    elif rg -q '^feat(\([^)]*\))?:' <<<"$messages"; then
        echo minor
    elif rg -q '^(fix|perf|refactor)(\([^)]*\))?:' <<<"$messages"; then
        echo patch
    else
        echo none
    fi
}

published() {
    local records
    records="$(curl -fsS --retry 2 https://index.crates.io/et/ar/etar)" || return
    rg -q '"vers":"'"$1"'"' <<<"$records"
}

retry=false
if [[ -z "$last_tag" ]]; then
    next="$current"
    reason='first release from Cargo.toml'
else
    tagged="${last_tag#v}"
    kind="$(release_kind)"
    if [[ "$current" != "$tagged" ]]; then
        if ! version_ge "$current" "$tagged"; then
            echo "Cargo.toml version $current is older than $last_tag" >&2
            exit 1
        fi
        if [[ "$kind" != none ]] && ! version_ge "$current" "$(next_version "$tagged" "$kind")"; then
            echo "Cargo.toml version $current is too small for a $kind release" >&2
            exit 1
        fi
        next="$current"
        reason='version set in Cargo.toml'
    elif [[ "$kind" != none ]]; then
        next="$(next_version "$tagged" "$kind")"
        reason="$kind changes since $last_tag"
    elif $preview; then
        echo "no conventional release changes since $last_tag"
        exit 0
    elif [[ "$(git rev-parse HEAD)" == "$(git rev-parse "$last_tag")" ]]; then
        next="$current"
        reason="retry $last_tag"
        retry=true
    elif published "$current"; then
        echo "$last_tag is already published"
        exit 0
    else
        echo "unpublished $last_tag has later commits; resolve the release before continuing" >&2
        exit 1
    fi
fi

echo "release v$next ($reason)"
if $preview; then
    exit 0
fi
if [[ "${GITHUB_ACTIONS:-}" != true || "${GITHUB_REPOSITORY:-}" != eliosai/etar || "${GITHUB_REF:-}" != refs/heads/main ]]; then
    echo 'release writes run only in the etar main GitHub Actions workflow' >&2
    exit 1
fi
if rg -q '^publish = false$' Cargo.toml; then
    echo 'Cargo.toml still disables publishing' >&2
    exit 1
fi
if [[ -n "$(git status --porcelain)" ]]; then
    echo 'release checkout must be clean' >&2
    exit 1
fi
: "${CARGO_REGISTRY_TOKEN:?crates.io token is required}"
: "${GH_TOKEN:?GitHub release token is required}"
if [[ "$(git ls-remote origin refs/heads/main | cut -f1)" != "$(git rev-parse HEAD)" ]]; then
    echo 'main moved after the release workflow started' >&2
    exit 1
fi

if ! $retry; then
    if [[ "$next" != "$current" ]]; then
        sed -i "s/^version = \"$current\"$/version = \"$next\"/" Cargo.toml
        sed -i "s/^etar = { path = \"\.\", version = \"$current\" }$/etar = { path = \".\", version = \"$next\" }/" Cargo.toml
        cargo update --workspace
    fi
    notes="$(mktemp)"
    range=()
    if [[ -n "$last_tag" ]]; then
        range=("$last_tag..HEAD")
    fi
    git-cliff --tag "v$next" --unreleased --strip all "${range[@]}" >"$notes"
    if [[ -f CHANGELOG.md ]]; then
        git-cliff --tag "v$next" --unreleased --prepend CHANGELOG.md "${range[@]}"
    else
        git-cliff --tag "v$next" --unreleased --output CHANGELOG.md "${range[@]}"
    fi
    cargo package -p etar --locked --allow-dirty
    git config user.name 'github-actions[bot]'
    git config user.email '41898282+github-actions[bot]@users.noreply.github.com'
    git add Cargo.toml Cargo.lock CHANGELOG.md
    if ! git diff --cached --quiet; then
        git commit -m "chore(release): v$next"
    fi
    git tag -a "v$next" -m "v$next"
    git push --atomic origin HEAD:refs/heads/main "refs/tags/v$next:refs/tags/v$next"
fi

if ! published "$next"; then
    cargo publish -p etar --locked
fi
if ! gh release view "v$next" >/dev/null 2>&1; then
    if $retry; then
        gh release create "v$next" --title "v$next" --generate-notes
    else
        gh release create "v$next" --title "v$next" --notes-file "$notes"
    fi
fi
