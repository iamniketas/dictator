# Orchestrator v2 — Руководство по запуску

## Что изменилось

### Старая схема (не работала)
```
Sonnet: "GLM, сделай X" → ждёт → получает ответ → проверяет → коммитит
         ↑_________________________________________↓
                    Sonnet делает всё
```

### Новая схема (автономная)
```
Sonnet: Создаёт задачу → Запускает GLM (фон) → Ждёт
                              ↓
                         GLM работает сам
                         (пишет в файлы, тестирует)
                              ↓
                         GLM завершил
                              ↓
Sonnet: Проверяет watermark → Делегирует Kimi (ревью)
                              ↓
                         Kimi проверяет, финализирует
                              ↓
Sonnet: Показывает сводку → Ждёт решения пользователя
```

---

## Быстрый старт

### 1. Остановить старый orchestrator
```bash
# Если запущен на порту 8000
pkill -f "orchestrator.py"
```

### 2. Запустить новый orchestrator
```bash
cd /path/to/dictator
python orchestrator_v2.py
```

### 3. Проверить работу
```bash
curl http://localhost:8000/health
# {"status": "ok", "features": ["background_tasks", "watermarking", ...]}
```

### 4. Установить git hook (защита от ручных правок)
```bash
# Для Linux/Mac:
chmod +x .git/hooks/pre-commit

# Для Windows (PowerShell):
# Hook работает автоматически через .git/hooks/pre-commit
```

---

## Пример рабочего процесса

### Шаг 1: Sonnet создаёт задачу
```bash
curl -X POST "http://localhost:8000/task/create" \
  -H "Content-Type: application/json" \
  -d '{
    "task": "Implement ChunkBuffer for 500ms audio windows",
    "context_files": ["src/audio.rs", "Cargo.toml"],
    "acceptance_criteria": [
      "ChunkBuffer::new(500) creates buffer",
      "push() adds samples",
      "cargo test chunks passes"
    ],
    "validation_command": "cargo test chunks --quiet",
    "output_file": "src/chunks.rs",
    "agent_type": "glm"
  }'

# Ответ: {"task_id": "task_a1b2c3...", "status": "pending"}
```

### Шаг 2: Sonnet запускает фоновое выполнение
```bash
curl -X POST "http://localhost:8000/task/task_a1b2c3/delegate?background=true"

# Ответ: {"status": "running", "mode": "background", ...}
```

### Шаг 3: Sonnet ждёт и проверяет статус
```bash
# Каждые 2-5 минут:
curl http://localhost:8000/task/task_a1b2c3/status

# Ответ:
# {
#   "status": "running",
#   "progress_percent": 60,
#   "current_step": "Generating code"
# }
```

### Шаг 4: GLM завершил → Sonnet запрашивает отчёт
```bash
curl http://localhost:8000/task/task_a1b2c3/report

# Ответ содержит:
# - summary: что сделано
# - detailed_log: пошаговые логи
# - files_generated: ["src/chunks.rs"]
# - agent_watermarks: верификация
# - validation_results: прошли ли тесты
```

### Шаг 5: Sonnet проверяет watermark
```bash
# Обязательная проверка перед отчётом пользователю:
head -1 src/chunks.rs
# Должно быть: // AGENT: glm | TASK: task_a1b2c3 | TIMESTAMP: ...

# Или через API:
curl http://localhost:8000/task/task_a1b2c3/verify
```

### Шаг 6: Sonnet делегирует Kimi на ревью
```bash
curl -X POST "http://localhost:8000/task/create" \
  -H "Content-Type: application/json" \
  -d '{
    "task": "Review and finalize src/chunks.rs from task_a1b2c3",
    "context_files": ["src/chunks.rs", "src/audio.rs"],
    "acceptance_criteria": [
      "Code quality acceptable",
      "No race conditions",
      "Documentation complete"
    ],
    "agent_type": "kimi"
  }'
```

---

## Защиты от "новорства" Sonnet

### 1. Git Hook (технический барьер)
При коммите проверяет marker в файлах:
```bash
$ git commit -m "fix"
# ❌ BLOCKED: src/chunks.rs
#    Missing AGENT marker — did you edit this manually?
```

