<p align="center">
  <img src="assets/banner.svg" alt="whycodes — быстрый, независимый от поставщика кодирующий агент для терминала, написанный на Rust" width="720">
</p>

<p align="center">
  <a href="https://olud.ai/tool/whycodes.html"><img src="https://olud.ai/badge.php?tool=whycodes" alt="olud.ai"></a>
  <a href="https://github.com/whycorporation/whycodes/actions/workflows/ci.yml"><img src="https://github.com/whycorporation/whycodes/actions/workflows/ci.yml/badge.svg" alt="CI"></a>
  <a href="https://github.com/whycorporation/whycodes/releases"><img src="https://img.shields.io/github/v/release/whycorporation/whycodes" alt="Release"></a>
  <a href="LICENSE"><img src="https://img.shields.io/badge/license-MIT-blue.svg" alt="License: MIT"></a>
  <a href="https://x.com/whycodesai"><img src="https://img.shields.io/badge/X-@whycodesai-black" alt="Подписаться на @whycodesai в X"></a>
  <a href="https://github.com/sponsors/whycorporation"><img src="https://img.shields.io/badge/sponsor-whycorporation-ea4aaa" alt="Спонсировать WhyCodes"></a>
</p>

<p align="center">
  <a href="README.md">English</a> · <a href="README.zh-CN.md">简体中文</a> · Русский · <a href="README.es.md">Español</a>
</p>

**Быстрый, независимый от поставщика кодирующий агент для терминала, написанный на Rust.**

WhyCodes читает, пишет и правит файлы, запускает команды, ищет по рабочей
области и управляет LLM через многоходовое использование инструментов —
в полноэкранном TUI или как машиночитаемый одноразовый CLI. Акцент на
небольшой нативный размер, экономный в простое TUI и рабочий процесс,
не привязанный к одному поставщику моделей.

<p align="center">
  <img src="assets/tui-home.svg" alt="Главный экран TUI whycodes" width="820">
</p>

## Особенности

- **Один нативный бинарник.** Нет обязательных runtime-зависимостей. Поиск
  выполняется в процессе (`ripgrep` не нужен); опциональные интеграции ОС,
  например Linux bubblewrap, усиливают песочницу, когда доступны.
- **Кроссплатформенность.** Один и тот же процесс на Linux, macOS и Windows.
  Linux запускается на каждом изменении CI; задания Windows и macOS доступны
  через мульти-ОС матрицу CI.
- **Любой поставщик.** Anthropic, OpenAI, Google, GitHub Copilot, Groq, xAI,
  DeepSeek, Ollama, OpenRouter, Mistral, Together или любой OpenAI-совместимый
  endpoint — с API-ключом или входом по подписке (`whycodes auth login`).
- **Под ваш проект.** Загружает инструкции проекта из `AGENTS.md` и
  подключается к существующим MCP-инструментам и языковым серверам по LSP.
- **Помнит вашу работу.** Сохраняемые сессии, память между сессиями
  (`MEMORY.md`, семантический recall) и опциональный индекс кода.

## Установка

### Скрипт установки (Linux, macOS)

```bash
curl -fsSL https://why.codes/install | bash
```

### Скрипт установки (Windows)

```powershell
irm https://why.codes/install.ps1 | iex
```

Добавляет `%LOCALAPPDATA%\Programs\whycodes` в пользовательский PATH. WSL
не требуется.

### Homebrew (macOS; Linuxbrew x86_64)

```bash
brew tap whycorporation/whycodes https://github.com/whycorporation/whycodes
brew install whycorporation/whycodes/whycodes
```

Homebrew 6+ отклоняет недоверенные сторонние tap. Полностью квалифицированная
установка доверяет только этой formula. Если tap уже добавлен и вы увидели
эту ошибку:

```bash
brew trust --formula whycorporation/whycodes/whycodes
brew install whycodes
```

### Из исходников

```bash
# Нужен libsqlite3 (pkg-config). Полностью статический бинарник:
# cargo build --release -p whycodes-cli --features bundled-sqlite
cargo build --release -p whycodes-cli
```

Обновление: `whycodes upgrade` (установка скриптом / cargo) или `brew upgrade
whycodes` (Homebrew). Интерактивные сессии TUI проверяют GitHub на новую
версию и спрашивают на главном экране перед установкой. Передайте
`--no-auto-update`, чтобы пропустить запрос. Скрипты установки сверяют
артефакты релиза с опубликованным `SHA256SUMS`. Скачиваемые бинарники и
инструкции по удалению:
[docs/packaging.md](docs/packaging.md).

<details>
<summary>Дополнения оболочки</summary>

```bash
eval "$(whycodes completions zsh)"    # ~/.zshrc
eval "$(whycodes completions bash)"   # ~/.bashrc
whycodes completions fish > ~/.config/fish/completions/whycodes.fish
```

</details>

## Быстрый старт

```bash
export ANTHROPIC_API_KEY="sk-ant-..."

whycodes -d ./my-project                            # интерактивный TUI
whycodes generate "Explain main.rs" -d ./my-project # одноразовый запуск
whycodes generate "Summarize the last commit" --format json
whycodes --continue                                 # продолжить последнюю сессию
whycodes -P openai -m gpt-4o generate "Refactor this module"
```

OAuth-вход по подписке (`whycodes auth login <provider>`) доступен только
после установки локального плагина `kind: "auth"` — WhyCodes не поставляет
сторонние OAuth-клиенты. См. [docs/auth.md](docs/auth.md).

Полное руководство — справочник CLI, клавиши TUI, slash-команды, агенты,
инструменты и конфигурация — в **[docs/guide.md](docs/guide.md)**.

