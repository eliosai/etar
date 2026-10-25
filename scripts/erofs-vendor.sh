#!/usr/bin/env bash
# Rebuild erofs/upstream from the pinned erofs-utils release and the patches in erofs/, keeping erofs/config.h
set -euo pipefail

version=1.9.4
sha256=7d135aa2550326a5acf20f53c518aea5a8900015ce50700044e40f818c31dd80
root="$(cd "$(dirname "$0")/.." && pwd)"
work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT

curl -fsSL -o "$work/source.tar.gz" \
  "https://github.com/erofs/erofs-utils/archive/refs/tags/v$version.tar.gz"
echo "$sha256  $work/source.tar.gz" | sha256sum -c --status -
tar -xzf "$work/source.tar.gz" -C "$work"
source="$work/erofs-utils-$version"
for patch in "$root"/erofs/*.patch; do
  patch --quiet --directory="$source" --strip=1 <"$patch"
done
rm -rf "$root/erofs/upstream"
mkdir -p "$root/erofs/upstream/mkfs" "$root/erofs/upstream/fsck"
cp -r "$source/COPYING" "$source/LICENSES" "$source/include" "$source/lib" "$root/erofs/upstream/"
cp "$source/mkfs/main.c" "$root/erofs/upstream/mkfs/"
cp "$source/fsck/main.c" "$root/erofs/upstream/fsck/"
find "$root/erofs/upstream/lib" -type f ! -name '*.c' ! -name '*.h' -delete
