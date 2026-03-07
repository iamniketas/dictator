#!/usr/bin/env bash
set -euo pipefail

if [[ $# -lt 6 ]]; then
  echo "Usage: $0 <build-products-dir> <app-name> <version> <public-ed-key> <feed-url> <bundle-id>"
  exit 1
fi

BUILD_PRODUCTS_DIR="$1"
APP_NAME="$2"
VERSION="$3"
PUBLIC_ED_KEY="$4"
FEED_URL="$5"
BUNDLE_ID="$6"

BIN_PATH="$BUILD_PRODUCTS_DIR/$APP_NAME"
APP_DIR="$BUILD_PRODUCTS_DIR/$APP_NAME.app"

if [[ ! -f "$BIN_PATH" ]]; then
  echo "Executable not found: $BIN_PATH"
  exit 1
fi

rm -rf "$APP_DIR"
mkdir -p "$APP_DIR/Contents/MacOS" "$APP_DIR/Contents/Frameworks" "$APP_DIR/Contents/Resources"

cp "$BIN_PATH" "$APP_DIR/Contents/MacOS/$APP_NAME"
chmod +x "$APP_DIR/Contents/MacOS/$APP_NAME"

cat > "$APP_DIR/Contents/Info.plist" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>CFBundleName</key>
  <string>$APP_NAME</string>
  <key>CFBundleDisplayName</key>
  <string>Dictator</string>
  <key>CFBundleIdentifier</key>
  <string>$BUNDLE_ID</string>
  <key>CFBundleExecutable</key>
  <string>$APP_NAME</string>
  <key>CFBundlePackageType</key>
  <string>APPL</string>
  <key>CFBundleShortVersionString</key>
  <string>$VERSION</string>
  <key>CFBundleVersion</key>
  <string>$VERSION</string>
  <key>LSMinimumSystemVersion</key>
  <string>14.0</string>
  <key>LSUIElement</key>
  <true/>
  <key>NSMicrophoneUsageDescription</key>
  <string>Dictator uses the microphone to record your voice for local transcription.</string>
  <key>SUPublicEDKey</key>
  <string>$PUBLIC_ED_KEY</string>
  <key>SUFeedURL</key>
  <string>$FEED_URL</string>
</dict>
</plist>
PLIST

for framework in "$BUILD_PRODUCTS_DIR"/*.framework "$BUILD_PRODUCTS_DIR"/PackageFrameworks/*.framework; do
  if [[ -d "$framework" ]]; then
    cp -R "$framework" "$APP_DIR/Contents/Frameworks/"
  fi
done

for bundle in "$BUILD_PRODUCTS_DIR"/*.bundle; do
  if [[ -d "$bundle" ]]; then
    cp -R "$bundle" "$APP_DIR/Contents/Resources/"
  fi
done

# Sparkle appcast generation validates Apple code signing.
# For CI release artifacts we use ad-hoc signing to satisfy bundle integrity checks.
codesign --force --deep --sign - "$APP_DIR"
codesign --verify --deep --strict "$APP_DIR"

echo "APP_DIR=$APP_DIR"
