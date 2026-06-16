# AGENTS.md

Guidance for AI coding agents working in this repository.

## Keeping This Document Current

**IMPORTANT**: If you notice discrepancies between this document and the actual codebase (e.g., different patterns, renamed modules):

1. **Ask the user**: "I noticed AGENTS.md says [X] but the codebase uses [Y]. Should I update AGENTS.md?"
2. **Don't silently follow outdated guidance** — verify patterns against actual code when uncertain

---

## Project Overview

ttsfx is an **OpenAI-compatible speech proxy** that intercepts onomatopoeia patterns in text, replaces them with cached sound effects (or generates fresh ones via ElevenLabs), concatenates all audio into a WAV stream, and returns it.

**HTTP Server**: Axum (0.8) with HTTP/2 support
**Audio**: symphonia for decoding, hound for WAV encoding
**Pattern Matching**: fancy-regex (lookaheads, named captures)

---

## Crate Structure

```
src/
├── config.rs       # Config loading (TOML + env vars), PatternFilter, PatternConfig
├── error.rs        # AppError, ErrorCode enum, err!/bail! macros, IntoResponse
├── utils.rs        # Partial<T>, PartialOrDefault, WarnOnError (serde_with adapters)
├── tracing_init.rs # `init_tracing` subscriber (EnvFilter, TTSFX_LOG)
├── lib.rs          # Public re-exports (Result, init_tracing)
├── cli.rs          # clap: `run` (default), `reindex`, `--listen`, `--config`
└── main.rs         # Binary entry point, server bootstrap

docs/               # Design docs (not compiled)
├── design-tts-sfx-proxy.md  # Full architecture + phase plan
└── research-tts-sfx-proxy.md # Research findings

sounds/             # Audio cache directory (sidecar .json metadata)
```

---

## Essential Commands

```bash
cargo check         # Check all Rust code (REQUIRED before committing)
cargo test          # Run all tests
```

**Local server:** If port 8787 (or your `--listen`) is already in use, the user often has `ttsfx` running in their own terminal to watch logs. **Do not kill that process** — use `curl` / `scripts/test_speech_samples.sh` against the existing instance, or ask them to restart after code changes.

---

## Code Philosophy

### Write Elegant, Idiomatic Rust

- **Leverage patterns** to write more functionality with less code
- **Abstract and reuse** — extract common logic into helpers, traits, or macros
- **Follow existing conventions** — observe patterns in the codebase and match them
- **Efficient code** — use iterators, combinators, and Rust's type system effectively
- **Less is more** — prefer concise, readable code over verbose implementations
- **Start minimal** — resist adding convenience methods "just in case." If `Struct { field: value }` works, don't add `Struct::new(value)`.
- **YAGNI** — You Aren't Gonna Need It. Add features when actually needed, not speculatively
- **Tests for purpose** — write tests for actual bugs, complex logic, or critical paths—not speculatively for every function

### No Gratuitous Constructors or Accessors
Don't add helper functions just for the sake of it. If a field has no invariant to protect, make it `pub` and skip the constructor/accessor pattern. Only add constructors or getters when:
- An invariant must be enforced (e.g., non-empty collections, valid ranges)
- A field is private by design and needs controlled access
Simple `Struct { field: value }` construction or direct field reads are fine — no wrapper needed.

### Comments

- **Use sparingly** — only when something is genuinely confusing or requires context
- **Don't plaster comments everywhere** — code should be self-documenting where possible
- **Good comment**: Explains *why* something non-obvious is done
- **Bad comment**: Restates what the code obviously does

---

## Error Handling

**DO** use the `err!` macro from crate root:

```rust
use crate::{err, Result};

// Basic error (literal message)
err!(NotFound, "Cache entry not found")

// Formatted message  
err!(AudioDecodeError, "Failed to decode {}: {}", path.display(), e)

// With additional data
err!(NotFound, "Sound not found", @data: { pattern_id: id })

// Early return
crate::bail!(NotFound, "Not found");

// Conditional check
crate::ensure!(!text.is_empty(), Validation, "Input text is empty");

// Wrap foreign error (preserves chain)
.map_err(|e| err!(Network, "TTS request failed", @external: e))?
```

**DO** preserve error chains with `@source:` or `@external:`:

- **`@source:`** — for errors WITH `From<E> for AppError` (io::Error, serde_json::Error)
- **`@external:`** — for errors WITHOUT `From<E>` (third-party library errors)

```rust
// GOOD - preserves error chain and code classification
.map_err(|e| err!(Network, "TTS request failed", @external: e))?

// BAD - loses the error chain and code classification
.map_err(|e| err!(Internal, "Failed: {}", e))?

// BAD - stringifies the error, loses type safety
.map_err(|e| AppError::new(ErrorCode::Network, e.to_string()))?
```

**DON'T** use `eyre!`, `anyhow!`, `panic!`, or `.unwrap()` without a safety net.

