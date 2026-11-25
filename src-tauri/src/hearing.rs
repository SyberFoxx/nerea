//! Push-to-talk: the microphone, the ears, and the recording between them.
//!
//! Recording is `pw-record` writing to stdout, which means raw samples and no
//! container: a recorder killed mid-sentence cannot leave a broken header
//! behind, because there is no header to break. The WAV is built afterwards,
//! from the bytes that actually arrived.
//!
//! Transcription is `voice/serve_stt.py`, supervised exactly like the voice
//! engine: adopted when it already listens, spawned on demand, stopped after a
//! quiet spell. It speaks HTTP on its own port and never blocks a reply.

use std::{
    net::{SocketAddr, TcpStream},
    path::PathBuf,
    process::{Child, Command, Stdio},
    sync::{
        Mutex,
        atomic::{AtomicU64, Ordering},
    },
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use serde::Deserialize;

const DEFAULT_PORT: u16 = 9881;
/// The very first run downloads the model; every run after it is seconds.
const START_TIMEOUT: Duration = Duration::from_secs(300);
const READY_POLL: Duration = Duration::from_millis(500);
/// After a failed attempt, do not try again for a while: loading is expensive.
const COOLDOWN: Duration = Duration::from_secs(180);
/// Stop the ears after this long without using them. Zero disables the reaper.
const DEFAULT_IDLE_SECS: u64 = 900;
/// A key that gets stuck must not record the whole afternoon.
const MAX_RECORD_SECS: u64 = 30;

static CHILD: Mutex<Option<Child>> = Mutex::new(None);
static RECORDING: Mutex<Option<Recording>> = Mutex::new(None);
static TOKEN: AtomicU64 = AtomicU64::new(0);
static LAST_USED: AtomicU64 = AtomicU64::new(0);
static LAST_FAILURE: AtomicU64 = AtomicU64::new(0);

struct Recording {
    child: Child,
    path: PathBuf,
    started: Instant,
    token: u64,
}

fn now_secs() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|elapsed| elapsed.as_secs()).unwrap_or_default()
}

pub fn port() -> u16 {
    std::env::var("NEREA_STT_PORT")
        .ok()
        .and_then(|value| value.parse().ok())
        .unwrap_or(DEFAULT_PORT)
}

fn idle_limit() -> u64 {
    std::env::var("NEREA_STT_IDLE_SECS")
        .ok()
        .and_then(|value| value.parse().ok())
        .unwrap_or(DEFAULT_IDLE_SECS)
}

/// The checkout this build belongs to: the ear lives inside it.
fn repo_root() -> Option<PathBuf> {
    if let Ok(exe) = std::env::current_exe() {
        for ancestor in exe.ancestors().skip(1) {
            if ancestor.join("voice").join("serve_stt.py").is_file() {
                return Some(ancestor.to_path_buf());
            }
        }
    }

    let here = PathBuf::from(env!("CARGO_MANIFEST_DIR")).parent()?.to_path_buf();
    here.join("voice").join("serve_stt.py").is_file().then_some(here)
}

fn python() -> Option<PathBuf> {
    if let Ok(path) = std::env::var("NEREA_STT_PYTHON") {
        let path = PathBuf::from(path);
        return path.is_file().then_some(path);
    }

    repo_root().map(|root| root.join("tools/voice/.venv/bin/python")).filter(|path| path.is_file())
}

fn script() -> Option<PathBuf> {
    repo_root().map(|root| root.join("voice/serve_stt.py"))
}

fn is_listening() -> bool {
    let address = SocketAddr::from(([127, 0, 0, 1], port()));
    TcpStream::connect_timeout(&address, Duration::from_millis(250)).is_ok()
}

fn spawn() -> Result<(), String> {
    if is_listening() {
        return Ok(());
    }

    let python = python().ok_or("no ear on this machine: missing tools/voice/.venv")?;
    let script = script().ok_or("no ear on this machine: missing voice/serve_stt.py")?;
    let root = repo_root().ok_or("no checkout to run the ear from")?;

    let log = std::env::temp_dir().join("nerea-stt.log");
    let stdout = std::fs::File::create(&log).map_err(|err| err.to_string())?;
    let stderr = stdout.try_clone().map_err(|err| err.to_string())?;

    let child = Command::new(python)
        .arg(script)
        .current_dir(&root)
        .env("NEREA_STT_PARENT", std::process::id().to_string())
        .env("NEREA_STT_PORT", port().to_string())
        .stdin(Stdio::null())
        .stdout(stdout)
        .stderr(stderr)
        .spawn()
        .map_err(|err| format!("could not start the ear: {err}"))?;

    let mut guard = CHILD.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    *guard = Some(child);
    Ok(())
}

