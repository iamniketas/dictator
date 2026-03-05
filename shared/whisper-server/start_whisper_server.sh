#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
cd "$SCRIPT_DIR"

WHISPER_PORT="${WHISPER_PORT:-5500}"
export WHISPER_PORT

echo "Starting Whisper server on port ${WHISPER_PORT}..."
echo "Model source: default repo path (models/faster-whisper-large-v2) unless WHISPER_MODEL_PATH/WHISPER_MODEL/CLI arg is set."

python3 whisper_server.py "$@"