Error codes:
| Code | HTTP Status | When to use |
|------|-------------|-------------|
| `Validation` | 400 Bad Request | Invalid request parameters (bad JSON, missing fields) |
| `Configuration` | 400 Bad Request | Config loading / parsing failure, invalid regex |
| `NotFound` | 404 Not Found | Cache entry not found, file missing |
| `Conflict` | 409 Conflict | Duplicate pattern ID already exists |
| `AudioDecodeError` | 502 Bad Gateway | Corrupt WAV/MP3, unsupported format or bit depth |
| `Network` | 502 Bad Gateway | TTS backend timeout, connection refused |
| `RateLimited` | 429 Too Many Requests | Backend API rate limit exceeded |
| `Serialization` | 500 Internal Server Error | JSON encoding/decoding failure |
| `Unauthorized` | 401 Unauthorized | Permission denied (file access) |
| `Io` | 500 Internal Server Error | Unhandled I/O errors (file system, etc.) |
| `Internal` | 500 Internal Server Error | Unexpected / unclassified errors |

---

## Testing

**DO**:
- Use `#[test]` for synchronous tests (config parsing, pattern matching)
- Keep test data simple and self-contained — no external dependencies in unit tests
- Test failure paths, not just happy path

**DON'T**:
- Don't write tests for every trivial getter/setter
- Don't mock complex external dependencies in unit tests — keep them isolated

Example:
```rust
#[test]
fn test_pattern_filter_matches() {
    let filter = PatternFilter { id: "boom".into(), priority: 100, regex: Regex::new(r"\b(?:BOOM|Boom)\b").unwrap(), overrides: None };
    assert_eq!(filter.matches("a BOOM sound"), Some((2, 6)));
    assert!(filter.matches("nothing here").is_none());
}
```

---

## Serde Conventions

- `#[serde(rename_all = "camelCase")]` — always camelCase for JSON APIs
- `#[skip_serializing_none]` from `serde_with` — cleaner than per-field skip
- **DON'T** use snake_case or lowercase for JSON field names
- **DON'T** create duplicate Raw/Final type pairs — use `serde_with` adapters instead:
  - `DisplayFromStr` — deserialize types via `FromStr` (e.g. `Regex`)
  - `PartialOrDefault<_>` — deserialize partial struct, fill missing fields from `Default`
  - `VecSkipError<_, WarnOnError>` — skip bad items in collections with a warning
- **DO** use `Partial<T>` for per-item overrides in hierarchical config (see Patterns below)

---

## Imports

**DO** prefer prelude re-exports for easier refactors:
```rust
use crate::{err, Result};  // when in the same crate
```

**DO** use inline aliases for conflicting type names:
```rust
use std::result::Result as StdResult;  // Alias to avoid conflict with crate Result
```

**DON'T** use full paths inline — they're harder to read and refactor:
```rust
// BAD - hard to follow, hard to refactor
fn handle() -> ::core::result::Result<T, crate::error::AppError> {
    let e = std::io::Error::new(std::io::ErrorKind::NotFound, "file");
    crate::err!(crate::error::ErrorCode::Io, e)
}

// GOOD - imports at top, clean code below
use crate::{err, Result};

fn handle() -> Result { /* ... */ }
```

---

## Patterns to Follow

### Hierarchical Config
One base struct (`PatternConfig`) defines overridable fields with correct defaults. Top-level config uses `PartialOrDefault` to fill missing fields from `Default`. Per-pattern overrides use `Partial<PatternConfig>`.

```rust
// Base config — one source of truth for defaults and fields
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PatternConfig {
    pub tts_base_url: String,
    pub volume_target_db: f64,
    pub cache_dir: PathBuf,
    // ...
}

impl Default for PatternConfig {
    fn default() -> Self {
        Self {
            tts_base_url: "https://api.openai.com/v1".into(),
            volume_target_db: -16.0,
            cache_dir: PathBuf::from("sounds"),
        }
    }
}

// Top-level config — missing fields get defaults for free
#[serde_as]
pub struct Config {
    #[serde_as(deserialize_as = "PartialOrDefault<_>")]
    pub overridable: PatternConfig,
    pub patterns: Vec<PatternFilter>,
}

// Per-pattern — partial overrides merge on top of base
pub struct PatternFilter {
    pub overrides: Option<Partial<PatternConfig>>,
    // ...
}
```

Adding a field to `PatternConfig` automatically propagates: Config gets the default, patterns can override it. No need to update multiple types.

### Pattern Matching Flow
```
text input → detect() (PatternEngine) → [(matched_text, pattern_id), ...]
              ↓
each match → resolve_sfx() (SoundResolver) → Cache(PathBuf) | Generate(GenerateData with recipe)
              ↓
each source → decode() (AudioProcessor) → AudioSegment { samples, sample_rate }
              ↓
all segments → normalize_volume() + concat_segments() → Vec<f32>
              ↓
encode_wav() (AudioProcessor) → WAV bytes response body
```

