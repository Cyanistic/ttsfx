use std::path::PathBuf;
use std::sync::Arc;

use chrono::Utc;

use crate::audio::{self, AudioSegment};
use crate::cache::{CacheMeta, Embeddings, write_metadata};
use crate::config::volume_gain;

use crate::pattern::{PatternMatch, match_patterns};
use crate::resolver::{GenerateData, Resolution};
use crate::state::{AppState, Fragment, FragmentKind, ResolvedAudio};
use crate::{Result, ensure, err};
use axum::Json;
use axum::extract::State;
use axum::http::header::{
    CONNECTION, CONTENT_LENGTH, CONTENT_TYPE, HOST, PROXY_AUTHENTICATE, PROXY_AUTHORIZATION, TE,
    TRAILER, TRANSFER_ENCODING, UPGRADE,
};
use axum::http::{HeaderMap, HeaderName, StatusCode};
use axum::response::{IntoResponse, Response};
use futures_util::future::join_all;
use serde_with::skip_serializing_none;
use std::time::Duration;
use tracing::info;
use tokio::time::timeout;

use serde::{Deserialize, Serialize};

/// OpenAI-style `response_format` for `/v1/audio/speech` (subset we encode).
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum SpeechResponseFormat {
    #[default]
    Wav,
    #[serde(alias = "mpeg")]
    Mp3,
}

impl SpeechResponseFormat {
    pub fn content_type(self) -> &'static str {
        match self {
            Self::Wav => "audio/wav",
            Self::Mp3 => "audio/mpeg",
        }
    }
}

/// Kokoro / OpenAI-compatible text normalization (passthrough to upstream TTS).
#[skip_serializing_none]
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct NormalizationOptions {
    pub normalize: Option<bool>,
    pub unit_normalization: Option<bool>,
    pub url_normalization: Option<bool>,
    pub email_normalization: Option<bool>,
    pub optional_pluralization_normalization: Option<bool>,
    pub phone_normalization: Option<bool>,
}

/// Upstream TTS parameters shared across every text chunk in one client request.
#[skip_serializing_none]
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TtsSettings {
    pub model: String,
    pub voice: String,
    /// Speaking rate (Kokoro: 0.25–4.0). Omitted on upstream calls when unset.
    pub speed: Option<f32>,
    pub normalization_options: Option<NormalizationOptions>,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct ProxyRequest {
    #[serde(flatten)]
    pub settings: TtsSettings,
    pub input: String,
    #[serde(default)]
    pub response_format: SpeechResponseFormat,
}

/// Body for upstream `/audio/speech` fragment calls (WAV in, non-streaming).
#[skip_serializing_none]
#[derive(Debug, Serialize)]
struct UpstreamSpeechBody<'a> {
    model: &'a str,
    voice: &'a str,
    input: &'a str,
    response_format: &'static str,
    stream: bool,
    speed: Option<f32>,
    normalization_options: Option<NormalizationOptions>,
}

