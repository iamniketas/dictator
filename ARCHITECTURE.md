# Dictator — Архитектура и План Реализации

## Обзор системы координации разработки

```
┌─────────────────────────────────────────────────────────────┐
│                    OPUS 4.5 (Архитектор)                    │
│  - Планирование и декомпозиция задач                        │
│  - Code review и оптимизация                                │
│  - Решение архитектурных вопросов                           │
│  - Создание задач в TASKS.md                                │
└─────────────────────┬───────────────────────────────────────┘
                      │ Задачи с инструкциями
                      ▼
┌─────────────────────────────────────────────────────────────┐
│              LOCAL MODEL (Исполнитель)                      │
│  - Читает текущую задачу из TASKS.md                        │
│  - Выполняет по инструкции                                  │
│  - Отмечает выполнение, пишет заметки                       │
│  - При проблемах — описывает в TASKS.md для Opus            │
└─────────────────────────────────────────────────────────────┘
```

## Структура проекта

```
dictator/
├── Cargo.toml                 # Главный manifest
├── Cargo.lock
├── CLAUDE.md                  # Инструкции для Claude
├── ARCHITECTURE.md            # Этот файл
├── TASKS.md                   # Текущие задачи для локальной модели
├── src/
│   ├── main.rs                # Entry point, инициализация
│   ├── lib.rs                 # Re-exports
│   ├── app.rs                 # Главный Application state
│   ├── config.rs              # Конфигурация, горячие клавиши
│   ├── audio/
│   │   ├── mod.rs
│   │   ├── capture.rs         # Захват аудио с микрофона (cpal)
│   │   └── vad.rs             # Voice Activity Detection
│   ├── transcribe/
│   │   ├── mod.rs
│   │   └── whisper.rs         # Whisper.cpp binding
│   ├── llm/
│   │   ├── mod.rs
│   │   └── ollama.rs          # Ollama API client
│   ├── ui/
│   │   ├── mod.rs
│   │   ├── tray.rs            # System tray icon
│   │   ├── overlay.rs         # Overlay window near cursor
│   │   └── animation.rs       # Recording indicator animation
│   ├── input/
│   │   ├── mod.rs
│   │   ├── hotkey.rs          # Global hotkey registration
│   │   └── inject.rs          # Text injection (SendInput)
│   └── pipeline/
│       ├── mod.rs
│       └── streaming.rs       # Audio → Whisper → LLM → Text pipeline
├── assets/
│   ├── icon.ico               # Tray icon
│   └── recording.ico          # Recording indicator
└── tests/
    └── integration/
```

## Технический стек

| Компонент | Crate | Версия | Назначение |
|-----------|-------|--------|------------|
| Windows API | `windows` | 0.58+ | Hotkeys, tray, overlay |
| Audio capture | `cpal` | 0.15+ | Кроссплатформенный аудио |
| Whisper | `whisper-rs` | 0.13+ | Binding к whisper.cpp |
| HTTP client | `reqwest` | 0.12+ | Ollama API |
| Async runtime | `tokio` | 1.40+ | Async I/O |
| Serialization | `serde` + `toml` | - | Конфигурация |
| Logging | `tracing` | 0.1+ | Structured logging |
| Channels | `crossbeam-channel` | 0.5+ | Lock-free очереди |

## Фазы реализации

### Фаза 1: Скелет приложения (MVP-0)
1. Cargo проект с зависимостями
2. System tray icon (появляется, можно закрыть)
3. Global hotkey регистрация
4. Базовая конфигурация из TOML

### Фаза 2: Аудио pipeline (MVP-1)
5. Захват аудио с микрофона
6. Streaming в ring buffer
7. Интеграция whisper-rs
8. Базовая транскрипция

### Фаза 3: UI overlay (MVP-2)
9. Layered window создание
10. Отрисовка текста
11. Recording indicator анимация
12. Позиционирование у курсора

### Фаза 4: LLM коррекция (MVP-3)
13. Ollama API client
14. Streaming коррекция текста
15. Финальная вставка текста

### Фаза 5: Polish
16. Настройки в tray menu
17. Выбор микрофона
18. Кастомные hotkeys
19. Словарь пользователя
