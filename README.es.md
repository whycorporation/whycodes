<p align="center">
  <img src="assets/banner.svg" alt="whycodes — un agente de programación rápido e independiente del proveedor para la terminal, escrito en Rust" width="720">
</p>

<p align="center">
  <a href="https://github.com/whycorporation/whycodes/actions/workflows/ci.yml"><img src="https://github.com/whycorporation/whycodes/actions/workflows/ci.yml/badge.svg" alt="CI"></a>
  <a href="https://github.com/whycorporation/whycodes/releases"><img src="https://img.shields.io/github/v/release/whycorporation/whycodes" alt="Release"></a>
  <a href="LICENSE"><img src="https://img.shields.io/badge/license-MIT-blue.svg" alt="License: MIT"></a>
  <a href="https://discord.gg/H2KsKYEdT8"><img src="https://img.shields.io/badge/Discord-join-5865F2?logo=discord&logoColor=white" alt="Discord de WhyCodes"></a>
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
  <img src="assets/tui-demo.svg" alt="La TUI de whycodes: de la pantalla de inicio a un turno completo con búsqueda, edición y tests" width="820">
</p>

## Por qué WhyCodes

- **Arranca al instante.** `whycodes --version` responde en 1.4 ms en Linux.
- **No consume nada en reposo.** La TUI pinta solo cuando algo cambia: una
  sesión en reposo dibuja 0 fotogramas por segundo.
- **Se mantiene pequeño.** Diez sesiones en reposo comparten 41.9 MB, unos
  3.1 MB por cada sesión extra; una sesión reanudada de 3000 mensajes ocupa
  34.7 MB.
- **Funciona con el modelo que elijas.** APIs alojadas, un servidor de
  modelos local o cualquier endpoint compatible con OpenAI — cambia por
  ejecución o a mitad de sesión.

