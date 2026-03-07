#!/usr/bin/env bash
set -euo pipefail

if [[ $# -lt 4 ]]; then
  echo "Usage: $0 <sparkle-dir> <updates-dir> <private-key-file> <download-url-prefix>"
  exit 1
fi

SPARKLE_DIR="$1"
UPDATES_DIR="$2"
PRIVATE_KEY_FILE="$3"
DOWNLOAD_URL_PREFIX="$4"

if [[ ! -d "$SPARKLE_DIR" ]]; then
  echo "Sparkle directory not found: $SPARKLE_DIR"
  exit 1
fi

if [[ ! -d "$UPDATES_DIR" ]]; then
  echo "Updates directory not found: $UPDATES_DIR"
  exit 1
fi

if [[ ! -f "$PRIVATE_KEY_FILE" ]]; then
  echo "Private key file not found: $PRIVATE_KEY_FILE"
  exit 1
fi

DERIVED_DATA="$(mktemp -d)"

xcodebuild \
  -project "$SPARKLE_DIR/Sparkle.xcodeproj" \
  -scheme generate_appcast \
  -configuration Release \
  -derivedDataPath "$DERIVED_DATA" \
  build >/dev/null

GENERATE_APPCAST_BIN="$DERIVED_DATA/Build/Products/Release/generate_appcast"
if [[ ! -x "$GENERATE_APPCAST_BIN" ]]; then
  echo "generate_appcast binary not found after build"
  exit 1
fi

"$GENERATE_APPCAST_BIN" \
  --ed-key-file "$PRIVATE_KEY_FILE" \
  --download-url-prefix "$DOWNLOAD_URL_PREFIX" \
  --maximum-deltas 0 \
  "$UPDATES_DIR"

APPCAST_PATH="$UPDATES_DIR/appcast.xml"
if [[ ! -f "$APPCAST_PATH" ]]; then
  echo "appcast.xml was not generated"
  exit 1
fi

echo "APPCAST_PATH=$APPCAST_PATH"
