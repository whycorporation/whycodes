<p align="center">
  <img src="assets/banner.svg" alt="whycodes — un agente de programación rápido e independiente del proveedor para la terminal, escrito en Rust" width="720">
</p>

<p align="center">
  <a href="https://olud.ai/tool/whycodes.html"><img src="https://olud.ai/badge.php?tool=whycodes" alt="olud.ai"></a>
  <a href="https://github.com/whycorporation/whycodes/actions/workflows/ci.yml"><img src="https://github.com/whycorporation/whycodes/actions/workflows/ci.yml/badge.svg" alt="CI"></a>
  <a href="https://github.com/whycorporation/whycodes/releases"><img src="https://img.shields.io/github/v/release/whycorporation/whycodes" alt="Release"></a>
  <a href="LICENSE"><img src="https://img.shields.io/badge/license-MIT-blue.svg" alt="License: MIT"></a>
  <a href="https://x.com/whycodesai"><img src="https://img.shields.io/badge/X-@whycodesai-black" alt="Seguir a @whycodesai en X"></a>
  <a href="https://github.com/sponsors/whycorporation"><img src="https://img.shields.io/badge/sponsor-whycorporation-ea4aaa" alt="Patrocinar WhyCodes"></a>
</p>

<p align="center">
  <a href="README.md">English</a> · <a href="README.zh-CN.md">简体中文</a> · <a href="README.ru.md">Русский</a> · Español
</p>

**Un agente de programación rápido e independiente del proveedor para la terminal, escrito en Rust.**

WhyCodes lee, escribe y edita archivos, ejecuta comandos, busca en el
espacio de trabajo y dirige un LLM mediante el uso de herramientas en varios
turnos — en una TUI a pantalla completa o como CLI de un solo disparo
legible por máquina. Se centra en un binario nativo pequeño, una TUI
eficiente en reposo y un flujo de trabajo que no está atado a un único
proveedor de modelos.

<p align="center">
  <img src="assets/tui-home.svg" alt="Pantalla de inicio de la TUI de whycodes" width="820">
</p>

## Destacados

- **Un solo binario nativo.** Sin dependencias de runtime obligatorias. La
  búsqueda es en proceso (no hace falta `ripgrep`); integraciones opcionales
  del SO como bubblewrap en Linux refuerzan el sandbox cuando están
  disponibles.
- **Multiplataforma.** El mismo flujo en Linux, macOS y Windows. Linux se
  ejecuta en cada cambio de CI; los trabajos de Windows y macOS están
  disponibles en la matriz CI multi-SO.
- **Cualquier proveedor.** Anthropic, OpenAI, Google, GitHub Copilot, Groq,
  xAI, DeepSeek, Ollama, OpenRouter, Mistral, Together, o cualquier endpoint
  compatible con OpenAI — con una clave API o un inicio de sesión de
  suscripción (`whycodes auth login`).
- **Se adapta a tu proyecto.** Carga instrucciones del proyecto desde
  `AGENTS.md` y se conecta a herramientas MCP existentes y servidores de
  lenguaje por LSP.
- **Recuerda tu trabajo.** Sesiones persistidas, memoria entre sesiones
  (`MEMORY.md`, recall semántico) y un índice de código opcional.

## Instalación

### Script de instalación (Linux, macOS)

```bash
curl -fsSL https://why.codes/install | bash
```

### Script de instalación (Windows)

```powershell
irm https://why.codes/install.ps1 | iex
```

Añade `%LOCALAPPDATA%\Programs\whycodes` al PATH de usuario. No hace falta
WSL.

### Homebrew (macOS; Linuxbrew x86_64)

```bash
brew tap whycorporation/whycodes https://github.com/whycorporation/whycodes
brew install whycorporation/whycodes/whycodes
```

Homebrew 6+ rechaza taps de terceros no confiables. La instalación con
nombre completo solo confía en esta formula. Si ya hiciste tap y viste ese
error:

```bash
brew trust --formula whycorporation/whycodes/whycodes
brew install whycodes
```

### Desde el código fuente

```bash
# Necesita libsqlite3 (pkg-config). Para un binario totalmente estático:
# cargo build --release -p whycodes-cli --features bundled-sqlite
cargo build --release -p whycodes-cli
```

Actualiza con `whycodes upgrade` (instalaciones por script / cargo) o
`brew upgrade whycodes` (Homebrew). Las sesiones TUI interactivas
comprueban GitHub en busca de una versión más nueva y preguntan en la
pantalla de inicio antes de instalar. Pasa `--no-auto-update` para omitir
el aviso. Los scripts de instalación verifican los artefactos de la
versión contra el `SHA256SUMS` publicado. Binarios descargables e
instrucciones de desinstalación:
[docs/packaging.md](docs/packaging.md).

<details>
<summary>Completados de shell</summary>

```bash
eval "$(whycodes completions zsh)"    # ~/.zshrc
eval "$(whycodes completions bash)"   # ~/.bashrc
whycodes completions fish > ~/.config/fish/completions/whycodes.fish
```

</details>

## Inicio rápido

```bash
export ANTHROPIC_API_KEY="sk-ant-..."

whycodes -d ./my-project                            # TUI interactiva
whycodes generate "Explain main.rs" -d ./my-project # un solo disparo
whycodes generate "Summarize the last commit" --format json
whycodes --continue                                 # reanudar la última sesión
whycodes -P openai -m gpt-4o generate "Refactor this module"
```

El inicio de sesión OAuth de suscripción (`whycodes auth login <provider>`)
solo está disponible tras instalar un plugin local `kind: "auth"` —
WhyCodes no incluye clientes OAuth de terceros. Véase
[docs/auth.md](docs/auth.md).

