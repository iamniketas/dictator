# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Language Preferences
- ALWAYS respond to the user in Russian (на русском языке).
- Keep technical terms and code snippets in their original form (English).

---

## Project Overview

**Dictator** is a voice dictation service for Windows that converts speech to text using local models. The application runs as a background service with system tray integration and is activated via hotkey.

## Technical Stack

- **Language:** Rust
- **Audio:** cpal (16 kHz, f32 samples)
- **Windows API:** windows-rs (system tray, hotkeys, text injection)
- **HTTP:** reqwest blocking (for Ollama API)
- **Config:** serde + toml
- **Transcription:** whisper-rs (planned, currently mock)

---

## СТАТУС ПРОЕКТА (обновлено 2025-01-25)

### ✅ Реализовано:
1. **System Tray** — иконка в трее с меню Exit (src/ui.rs)
2. **Global Hotkey** — Ctrl+Shift+D toggle записи (src/input.rs)
3. **Audio Capture** — запись с микрофона через cpal, 16 kHz, f32 (src/audio.rs)
4. **Ollama Client** — коррекция текста через Qwen3 30B (src/llm.rs)
5. **Text Injection** — вставка текста в активное окно через SendInput (src/input.rs)
6. **Config** — TOML конфигурация в %APPDATA%/dictator/ (src/config.rs)
7. **Pipeline** — audio → transcribe → Ollama → inject (src/main.rs)

### ⏳ В процессе:
- **Hide Console** — задача для Qwen в CURRENT_TASK.md
- **Release Build** — часть текущей задачи

### 📋 Следующие задачи:
- **Real Whisper** — заменить mock транскрипцию на whisper-rs
  - Требует: Visual Studio C++ Desktop workload + Clang
  - Модель: ggml-base.bin (~148 MB)
  - Пользователь устанавливает C++ tools

### 🔮 Будущее:
- Overlay UI (показ текста около курсора)
- Настраиваемый hotkey
- macOS support

---

## Работа с Qwen — ДВУХМОДЕЛЬНАЯ СИСТЕМА

### Концепция

В этом проекте используется координация двух моделей:
- **Opus (Claude)** — архитектор, планировщик, ревьюер
- **Qwen3-coder** — исполнитель кода (в отдельном инстансе Claude Code)

### Метафора

**Ты — извозчик, Qwen — рабочая лошадка.**
Твоя задача — направлять и проверять, а не толкать телегу вместо лошади.

### Твоя роль — ТОЛЬКО управление

**НЕЛЬЗЯ:**
- ❌ Автоматически исправлять код за Qwen
- ❌ Выполнять задачи вместо Qwen
- ❌ "Подчищать" ошибки без просьбы пользователя

**МОЖНО и НУЖНО:**
- ✅ Проверять результаты работы Qwen
- ✅ Диагностировать проблемы
- ✅ Создавать/корректировать задачи в `CURRENT_TASK.md`
- ✅ Улучшать инструкции
- ✅ Исправлять ТОЛЬКО если пользователь явно попросит

### Когда Qwen не справился

1. **Диагностируй** — найди что не так
2. **Объясни** пользователю
3. **Предложи:**
   - Упростить задачу
   - Разбить на шаги
   - Исправить инструкции
4. **НЕ исправляй сам** — пусть Qwen попробует снова

---

## Файлы координации

| Файл | Назначение |
|------|------------|
| `CURRENT_TASK.md` | Текущая задача для Qwen (ОДНА за раз!) |
| `EXECUTION_LOG.md` | Лог выполнения — Qwen пишет после каждого шага |
| `LOCAL_MODEL_PROMPT.md` | Инструкции для Qwen (прочитай перед первой задачей) |

### Формат задачи для Qwen (CURRENT_TASK.md)

```markdown
# CURRENT_TASK.md — Текущая задача

> **ПРАВИЛА РАБОТЫ:**
> 1. Выполняй ТОЛЬКО эту задачу
> 2. После КАЖДОГО шага пиши в EXECUTION_LOG.md
> 3. Копируй код ТОЧНО как написано
> 4. В конце выполни ПРОВЕРОЧНУЮ КОМАНДУ
> 5. Если проверка прошла — отметь задачу как DONE
>
> **ВАЖНО:** Используй ПОЛНЫЙ ПУТЬ к cargo:
> ```bash
> ~/.cargo/bin/cargo build
> ```

---

## Задача: [Название]

**Цель:** [Описание]

---

### Шаг 1: [Действие]

**Действие:** [Конкретная инструкция с Edit/Write/Bash]

**После выполнения:** Запиши в EXECUTION_LOG.md:
```
[ШАГ 1] Что сделал
```

---

### Шаг N: ПРОВЕРОЧНАЯ КОМАНДА

**Команда:**
```bash
~/.cargo/bin/cargo build 2>&1
```

---

**СТАТУС:** TODO / IN_PROGRESS / DONE / BLOCKED
```

### Критические правила для задач Qwen

1. **Полный путь к cargo:** `~/.cargo/bin/cargo` (не просто `cargo`)
2. **Микро-шаги:** Каждый шаг = одно атомарное действие
3. **Точный код:** Давать готовый код для копирования, без "додумывания"
4. **Проверка в конце:** Всегда `cargo build` или `cargo check`
5. **Логирование:** Qwen пишет в EXECUTION_LOG.md после каждого шага

---

## Проверка работы Qwen

Когда пользователь говорит "Qwen закончил":

1. **Прочитай EXECUTION_LOG.md** — проверь записи
2. **Прочитай изменённые файлы** — проверь код
3. **Запусти `cargo build`** — проверь компиляцию
4. **Сообщи результат:**
   - ✅ Успех → создай следующую задачу
   - ❌ Ошибка → диагностируй, НЕ исправляй сам

---

## Структура проекта

```
dictator/
├── Cargo.toml          # Dependencies: cpal, windows, reqwest, serde, toml, dirs, anyhow, tracing
├── src/
│   ├── lib.rs          # pub mod declarations
│   ├── main.rs         # Entry point, pipeline orchestration
│   ├── audio.rs        # AudioRecorder with cpal (thread-based)
│   ├── config.rs       # TOML config loader
│   ├── input.rs        # Hotkey listener + inject_text()
│   ├── llm.rs          # OllamaClient for text correction
│   ├── transcribe.rs   # Mock transcription (→ whisper-rs)
│   └── ui.rs           # System tray with Win32 API
├── CLAUDE.md           # Эти инструкции
├── CURRENT_TASK.md     # Задача для Qwen
├── EXECUTION_LOG.md    # Лог выполнения
└── LOCAL_MODEL_PROMPT.md # Инструкции для Qwen
```

---

## Быстрый старт после перезагрузки

1. **Проверь статус:** `cat CURRENT_TASK.md` — что делает/сделал Qwen
2. **Проверь лог:** `cat EXECUTION_LOG.md` — последние действия
3. **Проверь сборку:** `~/.cargo/bin/cargo build`
4. **Продолжай план:** Whisper после установки C++ tools

---

## C++ Toolchain для whisper-rs

**Статус:** Пользователь устанавливает Visual Studio C++ Desktop workload

**Требования:**
- Visual Studio 2022 с "Desktop development with C++"
- C++ Clang tools for Windows
- CMake (входит в VS)

**Проверка установки:**
```bash
# После установки и перезагрузки:
where cl  # Должен найти MSVC compiler
where cmake  # Должен найти CMake
```

**Следующий шаг после установки:**
1. Добавить `whisper-rs = "0.15"` в Cargo.toml
2. Скачать модель ggml-base.bin
3. Заменить mock в src/transcribe.rs
