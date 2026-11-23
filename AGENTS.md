# AGENTS.md

Working notes for anyone (human or agent) touching this repository. The README describes the
product; this file describes the rules, the commands and the traps.

## What this is

Nerea: a desktop companion for Linux. A UI-agnostic Rust core, a Tauri 2 shell with a single
transparent window, a cloned Japanese voice and a persistent conversation.

```
crates/nerea-core     providers · secrets · config · conversation · transcript · memory · recall · phrase bank
crates/nerea-cli      headless harness (clap, tokio)
src-tauri             Tauri shell: commands, window placement, voice engine supervision, update
ui                    plain HTML/CSS/JS, no bundler
voice                 phrase bank, bank renderer, engine launcher
packaging             .deb + tarball scripts and the desktop entry
docs/brand            generated logo, banner, social card, signature
odd/tasks             the logbook: specs, tasks, decisions, evidence
tools/voice           machine-local, gitignored: venv + GPT-SoVITS weights (~11 GB)
```

## Commands

```bash
cargo build -p nerea-desktop                 # debug build
cargo run -p nerea-desktop                   # the app
cargo test                                   # 78 tests, no network
cargo clippy --all-targets -- -D warnings
node --check ui/avatar.js && node --check ui/main.js

tools/voice/.venv/bin/python voice/serve_tts.py    # engine, 127.0.0.1:9880 (the app also starts it)
tools/voice/.venv/bin/python voice/render_bank.py  # re-render bank lines (resumable, by id/category)

npx @tauri-apps/cli@^2 build --bundles appimage
packaging/package.sh                          # .deb + tarball, same script CI runs
python3 docs/brand/make_brand.py              # regenerate every brand asset
```

## Invariants

- No `unwrap()` / `expect()` outside tests. `Result` + `?`; `thiserror` for errors.
- `nerea-core` must not depend on `tauri` or on a tokio runtime.
- API keys live in the OS keyring, never in `config.toml` or a log.
- A new provider is one entry in `ai/presets.rs`, not a new transport.
- Code, comments, docs and UI copy in English; the chat and the phrase bank in Spanish.
- Anything that costs meaningful resources asks first and says what it costs.

## Traps already paid for

- **UI assets are embedded at build time.** Editing `ui/` without `cargo build` changes nothing.
- **`node --check` the UI before building.** A duplicated `const` once killed the whole module and
  the window just sat at "cargando…".
- **Wayland forbids client-side positioning.** `set_position` and `outer_position` are lies; move the
  window with Hyprland's Lua dispatcher: `hyprctl dispatch 'hl.dsp.window.move({ x = 1, y = 2, window = "address:0x…" })'`.
- **GTK refuses to resize a non-resizable window**, and that same hint is what keeps her floating.
  Pin the size as `set_min_size == set_max_size`.
- **The history window must hide the avatar**, which is always on top, or she covers the text.
- **Background tasks need `AppState: Clone`**: its fields are `Arc`s on purpose.
- **TTS: `CUDAGraph=0` is mandatory.** Without it this GPT-SoVITS build captures a CUDA graph per
  input shape: 16-20 s per sentence instead of 0.5-1.8 s. The launcher sets it.
- **The TTS engine runs from its own directory**, so `ref_audio_path` must be absolute or the API
  answers `400 not exists`.
- **Never run two engines.** The app adopts an existing listener for that reason; two of them halve
  the speed.
- **`git pull --ff-only` refuses on a diverged branch** — the updater reports it instead of merging.

## Where state lives

| path | what |
|---|---|
| `~/.config/nerea/config.toml` | provider, fallback, `[voice] speak` |
| `~/.local/share/nerea/conversation.jsonl` | one JSON line per turn, the source of truth |
| `~/.local/share/nerea/memory.json` | running summary + the timestamp it covers |
| `/tmp/nerea-tts.log` | engine output when the app starts it |
| `$XDG_RUNTIME_DIR/nerea/line.wav` | the line being played |

`NEREA_DATA_DIR` relocates the first three; `NEREA_BANK_DIR` points at the rendered lines;
`NEREA_REF_AUDIO` at the reference clip; `NEREA_TTS_URL`, `NEREA_TTS_PORT`, `NEREA_TTS_IDLE_SECS`
and `NEREA_TTS_PYTHON` tune the engine.

## Debug switches

| variable | effect |
|---|---|
| `NEREA_DEBUG=1` | log window and run events |
| `NEREA_OPEN_PANEL=1` | open the chat panel without a pointer |
| `NEREA_OPEN_HISTORY=1` | open the history window |
| `NEREA_SUMMARIZE_NOW=1` | force one summary |
| `NEREA_RECALL_QUERY="…"` | print what recall would send |
| `NEREA_SAY_TEST="…"` | run one sentence through the whole voice path |
| `NEREA_AUTO_UPDATE=1` | pull, rebuild and restart if behind |

## Releasing

1. Bump the version in the three `Cargo.toml` files and `src-tauri/tauri.conf.json`.
2. `git tag -a vX.Y.Z -m "…" && git push origin vX.Y.Z`.
3. `.github/workflows/release.yml` builds on `ubuntu-22.04`, which is what keeps the artifacts at
   glibc 2.34 (Ubuntu 22.04+). `.deb`, tarball and AppImage land on the release.
4. `dist/` is local scratch and gitignored; the release is the artifact.

Never build the release artifacts on the rolling host: they would require glibc 2.39 and only run on
Ubuntu 24.04+.

## Logbook

`odd/tasks/` holds the specs (`S#`), tasks (`T#`), verbatim user requests (`L#`) and measured
evidence. Append to it; do not rewrite history. Engram mirrors the decisions under `nerea/*`.
