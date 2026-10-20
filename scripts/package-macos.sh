#!/usr/bin/env bash
# Build the universal macOS app bundle and archives.
set -euo pipefail

VERSION="${1:?usage: package-macos.sh <version>}"
DIST="dist"
APP="$DIST/Image2USB.app"

rustup target add x86_64-apple-darwin aarch64-apple-darwin

for target in x86_64-apple-darwin aarch64-apple-darwin; do
  cargo build --release --locked -p image2usb-cli -p image2usb-gui --target "$target"
done

mkdir -p "$APP/Contents/MacOS" "$APP/Contents/Resources"
lipo -create \
  "target/x86_64-apple-darwin/release/image2usb-gui" \
  "target/aarch64-apple-darwin/release/image2usb-gui" \
  -output "$APP/Contents/MacOS/Image2USB"
lipo -create \
  "target/x86_64-apple-darwin/release/image2usb" \
  "target/aarch64-apple-darwin/release/image2usb" \
  -output "$APP/Contents/MacOS/image2usb"

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

rm -f "$DIST"/Image2USB-"$VERSION"-macos-universal.{zip,dmg}
ditto -c -k --keepParent "$APP" "$DIST/Image2USB-$VERSION-macos-universal.zip"
hdiutil create -volname "Image2USB" -srcfolder "$APP" -ov -format UDZO \
  "$DIST/Image2USB-$VERSION-macos-universal.dmg"

if [[ -n "${APPLE_SIGNING_IDENTITY:-}" ]]; then
  codesign --force --sign "$APPLE_SIGNING_IDENTITY" \
    "$DIST/Image2USB-$VERSION-macos-universal.dmg"
  if [[ -n "${APPLE_ID:-}" && -n "${APPLE_TEAM_ID:-}" && -n "${APPLE_APP_PASSWORD:-}" ]]; then
    xcrun notarytool submit "$DIST/Image2USB-$VERSION-macos-universal.dmg" \
      --apple-id "$APPLE_ID" --team-id "$APPLE_TEAM_ID" \
      --password "$APPLE_APP_PASSWORD" --wait
    xcrun stapler staple "$DIST/Image2USB-$VERSION-macos-universal.dmg"
  fi
fi
