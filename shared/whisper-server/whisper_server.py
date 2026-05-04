#!/usr/bin/env python3
"""
Whisper HTTP Server - Fast transcription service with model caching.

Supports two execution backends:
- faster-whisper (existing path)
- transformers ASR pipeline (for model refs like Canary/Granite/Parakeet)
"""
import os
import sys
import tempfile
import tarfile
import shutil
from pathlib import Path
from flask import Flask, request, jsonify
import logging

# Configure logging
logging.basicConfig(level=logging.INFO)
logger = logging.getLogger(__name__)

def _configure_temp_dir():
    candidates = [
        os.getenv("NEMO_CACHE_DIR"),
        os.getenv("TEMP"),
        os.getenv("TMP"),
    ]
    for raw in candidates:
        if not raw:
            continue
        try:
            p = Path(raw).expanduser()
            p.mkdir(parents=True, exist_ok=True)
            tempfile.tempdir = str(p)
            logger.info("Using temp directory: %s", p)
            return
        except Exception as e:
            logger.warning("Failed to set temp directory '%s': %s", raw, e)

_configure_temp_dir()

app = Flask(__name__)
app.config["JSON_AS_ASCII"] = False
try:
    app.json.ensure_ascii = False  # Flask 2.3+/3.x JSON provider
except Exception:
    pass

# Global model instance (loaded once)
model = None
model_info = {}
model_runtime = "unknown"  # "faster_whisper" | "transformers_asr" | "nemo_asr"
SCRIPT_DIR = Path(__file__).resolve().parent
DEFAULT_LOCAL_MODEL_DIR = SCRIPT_DIR / "models" / "faster-whisper-large-v2"


def is_hf_like_ref(model_path: str) -> bool:
    path_obj = Path(model_path).expanduser()
    return (
        "/" in model_path
        and "\\" not in model_path
        and not model_path.startswith(".")
        and not model_path.startswith("~")
        and not path_obj.is_absolute()
        and ":" not in model_path
    )


def prefers_transformers_backend(model_path: str) -> bool:
    p = model_path.lower().strip()
    if not p:
        return False
    if p.startswith("nvidia/canary"):
        return True
    if p.startswith("ibm-granite/"):
        return True
    if p.startswith("nvidia/parakeet"):
        return True
    # For generic HF refs we also prefer transformers first.
    return is_hf_like_ref(model_path)


def has_nemo_artifact(model_path: str) -> bool:
    p = Path(model_path).expanduser()
    if p.is_file() and p.suffix.lower() == ".nemo":
        return True
    if p.is_dir():
        try:
            return any(child.suffix.lower() == ".nemo" for child in p.iterdir() if child.is_file())
        except Exception:
            return False
    return False


def resolve_transformers_device(preferred_device: str | None):
    try:
        import torch  # type: ignore
    except Exception as e:
        raise RuntimeError(
            "transformers backend requires torch. Install: pip install torch transformers accelerate"
        ) from e

    if preferred_device in ("cpu",):
        return -1, torch.float32

    cuda_available = torch.cuda.is_available()
    if preferred_device in ("cuda", "auto", None):
        if cuda_available:
            return 0, torch.float16
        return -1, torch.float32

    # Unknown preference -> safe auto
    if cuda_available:
        return 0, torch.float16
    return -1, torch.float32


def load_transformers_model(model_path, preferred_device=None):
    """Load HF model via transformers ASR pipeline."""
    global model, model_info, model_runtime

    try:
        from transformers import pipeline  # type: ignore
    except Exception as e:
        raise RuntimeError(
            "transformers backend requires 'transformers'. Install: pip install transformers accelerate"
        ) from e

    device_index, torch_dtype = resolve_transformers_device(preferred_device)
    logger.info(
        "Loading transformers ASR model '%s' on device=%s...",
        model_path,
        "cuda" if device_index >= 0 else "cpu",
    )

    asr = pipeline(
        task="automatic-speech-recognition",
        model=model_path,
        device=device_index,
        torch_dtype=torch_dtype,
    )

    model = asr
    model_runtime = "transformers_asr"
    model_info = {
        "path": model_path,
        "runtime": model_runtime,
        "device": "cuda" if device_index >= 0 else "cpu",
        "status": "loaded",
    }
    logger.info("Transformers ASR model loaded successfully")
    return True


