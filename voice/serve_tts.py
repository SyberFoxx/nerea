"""Launcher for GPT-SoVITS' own TTS API.

Two machine fixes live here, both with a reason:

1. torchaudio 2.9+ delegates audio I/O to torchcodec, whose prebuilt wheels do
   not load against the CUDA 12.8 torch build the RTX 50 series needs. Rather
   than pin an older torch, load/save go through soundfile.

2. CUDAGraph=0. This build of GPT-SoVITS has an "Accel" AR engine that captures
   a CUDA graph per input shape. Every new sentence has a new length, so every
   sentence paid ~16 s of capture: measured 16-18 s per request. With graphs
   off, the same sentences take 0.5-1.8 s, and the first request after startup
   is the slow one. The frames are the same; only the bookkeeping changes.

Usage, from the repository root:

    tools/voice/.venv/bin/python voice/serve_tts.py

NEREA_TTS_PORT and NEREA_TTS_CONFIG override the port and the weights config.
"""

import os
import runpy
import sys

import soundfile as sf
import torch
import torchaudio

HOST = "127.0.0.1"
PORT = os.environ.get("NEREA_TTS_PORT", "9880")
CONFIG = os.environ.get("NEREA_TTS_CONFIG", "GPT_SoVITS/configs/tts_infer.yaml")

# Graph capture per shape is the 16 s; measured, not guessed.
os.environ.setdefault("CUDAGraph", "0")

HERE = os.path.dirname(os.path.abspath(__file__))
ENGINE = os.path.join(os.path.dirname(HERE), "tools", "voice", "gpt-sovits")


def load(path, **_kwargs):
    data, sample_rate = sf.read(str(path), dtype="float32", always_2d=True)
    return torch.from_numpy(data.T.copy()), sample_rate


def save(path, source, sample_rate, **_kwargs):
    waveform = source.detach().cpu()
    if waveform.dim() == 2:
        waveform = waveform.transpose(0, 1)
    sf.write(str(path), waveform.numpy(), int(sample_rate))


torchaudio.load = load
torchaudio.save = save

os.chdir(ENGINE)
# api_v2 imports `tools.portable_runtime`, so the checkout must be on sys.path.
sys.path.insert(0, os.getcwd())

sys.argv = ["api_v2.py", "-a", HOST, "-p", PORT, "-c", CONFIG]
runpy.run_path("api_v2.py", run_name="__main__")
