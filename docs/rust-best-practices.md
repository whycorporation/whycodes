# Rust best-practice analizi

Tarih: 2026-08-30. Kapsam: workspace kaynak kodu (`crates/`, 24 crate, edition **2024**). Test/bench `unwrap` siteleri bilinçli olarak dışarıda bırakıldı; panic bütçesi de aynı kuralı kullanıyor.

Bu belge bir denetim raporu. Mevcut ratchet’ler (`panic_budget`, `swallowed_error_budget`, Clippy `-D warnings`, dependency boundaries) **iyi bir taban** kuruyor; sapmaların çoğu “CI yeşil olduğu için görünmeyen” dil/API alışkanlıkları.

Öncelik:

| Seviye | Anlam |
|--------|--------|
| P0 | Yanlışlık, kayıp bağlam veya ileride panik üretebilecek API |
| P1 | Rust 2024 / crate tasarımı ile çelişen, bakımı pahalı kalıplar |
| P2 | Tutarlılık, tahsis, Clippy allow yığını — kademeli temizlik |

---

## Özet

Proje, katmanlı crate grafiği, `thiserror` başlangıcı, `spawn_blocking` yardımcısı ve `Arc<[Message]>` gibi birkaç **doğru** hamle yapmış. Asıl sapmalar:

1. **God-file / god-struct.** `run.rs` ~9k satır, `TuiApp` 112 `pub` alan, `cli/src/main.rs` ~4.6k satır.
2. **Stringly-typed hata.** `whycodes_core::Error` neredeyse tamamen `String`. `Clone` Serde kolu artık mesajı korur (`Serde(String)`); domain varyantları hâlâ yok.
3. ~~**Kütüphane crate’lerinde `anyhow`.**~~ **ödendi (2026-08-30):** `lsp`/`mcp`/`storage`/`skill`/`memory`/`session` crate-yerel `thiserror`; `config` kamu yüzeyi `whycodes_core::Error`. `anyhow` uygulama sınırında (`cli`/`tui`/`server`/`agent`/`tools`/`llm`).
4. **`async_trait` + `tokio/full` leaf crate’lerde.** Edition 2024’te native `async fn` in traits var; `core` hâlâ `tokio` çekiyor (`anyhow` düştü).
5. ~~**22 adet `Number::from_f64(...).unwrap()`** LLM provider’larında~~ **ödendi (2026-08-30):** `openai_compat::{json_number, apply_sampling}` NaN/Inf’i atlar; panic bütçesi llm 22→0.
6. **Yutulan hatalar** TUI 49 / CLI 32 / agent 29 — ratchet var, sıfırlama yok.

Aşağıdaki bölümler kanıt + önerilen yön.

---

## Zaten iyi olanlar

Bunlar bilinçli ve korunmalı:

- **Crate katmanları** (`docs/architecture.md`) + `scripts/dependency_boundaries.json`.
- **Panic / swallowed-error ratchet.** Çoğu crate panic bütçesi 0.
- **Clippy `-D warnings`**, `cargo fmt`, `cargo-audit` ignore listesi tarihli.
- `tools/src/blocking.rs`: senkron FS/`Command` işini Tokio worker’dan ayırma.
- `LlmRequest.messages: Arc<[Message]>` + `messages_mut()` COW — clone maliyetine dair yorum doğru.
- `logging.rs` mutex poison’ı `expect` yerine `io::Error`’a çeviriyor.
- `auth` / `sdk` / `sandbox` `thiserror` ile **ayırt edilebilir** hata tipleri kullanıyor.
- `rustc-hash` workspace notu: “integrity için kullanma”.
- Release profili (`lto`, `panic = abort`) bilinçli; production `unwrap` bu yüzden daha pahalı.

---

## P0 — Hata modeli ve panik yüzeyleri

### 1. `whycodes_core::Error` string çantası