fn mark_used() {
    LAST_USED.store(now_secs(), Ordering::Relaxed);
}

/// Starts the ear if needed and waits until the model answers.
///
/// A cold start is slow on purpose: the first run downloads the weights, and
/// pretending that is instant would make the first sentence vanish.
pub async fn ensure() -> Result<(), String> {
    if is_listening() {
        mark_used();
    } else {
        let since_failure = now_secs().saturating_sub(LAST_FAILURE.load(Ordering::Relaxed));
        if LAST_FAILURE.load(Ordering::Relaxed) > 0 && since_failure < COOLDOWN.as_secs() {
            return Err(format!(
                "el oído falló hace {since_failure}s; lo reintento en {}s",
                COOLDOWN.as_secs().saturating_sub(since_failure)
            ));
        }

        spawn()?;
    }

    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(3))
        .build()
        .map_err(|err| err.to_string())?;
    let url = format!("http://127.0.0.1:{}/health", port());
    let deadline = Instant::now() + START_TIMEOUT;
    let mut announced = false;

    loop {
        if let Ok(response) = client.get(&url).send().await
            && let Ok(health) = response.json::<Health>().await
        {
            if health.ready {
                mark_used();
                return Ok(());
            }
            if let Some(error) = health.error {
                LAST_FAILURE.store(now_secs(), Ordering::Relaxed);
                return Err(format!("el oído no pudo cargar el modelo: {error}"));
            }
            if !announced {
                announced = true;
                eprintln!("nerea ear: cargando el modelo (la primera vez descarga)");
            }
        }

        if Instant::now() >= deadline {
            LAST_FAILURE.store(now_secs(), Ordering::Relaxed);
            return Err("el oído no arrancó a tiempo".to_owned());
        }
        tokio::time::sleep(READY_POLL).await;
    }
}

#[derive(Deserialize)]
struct Health {
    #[serde(default)]
    ready: bool,
    #[serde(default)]
    error: Option<String>,
}

#[derive(Deserialize)]
struct Transcript {
    #[serde(default)]
    text: String,
    #[serde(default)]
    seconds: f32,
    #[serde(default)]
    took: f32,
}

/// Where the raw samples land while the key is held.
fn scratch_path() -> PathBuf {
    std::env::var("XDG_RUNTIME_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|_| std::env::temp_dir())
        .join("nerea")
        .join("listening.raw")
}

pub fn is_recording() -> bool {
    RECORDING.lock().map(|guard| guard.is_some()).unwrap_or(false)
}

/// Starts recording. The key press is the only thing that calls this.
pub fn start() -> Result<(), String> {
    if is_recording() {
        return Err("ya estoy escuchando".to_owned());
    }

    let path = scratch_path();
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|err| format!("no puedo preparar la grabación: {err}"))?;
    }

    let output = std::fs::File::create(&path)
        .map_err(|err| format!("no puedo preparar la grabación: {err}"))?;

    // `arecord`, not `pw-record`: on this machine the native PipeWire capture
    // path delivers a signal pinned at full scale (measured: RMS 18800 for a
    // silent room, peak 32768 at every gain and every input source), while the
    // same microphone through the ALSA default lands at RMS 360. A saturated
    // signal is not noise to filter — the waveform is already destroyed.
    //
    // `-t raw` with no file means samples on stdout: nothing to corrupt when the
    // recorder is killed mid-sentence.
    let device = std::env::var("NEREA_MIC").unwrap_or_else(|_| "default".to_owned());
    let child = Command::new("arecord")
        .args(["-D", &device, "-f", "S16_LE", "-r", "16000", "-c", "1", "-t", "raw", "-q"])
        .stdin(Stdio::null())
        .stdout(output)
        .stderr(Stdio::null())
        .spawn()
        .map_err(|err| format!("no puedo abrir el micrófono: {err}"))?;

    let token = TOKEN.fetch_add(1, Ordering::Relaxed) + 1;
    let mut guard = RECORDING.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    *guard = Some(Recording { child, path, started: Instant::now(), token });
    drop(guard);

    // A key whose release never arrives stops itself.
    std::thread::spawn(move || {
        std::thread::sleep(Duration::from_secs(MAX_RECORD_SECS));
        let mut guard = RECORDING.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        if guard.as_ref().is_some_and(|recording| recording.token == token)
            && let Some(mut recording) = guard.take()
        {
            let _ = recording.child.kill();
            let _ = recording.child.wait();
            eprintln!("nerea ear: grabación cortada a los {MAX_RECORD_SECS}s");
        }
    });

    Ok(())
}

