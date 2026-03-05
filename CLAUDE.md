# CLAUDE.md

Instructions for Claude Code (claude.ai/code) working in this repository.

## Language Preferences
- ALWAYS respond to the user in Russian.
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

## Project Overview

Dictator is a **multi-platform local voice dictation** app. Each platform has a native client.

### Repository Structure (post-migration)

```
dictator/
  apps/
    windows/          # Rust + Win32 native client (v0.1.0-alpha)
      Cargo.toml
      src/
      examples/
      assets/
    macos/            # Swift + SwiftUI/AppKit (prototype)
      Package.swift
      Sources/
  shared/
    whisper-server/   # Python HTTP server for faster-whisper
    contracts/        # Cross-platform schemas
  docs/
    MACOS_ROADMAP.md
    REPO_STRUCTURE_PLAN.md
    archive/          # Historical session reports
  ROADMAP.md          # Feature prioritization
  ARCHITECTURE.md     # Technical architecture
  README.md           # Project hub
```

### Build Commands

```bash
# Windows client
cd apps/windows && ~/.cargo/bin/cargo build

# macOS client
cd apps/macos && swift build

# Whisper server
python shared/whisper-server/whisper_server.py
```

---

## Agent Orchestration System

```
You (Architect) -> Orchestrator (:8000) -> GLM (local, 32K) or Kimi (cloud, 256K)
```

### Available Agents

| Agent | Type | Context | Best For | Cost |
|-------|------|---------|----------|------|
| **GLM** | Local | 32K tokens | Boilerplate, tests, simple functions | Free |
| **Kimi** | Cloud | 256K tokens | Complex logic, algorithms, debugging | API credits |

---

## STRICT RULES

### FORBIDDEN:
- Edit/Write to `apps/windows/src/*.rs` or `apps/macos/Sources/**/*.swift` — only agents generate code
- `git commit` — you don't write code
- Fix "small errors" manually — delegate ALL fixes
- Read src/ to "understand" — read only reports

### MANDATORY:
- VERIFY agent watermarks: `head -1 apps/windows/src/file.rs` must contain `// AGENT:`
- USE Task tool with `run_in_background: true` for GLM/Kimi
- REVIEW `/task/{id}/report` — NOT code files
- DELEGATE fixes — never write yourself
- REPORT to user with: status, summary, files, validation

---

## How to Delegate Tasks

```bash
# 1. Create task (agent_type in JSON body!)
curl -s -X POST "http://localhost:8000/task/create" \
  -H "Content-Type: application/json" \
  -d '{
    "task": "Fix buffer clear bug",
    "description": "Add buffer.clear() in start_recording()",
    "context_files": ["apps/windows/src/audio.rs"],
    "acceptance_criteria": ["buffer cleared", "cargo check passes"],
    "validation_command": "cd apps/windows && cargo check",
    "output_file": "apps/windows/src/audio.rs",
    "agent_type": "kimi"
  }'

# 2. Delegate in background
curl -s -X POST "http://localhost:8000/task/{task_id}/delegate?background=true"

# 3. Check status
curl -s "http://localhost:8000/task/{task_id}/status"

# 4. Get report when completed
curl -s "http://localhost:8000/task/{task_id}/report"
```

---

## Roadmap Reference

See [ROADMAP.md](ROADMAP.md) for feature priorities:
- **Phase 1 (MVP+):** Smart hotkeys, waveform overlay, flexible injection, model selector
- **Phase 2 (Polish):** History, memory management (5 min default, configurable), LLM toggle
- **Phase 3 (Advanced):** Dictionary editor, Command Mode

For competitor details: see [COMPETITORS_ANALYSIS.md](COMPETITORS_ANALYSIS.md)

---

## Related Projects

- **Contora** (`../contora/`) — local audio recording and transcription (.NET/WinUI 3). Shares whisper runtime and models on Windows.

---

## Quick Commands

```bash
# Check orchestrator health
curl http://localhost:8000/

# Build Windows client
cd apps/windows && ~/.cargo/bin/cargo build

# Build macOS client
cd apps/macos && swift build

# Run whisper server
python shared/whisper-server/whisper_server.py

# Check Ollama status
ollama ps
```
