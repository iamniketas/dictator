# Следующая сессия — UI Overlay (исправление подхода)

## ✅ Выполнено (2026-01-30):

### Успехи:
1. **Баг #1 (Буфер залипает)** — ✅ ИСПРАВЛЕН И ПРОТЕСТИРОВАН
   - Тест: 3 диктовки подряд работают успешно без залипания

2. **main.rs откачен к рабочей версии**
   - Удалена багнутая streaming версия
   - Восстановлена Initial release + исправления

3. **Win32 overlay создан** — ✅ src/overlay_win32.rs
   - Агент: Kimi K2.5 (task_90bd0c92b582)
   - Файл компилируется без ошибок
   - НО: не протестирован отдельно

### Провалы и уроки:
1. **Попытка интеграции overlay провалилась** ❌
   - GLM: добавил неправильные импорты + несуществующий OverlayConfig
   - Kimi: добавил markdown pollution ("src/main.rs" как строка кода)
   - Ручное редактирование: сломало приложение (не запускается)

2. **Урок:** НИКОГДА не интегрировать непротестированный код
   - Сначала тестовый бинарник
   - Потом интеграция

### Текущее состояние:
- ✅ Компилируется (cargo check passes)
- ✅ Запись работает без залипания
- ✅ Расшифровка через Whisper работает
- ✅ src/overlay_win32.rs создан, но не протестирован
- ❌ Overlay не интегрирован в main.rs

---

## 🎯 План следующей сессии: Overlay с правильным подходом

### Принципы работы:

1. **Сначала тест, потом интеграция** — ОБЯЗАТЕЛЬНО
2. **Делегировать агентам** — никакого ручного редактирования
3. **Проверять markdown pollution** — агенты добавляют мусор
4. **Откатывать при неудаче** — не оставлять сломанный код

---

## Этап 1: Тестовый бинарник для overlay (КРИТИЧНО)

**Цель:** Проверить что Win32 overlay вообще работает отдельно от main.rs

### 1.1 Создать examples/test_overlay.rs

**Задача для Kimi:**
```
Создать examples/test_overlay.rs — простой тестовый бинарник для overlay_win32.

Требования:
1. Использовать dictator::overlay_win32::{OverlayWindow, OverlayConfig}
2. Создать окно с дефолтным конфигом
3. Показать тестовый текст: "🎤 Test overlay window"
4. Держать окно 5 секунд
5. Скрыть окно
6. Завершить программу

Код должен:
- Инициализировать logging (tracing_subscriber)
- Логировать все операции (create, show, hide)
- Обрабатывать ошибки с подробными сообщениями
- НЕ падать с паникой

Output ONLY valid Rust code, no markdown.
```

**Acceptance criteria:**
- Файл examples/test_overlay.rs создан
- cargo check passes
- cargo run --example test_overlay компилируется

**Делегировать:** Kimi (task)

### 1.2 Добавить test_overlay в Cargo.toml

**Задача для GLM:**
```
Добавить секцию [[example]] в Cargo.toml для test_overlay.

Добавить после [dependencies]:
[[example]]
name = "test_overlay"
path = "examples/test_overlay.rs"

Output ONLY valid TOML, no markdown.
```

**Acceptance criteria:**
- Секция добавлена в Cargo.toml
- cargo check passes

**Делегировать:** GLM (task)

### 1.3 Протестировать вручную

**Команды:**
```bash
# Собрать тестовый бинарник
cargo build --example test_overlay

# Запустить с логированием
RUST_LOG=debug cargo run --example test_overlay
```

**Проверить:**
1. ✅ Окно появляется в правом нижнем углу
2. ✅ Текст виден и читаем
3. ✅ Окно полупрозрачное (чёрный фон)
4. ✅ Окно исчезает через 5 секунд
5. ✅ Программа завершается без паники