```3:56:crates/core/src/error.rs
#[derive(Error, Debug)]
pub enum Error {
    #[error("Configuration error: {0}")]
    Config(String),
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
    #[error("Serialization error: {0}")]
    Serde(#[from] serde_json::Error),
    #[error("LLM error: {0}")]
    Llm(String),
    // Tool / Session / Agent / Provider / Http / Other — hepsi String
}

impl Clone for Error {
    fn clone(&self) -> Self {
        match self {
            Self::Serde(s) => Self::Serde(s.clone()),
            // ...
        }
    }
}
```

**Ödendi (2026-08-30, madde 2):** `Serde` artık `String` tutuyor; `From<serde_json::Error>` mesajı kopyalıyor. Sahte `from_str("not json")` yok. Domain varyantları (`RateLimited`, `ProviderHttp`, …) hâlâ açık.

Rust kitabı ve `thiserror` geleneği: **çağıranın eşleşebileceği** varyantlar, `#[source]` / `#[from]` ile zincir, `Display` kullanıcıya.

Burada:

- `Llm("rate limit")` ile `Llm("tls eof")` aynı tip. Retry / TUI / CI farklı davranamaz; `llm::error_class::classify` string parse etmek zorunda kalıyor.
- ~~`Clone` Serde kolu sahte JSON hatası üretiyordu.~~ Mesaj korunuyor; string çantası duruyor.
- `Http(String)` `reqwest::Error`’ı yutuyor; `auth::AuthError` ise `#[from] reqwest::Error` kullanıyor — tutarsız.

**Yön:** `Error`’ı domain varyantlarına ayır (`RateLimited { retry_after }`, `ProviderHttp { status, body }`, …) veya crate-yerel hataları `#[from]` ile sar. `Clone` gerekiyorsa `Arc<Error>` / `thiserror` + `#[error(transparent)]` veya mesajı `Arc<str>` tut.

`Error: Clone` ihtiyacı büyük ihtimalle event/TUI kopyasından geliyor — o zaman hata **değer** değil, **rapor** olmalı (`struct ErrorReport { kind, message }`).

### 2. Kütüphane API’sinde `anyhow`

`anyhow` binary / `main` için uygun. Kütüphane için `thiserror` (veya küçük yerel enum).

| Crate | `anyhow` | `thiserror` | Not |
|-------|----------|-------------|-----|
| `cli` | evet | hayır | Uygun (composition root) |
| `server` | evet | hayır | Handler’lar `anyhow` (uygulama sınırı) |
| `memory` | hayır | evet | `MemoryError` (ONNX indirme yolu dahil) |
| `session` | hayır | evet | `SessionError` |
| `config` | hayır | hayır | Kamu yüzey `whycodes_core::Error` |
| `llm`, `agent`, `tools`, `tui` | evet | evet | Uygulama / orkestrasyon sınırı |
| `lsp`, `mcp`, `storage`, `skill` | hayır | evet | Crate-yerel `Result` (2026-08-30) |
| `core`, `plugin`, `format` | hayır | `core` evet | Kullanılmayan `anyhow` düştü |
| `auth`, `sdk`, `sandbox` | hayır | evet | Hedef model |

**Yön:** `anyhow` yalnızca `whycodes-cli` (+ `server`/`tui`/`agent`/`tools`/`llm` uygulama sınırı) için. Leaf kütüphane crate’leri crate-yerel `thiserror` (veya `core::Error`).

### 3. `serde_json::Number::from_f64(...).unwrap()` × 22

`llm` panic bütçesinin tamamı bu kalıp:

```56:56:crates/llm/src/providers/openai.rs
crate::openai_compat::apply_sampling(&mut body, request);
```

**Ödendi (2026-08-30):** `json_number` / `set_json_f64` / `apply_sampling` NaN/Inf’i atlar. OpenAI-uyumlu gövdeler tek çağrı; Google/Ollama/Code Assist aynı helper’ı kendi path’lerinde kullanır. Panic bütçesi llm 22→0.

