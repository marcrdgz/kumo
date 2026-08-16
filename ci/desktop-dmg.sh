#!/bin/bash
# Package the kumo-desktop binary into a native macOS disk image.
#
# Usage: ci/desktop-dmg.sh <path-to-kumo-desktop-binary> <version>
#
# Produces dist/Kumo-<arch>.dmg (arm64 or x86_64 from `uname -m`). The desktop
# app's in-app updater looks for this exact asset name on the GitHub release to
# self-update, so keep the naming scheme stable.

set -euo pipefail

BIN="${1:?usage: desktop-dmg.sh <binary> <version>}"
VERSION="${2:?usage: desktop-dmg.sh <binary> <version>}"

case "$(uname -m)" in
  arm64) ARCH="arm64" ;;
  x86_64) ARCH="x86_64" ;;
  *) echo "unsupported architecture: $(uname -m)" >&2; exit 1 ;;
esac

WORK="$(mktemp -d)"
trap 'rm -rf "$WORK"' EXIT

APP="$WORK/Kumo.app"
mkdir -p "$APP/Contents/MacOS" "$APP/Contents/Resources"

cp "$BIN" "$APP/Contents/MacOS/kumo-desktop"
chmod 755 "$APP/Contents/MacOS/kumo-desktop"

cat > "$APP/Contents/Info.plist" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>CFBundleName</key>
  <string>Kumo</string>
  <key>CFBundleDisplayName</key>
  <string>Kumo</string>
  <key>CFBundleIdentifier</key>
  <string>es.rdgz.kumo</string>
  <key>CFBundleVersion</key>
  <string>${VERSION}</string>
  <key>CFBundleShortVersionString</key>
  <string>${VERSION}</string>
  <key>CFBundleExecutable</key>
  <string>kumo-desktop</string>
  <key>CFBundlePackageType</key>
  <string>APPL</string>
  <key>LSMinimumSystemVersion</key>
  <string>12.0</string>
  <key>NSHighResolutionCapable</key>
  <true/>
  <key>LSUIElement</key>
  <false/>
</dict>
</plist>
PLIST

mkdir -p dist
hdiutil create \
  -volname Kumo \
  -srcfolder "$APP" \
  -ov \
  -format UDZO \
  "dist/Kumo-${ARCH}.dmg"

echo "built dist/Kumo-${ARCH}.dmg"
