"""Speech to text for the push-to-talk path.

faster-whisper on CPU with int8 weights by default: the voice engine already
owns the GPU, and two models fighting for one card is a lesson this machine
already paid for.

The app posts a WAV and gets the text back. The model loads in a background
thread on startup, so a cold start answers `/health` with `ready: false` instead
of blocking the first request behind a ten-second load — and, the first time it
ever runs, behind a one-time model download.

Usage, from the repository root:

    tools/voice/.venv/bin/python voice/serve_stt.py

NEREA_STT_PORT (9881), NEREA_STT_MODEL (small), NEREA_STT_DEVICE (cpu),
NEREA_STT_COMPUTE (int8), NEREA_STT_LANG (es), NEREA_STT_PARENT.
"""

import io
import json
import os
import threading
import time
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

HOST = "127.0.0.1"
PORT = int(os.environ.get("NEREA_STT_PORT", "9881"))
MODEL = os.environ.get("NEREA_STT_MODEL", "small")
DEVICE = os.environ.get("NEREA_STT_DEVICE", "cpu")
COMPUTE = os.environ.get("NEREA_STT_COMPUTE", "int8")
LANG = os.environ.get("NEREA_STT_LANG", "es")

# The app passes its own pid: if it dies without cleaning up (SIGKILL, a crash),
# this process is reparented and leaves with it instead of holding the model.
PARENT = os.environ.get("NEREA_STT_PARENT")

_state = {"model": None, "error": None}
_lock = threading.Lock()


def watch_parent():
    parent = int(PARENT)
    while True:
        time.sleep(2)
        if os.getppid() != parent:
            os._exit(0)


def load():
    try:
        from faster_whisper import WhisperModel

        started = time.time()
        model = WhisperModel(MODEL, device=DEVICE, compute_type=COMPUTE)
        with _lock:
            _state["model"] = model
        print(f"stt: {MODEL} listo en {time.time() - started:.1f}s", flush=True)
    except Exception as err:  # the reason has to reach the client, whatever it is
        with _lock:
            _state["error"] = str(err)
        print(f"stt: no pude cargar el modelo: {err}", flush=True)


class Handler(BaseHTTPRequestHandler):
    protocol_version = "HTTP/1.1"

    def log_message(self, *_args):
        pass

    def reply(self, status, payload):
        body = json.dumps(payload, ensure_ascii=False).encode("utf-8")
        self.send_response(status)
        self.send_header("Content-Type", "application/json; charset=utf-8")
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)

    def do_GET(self):
        if self.path != "/health":
            return self.reply(404, {"error": "not found"})

        with _lock:
            ready = _state["model"] is not None
            error = _state["error"]
        return self.reply(200, {"ok": True, "ready": ready, "model": MODEL, "error": error})

    def do_POST(self):
        if self.path != "/transcribe":
            return self.reply(404, {"error": "not found"})

        length = int(self.headers.get("Content-Length") or 0)
        body = self.rfile.read(length) if length > 0 else b""
        if not body:
            return self.reply(400, {"error": "cuerpo vacío"})

        with _lock:
            model = _state["model"]
            error = _state["error"]
        if model is None:
            return self.reply(503, {"error": error or "el modelo todavía no está listo"})

        started = time.time()
        try:
            segments, info = model.transcribe(
                io.BytesIO(body),
                language=LANG,
                # Greedy decoding is faster and worse: with marginal audio it
                # falls off a cliff, and a five-second clip costs nothing.
                beam_size=5,
                # Push-to-talk already means "he is speaking": the key was held.
                # A VAD here only adds a way for a quiet sentence to vanish — it
                # did exactly that to a clip the microphone had recorded at full
                # scale. Real silence never reaches this point.
                vad_filter=False,
                # One utterance at a time: carrying text across calls is what
                # makes a short clip come back with the previous sentence glued
                # on, or with a loop of the same word.
                condition_on_previous_text=False,
            )
            text = " ".join(segment.text.strip() for segment in segments).strip()
        except Exception as err:  # noqa: BLE001 - answered, never fatal
            return self.reply(500, {"error": str(err)})

        return self.reply(
            200,
            {
                "text": text,
                "language": info.language,
                "seconds": round(info.duration, 2),
                "took": round(time.time() - started, 2),
            },
        )


def main():
    if PARENT:
        threading.Thread(target=watch_parent, daemon=True).start()

    print(f"stt: {HOST}:{PORT} modelo={MODEL} device={DEVICE} compute={COMPUTE}", flush=True)
    threading.Thread(target=load, daemon=True).start()

    server = ThreadingHTTPServer((HOST, PORT), Handler)
    server.serve_forever()


if __name__ == "__main__":
    main()