**Если тест провален:**
- Смотреть логи: что вернул Win32 API
- Проверить координаты окна (GetSystemMetrics)
- Проверить регистрацию window class (RegisterClassW)
- **НЕ переходить к интеграции!**

### 1.4 Исправить overlay_win32.rs (если нужно)

**Возможные проблемы:**
- Window class не регистрируется
- Координаты окна за пределами экрана
- ShowWindow не вызывается или возвращает ошибку
- Текст не отрисовывается (проблема с GDI)

**Делегировать исправления:** Kimi (с логами из теста)

---

## Этап 2: Интеграция overlay в main.rs (только после успешного теста!)

### 2.1 Проверить API overlay_win32

**Проверить вручную:**
```bash
grep "pub fn" src/overlay_win32.rs
```

**Убедиться что есть:**
- `pub fn new(config: OverlayConfig) -> Result<Self>`
- `pub fn show(&self, text: &str)`
- `pub fn hide(&self)`

**Если нет update_text():**
- Делегировать добавление метода Kimi

### 2.2 Интегрировать в main.rs

**Задача для Kimi:**
```
Интегрировать overlay_win32 в src/main.rs.

ВАЖНО:
1. Сохранить директиву: #![windows_subsystem = "windows"]
2. Импорт: use dictator::overlay_win32::{OverlayWindow, OverlayConfig};
3. Создание после recorder:
   let overlay_config = OverlayConfig::default();
   let overlay = Arc::new(OverlayWindow::new(overlay_config)?);
4. Клонировать для потока: let overlay_clone = overlay.clone();
5. Показ после info!("FINAL TEXT: ..."):
   overlay_clone.show(&final_text);
6. Скрытие после inject_text:
   std::thread::sleep(std::time::Duration::from_secs(2));
   overlay_clone.hide();

Проверить context файлы перед генерацией:
- src/main.rs (текущая версия)
- src/overlay_win32.rs (API методы)

НЕ использовать несуществующие методы!
Output ONLY valid Rust code, no markdown.
```

**Acceptance criteria:**
- cargo check passes
- cargo build --release passes
- НЕТ markdown pollution (проверить src/main.rs на "```rust" или "src/main.rs")

**Делегировать:** Kimi (task)

### 2.3 Проверка после интеграции

**Если cargo check failed:**
```bash
# Сразу откатить
git checkout -- src/main.rs

# Посмотреть что сломалось
cargo check 2>&1 | grep "error"

# Создать новую задачу с исправлениями для Kimi
```

**Если cargo check passes:**
```bash
# Пересобрать релиз
cargo build --release

# Проверить что запускается
./target/release/dictator.exe
# Должна появиться иконка в трее!
```

### 2.4 Тестирование overlay в main.rs

**Сценарий:**
1. Запустить dictator.exe
2. Проверить иконку в трее ✅
3. Ctrl+Shift+D → начать говорить
4. Ctrl+Shift+D → остановить
5. **Должно появиться окно в правом нижнем углу**
6. Окно должно показать текст
7. Окно должно исчезнуть через 2 сек

**Если overlay не появился:**
- Смотреть логи (RUST_LOG=debug)
- Проверить что `OverlayWindow::new()` не упал
- Проверить что `show()` вызвался
- Вернуться к Этапу 1 (тестовый бинарник)

---

## Этап 3: Улучшение overlay (опционально)

### 3.1 Показывать статус записи

**Добавить:**
```rust
HotkeyEvent::RecordStart => {
    overlay_clone.show("🎤 Запись...");
    recorder_clone.start_recording()?;
}

// После transcribe_audio:
overlay_clone.show("⏳ Расшифровка...");

// После correct_text:
overlay_clone.show(&format!("📝 {}", final_text));
```

**Делегировать:** GLM (простое изменение)

### 3.2 Настройки overlay

**Добавить в Config:**
```toml
[overlay]
enabled = true
position = "bottom-right"  # или "bottom-left", "top-right", etc
width = 400
height = 120
font_size = 14.0
```

