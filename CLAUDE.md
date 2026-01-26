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

## СТАТУС ПРОЕКТА (обновлено 2026-01-26)

### ✅ Реализовано (v0.1.0-alpha):
1. **System Tray** — иконка в трее с меню Exit (src/ui.rs)
2. **Global Hotkey** — Ctrl+Shift+D toggle записи (src/input.rs)
3. **Audio Capture** — запись с микрофона через cpal, автоконвертация в 16 kHz mono (src/audio.rs)
4. **Whisper HTTP Server** — faster-whisper с кэшированием модели в памяти (whisper_server.py)
5. **Ollama Client** — коррекция текста через GLM-4 (src/llm.rs)
6. **Text Injection** — вставка текста в активое окно через SendInput (src/input.rs)
7. **Config** — TOML конфигурация в %APPDATA%/dictator/ (src/config.rs)
8. **Pipeline** — audio → transcribe → Ollama → inject (src/main.rs)
9. **Release Build** — релиз без консоли, оптимизированный (target/release/)
10. **GitHub Release** — v0.1.0-alpha опубликован

### 🚧 ТЕКУЩАЯ ФАЗА: Визуальная оболочка + Streaming

**Цель:** Реал-тайм отображение транскрипции во время записи

**Архитектурные изменения:**
- **Streaming Transcription** — запись + транскрипция параллельно по чанкам (вместо записал→остановил→расшифровал)
- **Overlay UI** — всплывающее окно рядом с курсором, показывает текст в реальном времени
- **Chunk Processing** — делим аудио на чанки по N секунд, отправляем на Whisper параллельно

### 📋 Следующие задачи (приоритет):

#### 1. **Streaming Transcription Engine** 🔥 КРИТИЧНО
**Проблема:** Сейчас транскрипция происходит после остановки записи → долгая задержка
**Решение:** Chunked streaming — записываем + параллельно транскрибируем

**Архитектура:**
```
Recording Thread (cpal)
    ↓
Audio Buffer (растёт)
    ↓
Chunk Detector (каждые 3-5 сек или по паузе)
    ↓
Whisper HTTP Request (параллельный)
    ↓
Partial Text → Overlay UI
```

**Детали:**
- **VAD (Voice Activity Detection)** — определяем паузы, режем по паузам
- **Overlap** — чанки перекрываются на 0.5-1 сек (чтобы не терять слова на стыках)
- **Async Transcription** — несколько чанков в параллель к Whisper
- **Text Merging** — склеиваем частичные результаты

**Подзадачи:**
1. Добавить VAD в audio.rs (или использовать faster-whisper VAD через API)
2. Создать ChunkManager — делит буфер на чанки по паузам
3. Async HTTP запросы к Whisper (tokio + reqwest async)
4. Накопление и склейка partial results
5. Тестирование с длинными записями (1-5 минут)

#### 2. **Overlay UI** — Всплывающее окно транскрипции
**Функционал:**
- Полупрозрачное окно рядом с курсором
- Показывает текст в реальном времени (по мере транскрипции чанков)
- Автоскрытие через N секунд после окончания
- Настраиваемая позиция (около курсора / углы экрана)

**Технологии:**
- Win32 API: `CreateWindowEx` с `WS_EX_LAYERED` + `WS_EX_TOPMOST`
- DirectWrite или GDI+ для рендеринга текста
- Fade-in/fade-out анимация

**Подзадачи:**
1. Создать src/overlay.rs — модуль для Overlay окна
2. Рендеринг текста с переносом строк
3. Позиционирование около курсора (`GetCursorPos`)
4. Обновление текста из Chunk Manager
5. Настройки в config.toml (размер, прозрачность, позиция)

#### 3. **Улучшения конфигурации**
- Настраиваемый hotkey в UI (не только config.toml)
- Выбор устройства записи (сейчас default)
- Языковые профили (ru/en с разными моделями)

#### 4. **Рефакторинг под streaming**
- Разделить main.rs на модули (слишком много логики в одном месте)
- Создать TranscriptionEngine trait для разных движков (HTTP, local whisper.rs)
- State machine для Recording / Transcribing / Injecting

### 🔮 Будущее (после v0.2):
- Настраиваемый hotkey через UI
- macOS/Linux support
- Локальный whisper.rs (без HTTP сервера)
- Голосовые команды (не просто диктовка, а управление компьютером)

---

## Работа с GLM-4 — ДВУХМОДЕЛЬНАЯ СИСТЕМА

### Концепция

В этом проекте используется координация двух моделей:
- **Claude Opus (через Claude Code)** — архитектор, планировщик, ревьюер
- **GLM-4.7-flash (через Ollama)** — исполнитель кода (в отдельном инстансе Claude Code)

**Модель для разработки:** `glm-4.7-flash` — быстрая, хороша для кодинга и работы с текстом.

### Метафора

**Ты — извозчик, GLM — рабочая лошадка.**
Твоя задача — направлять и проверять, а не толкать телегу вместо лошади.

### Твоя роль — ТОЛЬКО управление

**НЕЛЬЗЯ:**
- ❌ Автоматически исправлять код за GLM
- ❌ Выполнять задачи вместо GLM
- ❌ "Подчищать" ошибки без просьбы пользователя

**МОЖНО и НУЖНО:**
- ✅ Проверять результаты работы GLM
- ✅ Диагностировать проблемы
- ✅ Создавать/корректировать задачи в `CURRENT_TASK.md`
- ✅ Улучшать инструкции
- ✅ Исправлять ТОЛЬКО если пользователь явно попросит

### Когда GLM не справился

1. **Диагностируй** — найди что не так
2. **Объясни** пользователю
3. **Предложи:**
   - Упростить задачу
   - Разбить на шаги
   - Исправить инструкции
4. **НЕ исправляй сам** — пусть GLM попробует снова

---

## Файлы координации

| Файл | Назначение |
|------|------------|
| `CURRENT_TASK.md` | Текущая задача для GLM (ОДНА за раз!) |
| `EXECUTION_LOG.md` | Лог выполнения — GLM пишет после каждого шага |
| `LOCAL_MODEL_PROMPT.md` | Инструкции для GLM (прочитай перед первой задачей) |

### Формат задачи для GLM (CURRENT_TASK.md)

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

### Критические правила для задач GLM

1. **Полный путь к cargo:** `~/.cargo/bin/cargo` (не просто `cargo`)
2. **Микро-шаги:** Каждый шаг = одно атомарное действие
3. **Точный код:** Давать готовый код для копирования, без "додумывания"
4. **Проверка в конце:** Всегда `cargo build` или `cargo check`
5. **Логирование:** GLM пишет в EXECUTION_LOG.md после каждого шага

---

## Проверка работы GLM

Когда пользователь говорит "GLM закончил":

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
├── CURRENT_TASK.md     # Задача для GLM
├── EXECUTION_LOG.md    # Лог выполнения
└── LOCAL_MODEL_PROMPT.md # Инструкции для GLM
```

---

## Быстрый старт после перезагрузки

1. **Проверь статус:** `cat CURRENT_TASK.md` — что делает/сделал GLM
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