def load_faster_whisper_model(model_path, preferred_device=None, preferred_compute_type=None):
    """Load faster-whisper model into memory."""
    global model, model_info, model_runtime

    try:
        from faster_whisper import WhisperModel
    except Exception as e:
        raise RuntimeError(
            "faster-whisper backend requires 'faster-whisper'. Install: pip install faster-whisper"
        ) from e

    device_candidates = [preferred_device] if preferred_device else ["auto", "cpu"]
    compute_candidates = [preferred_compute_type] if preferred_compute_type else ["int8_float16", "int8", "float16"]

    last_error = None
    for device in device_candidates:
        for compute_type in compute_candidates:
            try:
                logger.info(
                    "Loading faster-whisper model '%s' on device=%s, compute_type=%s...",
                    model_path,
                    device,
                    compute_type,
                )
                fw_model = WhisperModel(model_path, device=device, compute_type=compute_type)
                model = fw_model
                model_runtime = "faster_whisper"
                model_info = {
                    "path": model_path,
                    "runtime": model_runtime,
                    "device": device,
                    "compute_type": compute_type,
                    "status": "loaded",
                }
                logger.info("faster-whisper model loaded successfully on %s (%s)", device, compute_type)
                return True
            except Exception as e:
                last_error = e
                logger.warning("faster-whisper load failed on %s/%s: %s", device, compute_type, e)

    raise RuntimeError(f"faster-whisper load failed: {last_error}")


def resolve_nemo_file(model_path: str) -> Path:
    p = Path(model_path).expanduser()
    if p.is_file() and p.suffix.lower() == ".nemo":
        return p
    if p.is_dir():
        files = [f for f in p.iterdir() if f.is_file() and f.suffix.lower() == ".nemo"]
        if not files:
            raise RuntimeError(f"No .nemo file found in {p}")
        files.sort(key=lambda f: f.stat().st_size if f.exists() else 0, reverse=True)
        return files[0]
    raise RuntimeError(f"Invalid NeMo path: {model_path}")

def _sanitize_tar_member_name(name: str) -> str:
    normalized = name.replace("\\", "/")
    while normalized.startswith("./") or normalized.startswith(".\\"):
        normalized = normalized[2:]
    return normalized.lstrip("/")

def ensure_nemo_extracted(nemo_file: Path) -> Path:
    base_temp = Path(tempfile.gettempdir()).expanduser()
    extract_root = base_temp / "dictator-nemo"
    model_dir = extract_root / nemo_file.stem
    marker = model_dir / ".extracted.ok"
    cfg = model_dir / "model_config.yaml"
    ckpt = model_dir / "model_weights.ckpt"
    if marker.exists() and cfg.exists() and ckpt.exists():
        return model_dir

    if model_dir.exists():
        shutil.rmtree(model_dir, ignore_errors=True)
    model_dir.mkdir(parents=True, exist_ok=True)

    with tarfile.open(nemo_file, "r:*") as tf:
        for member in tf.getmembers():
            sanitized = _sanitize_tar_member_name(member.name)
            if not sanitized or sanitized.endswith("/"):
                continue
            target = (model_dir / sanitized).resolve()
            if not str(target).startswith(str(model_dir.resolve())):
                continue
            target.parent.mkdir(parents=True, exist_ok=True)
            extracted = tf.extractfile(member)
            if extracted is None:
                continue
            with extracted, open(target, "wb") as out:
                shutil.copyfileobj(extracted, out)

    if not cfg.exists() or not ckpt.exists():
        raise RuntimeError(f"NeMo archive extraction incomplete for {nemo_file}")
    marker.write_text("ok", encoding="utf-8")
    return model_dir


