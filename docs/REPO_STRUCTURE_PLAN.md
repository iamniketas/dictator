# Repo Structure Evolution Plan (Windows + macOS)

## Задача

Перейти к мультиплатформенной структуре (Windows + macOS) так, чтобы:

- Windows сборка продолжала проходить на каждом шаге.
- Новый macOS клиент развивался независимо.
- В репозитории не возникло хаоса в зависимостях и CI.

## Целевое состояние

```text
dictator/
  apps/
    windows/          # Текущий Rust клиент (после мягкой миграции)
    macos/            # Новый SwiftUI + AppKit клиент
  shared/
    contracts/        # JSON schema, примеры payload, state-модель
    docs/             # cross-platform решения и ADR
  tools/
    scripts/          # helper scripts для dev/release
  .github/workflows/
    windows.yml
    macos.yml
```

## Почему не переносить всё сразу

Резкий перенос текущего Rust приложения из корня в `apps/windows` создаёт риск:

- сломать команды пользователей (`cargo build` из root),
- сломать release scripts,
- получить шум в истории PR.

Лучше идти итеративно.

## Стратегия миграции без поломки Windows

### Шаг 1 (сейчас): Документация и фикс контракта

- Добавить roadmap и структурный план.
- Зафиксировать единые продуктовые контракты:
  - состояния pipeline,
  - config keys,
  - output modes (inject/clipboard).

**Риск для Windows:** нулевой.

### Шаг 2: Добавить `apps/macos` без трогания Rust-корня

- Создать `apps/macos` и вести разработку там.
- Текущий Windows клиент остаётся в root до стабилизации процесса.

**Риск для Windows:** минимальный (изоляция).

### Шаг 3: Ввести CI matrix по платформам

- `windows.yml`: `cargo check`, `cargo test`, release profile smoke.
- `macos.yml`: `xcodebuild`/`swift build` для macOS клиента.
- Обязательные статус-проверки для merge.

**Риск для Windows:** контролируемый, потому что windows pipeline отдельный и обязательный.

### Шаг 4: Мягкий перенос Windows в `apps/windows`

- Перенести код Windows клиента из root в `apps/windows`.
- В root оставить:
  - короткий bootstrap `README`,
  - ссылки на платформенные инструкции.
- Добавить совместимый alias-скрипт (например, `tools/scripts/build-windows.ps1`).

**Риск для Windows:** средний, поэтому делать только после зелёного CI на нескольких PR.

### Шаг 5: Shared contracts и ADR

- Создать `shared/contracts`:
  - `config.schema.json`,
  - `pipeline_events.schema.json`.
- Создать ADR-документы по спорным решениям:
  - hotkey strategy per OS,
  - text injection per OS,
  - transcription engine choices.

**Риск для Windows:** низкий, улучшает прозрачность.

## Правила, чтобы ничего не ломалось

1. До Шага 4 не менять текущий Windows entrypoint и команды сборки.
2. Любое изменение в Rust-ядре проверять на Windows CI.
3. Не объединять рефакторинг структуры и функциональные фичи в одном PR.
4. Перед крупным переносом делать freeze-ветку с быстрым rollback.

## Рекомендуемая политика веток и релизов

- Основная ветка: `main`.
- Платформенные инициативы:
  - `feature/macos-*`,
  - `feature/windows-*`.
- Теги:
  - `windows/vX.Y.Z`,
  - `macos/vX.Y.Z`.

Это позволит релизить платформы независимо.

## Что оптимизировать в кодовой базе далее

1. Отделить platform-independent pipeline интерфейсами (`trait`) от Win32 реализации.
2. Ограничить прямые вызовы Win32 в узких адаптерах.
3. Вынести общие настройки в единый schema-контракт.
4. Подготовить integration test сценарии, общие для обеих платформ (на уровне логики, не UI).

## Ближайшие практические шаги

1. Создать `apps/macos` каркас.
2. Добавить минимальный `windows.yml` CI для текущего Rust клиента.
3. После первого рабочего macOS MVP — перейти к Шагу 4 (перенос Windows в `apps/windows`).
