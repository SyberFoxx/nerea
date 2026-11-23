//! Live voice: the reply, spoken by the local engine.
//!
//! The engine is GPT-SoVITS on `127.0.0.1:9880` (see `voice/serve_tts.py`).
//! It is optional: when it is not running, this fails quietly and the app falls
//! back to the pre-rendered bank, so a missing engine never breaks a reply.

use std::{
    path::{Path, PathBuf},
    process::Command,
    time::Duration,
};

use serde::Serialize;

const DEFAULT_URL: &str = "http://127.0.0.1:9880";
const SYNTHESIS_TIMEOUT: Duration = Duration::from_secs(30);
/// What the small subtitle can hold.
const SUBTITLE_CHARS: usize = 160;

/// One spoken line, shaped exactly like a bank line so the UI treats it the same.
#[derive(Debug, Clone, Serialize)]
pub struct LiveLine {
    pub id: String,
    pub category: &'static str,
    pub ja: String,
    pub es: String,
    pub duration_ms: u64,
    pub played: bool,
}

fn base_url() -> String {
    std::env::var("NEREA_TTS_URL").unwrap_or_else(|_| DEFAULT_URL.to_owned())
}

/// The reference clip the voice was cloned from. Machine-local by nature.
pub fn reference_audio() -> Option<PathBuf> {
    if let Ok(path) = std::env::var("NEREA_REF_AUDIO") {
        let path = PathBuf::from(path);
        return path.is_file().then_some(path);
    }

    let from_bank = std::env::var("NEREA_BANK_DIR")
        .map(PathBuf::from)
        .ok()
        .and_then(|dir| dir.parent().and_then(|voice| voice.join("ref").join("semiramis-ref.wav").is_file().then(|| voice.join("ref").join("semiramis-ref.wav"))));

    if from_bank.is_some() {
        return from_bank;
    }

    // The engine runs from its own directory, so a relative path would point
    // nowhere once it gets there.
    let fallback = PathBuf::from("tools/voice/ref/semiramis-ref.wav");
    fallback.is_file().then(|| fallback.canonicalize().unwrap_or(fallback))
}

fn scratch_path() -> PathBuf {
    let dir = std::env::var_os("XDG_RUNTIME_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir);
    dir.join("nerea").join("line.wav")
}

/// Reads duration straight from the WAV headers instead of spawning ffprobe.
pub fn wav_duration_ms(path: &Path) -> Option<u64> {
    let bytes = std::fs::read(path).ok()?;
    if bytes.len() < 44 || &bytes[0..4] != b"RIFF" || &bytes[8..12] != b"WAVE" {
        return None;
    }

    let mut offset = 12;
    let (mut channels, mut rate, mut bits) = (1u64, 32_000u64, 16u64);

    while offset + 8 <= bytes.len() {
        let id = &bytes[offset..offset + 4];
        let size = u32::from_le_bytes(bytes[offset + 4..offset + 8].try_into().ok()?) as usize;
        let body = offset + 8;

        if id == b"fmt " && body + 16 <= bytes.len() {
            channels = u16::from_le_bytes([bytes[body + 2], bytes[body + 3]]) as u64;
            rate = u32::from_le_bytes([
                bytes[body + 4],
                bytes[body + 5],
                bytes[body + 6],
                bytes[body + 7],
            ]) as u64;
            bits = u16::from_le_bytes([bytes[body + 14], bytes[body + 15]]) as u64;
        } else if id == b"data" {
            let bytes_per_second = rate * channels * bits.max(8) / 8;
            if bytes_per_second == 0 {
                return None;
            }
            let available = size.min(bytes.len().saturating_sub(body)) as u64;
            return Some(available * 1000 / bytes_per_second);
        }

        offset = body + size + (size & 1);
    }

    None
}

/// Synthesizes the Japanese line and starts playing it detached.
pub async fn speak(ja: &str, spanish: &str, reference_text: &str) -> Result<LiveLine, String> {
    let ja = ja.trim();
    if ja.is_empty() {
        return Err("empty line".to_owned());
    }

    let reference = reference_audio().ok_or("no reference clip; set NEREA_REF_AUDIO")?;

    let mut url = reqwest::Url::parse(&base_url()).map_err(|err| err.to_string())?;
    url.path_segments_mut().map_err(|_| "bad tts url".to_owned())?.push("tts");
    url.query_pairs_mut()
        .append_pair("text", ja)
        .append_pair("text_lang", "ja")
        .append_pair("ref_audio_path", &reference.to_string_lossy())
        .append_pair("prompt_lang", "ja")
        .append_pair("prompt_text", reference_text)
        .append_pair("text_split_method", "cut0")
        .append_pair("media_type", "wav");

    let client = reqwest::Client::builder()
        .timeout(SYNTHESIS_TIMEOUT)
        .build()
        .map_err(|err| err.to_string())?;

    let response = client.get(url).send().await.map_err(|err| err.to_string())?;
    if !response.status().is_success() {
        let status = response.status();
        let detail = response.text().await.unwrap_or_default();
        return Err(format!("engine answered {status}: {}", detail.chars().take(300).collect::<String>()));
    }

    let audio = response.bytes().await.map_err(|err| err.to_string())?;
    if audio.len() < 1024 {
        return Err("engine returned no audio".to_owned());
    }

    let path = scratch_path();
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    std::fs::write(&path, &audio).map_err(|err| err.to_string())?;

    // Detached: the UI must not wait for the audio to finish.
    let _ = Command::new("pw-play").arg(&path).spawn();

    let subtitle: String = spanish.trim().chars().take(SUBTITLE_CHARS).collect();

    Ok(LiveLine {
        id: "live".to_owned(),
        category: "talking",
        ja: ja.to_owned(),
        es: subtitle,
        duration_ms: wav_duration_ms(&path).unwrap_or(2_500),
        played: true,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_missing_clip_is_reported_not_guessed() {
        // No env override in tests: whatever the machine has, the answer is a
        // path that exists or nothing at all.
        if let Some(path) = reference_audio() {
            assert!(path.is_file());
        }
    }

    #[test]
    fn the_scratch_file_stays_out_of_the_repository() {
        let path = scratch_path();
        assert!(path.ends_with("line.wav"));
        assert!(!path.starts_with("assets"));
    }
}