### No `Ref<T>` needed here — we don't have a database
This project doesn't use SurrealDB or entity references. We deal with:
- **Config** (struct from TOML/env)
- **PatternFilter** (compiled regex rules)
- **AppError** (structured error type with ErrorCode enum)

### HTTP Handler Pattern
Free-standing functions + AppState struct, no impl blocks on handler types:

```rust
#[derive(Clone)]
pub struct AppState { /* config + engine + cache + resolver */ }

// Handler is a free-standing function
pub async fn handle_speech(
    State(state): State<AppState>,  // axum state extractor
    Json(body): Json<ProxyRequest>,
) -> Result<Response> { /* ... */ }

// Error conversion via IntoResponse (defined in error.rs)
impl axum::response::IntoResponse for AppError { /* ... */ }
```

---

## Tracing & Observability

**Bootstrap** (`main.rs`): `dotenvy` → `color_eyre::install()` → `init_tracing` → `cli::Cli::parse_args()` → `run(listen)`.

**CLI** (`cli.rs`): default subcommand runs the server — `-L` / `--listen` (`TTSFX_LISTEN`), `--config` (`TTSFX_CONFIG`). `reindex` refreshes cache embeddings. Domain settings stay in `config.toml` / `TTSFX_OVERRIDABLE__*`. User-facing overview: `README.md`.

**Upstream TTS** (Kokoro/OpenAI-shaped): client JSON may include `speed` and `normalization_options` on the same body as `model`/`voice`; fragment sub-requests forward them with `stream: false` and `response_format: "wav"`.

**HTTP middleware** (`middleware.rs`): outer `request_id_middleware` (`x-request-id` in/out, `RequestId` in extensions) → `TraceLayer` with `RequestSpan` (`info_span!("request", id, method, path, ip, otel.name)`) and `StatusLevelOnResponse` (5xx → `error!`, 4xx → `warn!`, else `debug!` with `status`, `latency_ms`). Successful `/v1/audio/speech` work still logs at **info** inside `handle_speech`; access-line style completion is **debug** for 2xx so `TTSFX_LOG=info` stays readable.

**Env vars**:
- `TTSFX_LOG` — default `debug` for the `ttsfx` crate
- `RUST_LOG` — extra `tracing_subscriber` directives (merged with defaults)
- `TTSFX_LOG_TREE` — default **true**: **`SpanTreeFormat`** (message + fields first, then dimmed span chain like `request{id=… method=POST path=/v1/audio/speech}`)
- `TTSFX_LOG_TREE=0` — stock subscriber with file/line; `TTSFX_LOG_FILE=0` hides file/line in that mode

Default filter: **WARN** globally, then `ttsfx={TTSFX_LOG}`, then quiet deps (`hyper`, `tower`, `h2`, `reqwest`, `notify`).

**Levels**:
- `info!` — lifecycle (server up, config loaded, cache hit/miss summary, response ready)
- `debug!` — investigation (fragment split, upstream TTS, ffmpeg, cache index refresh)
- `trace!` — mechanical replay (rare here)
- `warn!` — expected degradation (bad regex at match time, override merge failed, watcher errors)
- `error!` — system problems (via `ResultExt::error()` or explicit `error!`)

**Structured fields**: use `key = value`, `%display`, `?debug` on events — not string interpolation in the message.

```rust
info!(path = %entry.filepath.display(), "cache hit");
warn!(pattern = %name, "regex error during matching");
```

**Instrumentation**: no proc-macro `#[traced]` in this crate — use `#[tracing::instrument]` directly:
- HTTP entry / user-visible work: `level = "info"`, `err(level = "warn")`, `skip` large args (`State`, headers, body)
- Internal async helpers: `level = "debug"`, same `err` default
- Put request identity on the span: `fields(model = %body.settings.model, input_len = body.input.len())`

**Errors**: `.warn()` / `.error()` from `ResultExt` in `error.rs` logs with `caller.file` / `caller.line`.

---

## Common Mistakes to Avoid

1. **Using `eyre!` or `.unwrap()` instead of proper error handling** — always use the `err!` macro
2. **Stringifying errors in messages** (breaks error chain) — use `@external:` for foreign errors
3. **Over-commenting** — let code be self-documenting where possible
4. **Copy-paste duplication** — extract common patterns into helpers
5. **Ignoring existing patterns in the codebase** — look at how similar code is written
6. **Not using `#[track_caller]` on error constructors** — helps with debugging
7. **Creating duplicate types for serde** — use `serde_with` adapters (`DisplayFromStr`, `PartialOrDefault`, `VecSkipError`) instead of Raw/Final pairs
8. **Over-investing in simple things** — don't create extra modules, files, or tests for trivial code. Invest design effort in the hard parts (like config hierarchy)
9. **Speculative abstractions** — don't add convenience methods, wrappers, or helpers "just in case"
