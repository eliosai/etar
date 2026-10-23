#!/usr/bin/env bash
# Build the pinned erofs-utils into target/tools/erofs-<version> with zstd and built-in deflate
set -euo pipefail

version=1.9.4
sha256=7d135aa2550326a5acf20f53c518aea5a8900015ce50700044e40f818c31dd80
root="$(cd "$(dirname "$0")/.." && pwd)/target/tools"
prefix="$root/erofs-$version"

if "$prefix/bin/mkfs.erofs" -V 2>/dev/null | head -1 | grep -q "$version"; then
  exit 0
fi
mkdir -p "$root"
tarball="$root/erofs-utils-v$version.tar.gz"
if [ ! -f "$tarball" ]; then
  curl -fsSL -o "$tarball" \
    "https://github.com/erofs/erofs-utils/archive/refs/tags/v$version.tar.gz"
fi
echo "$sha256  $tarball" | sha256sum -c -
rm -rf "$root/erofs-utils-$version"
tar -xzf "$tarball" -C "$root"
cd "$root/erofs-utils-$version"
./autogen.sh
./configure --prefix="$prefix" --disable-lz4 --disable-lzma --without-zlib --without-libdeflate \
  --with-libzstd --without-qpl --without-xxhash --without-libcurl --without-openssl \
  --without-libxml2 --without-json-c --without-libnl3 --without-uuid --without-selinux \
  --disable-fuse
make -j"$(nproc)"
make install
