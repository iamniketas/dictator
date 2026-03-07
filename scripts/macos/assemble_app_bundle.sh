#!/usr/bin/env bash
set -euo pipefail

if [[ $# -lt 6 ]]; then
  echo "Usage: $0 <build-products-dir> <executable-name> <version> <public-ed-key> <feed-url> <bundle-id>"
  exit 1
fi

BUILD_PRODUCTS_DIR="$1"
EXECUTABLE_NAME="$2"
VERSION="$3"
PUBLIC_ED_KEY="$4"
FEED_URL="$5"
BUNDLE_ID="$6"

APP_BUNDLE_NAME="${DICTATOR_BUNDLE_NAME:-Dictator}"
ICON_SOURCE_PNG="${DICTATOR_ICON_SOURCE_PNG:-}"
SIGN_IDENTITY="${DICTATOR_SIGN_IDENTITY:--}"

BIN_PATH="$BUILD_PRODUCTS_DIR/$EXECUTABLE_NAME"
APP_DIR="$BUILD_PRODUCTS_DIR/$APP_BUNDLE_NAME.app"

if [[ ! -f "$BIN_PATH" ]]; then
  echo "Executable not found: $BIN_PATH"
  exit 1
fi

rm -rf "$APP_DIR"
mkdir -p "$APP_DIR/Contents/MacOS" "$APP_DIR/Contents/Frameworks" "$APP_DIR/Contents/Resources"

cp "$BIN_PATH" "$APP_DIR/Contents/MacOS/$EXECUTABLE_NAME"
chmod +x "$APP_DIR/Contents/MacOS/$EXECUTABLE_NAME"

if [[ -n "$ICON_SOURCE_PNG" && -f "$ICON_SOURCE_PNG" ]]; then
  ICONSET_DIR="$(mktemp -d)"
  mkdir -p "$ICONSET_DIR/AppIcon.iconset"
  ICONSET_PATH="$ICONSET_DIR/AppIcon.iconset"
  sips -z 16 16 "$ICON_SOURCE_PNG" --out "$ICONSET_PATH/icon_16x16.png" >/dev/null
  sips -z 32 32 "$ICON_SOURCE_PNG" --out "$ICONSET_PATH/icon_16x16@2x.png" >/dev/null
  sips -z 32 32 "$ICON_SOURCE_PNG" --out "$ICONSET_PATH/icon_32x32.png" >/dev/null
  sips -z 64 64 "$ICON_SOURCE_PNG" --out "$ICONSET_PATH/icon_32x32@2x.png" >/dev/null
  sips -z 128 128 "$ICON_SOURCE_PNG" --out "$ICONSET_PATH/icon_128x128.png" >/dev/null
  sips -z 256 256 "$ICON_SOURCE_PNG" --out "$ICONSET_PATH/icon_128x128@2x.png" >/dev/null
  sips -z 256 256 "$ICON_SOURCE_PNG" --out "$ICONSET_PATH/icon_256x256.png" >/dev/null
  sips -z 512 512 "$ICON_SOURCE_PNG" --out "$ICONSET_PATH/icon_256x256@2x.png" >/dev/null
  sips -z 512 512 "$ICON_SOURCE_PNG" --out "$ICONSET_PATH/icon_512x512.png" >/dev/null
  sips -z 1024 1024 "$ICON_SOURCE_PNG" --out "$ICONSET_PATH/icon_512x512@2x.png" >/dev/null
  iconutil -c icns "$ICONSET_PATH" -o "$APP_DIR/Contents/Resources/Dictator.icns"
  rm -rf "$ICONSET_DIR"
fi

cat > "$APP_DIR/Contents/Info.plist" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>CFBundleName</key>
  <string>$APP_BUNDLE_NAME</string>
  <key>CFBundleDisplayName</key>
  <string>$APP_BUNDLE_NAME</string>
  <key>CFBundleIdentifier</key>
  <string>$BUNDLE_ID</string>
  <key>CFBundleExecutable</key>
  <string>$EXECUTABLE_NAME</string>
  <key>CFBundlePackageType</key>
  <string>APPL</string>
  <key>CFBundleIconFile</key>
  <string>Dictator</string>
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

# Sign nested code first, then sign app bundle.
if [[ "$SIGN_IDENTITY" == "-" ]]; then
  SIGN_ARGS=(--force --sign -)
else
  SIGN_ARGS=(--force --sign "$SIGN_IDENTITY" --timestamp --options runtime)
fi

if [[ -d "$APP_DIR/Contents/Frameworks" ]]; then
  while IFS= read -r -d '' framework; do
    codesign "${SIGN_ARGS[@]}" "$framework"
  done < <(find "$APP_DIR/Contents/Frameworks" -type d -name "*.framework" -print0)
fi

while IFS= read -r -d '' bundle; do
  codesign "${SIGN_ARGS[@]}" "$bundle"
done < <(find "$APP_DIR/Contents/Resources" -type d -name "*.bundle" -print0)

codesign "${SIGN_ARGS[@]}" "$APP_DIR"
codesign --verify --deep --strict "$APP_DIR"
spctl --assess --type execute --verbose=4 "$APP_DIR" || true

echo "APP_DIR=$APP_DIR"
