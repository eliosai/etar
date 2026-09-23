#!/usr/bin/env bash
set -euo pipefail

cd "$(dirname "$0")/../.."

test "$(readlink CLAUDE.md)" = AGENTS.md
test "$(readlink .claude/skills)" = ../.agents/skills

for skill in .agents/skills/*/SKILL.md; do
    directory="$(basename "$(dirname "$skill")")"
    name="$(sed -n 's/^name: //p' "$skill" | head -n 1)"
    if [[ "$directory" != "$name" ]]; then
        echo "$skill declares $name, expected $directory" >&2
        exit 1
    fi
done