def load_nemo_model(model_path, preferred_device=None):
    global model, model_info, model_runtime

    try:
        import torch  # type: ignore
        from nemo.collections.asr.models import ASRModel  # type: ignore
        from nemo.core.connectors.save_restore_connector import SaveRestoreConnector  # type: ignore
    except Exception as e:
        raise RuntimeError(
            "nemo backend requires nemo_toolkit[asr]. Install: pip install nemo_toolkit[asr]"
        ) from e

    nemo_file = resolve_nemo_file(model_path)
    extracted_dir = ensure_nemo_extracted(nemo_file)
    use_cuda = preferred_device in ("cuda", "auto", None) and torch.cuda.is_available()
    device = "cuda" if use_cuda else "cpu"
    logger.info("Loading NeMo ASR model '%s' on device=%s...", nemo_file, device)

    connector = SaveRestoreConnector()
    connector.model_extracted_dir = str(extracted_dir)
    asr_model = ASRModel.restore_from(
        restore_path=str(nemo_file),
        map_location=device,
        save_restore_connector=connector,
    )
    asr_model.eval()
    if use_cuda:
        asr_model = asr_model.cuda()
    else:
        asr_model = asr_model.cpu()

    model = asr_model
    model_runtime = "nemo_asr"
    model_info = {
        "path": str(nemo_file),
        "extracted_dir": str(extracted_dir),
        "runtime": model_runtime,
        "device": device,
        "status": "loaded",
    }
    logger.info("NeMo ASR model loaded successfully")
    return True


def load_model(model_path, preferred_device=None, preferred_compute_type=None):
    """Load model with runtime fallback order based on model ref."""
    global model_info

    runtime_hint = os.getenv("WHISPER_RUNTIME", "").strip().lower()
    try_nemo_first = runtime_hint == "nemo" or (runtime_hint == "" and has_nemo_artifact(model_path))
    try_transformers_first = runtime_hint == "transformers" or (
        runtime_hint == "" and prefers_transformers_backend(model_path)
    )

    loaders = []
    if try_nemo_first:
        loaders = ["nemo", "transformers", "faster_whisper"]
    elif try_transformers_first:
        loaders = ["transformers", "faster_whisper"]
    else:
        loaders = ["faster_whisper", "transformers"]

    errors = []
    for loader in loaders:
        try:
            if loader == "transformers":
                if load_transformers_model(model_path, preferred_device=preferred_device):
                    return True
            elif loader == "nemo":
                if load_nemo_model(model_path, preferred_device=preferred_device):
                    return True
            else:
                if load_faster_whisper_model(
                    model_path,
                    preferred_device=preferred_device,
                    preferred_compute_type=preferred_compute_type,
                ):
                    return True
        except Exception as e:
            errors.append(f"{loader}: {e}")
            logger.warning("Model load via %s failed: %s", loader, e)

    model_info = {"status": "failed", "error": " | ".join(errors)}
    logger.error("Failed to load model. Errors: %s", model_info["error"])
    return False


def resolve_model_name_or_path():
    """Resolve model from CLI/env with cross-platform defaults."""
    if len(sys.argv) > 1:
        return sys.argv[1]
    if os.getenv("WHISPER_MODEL_PATH"):
        return os.getenv("WHISPER_MODEL_PATH")
    if os.getenv("WHISPER_MODEL"):
        return os.getenv("WHISPER_MODEL")
    return str(DEFAULT_LOCAL_MODEL_DIR)