### 4. CLI `status.unwrap()` (kısa devre ile “güvenli”, yine de anti-pattern)

```3144:3148:crates/cli/src/main.rs
let status = std::process::Command::new("gh")
    .args(["pr", "list"])
    .status();
match status {
    Ok(s) if s.success() => {}
```

**Ödendi (2026-08-30):** `match status` — panic bütçesi cli 2→0. `async fn` içinde blocking `std::process::Command` hâlâ duruyor (`tokio::process` / `spawn_blocking` ayrı iş).

### 5. TUI: sabit string `parse().unwrap()`

```4378:4382:crates/tui/src/run.rs
let addr = std::net::SocketAddr::from(([127, 0, 0, 1], port));
std::net::TcpStream::connect_timeout(&addr, Duration::from_millis(80)).is_ok()
```

**Ödendi (2026-08-30):** `SocketAddr::from` — panic bütçesi tui 1→0.

`format` crate: gömülü tmTheme için `.expect("embedded tmTheme must parse")` — build-time asset; `const`/test ile CI’da doğrulanmış olsa `expect` kabul edilebilir. `include_str` + unit test “theme parses” daha idiomatic.

---

## P1 — Modül, API yüzeyi, async

### 6. God-file’lar (Rust’ta “bir dosya = bir kavram”)

| Dosya | Satır | Sorun |
|-------|------:|--------|
| `crates/tui/src/run.rs` | 9170 | Event loop + slash + session + compact + testler |
| `crates/cli/src/main.rs` | 4651 | Tüm alt komutlar tek binary dosyasında |
| `crates/agent/src/agent.rs` | 4470 | Turn loop + tool gate + swarm + compact |
| `crates/tui/src/ui/chat.rs` | 4112 | |
| `crates/tui/src/input.rs` | 3949 | |
| `crates/tui/src/app.rs` | 3875 | Durum + diyalog + input + paint ipuçları |
| `crates/session/src/session.rs` | 3386 | Persist + compact + prune |
| `crates/config/src/lib.rs` | 2580 | Tek `lib.rs`, modül yok |

Rust API guidelines: **küçük, odaklı modüller**; `lib.rs` re-export. `config` zaten “load / merge / validate” üç iş — `load.rs`, `merge.rs`, `schema.rs`, `types.rs` doğal kesit.

`cli`: `Commands` enum kalsın, gövdeler `cli/src/cmd/{auth,session,github}.rs`.

`run.rs` içindeki `#[cfg(test)]` blokları `run/` alt modülüne veya `tui/tests/`’e çıkmalı; 9k satırlık dosyada review imkânsız.

### 7. God-struct: her şey `pub`

Üretim struct’larında **tüm alanlar public** (örnekler):

| Struct | `pub` alan | private |
|--------|----------:|--------:|
| `TuiApp` | 112 | 0 |
| `MemorySettings` | 28 | 0 |
| `MemoryConfig` | 27 | 0 |
| `Config` | 19 | 0 |
| `SessionRuntime` | 23 | 0 |
| `ToolContext` | 11 | 0 |
| `LlmRequest` | 9 | 0 |

```1211:1236:crates/tui/src/app.rs
pub struct TuiApp {
    pub running: bool,
    pub mode: AppMode,
    pub key_context: KeymapContext,
    pub focus: FocusPane,
    pub messages: Vec<ChatMessage>,
    // … 100+ more public fields
    pub input_buffer: String,
```

Invariant (`needs_redraw`, dialog stack, focus) derleme ile korunmuyor. `mark_dirty()` var ama her alan dışarıdan yazılabiliyor.

Serde DTO’ları (`Config`, `ProviderConfig`) için `pub` normal. **Runtime state** (`TuiApp`, `SessionRuntime`, `ToolContext`) için değil: `pub(crate)` + metodlar.