**Делегировать:** Kimi (изменение config.rs + overlay_win32.rs)

---

## 📋 Чеклист следующей сессии

### Обязательно:
- [ ] 1.1 Создать examples/test_overlay.rs (Kimi)
- [ ] 1.2 Добавить в Cargo.toml (GLM)
- [ ] 1.3 Протестировать test_overlay вручную
- [ ] 1.4 Исправить баги (если есть)
- [ ] 2.1 Проверить API overlay_win32
- [ ] 2.2 Интегрировать в main.rs (Kimi)
- [ ] 2.3 Проверить компиляцию
- [ ] 2.4 Протестировать в реальной работе

### Опционально (если время):
- [ ] 3.1 Добавить статусы записи
- [ ] 3.2 Настройки overlay через config

---

## 🔧 Команды для работы

### Запуск окружения
```bash
# Оркестратор
python orchestrator_v2.py

# Whisper
python whisper_server.py

# Ollama
ollama serve
```

### Тестирование overlay
```bash
# Собрать тест
cargo build --example test_overlay

# Запустить с логами
RUST_LOG=debug cargo run --example test_overlay

# Если не работает — смотреть логи
```

### Интеграция
```bash
# Проверить компиляцию
cargo check

# Собрать релиз
cargo build --release

# Тестировать
./target/release/dictator.exe
```

### Откат при провале
```bash
# Откатить конкретный файл
git checkout -- src/main.rs

# Откатить все изменения
git checkout -- .
```

---

## ⚠️ Критичные правила

### 1. НИКОГДА не интегрировать непротестированный код
- Сначала test_overlay должен работать
- Только потом интеграция в main.rs

### 2. ВСЕГДА проверять markdown pollution
```bash
# После генерации агентом проверить:
head -10 <file>

# Искать мусор:
# - "```rust"
# - "src/file.rs" как строка кода
# - "// file.rs"
```

### 3. ВСЕГДА откатывать при провале
```bash
# Если cargo check failed или приложение не запускается:
git checkout -- <file>
cargo build --release
# Проверить что работает!
```

### 4. НЕ редактировать код вручную
- Только через агентов
- Даже если кажется "простым исправлением"
- Цена одной ошибки — сломанное приложение

---

## 📁 Файлы проекта

- `src/main.rs` — ✅ Рабочая версия (БЕЗ overlay)
- `src/audio.rs` — ✅ ИСПРАВЛЕН (buffer.clear + get_unprocessed_buffer)
- `src/overlay.rs` — ⚠️ Старый код (winit), не используется
- `src/overlay_win32.rs` — ✅ Создан, НО НЕ ПРОТЕСТИРОВАН
- `src/llm.rs` — ✅ ИСПРАВЛЕН (логирование Ollama)
- `src/transcribe.rs` — ✅ Работает + есть async версия
- `examples/test_overlay.rs` — ❌ НЕ СОЗДАН (нужно создать!)

---

## 🎯 Ожидаемый результат после сессии

### Минимум (обязательно):
- examples/test_overlay.rs работает ✅
- Окно показывается в правом нижнем углу ✅
- Текст читаем ✅

### Оптимум (цель):
- overlay интегрирован в main.rs ✅
- Приложение запускается ✅
- Overlay показывается после диктовки ✅

### Максимум (если успеем):
- Статусы записи ("🎤 Запись...", "⏳ Расшифровка...")
- Настройки overlay в config.toml

---

## 💡 Советы для следующей сессии

1. **Начни с теста** — не пропускай test_overlay!
2. **Проверяй логи** — включай RUST_LOG=debug
3. **Откатывай смело** — лучше откатить и переделать
4. **Доверяй агентам** — но проверяй markdown pollution
5. **Не торопись** — лучше потратить время на тест, чем чинить сломанное

Удачи! 🚀