def validate_local_model_path(model_path: str):
    """
    Validate local model directory if model_path looks like a filesystem path.
    Returns None when valid or not-applicable, otherwise returns error string.
    """
    path_obj = Path(model_path).expanduser()
    runtime_hint = os.getenv("WHISPER_RUNTIME", "").strip().lower()

    is_hf_ref = is_hf_like_ref(model_path)

    looks_like_path = (
        path_obj.is_absolute()
        or "\\" in model_path
        or model_path.startswith(".")
        or model_path.startswith("~")
        or path_obj.exists()
    ) and not is_hf_ref

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
        if runtime_hint == "transformers":
            # Explicit runtime override: allow transformers snapshots without model.bin.
            return None
        if runtime_hint == "nemo" and has_nemo_artifact(str(path_obj)):
            return None
        # transformers snapshots (Parakeet/Canary/Granite and similar) do not have model.bin
        # and should be accepted when config/tokenizer artifacts are present.
        has_transformers_artifacts = (
            (path_obj / "config.json").exists()
            and (
                (path_obj / "preprocessor_config.json").exists()
                or (path_obj / "tokenizer.json").exists()
                or (path_obj / "tokenizer_config.json").exists()
                or (path_obj / "generation_config.json").exists()
            )
        )
        if not has_transformers_artifacts:
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
    return jsonify({
        "status": "ok",
        "runtime": model_runtime,
        "model": model_info,
    })


def transcribe_with_faster_whisper(temp_path: str, language: str):
    segments, info = model.transcribe(
        temp_path,
        language=language,
        beam_size=5,
        vad_filter=True,
        vad_parameters=dict(min_silence_duration_ms=500),
    )
    text = " ".join([segment.text for segment in segments]).strip()
    return text, getattr(info, "language", language), getattr(info, "duration", None)


def transcribe_with_transformers(temp_path: str, language: str):
    # Keep params minimal for max model compatibility.
    out = model(temp_path)
    if isinstance(out, dict):
        text = (out.get("text") or "").strip()
    else:
        text = str(out).strip()
    return text, language, None


def transcribe_with_nemo(temp_path: str, language: str):
    # NeMo API may return str or hypothesis objects depending on model class.
    out = model.transcribe([temp_path], batch_size=1)
    text = ""
    if isinstance(out, list) and len(out) > 0:
        first = out[0]
        if isinstance(first, str):
            text = first
        else:
            text = getattr(first, "text", str(first))
    else:
        text = str(out)
    return text.strip(), language, None


@app.route('/transcribe', methods=['POST'])
def transcribe():
    if model is None:
        return jsonify({"error": "Model not loaded"}), 500

    if 'file' not in request.files:
        return jsonify({"error": "No file provided"}), 400

    audio_file = request.files['file']
    language = request.form.get('language', 'ru')

    temp_file = tempfile.NamedTemporaryFile(suffix='.wav', delete=False)
    temp_path = temp_file.name
    temp_file.close()
    audio_file.save(temp_path)

    try:
        logger.info("Transcribing audio in %s using %s...", language, model_runtime)
        if model_runtime == "transformers_asr":
            result, out_lang, duration = transcribe_with_transformers(temp_path, language)
        elif model_runtime == "nemo_asr":
            result, out_lang, duration = transcribe_with_nemo(temp_path, language)
        else:
            result, out_lang, duration = transcribe_with_faster_whisper(temp_path, language)

        logger.info("Transcription complete: %d chars", len(result))
        return jsonify({
            "text": result,
            "language": out_lang,
            "duration": duration,
            "runtime": model_runtime,
        })

    except Exception as e:
        logger.error("Transcription failed: %s", e)
        return jsonify({"error": str(e), "runtime": model_runtime}), 500

    finally:
        if os.path.exists(temp_path):
            os.remove(temp_path)


if __name__ == '__main__':
    model_path = resolve_model_name_or_path()
    device = resolve_device()
    compute_type = resolve_compute_type()

    model_path_validation_error = validate_local_model_path(model_path)
    if model_path_validation_error:
        logger.error(model_path_validation_error)
        sys.exit(1)

    if not load_model(model_path, preferred_device=device, preferred_compute_type=compute_type):
        logger.error("Failed to load model, exiting")
        sys.exit(1)

    port = int(os.getenv("WHISPER_PORT", "5000"))
    logger.info("Starting Whisper server on port %s...", port)
    app.run(host='127.0.0.1', port=port, debug=False, threaded=True)
