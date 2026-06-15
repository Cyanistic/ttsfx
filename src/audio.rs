use crate::{Result, bail, err};
use std::path::Path;
use std::process::Stdio;
use tokio::io::AsyncWriteExt;
use tokio::process::Command;

/// A decoded audio segment with f32 samples.
#[derive(Debug, Clone)]
pub struct AudioSegment {
    pub samples: Vec<f32>,
    pub sample_rate: u32,
    pub channels: u16,
}

/// Check that ffmpeg is available at startup.
pub async fn check_ffmpeg() -> Result<()> {
    let output = Command::new("ffmpeg")
        .args(["-version"])
        .output()
        .await
        .map_err(|e| err!(Configuration, "ffmpeg not found — install ffmpeg to use ttsfx", @external: e))?;
    if !output.status.success() {
        bail!(Configuration, "ffmpeg installed but not working");
    }
    Ok(())
}

/// Decode any audio format to f32 samples via ffmpeg.
pub async fn decode(bytes: &[u8], target_rate: u32) -> Result<AudioSegment> {
    let mut child = Command::new("ffmpeg")
        .args([
            "-hide_banner",
            "-loglevel",
            "error",
            "-i",
            "pipe:0",
            "-f",
            "s16le",
            "-ar",
            &target_rate.to_string(),
            "-ac",
            "1",
            "pipe:1",
        ])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| err!(Internal, "failed to spawn ffmpeg", @external: e))?;

    if let Some(mut stdin) = child.stdin.take() {
        stdin
            .write_all(bytes)
            .await
            .map_err(|e| err!(Internal, "failed to write to ffmpeg stdin", @external: e))?;
    }

    let output = child
        .wait_with_output()
        .await
        .map_err(|e| err!(Internal, "ffmpeg process failed", @external: e))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        bail!(AudioDecodeError, "ffmpeg decode failed: {}", stderr);
    }

    parse_pcm_s16le(&output.stdout, target_rate)
}

/// Decode a file on disk via ffmpeg.
pub async fn decode_file(path: &Path, target_rate: u32) -> Result<AudioSegment> {
    let child = Command::new("ffmpeg")
        .args([
            "-hide_banner",
            "-loglevel",
            "error",
            "-i",
            &path.to_string_lossy(),
            "-f",
            "s16le",
            "-ar",
            &target_rate.to_string(),
            "-ac",
            "1",
            "pipe:1",
        ])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| {
            err!(
                Internal,
                "failed to spawn ffmpeg for {}",
                path.display(),
                @external: e
            )
        })?;

    let output = child.wait_with_output().await.map_err(|e| {
        err!(
            Internal,
            "ffmpeg process failed for {}",
            path.display(),
            @external: e
        )
    })?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        bail!(
            AudioDecodeError,
            "ffmpeg failed to decode {}: {}",
            path.display(),
            stderr
        );
    }

    parse_pcm_s16le(&output.stdout, target_rate)
}

/// Parse raw little-endian s16le mono PCM from ffmpeg `-f s16le` output.
fn parse_pcm_s16le(pcm_bytes: &[u8], sample_rate: u32) -> Result<AudioSegment> {
    if pcm_bytes.len() % 2 != 0 {
        bail!(
            AudioDecodeError,
            "ffmpeg PCM length is not aligned to 16-bit samples ({} bytes)",
            pcm_bytes.len()
        );
    }
    let num_samples = pcm_bytes.len() / 2;
    let samples: Vec<f32> = (0..num_samples)
        .map(|i| {
            let raw = i16::from_le_bytes([pcm_bytes[i * 2], pcm_bytes[i * 2 + 1]]);
            raw as f32 / i16::MAX as f32
        })
        .collect();
    Ok(AudioSegment {
        samples,
        sample_rate,
        channels: 1,
    })
}

/// Compute RMS (root mean square) loudness of a sample buffer.
pub fn rms_level(samples: &[f32]) -> f32 {
    if samples.is_empty() {
        return 0.0;
    }
    let sum: f32 = samples.iter().map(|s| s * s).sum();
    (sum / samples.len() as f32).sqrt()
}

/// Adjust segment gain so its RMS matches the target level.
pub fn match_loudness(segment: &mut AudioSegment, target_rms: f32) {
    let current_rms = rms_level(&segment.samples);
    if current_rms < 1e-10 || target_rms < 1e-10 {
        return; // avoid division by zero on silence
    }
    let gain = target_rms / current_rms;
    for sample in &mut segment.samples {
        *sample = (*sample * gain).clamp(-1.0, 1.0);
    }
}

/// Concatenate segments into one. Returns `Err(segments)` if sample rate or
/// channel count don't match — caller gets the vec back to retry.
pub fn concat_segments(mut segments: Vec<AudioSegment>) -> Result<AudioSegment, Vec<AudioSegment>> {
    let Some(first) = segments.first() else {
        return Err(segments);
    };
    let rate = first.sample_rate;
    let channels = first.channels;

    if segments
        .iter()
        .any(|s| s.sample_rate != rate || s.channels != channels)
    {
        return Err(segments);
    }

    let samples: Vec<f32> = segments.drain(..).flat_map(|s| s.samples).collect();

    Ok(AudioSegment {
        samples,
        sample_rate: rate,
        channels,
    })
}

