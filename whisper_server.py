#!/usr/bin/env python3
"""
Whisper HTTP Server - Fast transcription service with model caching
Loads faster-whisper model once at startup and keeps it in memory (GPU).
"""
import os
import sys
import tempfile
from flask import Flask, request, jsonify
from faster_whisper import WhisperModel
import logging

# Configure logging
logging.basicConfig(level=logging.INFO)
logger = logging.getLogger(__name__)

app = Flask(__name__)

# Global model instance (loaded once)
model = None
model_info = {}

def load_model(model_path, device="cuda", compute_type="float16"):
    """Load Whisper model into memory"""
    global model, model_info

    try:
        logger.info(f"Loading model from {model_path} on {device}...")
        model = WhisperModel(model_path, device=device, compute_type=compute_type)
        model_info = {
            "path": model_path,
            "device": device,
            "compute_type": compute_type,
            "status": "loaded"
        }
        logger.info(f"Model loaded successfully on {device}")
        return True
    except Exception as e:
        logger.error(f"CUDA failed, trying CPU: {e}")
        try:
            model = WhisperModel(model_path, device="cpu", compute_type="int8")
            model_info = {
                "path": model_path,
                "device": "cpu",
                "compute_type": "int8",
                "status": "loaded (fallback)"
            }
            logger.info("Model loaded on CPU (fallback)")
            return True
        except Exception as e2:
            logger.error(f"Failed to load model: {e2}")
            model_info = {"status": "failed", "error": str(e2)}
            return False

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
    # Get model path from command line or environment
    model_path = sys.argv[1] if len(sys.argv) > 1 else os.getenv(
        "WHISPER_MODEL_PATH",
        r"C:\Users\Niketas\whisper\fwxxl\_models\faster-whisper-large-v2"
    )

    # Load model at startup
    if not load_model(model_path):
        logger.error("Failed to load model, exiting")
        sys.exit(1)

    # Start server
    port = int(os.getenv("WHISPER_PORT", "5000"))
    logger.info(f"Starting Whisper server on port {port}...")
    app.run(host='127.0.0.1', port=port, debug=False)