#[tracing::instrument(
    level = "info",
    skip(state, headers, body),
    fields(
        model = %body.settings.model,
        voice = %body.settings.voice,
        response_format = ?body.response_format,
        input_len = body.input.len(),
    ),
    err(level = "warn")
)]
pub async fn handle_speech(
    State(state): State<AppState>,
    mut headers: HeaderMap,
    Json(body): Json<ProxyRequest>,
) -> Result<Response> {
    ensure!(!body.input.is_empty(), Validation, "input text is empty");

    let patterns: Vec<_> = state.config.patterns.iter().map(|p| &p.filter).collect();
    let mut matches = match_patterns(patterns.into_iter(), &body.input);
    matches.sort_by_key(|m| m.start);

    let fragments = build_fragments(&body.input, &matches);
    ensure!(
        !fragments.is_empty(),
        Validation,
        "no audio fragments to synthesize"
    );
    if matches.is_empty() {
        info!(
            fragments = fragments.len(),
            "no onomatopoeia matched; entire input sent to TTS"
        );
    } else {
        info!(
            sfx_matches = matches.len(),
            fragments = fragments.len(),
            "speech request split into TTS and SFX fragments"
        );
    }

    let sample_rate = state.config.sample_rate;
    let settings = body.settings.clone();
    forward_client_headers(&mut headers);
    let forward_headers = Arc::new(headers);

    let futures: Vec<_> = fragments
        .into_iter()
        .map(|frag| {
            let state = state.clone();
            let settings = settings.clone();
            let forward_headers = Arc::clone(&forward_headers);
            let full_input = body.input.clone();
            async move {
                resolve_fragment(
                    &state,
                    frag,
                    &settings,
                    forward_headers.as_ref(),
                    &full_input,
                )
                .await
            }
        })
        .collect();

    let resolved: Vec<FragmentOutcome> = join_all(futures)
        .await
        .into_iter()
        .collect::<Result<Vec<_>>>()?;

    info!(fragments = resolved.len(), "all fragments resolved, decoding audio");

    let mut decoded: Vec<AudioSegment> = Vec::with_capacity(resolved.len());
    for (i, item) in resolved.iter().enumerate() {
        let kind = format!("{:?}", item.kind);
        let label = match &item.audio {
            ResolvedAudio::Bytes(b) => format!("bytes={}", b.len()),
            ResolvedAudio::File(p) => format!("file={}", p.display()),
        };
        info!(index = i, kind = %kind, %label, "decoding fragment");
        let fut = async {
            match &item.audio {
                ResolvedAudio::Bytes(b) => audio::decode(b, sample_rate).await,
                ResolvedAudio::File(p) => audio::decode_file(p, sample_rate).await,
            }
        };
        let seg = timeout(Duration::from_secs(60), fut)
            .await
            .map_err(|_| err!(Internal, "ffmpeg decode timed out after 60s"))??;
        info!(index = i, samples = seg.samples.len(), "decoded fragment");
        decoded.push(seg);
    }

    let mut ref_rms: Option<f32> = None;
    let mut segments: Vec<AudioSegment> = Vec::with_capacity(decoded.len());
    let mut crossfade_ms: Vec<u32> = Vec::with_capacity(decoded.len());

    for (item, mut seg) in resolved.into_iter().zip(decoded) {
        crossfade_ms.push(item.crossfade_ms);
        if item.kind == FragmentKind::Tts {
            if ref_rms.is_none() {
                ref_rms = Some(audio::rms_level(&seg.samples));
            }
        } else if let Some(target) = ref_rms {
            audio::match_loudness(&mut seg, target);
            let gain = volume_gain(item.volume_db);
            for s in &mut seg.samples {
                *s = (*s * gain).clamp(-1.0, 1.0);
            }
        }
        segments.push(seg);
    }

    let join_crossfade_ms: Vec<u32> = (0..segments.len().saturating_sub(1))
        .map(|i| crossfade_for_join(crossfade_ms[i], crossfade_ms[i + 1]))
        .collect();
    let join_crossfade_samples: Vec<usize> = join_crossfade_ms
        .iter()
        .map(|ms| ms_to_samples(*ms, sample_rate))
        .collect();

    let merged = audio::concat_segments_crossfaded_variable(segments, &join_crossfade_samples)
        .map_err(|_| err!(Internal, "audio segments sample rate or channel mismatch"))?;

    let format = body.response_format;
    let bytes = match format {
        SpeechResponseFormat::Mp3 => audio::encode_mp3(&merged.samples, merged.sample_rate).await?,
        SpeechResponseFormat::Wav => audio::encode_wav(&merged.samples, merged.sample_rate).await?,
    };

    info!(
        bytes = bytes.len(),
        duration_ms = (merged.samples.len() as u64 * 1000 / merged.sample_rate as u64),
        "speech response ready"
    );

    Ok((
        StatusCode::OK,
        [(CONTENT_TYPE, format.content_type())],
        bytes,
    )
        .into_response())
}

fn ms_to_samples(ms: u32, sample_rate: u32) -> usize {
    (ms as u64 * sample_rate as u64 / 1000) as usize
}

/// Crossfade when either side of the join is SFX (uses that pattern's `crossfade_ms`).
fn crossfade_for_join(left_ms: u32, right_ms: u32) -> u32 {
    left_ms.max(right_ms)
}

fn build_fragments(input: &str, matches: &[PatternMatch]) -> Vec<Fragment> {
    let mut out = Vec::new();
    let mut cursor = 0;
    for m in matches {
        if m.start > cursor {
            let text = input[cursor..m.start].to_string();
            if !text.is_empty() {
                out.push(Fragment::Tts { text });
            }
        }
        out.push(Fragment::Sfx {
            pattern_index: m.pattern_index,
            text: input[m.start..m.end].to_string(),
            start: m.start,
            end: m.end,
        });
        cursor = m.end;
    }
    if cursor < input.len() {
        let text = input[cursor..].to_string();
        if !text.is_empty() {
            out.push(Fragment::Tts { text });
        }
    }
    out
}

struct FragmentOutcome {
    kind: FragmentKind,
    audio: ResolvedAudio,
    volume_db: f64,
    crossfade_ms: u32,
}

