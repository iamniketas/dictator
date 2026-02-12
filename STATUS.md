# Dictator — Текущий статус проекта

> **Дата:** 2026-01-30  
> **Версия:** v0.2.1  
> **Статус:** 🟢 Работает стабильно

---

## ✅ Что работает

### Ядро приложения
| Компонент | Статус | Примечание |
|-----------|--------|------------|
| Системный трей | ✅ | Иконка, меню Exit, переключатель "Стриминг" |
| Глобальный hotkey | ✅ | Ctrl+Shift+D |
| Аудио запись | ✅ | cpal, 16kHz mono, очистка буфера |
| Whisper транскрипция | ✅ | HTTP запросы к whisper_server.py |
| LLM коррекция | ✅ | Ollama API (опционально) |
| Вставка текста | ✅ | В активное окно + буфер обмена |

### Стриминг (v0.2.1)
| Функция | Статус | Примечание |
|---------|--------|------------|
| Переключатель в трее | ✅ | Галочка "Стриминг" |
| ChunkDetector | ✅ | Каждые 3 секунды (polling) |
| Параллельная транскрипция | ✅ | В отдельном потоке |
| Partial results | ✅ | Накапливающийся текст |
| Отображение в overlay | ✅ | Прокрутка последних 5 строк |
| Финальная обработка | ✅ | Последний чанк не теряется |

### Overlay UI
| Функция | Статус | Примечание |
|---------|--------|------------|
| Win32 окно | ✅ | winit + GDI |
| Прозрачность | ✅ | Layered window |
| Перемещение мышью | ✅ | Drag & drop |
| REC индикатор | ✅ | Пульсирующий круг |
| Прокрутка текста | ✅ | Последние 5 строк |
| Позиционирование | ✅ | Над курсором |

---

## ⚠️ Известные проблемы

### Некритичные
- [x] Неиспользуемые импорты ✅
- [x] Необработанные `Result` ✅
- [x] Deprecated поля ✅

### Особенности
- Стриминг: текст собран из чанков → может быть нарушена пунктуация
- Полная транскрипция: 2.6с vs 0.1с стриминга, но лучше качество

---

## 📁 Структура проекта

```
src/
├── main.rs           # Точка входа, event loop
├── lib.rs            # Модульные экспорты
├── audio.rs          # Аудио запись (cpal)
├── streaming.rs      # Стриминг транскрипции
├── transcribe.rs     # Whisper HTTP клиент
├── llm.rs            # Ollama клиент
├── input.rs          # Hotkey + вставка текста
├── config.rs         # Конфигурация (TOML)
├── ui.rs             # Системный трей + меню
└── overlay_win32.rs  # Overlay с прокруткой

examples/
├── test_overlay.rs   # Тест overlay
└── benchmark.rs      # Бенчмарк скорости
```

---

## 🔧 Конфигурация

```toml
[streaming]
enabled = false        # Начальное состояние (можно менять в трее)
poll_interval = 3      # Интервал polling (сек)

[ollama]
enabled = false        # LLM коррекция (влияет на скорость)
url = "http://localhost:11434"
model = "glm-4.7-flash"
```

---

## 🚀 Режимы работы

| Режим | Скорость | Качество | Когда использовать |
|-------|----------|----------|-------------------|
| **Стриминг ON** ☑ | 0.1s | Среднее | Быстрые команды, черновики |
| **Стриминг OFF** ☐ | 2.6s | Высокое | Письма, документы, коммуникация |

**Переключение:** Правый клик на иконку трея → "Стриминг"

---

## 📊 Метрики (v0.2.1)

| Параметр | Значение |
|----------|----------|
| Версия | v0.2.1 |
| Размер бинарника | ~4.8 MB |
| Время запуска | ~1-2 сек |
| Стриминг (hotkey→текст) | ~0.1s |
| Полная транскрипция | ~2.6s |
| Ускорение стриминга | 18x |

---

## 📝 Примечания

- Стриминг: синхронный polling каждые 3 сек
- Overlay: потокобезопасный, прокрутка последних 5 строк
- Буфер чистится между записями (нет залипания)
- Windows only (Win32 API)

---

*Последнее обновление: 2026-01-30*

---

## 2026-02-12 Update (v0.2.2 candidate)

- Verified repository sync before changes: `main` and `origin/main` were equal at `bf36b6b`.
- Overlay status improved:
  - During recording shows elapsed seconds and estimated buffer size in MB.
  - During non-streaming transcription shows live progress with elapsed time and ETA.
  - After transcription briefly shows stats: words and characters.
- Tray menu improved:
  - Streaming chunk length can now be selected: `3s`, `8s`, `15s`.
  - Selected chunk length is used for next and subsequent streaming sessions.

## 2026-02-12 Update (v0.2.3)

- Streaming/overlay UX fixes:
  - Recording no longer blocks UI while Whisper warmup is in progress.
  - Overlay now has two independent zones: status (top) and text (bottom).
  - Streaming status and last partial text are shown simultaneously.
- Overlay visibility improved:
  - Increased background opacity (nearly opaque).
  - Added border for readability on bright backgrounds.
- Streaming defaults:
  - Default chunk duration changed to 15s.
  - Tray chunk fallback default is now 15s.
