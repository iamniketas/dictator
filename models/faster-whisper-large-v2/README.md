# Local Whisper Model Placement

Put your converted `faster-whisper` model files into this directory:

`models/faster-whisper-large-v2`

Minimum required file:

- `model.bin`

Typical converted model directory also includes files like:

- `config.json`
- `tokenizer.json`
- `vocabulary.json` (depends on converter/model)

When this folder is populated, you can start the server from repository root:

```bash
python3 whisper_server.py
```

The server now defaults to this local folder when no model argument/env is provided.
