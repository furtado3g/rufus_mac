#!/usr/bin/env bash
# Build the universal macOS app bundle and archives.
#
# Usage:
#   package-macos.sh <version>              universal app, no native NTFS path
#   package-macos.sh <version> --native     host-only app
#
# Set IMAGE2USB_WINDOWS_MEDIA_PREFIX to the absolute install prefix produced by
# scripts/build-windows-media.sh to include the native NTFS + UEFI:NTFS path.
# That path is native-only; universal builds need per-architecture native
# libraries and are intentionally refused for now.
set -euo pipefail

VERSION="${1:?usage: package-macos.sh <version> [--native]}"
MODE="${2:-}"
if [[ -n "$MODE" && "$MODE" != "--native" ]]; then
  echo "unknown option: $MODE" >&2
  exit 2
fi

NATIVE=0
if [[ "$MODE" == "--native" ]]; then
  NATIVE=1
fi

FEATURES=""
if [[ -n "${IMAGE2USB_WINDOWS_MEDIA_PREFIX:-}" ]]; then
  if [[ "$NATIVE" -eq 0 ]]; then
    echo "IMAGE2USB_WINDOWS_MEDIA_PREFIX requires --native until native libraries are available for every architecture" >&2
    exit 2
  fi
  if [[ ! -d "$IMAGE2USB_WINDOWS_MEDIA_PREFIX" ]]; then
    echo "native prefix does not exist: $IMAGE2USB_WINDOWS_MEDIA_PREFIX" >&2
    exit 2
  fi
  FEATURES="image2usb-cli/windows-media,image2usb-gui/windows-media"
fi

HOST_TARGET="$(rustc -vV | sed -n 's/^host: //p')"
if [[ "$NATIVE" -eq 1 ]]; then
  TARGETS=("$HOST_TARGET")
  ARTIFACT_SUFFIX="macos-$(uname -m)"
  if [[ -n "$FEATURES" ]]; then
    ARTIFACT_SUFFIX="$ARTIFACT_SUFFIX-ntfs"
  fi
else
  TARGETS=(x86_64-apple-darwin aarch64-apple-darwin)
  ARTIFACT_SUFFIX="macos-universal"
  rustup target add "${TARGETS[@]}"
fi

DIST="dist"
APP="$DIST/Image2USB.app"

for target in "${TARGETS[@]}"; do
  if [[ -n "$FEATURES" ]]; then
    cargo build --release --locked -p image2usb-cli -p image2usb-gui --target "$target" \
      --features "$FEATURES"
  else
    cargo build --release --locked -p image2usb-cli -p image2usb-gui --target "$target"
  fi
done

mkdir -p "$APP/Contents/MacOS" "$APP/Contents/Resources"
if [[ "${#TARGETS[@]}" -eq 1 ]]; then
  cp "target/${TARGETS[0]}/release/image2usb-gui" "$APP/Contents/MacOS/Image2USB"
  cp "target/${TARGETS[0]}/release/image2usb" "$APP/Contents/MacOS/image2usb-cli"
else
  lipo -create \
    "target/x86_64-apple-darwin/release/image2usb-gui" \
    "target/aarch64-apple-darwin/release/image2usb-gui" \
    -output "$APP/Contents/MacOS/Image2USB"
  lipo -create \
    "target/x86_64-apple-darwin/release/image2usb" \
    "target/aarch64-apple-darwin/release/image2usb" \
    -output "$APP/Contents/MacOS/image2usb-cli"
fi

cat > "$APP/Contents/Info.plist" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>CFBundleName</key><string>Image2USB</string>
  <key>CFBundleDisplayName</key><string>image2usb</string>
  <key>CFBundleIdentifier</key><string>io.image2usb.app</string>
  <key>CFBundleVersion</key><string>${VERSION}</string>
  <key>CFBundleShortVersionString</key><string>${VERSION}</string>
  <key>CFBundlePackageType</key><string>APPL</string>
  <key>CFBundleExecutable</key><string>Image2USB</string>
  <key>LSMinimumSystemVersion</key><string>12.0</string>
  <key>NSHighResolutionCapable</key><true/>
  <key>NSHumanReadableCopyright</key><string>GPL-3.0-or-later</string>
</dict>
</plist>
PLIST

cp LICENSE THIRD_PARTY_NOTICES.md "$APP/Contents/Resources/"

# Signing and notarization are configured through CI secrets. Without them the
# bundle gets an ad-hoc signature: arm64 macOS refuses to run unsigned code, and
# `lipo` invalidates the per-architecture ad-hoc signatures anyway.
if [[ -n "${APPLE_SIGNING_IDENTITY:-}" ]]; then
  codesign --force --deep --options runtime --timestamp \
    --sign "$APPLE_SIGNING_IDENTITY" "$APP"
else
  codesign --force --deep --sign - "$APP"
fi

rm -f "$DIST"/Image2USB-"$VERSION"-"$ARTIFACT_SUFFIX".{zip,dmg}
ditto -c -k --keepParent "$APP" "$DIST/Image2USB-$VERSION-$ARTIFACT_SUFFIX.zip"
hdiutil create -volname "Image2USB" -srcfolder "$APP" -ov -format UDZO \
  "$DIST/Image2USB-$VERSION-$ARTIFACT_SUFFIX.dmg"

if [[ -n "${APPLE_SIGNING_IDENTITY:-}" ]]; then
  codesign --force --sign "$APPLE_SIGNING_IDENTITY" \
    "$DIST/Image2USB-$VERSION-$ARTIFACT_SUFFIX.dmg"
  if [[ -n "${APPLE_ID:-}" && -n "${APPLE_TEAM_ID:-}" && -n "${APPLE_APP_PASSWORD:-}" ]]; then
    xcrun notarytool submit "$DIST/Image2USB-$VERSION-$ARTIFACT_SUFFIX.dmg" \
      --apple-id "$APPLE_ID" --team-id "$APPLE_TEAM_ID" \
      --password "$APPLE_APP_PASSWORD" --wait
    xcrun stapler staple "$DIST/Image2USB-$VERSION-$ARTIFACT_SUFFIX.dmg"
  fi
fi