/// Stops recording and returns the WAV, or the reason there is no sentence.
pub fn stop() -> Result<Vec<u8>, String> {
    let recording = {
        let mut guard = RECORDING.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        guard.take().ok_or_else(|| "no estaba escuchando".to_owned())?
    };

    let mut child = recording.child;
    let _ = child.kill();
    let _ = child.wait();

    let seconds = recording.started.elapsed().as_secs_f32();
    if !nerea_core::hearing::long_enough(seconds) {
        let _ = std::fs::remove_file(&recording.path);
        return Err("muy corto".to_owned());
    }

    let mut samples = std::fs::read(&recording.path)
        .map_err(|err| format!("no pude leer la grabación: {err}"))?;

    // The last utterance stays in the runtime dir (tmpfs, overwritten every
    // time) because "she doesn't hear me" cannot be debugged without the audio
    // that reached her: the peak alone says whether the microphone was quiet.
    let peak = samples
        .as_chunks::<2>()
        .0
        .iter()
        .map(|pair| i16::from_le_bytes(*pair).unsigned_abs())
        .max()
        .unwrap_or(0);
    eprintln!(
        "nerea ear: {seconds:.1}s grabados, pico {peak}, audio en {}",
        recording.path.display()
    );

    if peak <= 200 {
        return Err("silencio".to_owned());
    }

    // A quiet microphone is not a quiet room: lift the level before asking the
    // model to find words in it.
    nerea_core::hearing::normalize(&mut samples);

    Ok(nerea_core::hearing::wav(&samples))
}

/// Sends the audio to the ear and returns what it heard.
pub async fn transcribe(audio: Vec<u8>) -> Result<String, String> {
    ensure().await?;
    mark_used();

    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(120))
        .build()
        .map_err(|err| err.to_string())?;
    let url = format!("http://127.0.0.1:{}/transcribe", port());

    let response = client.post(&url).body(audio).send().await.map_err(|err| err.to_string())?;
    let status = response.status();
    let body = response.text().await.unwrap_or_default();

    if !status.is_success() {
        return Err(format!("el oído respondió {status}: {}", body.chars().take(200).collect::<String>()));
    }

    let transcript: Transcript =
        serde_json::from_str(&body).map_err(|err| format!("respuesta ilegible del oído: {err}"))?;

    eprintln!(
        "nerea ear: {}s de audio en {}s de reloj",
        transcript.seconds, transcript.took
    );
    Ok(transcript.text)
}

/// Stops the ear: the model is half a gigabyte of memory nobody needs at rest.
pub fn stop_engine() {
    let mut guard = CHILD.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    if let Some(mut child) = guard.take() {
        let _ = child.kill();
        let _ = child.wait();
    }
}

/// Kills the ear after a quiet spell, unless it was used recently.
pub fn reap() {
    let limit = idle_limit();
    if limit == 0 {
        return;
    }

    let idle = now_secs().saturating_sub(LAST_USED.load(Ordering::Relaxed));
    if LAST_USED.load(Ordering::Relaxed) > 0 && idle >= limit && !is_recording() {
        eprintln!("nerea ear: {idle}s sin usarse, lo cierro");
        stop_engine();
        LAST_USED.store(0, Ordering::Relaxed);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_scratch_path_is_inside_the_runtime_dir() {
        let path = scratch_path();
        assert!(path.ends_with("nerea/listening.raw"), "{}", path.display());
    }

}