/// Concatenate with a linear crossfade per join. `crossfade_samples[i]` is the overlap between
/// segment `i` and `i + 1`; length must be `segments.len().saturating_sub(1)`. `0` = hard join.
pub fn concat_segments_crossfaded_variable(
    segments: Vec<AudioSegment>,
    crossfade_samples: &[usize],
) -> Result<AudioSegment, Vec<AudioSegment>> {
    if segments.is_empty() {
        return Err(segments);
    }
    if segments.len() == 1 {
        return Ok(segments.into_iter().next().unwrap());
    }
    if crossfade_samples.len() != segments.len() - 1 {
        return Err(segments);
    }

    let rate = segments[0].sample_rate;
    let channels = segments[0].channels;
    if segments
        .iter()
        .any(|s| s.sample_rate != rate || s.channels != channels)
    {
        return Err(segments);
    }

    let mut out = segments[0].samples.clone();
    for (seg, &cf) in segments.into_iter().skip(1).zip(crossfade_samples.iter()) {
        let next = seg.samples;
        if cf < 2 {
            out.extend(next);
            continue;
        }
        let n = cf.min(out.len()).min(next.len());
        if n < 2 {
            out.extend(next);
            continue;
        }
        let tail_start = out.len() - n;
        for i in 0..n {
            let t = i as f32 / (n - 1) as f32;
            let a = out[tail_start + i];
            let b = next[i];
            out[tail_start + i] = a * (1.0 - t) + b * t;
        }
        out.extend_from_slice(&next[n..]);
    }

    Ok(AudioSegment {
        samples: out,
        sample_rate: rate,
        channels,
    })
}

/// Encode f32 samples as MP3 via ffmpeg.
pub async fn encode_mp3(samples: &[f32], sample_rate: u32) -> Result<Vec<u8>> {
    encode_ffmpeg(samples, sample_rate, "mp3", "libmp3lame", &["-q:a", "2"]).await
}

/// Encode f32 samples as 16-bit PCM WAV in memory (no ffmpeg — PCM is just header + samples).
pub async fn encode_wav(samples: &[f32], sample_rate: u32) -> Result<Vec<u8>> {
    let pcm: Vec<i16> = samples
        .iter()
        .map(|&s| (s.clamp(-1.0, 1.0) * i16::MAX as f32) as i16)
        .collect();
    let data_size = pcm.len() * 2;
    let mut out = Vec::with_capacity(44 + data_size);
    out.extend_from_slice(b"RIFF");
    out.extend_from_slice(&(36u32 + data_size as u32).to_le_bytes());
    out.extend_from_slice(b"WAVEfmt ");
    out.extend_from_slice(&16u32.to_le_bytes());
    out.extend_from_slice(&1u16.to_le_bytes()); // PCM
    out.extend_from_slice(&1u16.to_le_bytes()); // mono
    out.extend_from_slice(&sample_rate.to_le_bytes());
    out.extend_from_slice(&(sample_rate * 2).to_le_bytes()); // byte rate
    out.extend_from_slice(&2u16.to_le_bytes()); // block align
    out.extend_from_slice(&16u16.to_le_bytes()); // bits per sample
    out.extend_from_slice(b"data");
    out.extend_from_slice(&(data_size as u32).to_le_bytes());
    for s in pcm {
        out.extend_from_slice(&s.to_le_bytes());
    }
    Ok(out)
}

/// Generic ffmpeg encode: pipe raw PCM in, get encoded bytes out.
async fn encode_ffmpeg(
    samples: &[f32],
    sample_rate: u32,
    format: &str,
    codec: &str,
    extra_args: &[&str],
) -> Result<Vec<u8>> {
    let pcm: Vec<u8> = samples
        .iter()
        .flat_map(|&s| {
            let pcm = (s * i16::MAX as f32) as i16;
            pcm.to_le_bytes()
        })
        .collect();

    let rate_str = sample_rate.to_string();
    let mut args = vec![
        "-hide_banner",
        "-loglevel",
        "error",
        "-f",
        "s16le",
        "-ar",
        &rate_str,
        "-ac",
        "1",
        "-i",
        "pipe:0",
        "-f",
        format,
        "-acodec",
        codec,
    ];
    args.extend_from_slice(extra_args);
    args.push("pipe:1");

    let mut child = Command::new("ffmpeg")
        .args(&args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| {
            err!(
                Internal,
                "failed to spawn ffmpeg for encoding",
                @external: e
            )
        })?;

    if let Some(mut stdin) = child.stdin.take() {
        stdin
            .write_all(&pcm)
            .await
            .map_err(|e| err!(Internal, "failed to write PCM to ffmpeg", @external: e))?;
    }

    let output = child
        .wait_with_output()
        .await
        .map_err(|e| err!(Internal, "ffmpeg encode failed", @external: e))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        bail!(AudioDecodeError, "ffmpeg encode failed: {}", stderr);
    }

    Ok(output.stdout)
}
