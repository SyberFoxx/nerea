//! Hearing her owner: the parts of push-to-talk that are pure.
//!
//! Recording and transcription need a microphone and a model, so they live in
//! the shell. What can be decided without either — turning raw samples into a
//! WAV, and turning a raw transcript into something worth answering — is here,
//! where it can be tested.

/// Push-to-talk shorter than this is a key that slipped, not a sentence.
pub const MIN_SECONDS: f32 = 0.35;

/// Sample rate the model wants. `pw-record` is asked for the same one.
pub const SAMPLE_RATE: u32 = 16_000;

/// What Whisper writes when it hears silence, in this language. Whole-clip
/// matches only: somebody may well say "gracias" and mean it.
const SILENCE_ARTIFACTS: &[&str] = &[
    "gracias",
    "gracias.",
    "¡gracias!",
    "gracias por ver el video",
    "gracias por ver el vídeo",
    "subtítulos realizados por la comunidad de amara.org",
    "subtitulado por",
    "¡suscríbete!",
    "suscríbete",
    "suscríbete al canal",
    "no olvides suscribirte",
    "dale like y suscríbete",
    "¡gracias por ver el video!",
    "¡gracias por ver el vídeo!",
    "música",
    "[música]",
    "(música)",
    "[aplausos]",
    "(aplausos)",
    "...",
];

/// A 44-byte RIFF header for 16-bit mono or stereo PCM.
///
/// `pw-record --raw` writes samples and no container, precisely so a killed
/// recorder cannot leave a half-written header behind: the header is built here,
/// once, from the bytes that actually arrived.
pub fn wav_header(channels: u16, data_len: usize) -> Vec<u8> {
    let byte_rate = SAMPLE_RATE * u32::from(channels) * 2;
    let block_align = channels * 2;

    let mut header = Vec::with_capacity(44);
    header.extend_from_slice(b"RIFF");
    header.extend_from_slice(&((36 + data_len) as u32).to_le_bytes());
    header.extend_from_slice(b"WAVE");
    header.extend_from_slice(b"fmt ");
    header.extend_from_slice(&16u32.to_le_bytes());
    header.extend_from_slice(&1u16.to_le_bytes());
    header.extend_from_slice(&channels.to_le_bytes());
    header.extend_from_slice(&SAMPLE_RATE.to_le_bytes());
    header.extend_from_slice(&byte_rate.to_le_bytes());
    header.extend_from_slice(&block_align.to_le_bytes());
    header.extend_from_slice(&16u16.to_le_bytes());
    header.extend_from_slice(b"data");
    header.extend_from_slice(&(data_len as u32).to_le_bytes());
    header
}

/// Header plus samples, ready to post.
pub fn wav(samples: &[u8]) -> Vec<u8> {
    let mut out = wav_header(1, samples.len());
    out.extend_from_slice(samples);
    out
}

/// The sentence, or nothing at all.
///
/// A leading "Nerea" is how he addresses her, not part of the question. Silence
/// artefacts are what the model writes when there was no speech, and handing
/// those to her would make her answer a sentence nobody said.
pub fn clean(transcript: &str) -> Option<String> {
    let collapsed = transcript.split_whitespace().collect::<Vec<_>>().join(" ");
    let text = strip_name(&collapsed).trim();

    if text.chars().count() < 2 {
        return None;
    }

    let lower = text.to_lowercase();
    if SILENCE_ARTIFACTS.contains(&lower.as_str()) {
        return None;
    }

    // A clip that came back as brackets only is a caption, not a sentence.
    if text.starts_with('[') && text.ends_with(']') {
        return None;
    }

    // The model was trained on subtitled video and falls back to crediting them
    // whenever the clip is mostly silence: two measured variants in a row.
    if lower.starts_with("subtítulos") || lower.starts_with("subtitulos") {
        return None;
    }

    Some(text.to_owned())
}

/// Drops one leading "Nerea" and the punctuation that followed it. Anywhere
/// else in the sentence the name is part of what he said.
fn strip_name(text: &str) -> &str {
    let lower = text.to_lowercase();
    if !lower.starts_with("nerea") {
        return text;
    }

    // Separators only: a leading «¿» belongs to what he said, not to her name.
    text["nerea".len()..]
        .trim_start_matches(|character: char| character.is_whitespace() || ",;:.-–—".contains(character))
}

/// What a healthy recording should peak at, in 16-bit units.
const TARGET_PEAK: i32 = 9_000;
/// Never amplify past this, or the room noise becomes the loudest thing.
const MAX_GAIN: i32 = 12_000;

/// Lifts a quiet recording to a level the model can hear.
///
/// A microphone at 18% of its range peaks around -45 dB, which is silence as far
/// as speech recognition is concerned: it was, measured, the reason a whole
/// sentence came back as "no te escuché". The app compensates instead of asking
/// the user to keep his system gain at a specific number.
pub fn normalize(samples: &mut [u8]) {
    let peak = samples
        .as_chunks::<2>()
        .0
        .iter()
        .map(|pair| i32::from(i16::from_le_bytes(*pair).unsigned_abs()))
        .max()
        .unwrap_or(0);
    if peak == 0 || peak >= TARGET_PEAK {
        return;
    }

    let gain = (TARGET_PEAK * 1_000 / peak).min(MAX_GAIN);
    for chunk in samples.as_chunks_mut::<2>().0 {
        let value = i32::from(i16::from_le_bytes(*chunk));
        let scaled = (value * gain / 1_000).clamp(i32::from(i16::MIN), i32::from(i16::MAX));
        *chunk = (scaled as i16).to_le_bytes();
    }
}

