#!/usr/bin/env bash
set -euo pipefail

if [[ $# -lt 3 ]]; then
  echo "Usage: $0 <app-path> <version> <output-dir>"
  exit 1
fi

APP_PATH="$1"
VERSION="$2"
OUTPUT_DIR="$3"

if [[ ! -d "$APP_PATH" ]]; then
  echo "App bundle not found: $APP_PATH"
  exit 1
fi

mkdir -p "$OUTPUT_DIR"

APP_BASENAME="DictatorMac"
ZIP_PATH="$OUTPUT_DIR/${APP_BASENAME}-${VERSION}-macOS.zip"
DMG_PATH="$OUTPUT_DIR/${APP_BASENAME}-${VERSION}-macOS.dmg"

rm -f "$ZIP_PATH" "$DMG_PATH"

ditto -c -k --sequesterRsrc --keepParent "$APP_PATH" "$ZIP_PATH"

TMP_DMG_DIR="$(mktemp -d)"
cp -R "$APP_PATH" "$TMP_DMG_DIR/"
hdiutil create \
  -volname "Dictator" \
  -srcfolder "$TMP_DMG_DIR" \
  -ov \
  -format UDZO \
  "$DMG_PATH"
rm -rf "$TMP_DMG_DIR"

echo "ZIP_PATH=$ZIP_PATH"
echo "DMG_PATH=$DMG_PATH"
