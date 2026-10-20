#!/usr/bin/env bash
# Build the portable Linux archive for the current architecture.
#
# Release CI runs this on native x86_64 and aarch64 runners. Building both
# architectures on one host would require a full cross toolchain (and ARM64
# GTK/Wayland development libraries for the GUI), so the script deliberately
# builds only the host architecture.
set -euo pipefail

VERSION="${1:?usage: package-linux.sh <version>}"
DIST="dist"
HOST="$(rustc -vV | sed -n 's/^host: //p')"

case "$HOST" in
  x86_64-unknown-linux-gnu) ARCH="x86_64" ;;
  aarch64-unknown-linux-gnu) ARCH="aarch64" ;;
  *)
    echo "unsupported host: $HOST (expected x86_64 or aarch64 Linux)" >&2
    exit 1
    ;;
esac

mkdir -p "$DIST"
cargo build --release --locked -p image2usb-cli -p image2usb-gui

stage="$(mktemp -d)"
cp target/release/image2usb "$stage/"
cp target/release/image2usb-gui "$stage/"
cp LICENSE THIRD_PARTY_NOTICES.md README.md "$stage/"
tar -czf "$DIST/image2usb-$VERSION-linux-$ARCH.tar.gz" -C "$stage" .
rm -rf "$stage"

# AppImage packaging is added in phase 0.5. The tarball above is the portable
# artifact and runs from any directory.
