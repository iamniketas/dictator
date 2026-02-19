# Dictator macOS Roadmap (SwiftUI + AppKit)

## Цели

- Нативный macOS UX по Apple HIG (menu bar, settings, permissions, keyboard shortcuts).
- Максимальная производительность транскрибации на Apple Silicon (M1-M5).
- Сохранение функционального паритета с Windows по ключевому pipeline.

## Технические принципы

1. Платформенный UI и системная интеграция пишутся нативно на SwiftUI + AppKit.
2. Тяжёлая обработка аудио и ML работает off-main-thread.
3. Локальная транскрибация оптимизируется под Metal/ANE, а не переносится 1:1 из Windows-стека.
4. API-поведение продукта остаётся единым: горячая клавиша, запись, транскрибация, опциональная коррекция, вставка текста.

## Рекомендуемый стек для macOS

- UI: `SwiftUI` + `AppKit` bridge (`NSStatusItem`, `NSMenu`, window management).
- Audio capture: `AVAudioEngine` / `AVAudioSession`-style поток с буферизацией.
- Transcription engine (приоритет):
  1. `WhisperKit` (Core ML, Metal/ANE-friendly).
  2. `whisper.cpp` с Metal/Core ML как fallback.
- LLM correction: Ollama HTTP-клиент (как в Windows, для согласованного UX).
- Text insertion:
  - primary: pasteboard + synth `Cmd+V` через `CGEvent`,
  - permissions: Accessibility + Microphone.

## Архитектура macOS клиента

```text
MenuBar App (SwiftUI/AppKit)
    -> HotkeyManager (Carbon/EventTap)
    -> AudioCaptureService (AVAudioEngine)
    -> TranscriptionService (WhisperKit/whisper.cpp)
    -> OptionalCorrectionService (Ollama HTTP)
    -> TextOutputService (Pasteboard + CGEvent)
    -> SettingsStore (UserDefaults + config file)
```

## Этапы реализации

### Phase 0: Foundation (MVP skeleton)

- Создать `apps/macos/` с Xcode project или Swift package app.
- Реализовать menu bar icon + menu items + settings window.
- Добавить permission flow:
  - Microphone,
  - Accessibility (для text injection и глобального hotkey).
- Добавить наблюдаемую модель состояния приложения (recording/transcribing/error).

**Критерий готовности:** приложение запускается как menu bar app, показывает нативные экраны, проходит запросы permissions.

### Phase 1: Record + Stop + Local Transcript

- Реализовать запись микрофона в ring buffer.
- Подключить `TranscriptionService` через WhisperKit.
- Реализовать режим "полная транскрибация после stop".
- Вывести результат в окно статуса и copy-to-clipboard.

**Критерий готовности:** после hotkey приложение пишет аудио и возвращает текст локально.

### Phase 2: Text Injection + Hotkey polish

- Добавить надёжный глобальный hotkey.
- Реализовать вставку текста в активное приложение.
- Добавить fallback при отказе в Accessibility permission.

**Критерий готовности:** end-to-end сценарий "говорю -> получаю вставленный текст" работает стабильно.

### Phase 3: Streaming mode

- Реализовать chunk/stream pipeline.
- Показывать partial results в overlay/status panel.
- Поддержать переключение streaming chunk (3/8/15s или adaptive).

**Критерий готовности:** realtime partial text без подвисаний UI.

### Phase 4: Quality + Performance tuning

- Профилирование через Instruments:
  - Time Profiler,
  - Allocations,
  - Energy Log.
- Подбор compute mode (CPU/GPU/ANE) и размера чанков.
- Стабилизация latency budget и memory budget.

**Критерий готовности:** стабильный UX на M1 base и M3/M4 class devices.

## Производительность Apple Silicon: практические правила

1. Не запускать inference на main actor.
2. Использовать prewarm модели при старте приложения.
3. Обрабатывать аудио потоками fixed-size chunks.
4. Снижать лишние копирования буферов.
5. Для длительной записи использовать bounded ring buffer, чтобы держать memory под контролем.

## Совместимость с Windows версией

- Поведенческий контракт сохраняется:
  - Hotkey start/stop,
  - Streaming toggle,
  - Optional LLM correction,
  - Text output into active app.
- UI/UX намеренно платформенно-нативный и может отличаться по layout/controls.

## Риски и смягчение

- Разрешения macOS могут ломать первый UX:
  - добавить onboarding экран с понятным чеклистом.
- Разные модели устройств M1-M5 дают разные performance профили:
  - хранить пресеты профилей и auto-tune.
- Вставка текста в sandboxed приложения:
  - fallback на clipboard-only режим.

## План ближайших итераций

1. Собрать `apps/macos` skeleton (menu bar + settings + permissions).
2. Подключить local transcription на одном языке и одном качестве модели.
3. Внедрить text injection с корректным failover.
4. После стабилизации включить streaming mode.
