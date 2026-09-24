#!/usr/bin/env bash
set -euo pipefail

cd "$(dirname "$0")/.."
source_root="$PWD"
scratch="$(mktemp -d)"
trap 'rm -r -- "$scratch"' EXIT

cargo init --quiet --lib --name etar --vcs none "$scratch/repo"
cargo generate-lockfile --quiet --offline --manifest-path "$scratch/repo/Cargo.toml"
mkdir "$scratch/repo/scripts" "$scratch/bin"
cp "$source_root/scripts/release.sh" "$source_root/scripts/semver-check.sh" "$scratch/repo/scripts/"
printf '#!/bin/sh\nexit 23\n' > "$scratch/bin/cargo-semver-checks"
chmod +x "$scratch/bin/cargo-semver-checks"
git -C "$scratch/repo" init --quiet
git -C "$scratch/repo" -c user.name=test -c user.email=test@example.invalid \
    -c commit.gpgsign=false add .
git -C "$scratch/repo" -c user.name=test -c user.email=test@example.invalid \
    -c commit.gpgsign=false commit --quiet -m 'chore: baseline'
git -C "$scratch/repo" tag v0.1.0
git -C "$scratch/repo" -c user.name=test -c user.email=test@example.invalid \
    -c commit.gpgsign=false commit --quiet --allow-empty -m 'fix: exercise patch selection'

if output="$(PATH="$scratch/bin:$PATH" bash "$scratch/repo/scripts/release.sh" --dry-run 2>&1)"; then
    echo 'release planning accepted a failed semver check' >&2
    exit 1
fi
if rg -q '^release v' <<<"$output"; then
    echo 'release planning selected a version after a failed semver check' >&2
    exit 1
fi
if ! rg -q 'semver checks failed; refusing to select a release' <<<"$output"; then
    echo 'release planning failed without reporting the semver-tool error' >&2
    exit 1
fi

expect_plan() {
    local expected="$1" actual
    actual="$(PATH="$scratch/bin:$PATH" bash "$scratch/repo/scripts/release.sh" --dry-run)"
    if [[ "$actual" != "$expected" ]]; then
        printf 'expected: %s\nactual: %s\n' "$expected" "$actual" >&2
        exit 1
    fi
}

commit_from_tag() {
    git -C "$scratch/repo" switch --quiet --detach v0.1.0
    git -C "$scratch/repo" -c user.name=test -c user.email=test@example.invalid \
        -c commit.gpgsign=false commit --quiet --allow-empty -m "$1"
}

printf '#!/bin/sh\nexit 0\n' > "$scratch/bin/cargo-semver-checks"
expect_plan 'release v0.1.1 (patch changes since v0.1.0)'

commit_from_tag 'feat: exercise minor selection'
expect_plan 'release v0.2.0 (minor changes since v0.1.0)'

commit_from_tag 'chore: exercise no-release selection'
expect_plan 'no conventional release changes since v0.1.0'

commit_from_tag 'fix: exercise API-break selection'
printf '#!/bin/sh\ncase "$*" in *"--release-type minor"*) exit 1 ;; *) exit 0 ;; esac\n' \
    > "$scratch/bin/cargo-semver-checks"
expect_plan 'release v0.2.0 (major changes since v0.1.0)'
