<div align="center">

<img src="assets/icons/ttsfx-header.png" width="256" height="256" alt="ttsfx icon"/>

# ttsfx

**A speech API proxy that replaces onomatopoeia in TTS input with cached or generated sound effects.**

[Features](#features) •
[Getting Started](#getting-started) •
[Dependencies](#dependencies) •
[Configuration](#configuration) •
[CLI Usage](#cli-usage) •
[Building](#building)

</div>

## Features

- Drop-in `POST /v1/audio/speech` handler (same JSON shape your TTS client already sends)
- Match comic / onomatopoeia words with regex rules in `config.toml` (no recompile to add patterns)
- Serve cached SFX from disk when a hit matches; call ElevenLabs on miss and save for next time
- Stitch SFX with normal speech from your configured upstream TTS (model, voice, `speed`, etc.)
- Return a single WAV (decode/encode via ffmpeg)
- Optional embedding-based cache matching; `reindex` to refresh vectors on stored recipes
- Per-pattern overrides (volume, crossfade, prompts, cache tags) on top of global defaults

## Getting Started

1. Install the [dependencies](#dependencies) (at minimum **ffmpeg** and a Rust toolchain).
2. Copy or edit `config.toml` for your TTS URL, embedding service (if used), and ElevenLabs settings.
3. Set `ELEVENLABS_API_KEY` when you want new SFX generated (or configure `sfx_api_key` in TOML).
4. Run the server:

   ```bash
   cargo run --release
   ```

5. Point your speech client at `http://127.0.0.1:8787` (or whatever you pass to `-L` / `--listen`).

<details>
<summary>Quick test</summary>

```bash
curl -sS -o out.wav -X POST http://127.0.0.1:8787/v1/audio/speech \
  -H 'Content-Type: application/json' \
  -d '{
    "model": "kokoro",
    "voice": "af_heart",
    "input": "Hello. BOOM Done.",
    "response_format": "wav"
  }'
```

Or run `./scripts/test_speech_samples.sh` against a running instance (`TTSFX_URL` overrides the base URL).

</details>

A `.env` file in the project root is loaded on startup if you use one.

## Dependencies

<details>
<summary>ffmpeg (required)</summary>

> Used for all audio decode/encode. Must be on your `PATH`.
>
> | Platform | Notes |
> |----------|--------|
> | macOS | `brew install ffmpeg` |
> | Debian/Ubuntu | `sudo apt install ffmpeg` |
> | Arch | `sudo pacman -S ffmpeg` |
> | Windows | Install from [ffmpeg.org](https://ffmpeg.org/download.html) or a package manager |

</details>

- **Upstream TTS** — any OpenAI-compatible speech endpoint you set as `tts_base_url` in config (e.g. local Kokoro, OpenAI, etc.)
- **Embeddings API** (optional) — OpenAI-compatible embeddings URL if you use `match_mode = { mode = "embeddings", ... }` in config
- **ElevenLabs** (when generating new SFX) — API key via `ELEVENLABS_API_KEY` or `sfx_api_key` in config

All Rust crate dependencies are in `Cargo.toml` and linked into the binary; you do not install them separately.

## Configuration

Everything is in **`config.toml`** at the repo root (or pass `--config`). The file header there explains structure and first-time setup; below is a short map.

| Area | Keys | Notes |
|------|------|--------|
| Speech upstream | `tts_base_url` | OpenAI-shaped `/v1/audio/speech` backend for non-SFX text |
| SFX generation | `sfx_base_url`, `sfx_api_key`, `sfx_output_format` | ElevenLabs on cache miss |
| Cache on disk | `cache_dir` (optional) | Audio + `.json` metadata; default `sound_cache/` if omitted |
| Cache lookup | `match_mode` | `levenshtein` (simple) or `embeddings` (needs `embed_*` + optional `ttsfx reindex`) |
| Recipes | `context`, `sfx_prompt_template` | Minijinja prompt for generate + embed; see comments in TOML |
| Patterns | `[[patterns]]` | `regex`, `priority` (higher first), optional `cache_tag`, `[patterns.overrides]` |

<details>
<summary>Minimal example (Levenshtein, one pattern)</summary>

No embedding service required. Still need TTS URL, ElevenLabs key for new SFX, and ffmpeg.

```toml
[overridable]
match_mode = { mode = "levenshtein", threshold = 1 }
tts_base_url = "http://127.0.0.1:8880/v1"
sfx_base_url = "https://api.elevenlabs.io"
sfx_api_key = { env = "ELEVENLABS_API_KEY" }
sfx_output_format = "mp3_44100_128"
volume_target_db = 0

[[patterns]]
name = "boom"
priority = 100
regex = "\\b(?:BOOM|Boom)\\b"
```

</details>

Environment overrides for nested settings use double underscores, e.g. `TTSFX_OVERRIDABLE__TTS_BASE_URL`.

Logging: `TTSFX_LOG` (crate level, default `debug`), `RUST_LOG`, `TTSFX_LOG_TREE` (`0` for plain line format). See `AGENTS.md` for contributor/agent conventions.

## CLI Usage

With no subcommand, ttsfx runs the HTTP server.

```
ttsfx — speech proxy that turns onomatopoeia in TTS input into sound effects

Usage: ttsfx [OPTIONS] [COMMAND]

Commands:
  run                  Run the HTTP server (default)
  reindex              Refresh embedding vectors for cached SFX recipes
  help                 Print help

Options:
  -L, --listen <LISTEN>   Bind address [env: TTSFX_LISTEN=] [default: 0.0.0.0:8787]
      --config <CONFIG>   Config TOML path [env: TTSFX_CONFIG=] [default: config.toml]
  -h, --help
  -V, --version
```

`reindex` supports `--dry-run`, `--force`, and optional `--before` / `--after` time filters. Run `cargo run -- --help` or `cargo run -- reindex --help` for the full text.

## Building

Building ttsfx requires an up-to-date [Rust toolchain](https://www.rust-lang.org/tools/install) and [ffmpeg](#dependencies) on your `PATH`.

1. Clone the repo and `cd` into it.
2. Build:

   ```bash
   cargo build --release
   ```

The binary is at `target/release/ttsfx`.

```bash
cargo check
cargo test
```