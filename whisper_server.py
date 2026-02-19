#!/usr/bin/env python3
"""
Whisper HTTP Server - Fast transcription service with model caching
Loads faster-whisper model once at startup and keeps it in memory (GPU).
"""
import os
import sys
import tempfile
from pathlib import Path
from flask import Flask, request, jsonify
from faster_whisper import WhisperModel
import logging

# Configure logging
logging.basicConfig(level=logging.INFO)
logger = logging.getLogger(__name__)

app = Flask(__name__)
app.config["JSON_AS_ASCII"] = False
try:
    app.json.ensure_ascii = False  # Flask 2.3+/3.x JSON provider
except Exception:
    pass

# Global model instance (loaded once)
model = None
model_info = {}
SCRIPT_DIR = Path(__file__).resolve().parent
DEFAULT_LOCAL_MODEL_DIR = SCRIPT_DIR / "models" / "faster-whisper-large-v2"

def load_model(model_path, preferred_device=None, preferred_compute_type=None):
    """Load Whisper model into memory"""
    global model, model_info

    # Prefer explicit values from env, otherwise use robust defaults.
    device_candidates = [preferred_device] if preferred_device else ["auto", "cpu"]
    compute_candidates = [preferred_compute_type] if preferred_compute_type else ["int8_float16", "int8", "float16"]

    last_error = None
    for device in device_candidates:
        for compute_type in compute_candidates:
            try:
                logger.info(f"Loading model '{model_path}' on device={device}, compute_type={compute_type}...")
                model = WhisperModel(model_path, device=device, compute_type=compute_type)
                model_info = {
                    "path": model_path,
                    "device": device,
                    "compute_type": compute_type,
                    "status": "loaded"
                }
                logger.info(f"Model loaded successfully on {device} ({compute_type})")
                return True
            except Exception as e:
                last_error = e
                logger.warning(f"Model load failed on {device}/{compute_type}: {e}")

    model_info = {"status": "failed", "error": str(last_error)}
    logger.error(f"Failed to load model: {last_error}")
    return False

def resolve_model_name_or_path():
    """Resolve model from CLI/env with cross-platform defaults."""
    if len(sys.argv) > 1:
        return sys.argv[1]
    if os.getenv("WHISPER_MODEL_PATH"):
        return os.getenv("WHISPER_MODEL_PATH")
    if os.getenv("WHISPER_MODEL"):
        return os.getenv("WHISPER_MODEL")
    # Prefer repository-local model directory by default.
    return str(DEFAULT_LOCAL_MODEL_DIR)

def validate_local_model_path(model_path: str):
    """
    Validate local model directory if model_path looks like a filesystem path.
    Returns None when valid or not-applicable, otherwise returns error string.
    """
    path_obj = Path(model_path).expanduser()

    # Heuristic: treat as local path if absolute, contains path separators,
    # starts with '.'/'~', or exists on disk.
    looks_like_path = (
        path_obj.is_absolute()
        or "/" in model_path
        or "\\" in model_path
        or model_path.startswith(".")
        or model_path.startswith("~")
        or path_obj.exists()
    )

    if not looks_like_path:
        return None

    if not path_obj.exists():
        return (
            f"Model path does not exist: {path_obj}\n"
            f"Place a converted faster-whisper model at:\n"
            f"  {DEFAULT_LOCAL_MODEL_DIR}\n"
            f"Expected file: model.bin"
        )

    if path_obj.is_dir() and not (path_obj / "model.bin").exists():
        return (
            f"Model directory exists but model.bin is missing: {path_obj}\n"
            f"Expected file: {path_obj / 'model.bin'}"
        )

    return None

def resolve_device():
    return os.getenv("WHISPER_DEVICE")

def resolve_compute_type():
    return os.getenv("WHISPER_COMPUTE_TYPE")

@app.route('/health', methods=['GET'])
def health():
    """Health check endpoint"""
    return jsonify({
        "status": "ok",
        "model": model_info
    })

@app.route('/transcribe', methods=['POST'])
def transcribe():
    """
    Transcribe audio file

    Request:
        - file: audio file (WAV, MP3, etc.)
        - language: language code (optional, default: auto)

    Response:
        {"text": "transcribed text"}
    """
    if model is None:
        return jsonify({"error": "Model not loaded"}), 500

    # Get audio file
    if 'file' not in request.files:
        return jsonify({"error": "No file provided"}), 400

    audio_file = request.files['file']
    language = request.form.get('language', 'ru')

    # Save to temp file (Windows-compatible)
    temp_file = tempfile.NamedTemporaryFile(suffix='.wav', delete=False)
    temp_path = temp_file.name
    temp_file.close()
    audio_file.save(temp_path)

    try:
        # Transcribe
        logger.info(f"Transcribing audio in {language}...")
        segments, info = model.transcribe(
            temp_path,
            language=language,
            beam_size=5,
            vad_filter=True,
            vad_parameters=dict(min_silence_duration_ms=500)
        )

        # Collect text
        result = " ".join([segment.text for segment in segments])
        logger.info(f"Transcription complete: {len(result)} chars")

        return jsonify({
            "text": result.strip(),
            "language": info.language,
            "duration": info.duration
        })

    except Exception as e:
        logger.error(f"Transcription failed: {e}")
        return jsonify({"error": str(e)}), 500

    finally:
        # Clean up temp file
        if os.path.exists(temp_path):
            os.remove(temp_path)

if __name__ == '__main__':
    # Get model from command line / env with platform-neutral defaults.
    model_path = resolve_model_name_or_path()
    device = resolve_device()
    compute_type = resolve_compute_type()

    model_path_validation_error = validate_local_model_path(model_path)
    if model_path_validation_error:
        logger.error(model_path_validation_error)
        sys.exit(1)

    # Load model at startup
    if not load_model(model_path, preferred_device=device, preferred_compute_type=compute_type):
        logger.error("Failed to load model, exiting")
        sys.exit(1)

    # Start server
    port = int(os.getenv("WHISPER_PORT", "5000"))
    logger.info(f"Starting Whisper server on port {port}...")
    app.run(host='127.0.0.1', port=port, debug=False, threaded=True)