async fn resolve_fragment(
    state: &AppState,
    frag: Fragment,
    settings: &TtsSettings,
    forward_headers: &HeaderMap,
    full_input: &str,
) -> Result<FragmentOutcome> {
    let kind = FragmentKind::from(&frag);
    match frag {
        Fragment::Tts { text } => {
            info!(kind = "tts", chars = text.len(), "resolving fragment");
            let bytes = forward_tts(state, settings, &text, forward_headers).await?;
            info!(kind = "tts", bytes = bytes.len(), "fragment ready");
            Ok(FragmentOutcome {
                kind,
                audio: ResolvedAudio::Bytes(bytes),
                volume_db: state.config.overridable.volume_target_db,
                crossfade_ms: 0,
            })
        }
        Fragment::Sfx {
            pattern_index,
            text,
            start,
            end,
        } => {
            info!(kind = "sfx", text = %text, "resolving fragment");
            let m = PatternMatch {
                start,
                end,
                pattern_index,
            };
            match state.resolver.resolve_sfx(&m, full_input, &text).await? {
                Resolution::Cache(hit) => {
                    info!(kind = "sfx", path = %hit.path.display(), "fragment ready (cache)");
                    Ok(FragmentOutcome {
                        kind,
                        audio: ResolvedAudio::File(hit.path),
                        volume_db: hit.volume_db,
                        crossfade_ms: hit.crossfade_ms,
                    })
                }
                Resolution::Generate(generate_data) => {
                    let (bytes, _path) = generate_and_cache(state, &generate_data).await?;
                    info!(kind = "sfx", bytes = bytes.len(), "fragment ready (generated)");
                    Ok(FragmentOutcome {
                        kind,
                        audio: ResolvedAudio::Bytes(bytes),
                        volume_db: generate_data.pattern_config.volume_target_db,
                        crossfade_ms: generate_data.pattern_config.crossfade_ms,
                    })
                }
            }
        }
    }
}

#[tracing::instrument(
    level = "debug",
    skip(state, settings, forward_headers),
    fields(input_len = input.len(), model = %settings.model, voice = %settings.voice),
    err(level = "warn")
)]
async fn forward_tts(
    state: &AppState,
    settings: &TtsSettings,
    input: &str,
    forward_headers: &HeaderMap,
) -> Result<Vec<u8>> {
    let url = format!(
        "{}/audio/speech",
        state.config.overridable.tts_base_url.trim_end_matches('/')
    );
    let body = UpstreamSpeechBody {
        model: &settings.model,
        voice: &settings.voice,
        input,
        response_format: "wav",
        stream: false,
        speed: settings.speed,
        normalization_options: settings.normalization_options.clone(),
    };
    let mut req = state.http.post(&url).json(&body);
    for (name, value) in forward_headers.iter() {
        req = req.header(name, value);
    }
    let resp = req
        .send()
        .await
        .map_err(|e| err!(Network, "TTS request failed: {}", url, @source: e))?
        .error_for_status()
        .map_err(|e| err!(Network, "TTS backend error", @source: e))?;
    resp.bytes()
        .await
        .map(|b| b.to_vec())
        .map_err(|e| err!(Network, "failed to read TTS body", @source: e))
}

#[tracing::instrument(
    level = "debug",
    skip(state),
    fields(text = %generate_data.text, name = ?generate_data.name),
    err(level = "warn")
)]
async fn generate_and_cache(
    state: &AppState,
    generate_data: &GenerateData,
) -> Result<(Vec<u8>, PathBuf)> {
    let audio_bytes = state.resolver.generate(generate_data).await?;
    let ext = sfx_cache_extension(&generate_data.pattern_config.sfx_output_format);
    let hash = blake3::hash(&audio_bytes).to_hex().to_string();
    let filename = format!("{hash}.{ext}");
    let filepath = state.config.overridable.cache_dir.join(&filename);
    tokio::fs::write(&filepath, &audio_bytes)
        .await
        .map_err(|e| err!(Io, "failed to write cache file: {}", filepath.display(), @source: e))?;
    let (embed_model, embedded_at, embedding) = if let Some(ref emb) = generate_data.query_embedding
    {
        (
            Some(generate_data.pattern_config.embed_model.clone()),
            Some(Utc::now()),
            Some(Embeddings::from(emb.clone())),
        )
    } else {
        (None, None, None)
    };
    let meta = CacheMeta {
        matched_text: generate_data.text.clone(),
        context: None,
        version: String::new(),
        cache_tag: generate_data.cache_tag.clone(),
        recipe: generate_data.recipe.clone(),
        embed_model,
        embedded_at,
        embedding,
    };
    write_metadata(&filepath, &meta).await?;
    info!(path = %filepath.display(), bytes = audio_bytes.len(), "sfx cached");
    Ok((audio_bytes, filepath))
}

/// Strip hop-by-hop / body headers unsuitable for upstream TTS sub-requests (`json()` sets body headers).
fn forward_client_headers(headers: &mut HeaderMap) {
    const SKIP: &[HeaderName] = &[
        HOST,
        CONNECTION,
        CONTENT_LENGTH,
        CONTENT_TYPE,
        TRANSFER_ENCODING,
        TE,
        TRAILER,
        UPGRADE,
        PROXY_AUTHORIZATION,
        PROXY_AUTHENTICATE,
    ];

    for name in SKIP {
        headers.remove(name);
    }
}

fn sfx_cache_extension(output_format: &str) -> &'static str {
    let f = output_format.to_ascii_lowercase();
    if f.starts_with("wav") { "wav" } else { "mp3" }
}
