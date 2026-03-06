#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
APP_DIR="$ROOT_DIR/apps/macos"

echo "[macOS smoke] Building DictatorMac package..."
cd "$APP_DIR"
swift --version
echo "[macOS smoke] xcode-select: $(xcode-select -p)"
echo "[macOS smoke] sdk: $(xcrun --sdk macosx --show-sdk-path)"

# Avoid stale module cache collisions when the same project was built from another path.
rm -rf "$APP_DIR/.build"

if [[ "$(xcode-select -p)" == "/Library/Developer/CommandLineTools" ]]; then
  echo "[macOS smoke] warning: CommandLineTools selected. If build fails with SDK/toolchain mismatch, switch to full Xcode."
fi

swift package resolve
swift build

echo "[macOS smoke] OK"
