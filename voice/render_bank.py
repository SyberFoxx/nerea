#!/usr/bin/env python3
"""Render voice/bank.json to WAV files with the cloned voice.

Requires the local GPT-SoVITS API (tools/voice/serve_tts.py) and the reference
slice. Resumable: existing WAVs are skipped, so it can be run again after an
interruption.

Usage:
    voice/../tools/voice/.venv/bin/python voice/render_bank.py            # everything
    ... render_bank.py micro tease                                         # by id or category
"""

import json
import os
import pathlib
import sys
import time
import urllib.parse
import urllib.request

ROOT = pathlib.Path(__file__).resolve().parent.parent
BANK = ROOT / "voice" / "bank.json"
OUT = ROOT / "assets" / "voice" / "bank"
API = os.environ.get("NEREA_TTS_URL", "http://127.0.0.1:9880")
REF = os.environ.get(
    "NEREA_REF_AUDIO", str(ROOT / "tools" / "voice" / "ref" / "semiramis-ref.wav")
)


def synth(text: str, prompt: str, timeout: int = 300) -> bytes:
    params = {
        "text": text,
        "text_lang": "ja",
        "ref_audio_path": REF,
        "prompt_lang": "ja",
        "prompt_text": prompt,
        "text_split_method": "cut0",
        "media_type": "wav",
    }
    url = f"{API}/tts?" + urllib.parse.urlencode(params)
    with urllib.request.urlopen(url, timeout=timeout) as response:
        return response.read()


def main() -> None:
    bank = json.loads(BANK.read_text(encoding="utf-8"))
    prompt = bank["reference"]["prompt_ja"]
    entries = bank["entries"]

    wanted = sys.argv[1:]
    if wanted:
        entries = [
            entry
            for entry in entries
            if any(token in entry["id"] or token == entry["category"] for token in wanted)
        ]

    OUT.mkdir(parents=True, exist_ok=True)
    done = skipped = failed = 0
    started = time.time()

    for index, entry in enumerate(entries, 1):
        dest = OUT / f"{entry['id']}.wav"
        if dest.exists():
            skipped += 1
            continue

        at = time.time()
        try:
            audio = synth(entry["ja"], prompt)
        except Exception as error:  # noqa: BLE001 - keep going, report at the end
            failed += 1
            print(f"[{index}/{len(entries)}] {entry['id']} FAILED: {error}", flush=True)
            continue

        dest.write_bytes(audio)
        done += 1
        print(
            f"[{index}/{len(entries)}] {entry['id']} "
            f"{len(audio) / 1024:.0f} KB in {time.time() - at:.0f}s",
            flush=True,
        )

    print(
        f"done={done} skipped={skipped} failed={failed} "
        f"elapsed={time.time() - started:.0f}s -> {OUT}",
        flush=True,
    )


if __name__ == "__main__":
    main()