/// Whether the recording is long enough to be a sentence.
pub fn long_enough(seconds: f32) -> bool {
    seconds >= MIN_SECONDS
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_header_is_a_wav_a_decoder_accepts() {
        let header = wav_header(1, 32_000);
        assert_eq!(header.len(), 44);
        assert_eq!(&header[0..4], b"RIFF");
        assert_eq!(&header[8..12], b"WAVE");
        assert_eq!(&header[12..16], b"fmt ");
        assert_eq!(&header[36..40], b"data");

        // 36 + the samples, and the sample count, both little endian.
        assert_eq!(u32::from_le_bytes(header[4..8].try_into().expect("riff size")), 36 + 32_000);
        assert_eq!(u32::from_le_bytes(header[40..44].try_into().expect("data size")), 32_000);
        assert_eq!(u16::from_le_bytes(header[20..22].try_into().expect("channels")), 1);
        assert_eq!(u32::from_le_bytes(header[24..28].try_into().expect("rate")), SAMPLE_RATE);
        assert_eq!(
            u32::from_le_bytes(header[28..32].try_into().expect("byte rate")),
            SAMPLE_RATE * 2
        );
    }

    #[test]
    fn the_file_is_the_header_plus_the_samples() {
        let samples = vec![0u8; 1_000];
        let file = wav(&samples);
        assert_eq!(file.len(), 44 + samples.len());
        assert_eq!(&file[44..], &samples[..]);
    }

    #[test]
    fn the_name_she_is_called_by_is_not_part_of_the_question() {
        assert_eq!(clean("Nerea, ¿qué hora es?").as_deref(), Some("¿qué hora es?"));
        assert_eq!(clean("nerea decime algo").as_deref(), Some("decime algo"));
        assert_eq!(clean(" Nerea:   ¿estás ahí? ").as_deref(), Some("¿estás ahí?"));
    }

    #[test]
    fn the_name_inside_a_sentence_is_kept() {
        assert_eq!(
            clean("le dije a Nerea que venga").as_deref(),
            Some("le dije a Nerea que venga")
        );
    }

    #[test]
    fn silence_artifacts_are_not_a_sentence() {
        for artifact in [
            "¡Gracias!",
            "Gracias.",
            "¡Suscríbete!",
            "suscríbete al canal",
            "subtítulos en español de la iglesia de jesucristo de los santos de los últimos días",
            "[Música]",
            "(música)",
            "...",
            "   ",
            "Nerea",
        ] {
            assert!(clean(artifact).is_none(), "{artifact:?} should be dropped");
        }
    }

    #[test]
    fn a_real_sentence_survives_with_its_punctuation() {
        let said = "me gustaría saber qué opinás de todo esto, ¿vos qué harías?";
        assert_eq!(clean(said).as_deref(), Some(said));
    }

    #[test]
    fn whitespace_is_collapsed() {
        assert_eq!(clean("  decime   algo  ").as_deref(), Some("decime algo"));
    }

    fn peak_of(samples: &[u8]) -> i32 {
        samples
            .as_chunks::<2>()
            .0
            .iter()
            .map(|pair| i32::from(i16::from_le_bytes(*pair).unsigned_abs()))
            .max()
            .unwrap_or(0)
    }

    fn samples(values: &[i16]) -> Vec<u8> {
        values.iter().flat_map(|value| value.to_le_bytes()).collect()
    }

    #[test]
    fn a_quiet_recording_is_lifted_into_range() {
        // 18% of full scale: the level that made a real sentence vanish.
        let mut quiet = samples(&[0, 400, -400, 800, -800]);
        normalize(&mut quiet);

        let peak = peak_of(&quiet);
        assert!((9_000..=9_400).contains(&peak), "peak {peak}");
    }

    #[test]
    fn a_loud_recording_is_left_alone() {
        let original = samples(&[0, 20_000, -20_000]);
        let mut loud = original.clone();
        normalize(&mut loud);
        assert_eq!(loud, original);
    }

    #[test]
    fn normalization_never_overflows_a_sample() {
        let mut tiny = samples(&[1, -1, 3]);
        normalize(&mut tiny);
        for pair in tiny.as_chunks::<2>().0 {
            let value = i32::from(i16::from_le_bytes(*pair));
            assert!(value.abs() < i32::from(i16::MAX), "{value}");
        }
    }

    #[test]
    fn silence_stays_silent() {
        let mut nothing = samples(&[0, 0, 0]);
        normalize(&mut nothing);
        assert_eq!(peak_of(&nothing), 0);
    }

    #[test]
    fn a_key_that_slipped_is_not_a_sentence() {
        assert!(!long_enough(0.1));
        assert!(!long_enough(MIN_SECONDS - 0.01));
        assert!(long_enough(MIN_SECONDS));
        assert!(long_enough(2.5));
    }
}
