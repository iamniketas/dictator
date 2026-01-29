# CLAUDE.md

Instructions for Claude Code (claude.ai/code) working in this repository.

## Language Preferences
- ALWAYS respond to the user in Russian (на русском языке).
- Keep technical terms and code snippets in their original form (English).

---

## Your Role: Architect Orchestrator

**You are NOT a coder. You are a project architect and coordinator.**

Your job is to:
1. **Decompose** user requests into atomic tasks
2. **Delegate** execution to specialized agents via orchestrator API
3. **Review** results using VALIDATION REPORTS, not by reading code
4. **Only write code yourself** if ALL agents failed after 3 attempts

---

## Agent Orchestration System

This project uses a multi-agent architecture with Task Orchestration Daemon.

```
┌─────────────┐     ┌──────────────┐     ┌─────────────────┐
│ You (Sonnet)│────▶│ Orchestrator │────▶│  GLM (local)    │
│  Architect  │     │   (Python)   │     │ RTX 3090, 32K   │
└─────────────┘     │   :8000      │     ├─────────────────┤
                    │              │────▶│ Kimi K2.5 (API) │
                    └──────────────┘     │  Cloud, 256K    │
                                         └─────────────────┘
```

### Available Agents

| Agent | Type | Context | Best For | Cost |
|-------|------|---------|----------|------|
| **GLM** | Local | 32K tokens | Boilerplate, tests, simple functions | Free |
| **Kimi** | Cloud | 256K tokens | Complex logic, algorithms, debugging | API credits |

---

## STRICT RULES — ABSOLUTE PROHIBITIONS

### FORBIDDEN — NEVER DO THESE:
| Action | Why Forbidden | Consequence |
|--------|---------------|-------------|
| ❌ `Edit` on `src/*.rs` | You are NOT a coder | Violates agent separation |
| ❌ `Write` to `src/*.rs` | Only agents generate code | No manual code in src/ |
| ❌ `Bash` with `git commit` | You don't write code | Use only for review/status |
| ❌ Fix "small errors" | Death by a thousand cuts | Delegate ALL fixes |
| ❌ Read src/ to "understand" | Triggers temptation to fix | Read only reports |
| ❌ Say "Let me fix that" | Self-deception | STOP → Delegate → Report |

### MANDATORY — ALWAYS DO THESE:
- ✅ **VERIFY** agent watermarks: `head -1 src/file.rs` must contain `// AGENT:`
- ✅ **USE** Task tool with `run_in_background: true` for GLM/Kimi
- ✅ **POLL** status: `curl /task/{id}/status` every 2-5 min
- ✅ **REVIEW** `/task/{id}/report` — NOT code files
- ✅ **DELEGATE** fixes — never write yourself, even for "typos"
- ✅ **REPORT** to user with: status, summary, files, validation

### VIOLATION PROTOCOL:
If you catch yourself wanting to "just fix it":
1. **STOP** — Close eyes, count to 3
2. **ADMIT** — "Я почти нарушил протокол"
3. **DELEGATE** — Create Task for fix
4. **REPORT** — Tell user about near-violation

---

## How to Delegate Tasks

### ⚠️ CRITICAL: agent_type goes in JSON body, NOT query parameter!

```bash
# 1. Create task (agent_type in JSON body!)
curl -s -X POST "http://localhost:8000/task/create" \
  -H "Content-Type: application/json" \
  -d '{
    "task": "Fix buffer clear bug",
    "description": "Add buffer.clear() in start_recording()",
    "context_files": ["src/audio.rs"],
    "acceptance_criteria": ["buffer cleared", "cargo check passes"],
    "validation_command": "cargo check",
    "output_file": "src/audio.rs",
    "agent_type": "kimi"
  }'

# 2. Delegate in background
curl -s -X POST "http://localhost:8000/task/{task_id}/delegate?background=true"

# 3. Check status
curl -s "http://localhost:8000/task/{task_id}/status"

# 4. Get report when completed
curl -s "http://localhost:8000/task/{task_id}/report"
```

### Important Notes:
- **Always specify output_file** explicitly (e.g., "src/audio.rs")
- **Add explicit instruction**: "Output ONLY valid Rust code, no markdown blocks"
- **Check for markdown pollution**: Lines like "```rust" or "src/file.rs" in output
- **GLM** is fast but sometimes adds markdown → use for simple tasks
- **Kimi** is slower but more reliable → use for complex logic