## Возможности

| | |
|---|---|
| **Агенты** | Основные: `build` (полный доступ), `plan` и `ask` (только чтение); субагенты `general`, `explore` и `scout` через инструмент `task`; параллельные воркеры в git worktree через `swarm` |
| **Инструменты** | Правка/патч файлов, поиск в процессе, shell, git и GitHub, веб-загрузка/поиск, браузер на CDP, фоновые задачи, планирование и список дел |
| **Сессии** | Сохраняются по проекту; возобновление через `--continue` / `--resume`, импорт транскриптов из других агентских CLI, обмен через локальный сервер |
| **Память** | Редактируемый человеком `MEMORY.md`, семантические факты с эмбеддингами, опциональный RAG-индекс кода — всё по проекту, всё опционально |
| **Headless / CI** | `generate` с `--format json` или `stream-json` (NDJSON), несколько промптов параллельно, ненулевой код выхода при ошибке |
| **Безопасность** | Разрешения по инструменту, анализ риска shell-команд, опциональная ОС-песочница (bubblewrap), HTTP-белый список доменов и хуки инструментов |
| **Расширяемость** | MCP-серверы (stdio и HTTP), языковые серверы LSP, навыки, shell-плагины, свои slash-команды, темы |
| **Встраивание** | SDK на Rust и TypeScript поверх протокола демона v1 (`whycodes serve`) |

## Производительность

Последний замер, Windows AMD64, 2026-09-11 (Ryzen 7 3800X). Linux
2026-09-02 остаётся последним снимком PTY / PSS — не сравнивать первый
кадр Windows с ~12 мс PTY на Linux. Метод и машины в
[docs/benchmarks.md](docs/benchmarks.md):

| Метрика | Windows 2026-09-11 | Linux 2026-09-02 |
|---|---|---|
| 1 сессия PSS | — (только `/proc`) | **10.5 MB** |
| 10 сессий PSS | — | **32.0 MB** (~2.4 MB каждая дополнительная) |
| `--version` | **13.8 ms** | **1.4 ms** |
| Первый кадр (harness, in-proc) | **56 ms** (наследование консоли) | **12 ms** (PTY 80×24) |
| Перерисовки в простое (harness, 3 с) | **0.0 /s** | **0.3 /s** |

TUI рисует только при изменениях. В этом прогоне Windows 3 с простоя
harness — **0.0 перерисовок/с** (Linux 2026-09-02 было 0.3/s); целевой
показатель продукта по-прежнему **0 перерисовок/с**, а не гонка FPS.

Покрытие строк рабочей области **85.58%** (Linux x86_64, 2026-08-21). CI
падает ниже 82%; двенадцать базовых crate держат 100% покрытия строк
продакшен-кода — см. [docs/coverage.md](docs/coverage.md).

## Документация

| Документ | О чём |
|---|---|
| [docs/guide.md](docs/guide.md) | Использование: CLI, TUI, агенты, инструменты, конфиг, SDK |
| [docs/auth.md](docs/auth.md) | API-ключи, OAuth, импорт учётных данных |
| [docs/architecture.md](docs/architecture.md) | Карта crate и слои |
| [docs/roadmap.md](docs/roadmap.md) | Текущий фокус и отложенная работа |
| [docs/knowhow.md](docs/knowhow.md) | Добытые потом баги (TUI, tty, тихие выходы) |
| [docs/tui-term-matrix.md](docs/tui-term-matrix.md) | Ручная проверка TUI на Alacritty / Kitty / VTE |
| [docs/benchmarks.md](docs/benchmarks.md) | Измерение старта, RSS, перерисовок в простое |
| [docs/coverage.md](docs/coverage.md) | Измерение покрытия строк |
| [docs/budgets.md](docs/budgets.md) | Бюджеты качества CI |
| [docs/packaging.md](docs/packaging.md) | Homebrew, установщики, счётчики загрузок релиза |

## Участие

Вклады приветствуются. [CONTRIBUTING.md](CONTRIBUTING.md) — короткий путь
от клона до слитого изменения; [AGENTS.md](AGENTS.md) содержит правила для
кодирующих агентов в этом репозитории. Участвуя, вы соглашаетесь с
[Кодексом поведения](CODE_OF_CONDUCT.md). Помощь и отчёты об ошибках:
[SUPPORT.md](SUPPORT.md). Уязвимости сообщайте через
[SECURITY.md](SECURITY.md), не через публичные issue.

## Спонсоры

WhyCodes разрабатывается независимо и нуждается в финансировании, чтобы
продолжать выпускать версии. Спонсируйте проект на
[GitHub Sponsors](https://github.com/sponsors/whycorporation).

## История звёзд

<p align="center">
  <a href="https://www.star-history.com/?repos=whycorporation%2Fwhycodes&type=date&legend=top-left">
    <picture>
      <source media="(prefers-color-scheme: dark)" srcset="https://api.star-history.com/chart?repos=whycorporation/whycodes&type=date&theme=dark&legend=top-left" />
      <source media="(prefers-color-scheme: light)" srcset="https://api.star-history.com/chart?repos=whycorporation/whycodes&type=date&legend=top-left" />
      <img alt="Star History Chart" src="https://api.star-history.com/chart?repos=whycorporation/whycodes&type=date&legend=top-left" />
    </picture>
  </a>
</p>

## Лицензия

[MIT](LICENSE) · [why.codes](https://why.codes) · [X @whycodesai](https://x.com/whycodesai) · [Спонсор](https://github.com/sponsors/whycorporation)
