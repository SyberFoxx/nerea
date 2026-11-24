# AGENTS.md

Working notes for anyone (human or agent) touching this repository. The README describes the
product; this file describes the rules, the commands and the traps.

## What this is

Nerea: a desktop companion for Linux. A UI-agnostic Rust core, a Tauri 2 shell with a single
transparent window, a cloned Japanese voice and a persistent conversation.

```
crates/nerea-core     providers · secrets · config · conversation · transcript · memory · recall · persona · tools · phrase bank
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

## Who she is

`crates/nerea-core/src/persona.rs` holds her whole self — name, family, history, temper, voice — and
the machine contract that keeps her useful. It travels with **every** request, so it has a size test:
if the persona grows past its budget, the test fails instead of quietly costing tokens on every turn.
Change her there, never in a provider or a command.

## What she can do with the machine

Tier A only: **read**. `crates/nerea-core/src/tools` holds three tools — `read_file`,
`list_dir`, `search` — and no shell, ever. They are declared to the API as real tools
(`tools::specs()`), so the model returns a `tool_calls` field instead of writing a format into
its prose. That is deliberate: with a JSON block in the prompt a flash model would announce
"voy a buscar" and stop, and the JSON could leak into the chat. The API cannot half-say a call.

- Text and code are read as-is; PDF (text layer, no OCR), `.docx`, `.odt`, EPUB and HTML are
  unpacked first. Anything with a NUL byte is refused, not dumped.
- `tools::guard` resolves `~` and symlinks **before** applying the rules, so a link cannot walk
  around them. `~/.ssh`, `~/.gnupg`, `~/.pki`, the browser profiles, her own data and config
  dirs, `/etc/shadow`, `/proc`, `/sys` and credential-shaped names (`id_rsa`, `.env`, `*.pem`,
  `*.key`) are never readable.
- Every call appends one line to `~/.local/share/nerea/audit.jsonl`. Observations are `user`
  turns in the live conversation but never in the transcript, and they are clipped at ~12k
  characters: three looks per message, no more.
- Round trip: the reply streams as text; when it ends with `tool_calls`, the app runs each one,
  pushes the assistant turn plus one `role: "tool"` result per call, and asks again — at most
  three rounds per message. A tool result is bound to its call id, never free-floating text.

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
- **GTK resizes a window only in one direction.** A non-resizable window grows to satisfy a new
  minimum and never shrinks to satisfy a new maximum, so the chat could open and never close. The
  fix is `resizable: true` with the size pinned as `set_min_size == set_max_size`: the pin still
  keeps her floating (the compositor reads a fixed size), and GTK recomputes on the bounds change.
- **A floating window is resized around its centre**, so an anchored resize needs the placement
  computed *before* the resize and applied in the same breath (`pre_x = x + (w - old_w) / 2`).
  Reading the geometry back afterwards is what made her walk away from her corner, every time.
- **The page reflows while the window resizes**: hiding it (`visibility: hidden`) around the swap is
  what stops the sprite stretching across the screen.
- **A second Tauri window in the same app did not load the page's CSS or script** (the same URL that
  works for the first window), so the chat lives in one window that resizes, not two windows.
- **The history window must hide the avatar**, which is always on top, or she covers the text.
- **Background tasks need `AppState: Clone`**: its fields are `Arc`s on purpose.
- **TTS: `CUDAGraph=0` is mandatory.** Without it this GPT-SoVITS build captures a CUDA graph per
  input shape: 16-20 s per sentence instead of 0.5-1.8 s. The launcher sets it.
- **The TTS engine runs from its own directory**, so `ref_audio_path` must be absolute or the API
  answers `400 not exists`.
- **Never run two engines.** The app adopts an existing listener for that reason; two of them halve
  the speed.
- **`crate-type = ["staticlib", ...]` burns disk.** `staticlib` is only for Android/iOS and each
  rebuild leaves an 874 MB archive behind in a fresh `target/debug/build/<crate>/<hash>/out`,
  which Cargo never prunes: 29 rebuilds became 41 GB. Desktop builds need `["cdylib", "rlib"]`.
- **A tool result must follow its call.** `Conversation::trimmed` drops leading `role: "tool"`
  messages when the budget cuts through a round; without that, the API rejects the prompt.
- **`git pull --ff-only` refuses on a diverged branch** — the updater reports it instead of merging.

## Where state lives

| path | what |
|---|---|
| `~/.config/nerea/config.toml` | provider, fallback, `[voice] speak` |
| `~/.local/share/nerea/conversation.jsonl` | one JSON line per turn, the source of truth |
| `~/.local/share/nerea/memory.json` | running summary + the timestamp it covers |
| `~/.local/share/nerea/audit.jsonl` | one line per tool call: when, what, arguments, outcome |
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
| `NEREA_ASK="…"` | send that message once, three seconds after boot, and log the whole path |
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
