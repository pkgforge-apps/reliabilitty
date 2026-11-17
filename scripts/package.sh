#!/bin/sh
# Packages the static Linux build into a single tarball.
# Usage: scripts/package.sh <rust-target> <platform>
set -eu
cd "$(dirname "$0")/.."

target=$1
platform=$2
app=reliabilitty
bin=target/$target/release/rtty
name=$app-$platform
stage=dist/$name

rm -rf "$stage" "dist/$name.tar.xz"
mkdir -p "$stage"
cp LICENSE README.md "$bin" "$stage/"
mkdir -p "$stage/share/applications"
cp packaging/linux/app.desktop "$stage/share/applications/$app.desktop"
mkdir -p "$stage/share/icons/hicolor/scalable/apps"
cp assets/icon.svg "$stage/share/icons/hicolor/scalable/apps/$app.svg"
for size in 16 24 32 48 64 128 256 512; do
    dir=$stage/share/icons/hicolor/${size}x$size/apps
    mkdir -p "$dir"
    cp "assets/icons/$size.png" "$dir/$app.png"
done
tar -C dist -cJf "dist/$name.tar.xz" "$name"
