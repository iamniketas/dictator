# Streaming Transcription — План реализации

> **Статус:** ✅ РЕАЛИЗОВАНО (v0.2.0-alpha, 2026-01-30)

---

## ✅ Что было реализовано

### Архитектура (v0.2.0)

```
Recording Thread (cpal)
    ↓
Audio Buffer (растёт)
    ↓
ChunkDetector (каждые 3 сек)
    ↓
Whisper HTTP Request (в отдельном потоке)
    ↓
Partial Text → Overlay UI (в реальном времени)
    ↓
При остановке: финальная склейка + LLM коррекция + Text Injection
```

### Реализованные компоненты

#### 1. Streaming Module (`src/streaming.rs`) ✅
- **Synchronous polling** — упрощённая версия без async/tokio
- Поток опрашивает аудио буфер каждые 3 секунды
- Отправляет partial results в main thread через channel
- Корректная остановка с получением final text

```rust
pub struct StreamingTranscriber {
    event_tx: mpsc::Sender<StreamingEvent>,
    stop_signal: Arc<AtomicBool>,
    thread_handle: Option<JoinHandle<()>>,
    language: String,
}
```

#### 2. Audio Module (`src/audio.rs`) ✅
- `get_unprocessed_buffer()` — получение буфера без остановки записи
- Отслеживание последнего обработанного индекса
- Конвертация в 16kHz mono
- Корректная очистка буфера при `Start`

#### 3. Overlay UI (`src/overlay_win32.rs`) ✅
- Потокобезопасное обновление текста
- REC индикатор с анимацией (пульсация каждую секунду)
- Поддержка partial text и final text
- Win32 GDI рендеринг

#### 4. Интеграция в `main.rs` ✅
- Создание `StreamingTranscriber` при `RecordStart`
- Обработка `PartialText` и `FinalText` событий
- Fallback на обычную транскрипцию если стриминг выключен

---

## Конфигурация

```toml
[streaming]
enabled = false        # Включить стриминг
poll_interval = 3      # Интервал polling (сек)
```

---

## Поток данных

### При старте записи
```rust
HotkeyEvent::RecordStart => {
    accumulated_text.clear();
    streaming_transcriber = Some(StreamingTranscriber::new(...));
    streaming_transcriber.start(recorder.clone());
}
```

### Во время записи (каждые 3 сек)
```rust
// В потоке стриминга:
1. Получить буфер: recorder.get_unprocessed_buffer()
2. Отправить в Whisper: transcribe_audio(audio_chunk, language)
3. Отправить результат: event_tx.send(PartialText(text))

// В main thread:
StreamingEvent::PartialText(text) => {
    accumulated_text = text;
    overlay.update_partial_text(&text);
}
```

### При остановке записи
```rust
HotkeyEvent::RecordStop => {
    streaming_transcriber.stop();  // Ждём завершения потока
    
    // Получаем final text с таймаутом
    while timeout_not_reached {
        match streaming_rx.recv_timeout(...) {
            FinalText(text) => accumulated_text = text,
            ...
        }
    }
    
    // Коррекция через Ollama
    // Вставка текста
}
```

---

## Результаты тестирования

### ✅ Работает
- Стриминг собирается без ошибок
- Partial text отображается в overlay
- Нет залипания буфера между записями
- Корректная финальная транскрипция
- LLM коррекция работает

### ⚠️ Известные ограничения
- Фиксированный интервал 3 сек (не VAD)
- Нет склейки partial results (показывается только текущий чанк)
- REC анимация — простое toggle каждую секунду

---

## История изменений

### v0.2.0-alpha (2026-01-30)
- ✅ Первая рабочая версия стриминга
- ✅ Синхронный polling вместо async
- ✅ Интеграция с overlay
- ✅ Исправлен баг с залипанием буфера

### v0.1.0-alpha (ранее)
- Базовая архитектура без стриминга
- Транскрипция только после остановки

---

## Решения

### Почему синхронный polling вместо async?
- **Простота** — меньше кода, легче отлаживать
- **Стабильность** — нет проблем с tokio runtime
- **Достаточность** — 3 секунды приемлемая задержка для диктовки

### Как обрабатывать partial results?
- Накапливаем текст в `accumulated_text`
- Показываем только текущий чанк в overlay
- При остановке используем накопленный текст

---

*Документ обновлён: 2026-01-30*