`ToolContext.working_dir: String` — yol için `PathBuf` / `&Path`. Aynı kalıp `plugin::HookContext`, tool iç fonksiyonları.

### 8. `#[allow(clippy::too_many_arguments)]` yığını

Agent, CLI, TUI, tools: onlarca allow. Clippy’nin önerisi **context struct / builder**.

`run_turn_with_events` artık `TurnOpts` alıyor (2026-08-30). TUI render / `memory_retain` / `force_stop_turn` allow’ları duruyor.

### 9. Edition 2024 hâlâ `async_trait`

76 `#[async_trait]` (Tool, LlmProvider, PermissionPrompter, …). Rust 1.75+ **native async fn in traits**; edition 2024’te bu varsayılan.

Maliyet: ekstra crate, box’lı future, daha kötü hata mesajı. `LlmProvider::stream` zaten `Pin<Box<dyn Stream...>>` dönüyor — native trait + type alias yeterli:

```rust
type EventStream = Pin<Box<dyn Stream<Item = Result<StreamEvent>> + Send>>;
async fn stream(&self, ...) -> Result<EventStream>;
```

RPITIT (`impl Stream`) object-safe değil; registry `Box<dyn LlmProvider>` tuttuğu için boxed return kalabilir — `async_trait` yine gerekmez.

### 10. `tokio` `features = ["full"]` workspace-wide

```toml
tokio = { version = "1", features = ["full"] }
```

`core`, `storage`, `skill`, `function` gibi leaf’ler de `full` alır (macros, process, net, time, rt-multi-thread, io-util, …). Derleme süresi, `target` boyutu, “bu crate neden runtime çekiyor?” belirsizliği.

`core`’un `tokio` bağımlılığı muhtemelen `async_trait` Tool yüzünden — native async ile `core` senkron kalabilir (`Tool` future’su implementor’da).

**Yön:** workspace `tokio` default-features kapat; crate bazında `rt`, `macros`, `sync`, `time`, `process`. `full` yalnız `cli` / `tui` / `server`.

### 11. Aşırı geniş `pub mod`

`tui/src/lib.rs` neredeyse her dosyayı `pub mod` yapıyor (`heap`, `cell_grid`, `bench`, `hit_area`, …). Dış crate’lerin (`cli`) ihtiyacı `run`, `TuiApp`, `TuiRunOptions`.

Rust API guidelines: **küçük kamu yüzey**; gerisi `pub(crate) mod`. `agent/src/lib.rs` 19 `pub mod` — `intent`, `magic_keywords`, `thinking_acc` muhtemelen crate-içi.

`tools` düz `pub use` (okunur kısa yollar) iyi örnek; TUI/agent aynı disiplini uygulamıyor.

### 12. Async içinde senkron I/O (kısmen düzeltilmiş)

`tools/blocking.rs` doğru. Hâlâ:

- `agent.rs`: `std::fs::create_dir_all` / `write` / `read_dir` / `remove_dir_all` async turn içinde.
- `cli cmd_github`: `std::process::Command` async `main` yolunda.
- `tui share_server_up`: event loop’tan `TcpStream::connect_timeout` (80 ms bound — kabul edilebilir ama `try_nonblocking` / ayrı task daha temiz).
- `memory/onnx.rs`: senkron HTTP indirme (`ensure_model`) — çağrı yeri async ise `spawn_blocking` şart.

LSP istemcisi `tokio::sync::Mutex` ile stdin/stdout kilitliyor; `next_id` için `AtomicI64` yeterli (mutex değil).

Agent `activated_tools` / `cwd_override` için `std::sync::Mutex` async metodlarda — kısa kritik bölümse tamam; `.lock().unwrap()` poison politikası net olmalı (`logging` gibi `map_err`).

---

## P2 — Tahsis, koleksiyonlar, tutarlılık

### 13. `HashMap` vs `FxHashMap`