Medido en una sola máquina; método y salvedades en [Rendimiento](#rendimiento).

## Destacados

- **Un solo binario nativo.** Nada más que instalar: la búsqueda se ejecuta
  en proceso y, en Windows, los comandos se ejecutan sin WSL (a través de
  Git for Windows si está presente; si no, con un shell embebido compatible
  con bash).
- **Multiplataforma.** El mismo flujo en Linux, macOS y Windows.
- **Cualquier proveedor.** Anthropic, OpenAI, Google, GitHub Copilot, Groq,
  xAI, DeepSeek, Ollama, OpenRouter, Mistral, Together, servidores locales
  como LM Studio, vLLM y llama.cpp, o cualquier endpoint compatible con
  OpenAI.
- **Se adapta a tu proyecto.** Carga instrucciones del proyecto desde
  `AGENTS.md` y se conecta a herramientas MCP existentes y servidores de
  lenguaje por LSP.
- **Recuerda tu trabajo.** Sesiones persistidas que puedes ramificar y
  reanudar, memoria entre sesiones (`MEMORY.md`, recall semántico) y un
  índice de código opcional.

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

<details>
<summary>Homebrew dice que el tap no es confiable</summary>

Homebrew 6+ rechaza taps de terceros no confiables. La instalación con
nombre completo de arriba solo confía en esta formula. Si ya hiciste tap y
viste ese error:

```bash
brew trust --formula whycorporation/whycodes/whycodes
brew install whycodes
```

</details>

### Desde el código fuente

```bash
# Necesita libsqlite3 (pkg-config). Para un binario totalmente estático:
# cargo build --release -p whycodes-cli --features bundled-sqlite
cargo build --release -p whycodes-cli
```

Actualiza con `whycodes upgrade` (instalaciones por script / cargo) o
`brew upgrade whycodes` (Homebrew). Las sesiones TUI interactivas
comprueban GitHub en busca de una versión más nueva y preguntan en la
pantalla de inicio antes de instalar; pasa `--no-auto-update` para omitir
la comprobación. Los scripts de instalación verifican los artefactos de la
versión contra el `SHA256SUMS` publicado. Binarios descargables e
instrucciones de desinstalación:
[docs/packaging.md](docs/packaging.md). Cómo se firman los binarios de
Windows: [Code signing policy](docs/code-signing-policy.md).

La configuración y las sesiones viven en `~/.whycodes`
(`%USERPROFILE%\.whycodes` en Windows), o en `$WHYCODES_HOME` si está
definida.

<details>
<summary>Actualizar desde 0.6.4 o anteriores</summary>

0.6.5 sacó la configuración y las sesiones del directorio de plataforma
que se usaba antes (`~/.config/whycodes` o
`~/.config/com.whycorporation.whycodes` en Linux,
`~/Library/Application Support/com.whycorporation.whycodes` en macOS,
`%APPDATA%\whycorporation\whycodes` en Windows). El primer arranque copia
`config.toml`, `auth.json` y `whycodes.db` cuando el directorio nuevo está
vacío, y deja los archivos antiguos en su sitio.

</details>

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
whycodes -P openai -m gpt-5 generate "Refactor this module"
```

¿Sin clave API? Ejecuta un modelo en local:

```bash
whycodes -P ollama -m qwen2.5-coder               # Ollama
whycodes provider discover --save                 # encuentra LM Studio, vLLM, llama.cpp
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
| **Agentes** | `build` (acceso completo), `plan` y `ask` (solo lectura), y `whycodes`, que explica y repara su propia configuración; subagentes `general`, `explore` y `scout` mediante la herramienta `task`, que opcionalmente devuelven un resultado tipado; workers en paralelo en git worktrees vía `swarm`, observados y dirigidos desde el hub de agentes (`/agents`) |
| **Modelos** | Prewalk planifica un turno con un modelo potente y pasa las ediciones a uno más barato; cambia de proveedor o modelo por ejecución o a mitad de sesión (`/models`); compactación nativa del proveedor para Anthropic, opcional |
| **Herramientas** | Edición/parche de archivos, notebooks de Jupyter como celdas, búsqueda en proceso, shell, git y GitHub, fetch/búsqueda web, un navegador impulsado por CDP, trabajos en segundo plano con `wait`, programación y seguimiento de tareas; `ast_grep` / `ast_edit` en compilaciones con la feature `ast-tools` |
| **Sesiones** | Persistidas por proyecto; reanudar con `--continue` / `--resume`, ramificar y volver a turnos anteriores con `/branch` y `/tree`, importar transcripciones de otros CLI de agentes, compartir por el servidor local |
| **Dirección** | Encola un mensaje para después del turno (`/queue`) o interrumpe entre llamadas a herramientas (`/now`); reglas de stream que interrumpen al modelo cuando su salida coincide (`whycodes rules`) |
| **Memoria** | `MEMORY.md` editable por humanos, hechos semánticos con embeddings, índice RAG de código opcional — todo por proyecto, todo opcional |
| **Headless / CI** | `generate` con `--format json` o `stream-json` (NDJSON), varios prompts en paralelo, salida distinta de cero si falla; `whycodes slop` informa de ΔLOC / verbosidad / erosión en el diff |
| **Seguridad** | Preajustes de aprobación (`--approval-mode auto\|important\|manual`), permisos por herramienta, análisis de riesgo de comandos shell, sandbox de SO opcional (bubblewrap), listas blancas de dominios HTTP y ganchos de herramientas |
| **Extensibilidad** | Servidores MCP (stdio y HTTP), servidores de lenguaje LSP, skills, plugins de shell, comandos slash personalizados, temas |
| **Integración** | SDK de Rust y TypeScript sobre el protocolo del daemon v1 (`whycodes serve`) |

## Rendimiento

Medido el 2026-10-16 en una sola máquina (Ryzen 7 3800X): Windows 11
nativo y Linux bajo WSL2. Método, mediciones anteriores y salvedades en
[docs/benchmarks.md](docs/benchmarks.md):

| Métrica | Windows | Linux (WSL2) |
|---|---|---|
| `--version` | **12.9 ms** | **1.4 ms** |
| Primer fotograma (harness, in-proc) | **0.25 ms** | **0.08 ms** |
| Redibujos en reposo (3 s) | **0.0 /s** | **0.0 /s** |
| PSS de 1 sesión en reposo | — (solo `/proc`) | **13.6 MB** |
| PSS de 10 sesiones en reposo | — | **41.9 MB** (~3.1 MB cada extra) |
| PSS de una sesión reanudada de 3000 mensajes | — | **34.7 MB** |

La TUI pinta solo cuando algo cambia, así que una sesión en reposo no
redibuja nada. Una sesión reanudada de 3000 mensajes ocupa 34.7 MB; antes
del trabajo de memoria de octubre eran 216 MB. El harness del primer
fotograma mide un solo pintado de la pantalla inicial, no la pantalla de
inicio completa.

CI falla por debajo del 98.5% de cobertura de líneas del workspace y
mantiene veinticuatro crates en 100% de cobertura de líneas de código de
producción — véase [docs/coverage.md](docs/coverage.md).

## Documentación

| Documento | Qué cubre |
|---|---|
| [docs/guide.md](docs/guide.md) | Uso: CLI, TUI, agentes, herramientas, config, SDK |
| [docs/tools/](docs/tools/README.md) | Una página por herramienta integrada |
| [docs/config.md](docs/config.md) | Cada clave de configuración con su valor por defecto y su variable de entorno |
| [docs/providers.md](docs/providers.md) | Catálogo de proveedores: particularidades, autenticación, manejo de modelos |
| [docs/auth.md](docs/auth.md) | Claves API, OAuth, importación de credenciales |
| [docs/architecture.md](docs/architecture.md) | Mapa de crates y capas |
| [docs/internals/](docs/internals/) | Bucle de turnos, puerta de permisos, compactación, caché de prompts, reintentos, MCP |
| [docs/roadmap.md](docs/roadmap.md) | Enfoque actual y trabajo aplazado |
| [docs/knowhow.md](docs/knowhow.md) | Bugs difíciles de ganar (TUI, tty, salidas silenciosas) |
| [docs/tui-term-matrix.md](docs/tui-term-matrix.md) | Paso manual de TUI en Alacritty / Kitty / VTE |
| [docs/benchmarks.md](docs/benchmarks.md) | Medir arranque, RSS, dibujos en reposo |
| [docs/coverage.md](docs/coverage.md) | Medir cobertura de líneas |
| [docs/budgets.md](docs/budgets.md) | Presupuestos de calidad de CI |
| [docs/packaging.md](docs/packaging.md) | Homebrew, instaladores, recuentos de descargas |
| [docs/code-signing-policy.md](docs/code-signing-policy.md) | Code signing policy (política de firma de código) |

## Contribuir

Las contribuciones son bienvenidas. [CONTRIBUTING.md](CONTRIBUTING.md) es
el camino corto de clonar a un cambio fusionado; [AGENTS.md](AGENTS.md)
contiene las reglas para agentes de programación que trabajan en este
repositorio. Al participar aceptas el
[Código de conducta](CODE_OF_CONDUCT.md). Ayuda e informes de errores:
[SUPPORT.md](SUPPORT.md). Reporta vulnerabilidades a través de
[SECURITY.md](SECURITY.md), no en issues públicas.

## Comunidad

<p>
  <a href="https://discord.gg/H2KsKYEdT8"><img src="https://img.shields.io/badge/Discord-join-5865F2?logo=discord&logoColor=white" alt="Discord de WhyCodes"></a>
  <a href="https://t.me/whycodesai"><img src="https://img.shields.io/badge/Telegram-whycodesai-26A5E4?logo=telegram&logoColor=white" alt="Telegram de WhyCodes"></a>
  <a href="https://x.com/whycodesai"><img src="https://img.shields.io/badge/X-@whycodesai-black" alt="Seguir a @whycodesai en X"></a>
  <a href="https://www.youtube.com/@whycodesai"><img src="https://img.shields.io/badge/YouTube-@whycodesai-FF0000?logo=youtube&logoColor=white" alt="YouTube de WhyCodes"></a>
  <a href="https://olud.ai/tool/whycodes.html"><img src="https://olud.ai/badge.php?tool=whycodes" alt="olud.ai"></a>
</p>

Sitio web: [why.codes](https://why.codes).

## Patrocinadores

WhyCodes se desarrolla de forma independiente y necesita financiación para
seguir publicando. Patrocina el proyecto en
[GitHub Sponsors](https://github.com/sponsors/whycorporation).

<a href="https://github.com/sponsors/whycorporation"><img src="https://img.shields.io/badge/sponsor-whycorporation-ea4aaa" alt="Patrocinar WhyCodes"></a>

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

[MIT](LICENSE)