La guía completa — referencia CLI, teclas TUI, comandos slash, agentes,
herramientas y configuración — está en **[docs/guide.md](docs/guide.md)**.

## Funciones

| | |
|---|---|
| **Agentes** | Primarios: `build` (acceso completo), `plan` y `ask` (solo lectura); subagentes `general`, `explore` y `scout` mediante la herramienta `task`; workers en paralelo en git worktrees vía `swarm` |
| **Herramientas** | Edición/parche de archivos, búsqueda en proceso, shell, git y GitHub, fetch/búsqueda web, un navegador impulsado por CDP, trabajos en segundo plano, programación y seguimiento de tareas |
| **Sesiones** | Persistidas por proyecto; reanudar con `--continue` / `--resume`, importar transcripciones de otros CLI de agentes, compartir por el servidor local |
| **Memoria** | `MEMORY.md` editable por humanos, hechos semánticos con embeddings, índice RAG de código opcional — todo por proyecto, todo opcional |
| **Headless / CI** | `generate` con `--format json` o `stream-json` (NDJSON), varios prompts en paralelo, salida distinta de cero si falla |
| **Seguridad** | Permisos por herramienta, análisis de riesgo de comandos shell, sandbox de SO opcional (bubblewrap), listas blancas de dominios HTTP y ganchos de herramientas |
| **Extensibilidad** | Servidores MCP (stdio y HTTP), servidores de lenguaje LSP, skills, plugins de shell, comandos slash personalizados, temas |
| **Integración** | SDK de Rust y TypeScript sobre el protocolo del daemon v1 (`whycodes serve`) |

## Rendimiento

TUI en reposo, Linux x86_64, 2026-09-02 (método y máquina en
[docs/benchmarks.md](docs/benchmarks.md)):

| Métrica | Resultado |
|---|---|
| 1 sesión PSS | **10.5 MB** |
| 10 sesiones PSS | **32.0 MB** (~2.4 MB cada extra) |
| `--version` | **1.4 ms** |
| Primer fotograma (harness, in-proc) | **12 ms** |
| Redibujos en reposo (harness, 3 s) | **0.3 /s** |

La TUI pinta solo cuando algo cambia. El reposo de 3 s de este harness es
**0.3 redibujos/s** (igual que `ea098af` / `50e05d8`; las puertas idle-zero
no restauraron un cero duro); el objetivo del producto sigue siendo
**0 redibujos/s**, no una carrera de fotogramas por segundo.

La cobertura de líneas del workspace es **85.58%** (Linux x86_64,
2026-08-21). CI falla por debajo del 82%, con doce crates fundacionales
en 100% de cobertura de líneas de código de producción — véase
[docs/coverage.md](docs/coverage.md).

## Documentación

| Documento | Qué cubre |
|---|---|
| [docs/guide.md](docs/guide.md) | Uso: CLI, TUI, agentes, herramientas, config, SDK |
| [docs/auth.md](docs/auth.md) | Claves API, OAuth, importación de credenciales |
| [docs/architecture.md](docs/architecture.md) | Mapa de crates y capas |
| [docs/roadmap.md](docs/roadmap.md) | Enfoque actual y trabajo aplazado |
| [docs/knowhow.md](docs/knowhow.md) | Bugs difíciles de ganar (TUI, tty, salidas silenciosas) |
| [docs/tui-term-matrix.md](docs/tui-term-matrix.md) | Paso manual de TUI en Alacritty / Kitty / VTE |
| [docs/benchmarks.md](docs/benchmarks.md) | Medir arranque, RSS, dibujos en reposo |
| [docs/coverage.md](docs/coverage.md) | Medir cobertura de líneas |
| [docs/budgets.md](docs/budgets.md) | Presupuestos de calidad de CI |
| [docs/packaging.md](docs/packaging.md) | Homebrew, instaladores, recuentos de descargas |

## Contribuir

Las contribuciones son bienvenidas. [CONTRIBUTING.md](CONTRIBUTING.md) es
el camino corto de clonar a un cambio fusionado; [AGENTS.md](AGENTS.md)
contiene las reglas para agentes de programación que trabajan en este
repositorio. Al participar aceptas el
[Código de conducta](CODE_OF_CONDUCT.md). Ayuda e informes de errores:
[SUPPORT.md](SUPPORT.md). Reporta vulnerabilidades a través de
[SECURITY.md](SECURITY.md), no en issues públicas.

## Patrocinadores

WhyCodes se desarrolla de forma independiente y necesita financiación para
seguir publicando. Patrocina el proyecto en
[GitHub Sponsors](https://github.com/sponsors/whycorporation).

## Historial de estrellas

<p align="center">
  <a href="https://www.star-history.com/?repos=whycorporation%2Fwhycodes&type=date&legend=top-left">
    <picture>
      <source media="(prefers-color-scheme: dark)" srcset="https://api.star-history.com/chart?repos=whycorporation/whycodes&type=date&theme=dark&legend=top-left" />
      <source media="(prefers-color-scheme: light)" srcset="https://api.star-history.com/chart?repos=whycorporation/whycodes&type=date&legend=top-left" />
      <img alt="Star History Chart" src="https://api.star-history.com/chart?repos=whycorporation/whycodes&type=date&legend=top-left" />
    </picture>
  </a>
</p>

## Licencia

[MIT](LICENSE) · [why.codes](https://why.codes) · [X @whycodesai](https://x.com/whycodesai) · [Patrocinar](https://github.com/sponsors/whycorporation)