Workspace `rustc-hash` var ve yorumu doğru. Kullanım: **HashMap ~153, FxHashMap ~13**. Güvenilmeyen anahtar (kullanıcı/id) için SipHash doğru; provider id, tool adı, model id **güvenilir dahili** anahtar — `FxHashMap` (zaten `ProviderRegistry`).

`config::Config` serde yüzünden `std::collections::HashMap` tutmak zorunda olabilir; runtime registry’ler değil.

### 14. Clone / `to_string` sıcak noktaları

Üretim `.clone()` (kabaca): `agent.rs` 184, `tui/run.rs` 115, `tui/app.rs` 75, `config/lib.rs` 70, `cli/main.rs` 57, `server/v1.rs` 48.

Bir kısmı `Arc` paylaşımı (`Arc::clone`) — doğru. Bir kısmı `String` yol / mesaj kopyası. `Cow<'_, str>` neredeyse yok (1 hit). `working_dir: impl AsRef<Path>` ve `&str` tutmak birçok clone’u keser.

`Tool::definition()` her seferinde `name().to_string()` + `description().to_string()` — tool listesi turn başına bir kez cache’lenebilir.

### 15. `#[must_use]` yok

Workspace’te `#[must_use]` yok. `Result` zaten must-use; `ClaimResult`, `PreHookDecision`, `Permission` kararları değil. Builder-benzeri `TuiApp` mutator’ları sessizce yok sayılabilir.

### 16. `workspace.lints` / `rust-version`

`rust-version = "1.91"` (kod `floor_char_boundary` / `payload_as_str` kullanıyor) ve `[workspace.lints]` her crate’te. `unwrap_used` henüz yok — panic bütçesi ratchet.

```toml
[workspace.package]
rust-version = "1.91"

[workspace.lints.rust]
unsafe_op_in_unsafe_fn = "warn"
```

### 17. Testlerde `unsafe { env::set_var }`

Rust 2024’te `set_var` unsafe. Testler `config`, `auth`, `sdk` içinde process-global env’i kilitliyor (`Mutex<()>`). Doğru kaçış; daha idiomatic: `temp_env` / `temp-env` crate veya `WHYCODES_HOME`’u fonksiyon argümanı yapmak (zaten `paths` var — env’e daha az bağımlı test).

### 18. `core` → `index` ve `tokio`

Mimari: `core` leaf. Gerçekte `whycodes-index` + `tokio` + `anyhow` + `async-trait`. `ToolContext.file_index: Option<Arc<WorkspaceIndex>>` bu kenarı zorluyor.

Daha temiz: `file_index` trait nesnesi (`dyn FileEnumerate`) `core`’da, `index` implementasyonu `tools`/`cli`’de. Döngü yok, `core` tekrar I/O’suz kalır.

### 19. `ToolResult` string + `is_error: bool`

```rust
async fn execute(...) -> ToolResult; // Result değil
```

Rust’ta bu `Result<ToolOutput, ToolError>`. `is_error: true` + `"Error: …"` LLM’e giden tel protokolü olabilir; **içeride** yine `Result` olup serileştirmede string’e çevrilmeli. Aksi halde `?` yok, `map_err` yok, hata türü yok.

### 20. Provider kopyala-yapıştır

`temperature`/`top_p` OpenAI-uyumlu gövdeler `apply_sampling` ile tek yerde. Google/Ollama/Code Assist hâlâ kendi anahtarlarını (`generationConfig`, `options`) yazıyor — aynı helper, farklı path.

`#[async_trait] impl LlmProvider` her dosyada neredeyse aynı `complete`/`stream` iskeleti.

---

## Crate bazında kısa notlar