### 2. Verify Script (для Sonnet)
```bash
# Linux/Mac:
./.claude/verify-origin.sh src/chunks.rs

# Windows PowerShell:
.claude\verify-origin.ps1 src/chunks.rs

# Проверить все:
./.claude/verify-origin.sh --all
```

### 3. Watermark Endpoint
```bash
curl http://localhost:8000/task/task_a1b2c3/verify
# {
#   "verification": {"verified": true, "agent": "glm", ...},
#   "sonnet_safe": true
# }
```

---

## Sonnet: Твой новый рабочий процесс

### Когда пользователь просит фичу:

**❌ Старый способ (запрещён):**
```
"Понял, посмотрю код... [читает src/*.rs] ...
А, вот проблема, исправлю... [Edit] ... Готово!"
```

**✅ Новый способ (обязателен):**
```
"Понял. Разбираю на задачи:
- Task 1: GLM — chunks.rs (15 мин)
- Task 2: Kimi — review (10 мин)

Запускаю GLM в фоне..."

[через 15 мин]
"GLM завершил. Проверяю watermark... ✅
Запускаю Kimi на review..."

[через 10 мин]
"Kimi завершил review. Сводка:
- Код: ✅ проходит тесты
- Качество: ✅ одобрено
- Файл: src/chunks.rs

Релизим?"
```

### Твои инструменты:

| Что нужно | Как делать | Запрещено |
|-----------|-----------|-----------|
| Создать код | Task → GLM (фон) | Edit/Write в src/ |
| Проверить качество | Task → Kimi (фон) | Читать код самому |
| Проверить статус | curl /task/{id}/status | Постоянно спрашивать пользователя |
| Отчитаться | curl /task/{id}/report | Показывать сырой код |
| Исправить ошибку | Task → Kimi ("Fix task_X") | Исправить самому |

---

## API Endpoints

### Основные
```
POST   /task/create              # Создать задачу
POST   /task/{id}/delegate       # Запустить (background=true для фона)
GET    /task/{id}/status         # Текущий статус + прогресс
GET    /task/{id}/report         # Детальный отчёт
GET    /task/{id}/verify         # Проверить watermark
GET    /tasks                    # Список задач
GET    /health                   # Проверка работы
```

### Формат Report Response
```json
{
  "task_id": "task_a1b2c3",
  "status": "completed",
  "agent_type": "glm",
  "summary": "## Task Report: task_a1b2c3...",
  "detailed_log": [
    {"timestamp": "...", "level": "INFO", "step": "INIT", "message": "Starting..."},
    {"timestamp": "...", "level": "SUCCESS", "step": "COMPLETE", "message": "Done"}
  ],
  "files_generated": ["src/chunks.rs"],
  "validation_results": {"passed": true, ...},
  "agent_watermarks": [{"verified": true, "agent": "glm", ...}],
  "time_elapsed_seconds": 847
}
```

---

## Проверка перед использованием

```bash
# 1. Orchestrator запущен?
curl http://localhost:8000/health

# 2. Git hook установлен?
cat .git/hooks/pre-commit | head -5
# Должно быть: "# Agent Origin Verification"

# 3. Verify scripts есть?
ls .claude/verify-origin.sh      # Linux/Mac
ls .claude/verify-origin.ps1     # Windows

# 4. Conductor prompt обновлён?
cat .claude/conductor-prompt.md | head -10
```

---

## Частые проблемы

### "Git hook блокирует мои коммиты"
**Причина:** Вы редактировали файл вручную, нет AGENT marker.
**Решение:** Не редактируйте src/*.rs вручную. Всё через orchestrator.

### "GLM долго работает, нет статуса"
**Причина:** Фоновая задача не обновляет статус.
**Решение:** orchestrator_v2.py исправляет это — обновление каждые 10%.

### "Sonnet всё равно читает код"
**Причина:** Привычка.
**Решение:** Читайте только validation_report и task/{id}/report.

---

## Готовность к запуску

- [ ] orchestrator_v2.py запущен на :8000
- [ ] Git hook установлен (pre-commit)
- [ ] Verify scripts на месте
- [ ] CLAUDE.md обновлён с запретами
- [ ] .claude/conductor-prompt.md создан
- [ ] Ollama запущен с glm-4.7-flash
- [ ] Kimi API ключ работает

**Когда всё готово — Sonnet может начать работу как дирижёр!**