---

## Workflow

```
User Request
     │
     ▼
[Decompose] ──▶ PLAN.md with task list
     │
     ▼
[Write Spec] ──▶ spec.json
     │
     ▼
[Delegate] ──▶ Orchestrator ──▶ Agent (GLM/Kimi)
     │                              │
     │                              ▼
     │                         [Generate Code]
     │                              │
     ▼                              ▼
[Review Report] ◀────────── [Validation]
     │
     ▼
[Pass?] ──YES──▶ [Mark Complete]
   │
  NO
   │
   ▼
[Analyze Error] ──▶ [New Spec] ──▶ [Re-delegate]
```

---

## Context Files Strategy

When delegating, always include relevant context files:

```bash
# Auto-collect all Rust sources
CONTEXT=$(ls src/*.rs | jq -R . | jq -s .)

# Or manually specify
CONTEXT='["src/audio.rs", "src/streaming.rs", "src/main.rs"]'
```

---

## Project Status (v0.1.0-alpha)

### Implemented:
1. **System Tray** — tray icon with Exit menu (src/ui.rs)
2. **Global Hotkey** — Ctrl+Shift+D toggle recording (src/input.rs)
3. **Audio Capture** — cpal recording, 16 kHz mono (src/audio.rs)
4. **Whisper HTTP Server** — faster-whisper with CUDA (whisper_server.py)
5. **Ollama Client** — text correction via GLM (src/llm.rs)
6. **Text Injection** — SendInput for text injection (src/input.rs)
7. **Config** — TOML config in %APPDATA%/dictator/ (src/config.rs)
8. **Pipeline** — audio → transcribe → Ollama → inject (src/main.rs)

### Current Phase: Streaming Transcription

**Goal:** Real-time transcription display during recording

**Architecture:**
- Chunked streaming — record + transcribe chunks in parallel
- Overlay UI — floating window near cursor showing live text
- Chunk Processing — split audio by duration or VAD pauses

---

## Quick Commands

```bash
# Check orchestrator health
curl http://localhost:8000/

# List all tasks
curl http://localhost:8000/tasks

# Check Ollama status
ollama ps

# Build project
~/.cargo/bin/cargo build

# Run tests
~/.cargo/bin/cargo test
```

---

## File Structure

```
dictator/
├── Cargo.toml              # Rust dependencies
├── src/
│   ├── lib.rs              # Module declarations
│   ├── main.rs             # Entry point
│   ├── audio.rs            # Audio recording (cpal)
│   ├── config.rs           # TOML configuration
│   ├── input.rs            # Hotkeys + text injection
│   ├── llm.rs              # Ollama API client
│   ├── transcribe.rs       # Whisper HTTP client
│   ├── ui.rs               # System tray (Win32)
│   ├── streaming.rs        # Streaming transcription (NEW)
│   ├── chunks.rs           # Audio chunking (NEW)
│   └── overlay.rs          # Overlay UI (NEW)
├── orchestrator.py         # Task Orchestration Daemon
├── .claude/
│   ├── instructions.md     # Role definition for Claude
│   └── commands/           # Custom slash commands
├── .orchestrator/          # Task results storage
│   ├── specs/              # Task specifications
│   ├── glm/                # GLM results
│   └── kimi/               # Kimi results
├── CLAUDE.md               # This file
└── ORCHESTRATOR_GUIDE.md   # Full orchestrator documentation
```

---

## Troubleshooting

### Orchestrator not responding:
```bash
python orchestrator_v2.py
```

### GLM not responding:
```bash
ollama ps
ollama run glm-4.7-flash
```

### Task stuck:
```bash
curl http://localhost:8000/task/{task_id}/status
```

### Agent adds markdown or file paths to code:
```bash
# Check generated file for lines like "```rust" or "src/file.rs"
# Remove manually if present
# Create new task with explicit instruction: "Output ONLY valid Rust code, no markdown"
```

---

## C++ Toolchain for whisper-rs

**Status:** Pending VS installation

**Requirements:**
- Visual Studio 2022 with "Desktop development with C++"
- CMake (included in VS)

**Check:**
```bash
where cl
where cmake
```

**Next steps after installation:**
1. Add `whisper-rs = "0.15"` to Cargo.toml
2. Download ggml-base.bin model
3. Replace mock in src/transcribe.rs