| Crate | Asıl sapma |
|-------|------------|
| **core** | String `Error` (Serde clone mesajı korunuyor), `tokio`, `ToolContext` String path, `index` bağımlılığı |
| **config** | 2580 satır tek `lib.rs`, her alan `pub`; kamu yüzey `core::Error` |
| **llm** | `async_trait`, string `Error::Llm`, provider tekrarı (sampling unwrap’ları ödendi) |
| **agent** | 4.4k satır, kalan too_many_arguments, 184 clone, sync Mutex + sync fs |
| **tui** | 9k `run.rs`, 112 alanlı `TuiApp`, yutulan hatalar, her modül `pub` |
| **cli** | 4.6k `main.rs`, `anyhow` OK, blocking `Command`, yutulan hatalar |
| **lsp/mcp** | Crate-yerel `thiserror`; `dead_code` Child tutma (yorumlu — `ManuallyDrop`/`AbortOnDrop` daha net) |
| **memory** | `MemoryError`; senkron indirme, 28 alanlı settings |
| **storage** | Crate-yerel `StorageError` (SQLite senkron `rusqlite`) |
| **session** | 3.3k satır, `SessionError` |
| **format** | Gömülü theme `expect` (düşük risk) |
| **auth/sdk/sandbox/command-risk** | En yakın idiomatic küme |

---

## Önerilen sıra (kod değişikliği değil, yol haritası)

1. ~~**`json_number(f64)`**~~ **ödendi** — llm panic bütçesi 22→0.
2. ~~**`Error::clone` Serde kolu**~~ **ödendi** — mesaj korunuyor; string varyantlarını daraltmak ayrı iş.
3. ~~**`SocketAddr::from` + CLI `match status`**~~ **ödendi** — cli 2→0, tui 1→0.
4. ~~**`anyhow`’i leaf crate’lerden çıkar**~~ **ödendi (2026-08-30):** `lsp`/`mcp`/`storage`/`skill`/`memory`/`session` crate-yerel `thiserror`; `config` `core::Error`; `plugin`/`format`/`core` kullanılmayan `anyhow` düştü.
5. ~~**`TurnOpts`**~~ **ödendi** — `run_turn_with_events` tek `TurnOpts`. TUI `force_stop_turn` / render allow’ları duruyor.
6. **`config/src/` modüllere böl**; `cli/src/cmd/`; `tui/src/run/` (loop, slash, persist, tests).
7. **`TuiApp` alanlarını `pub(crate)`** + invariant metodları; diğer crate’ler zaten `run()` kullanıyor.
8. **`async_trait` → native async** (önce `Tool`, sonra `LlmProvider`).
9. **`tokio` feature kesimi**; `core`’dan `tokio` düşür (`anyhow` düştü).
10. ~~**`workspace.lints` + `rust-version`**~~ **ödendi:** her crate `rust-version.workspace` + `[lints] workspace = true`; `unsafe_op_in_unsafe_fn = warn`. `unwrap_used` henüz yok (panic bütçesi ratchet).

1–5, 10 ödendi (2026-08-30). God-file / `TuiApp` / `async_trait` ayrı follow-up. Ratchet dosyaları her düşüşte güncellenmeli (sayıyı yükseltmeden).

---

## Metrikler (üretim kodu, test hariç)

| Metrik | Değer |
|--------|------:|
| Panic-like (`unwrap`/`expect`) | format 1 (`expect` gömülü tmTheme); llm/cli/tui 0 (2026-08-30) |
| Yutulan hata bütçesi | tui 45, cli 32, agent 28, tools 9, memory 8, core 7, format 0 |
| `#[async_trait]` | 76 |
| `HashMap` / `FxHashMap` hit | ~153 / ~13 |
| `Cow<` | 1 |
| `#[must_use]` | 0 |
| 800+ satır `.rs` | 30+ dosya |
| `TuiApp` public alan | 112 |

Ölçüm: `crates/**/*.rs`, `tests/` / `*_tests.rs` / `#[cfg(test)]` hariç; panic sayımı `scripts/check_panic_budget.py` ile aynı fikirde.
