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
cargo test                                   # 209 tests, no network
cargo clippy --all-targets -- -D warnings
node --check ui/avatar.js && node --check ui/main.js

tools/voice/.venv/bin/python voice/serve_tts.py    # engine, 127.0.0.1:9880 (the app also starts it)
tools/voice/.venv/bin/python voice/render_bank.py  # re-render bank lines (resumable, by id/category)

npx @tauri-apps/cli@^2 build --bundles appimage
packaging/package.sh                          # .deb + tarball, same script CI runs
python3 docs/brand/make_brand.py              # logo, social card, signature (banner is hand art)
```

## Who she is

Her written voice is **Spanish**. `persona.rs` says it in so many words: no kana, no kanji in the
chat — a kana in the middle of a sentence reads as a bug, not as a tic. Her tic is «vaya, vaya»,
and the Japanese translator turns it back into あらあら for the spoken line. A test fails if any
kana or kanji ever comes back into the character sheet.

`crates/nerea-core/src/persona.rs` holds her whole self — name, family, history, temper, voice — and
the machine contract that keeps her useful. It travels with **every** request, so it has a size test:
if the persona grows past its budget, the test fails instead of quietly costing tokens on every turn.
Change her there, never in a provider or a command.

## What she can do with the machine

Sound: push-to-talk records with `arecord` (not `pw-record`: see the trap below), transcribes
with `faster-whisper` in the same venv as the voice, and answers in the chat. A named pipe
(`$XDG_RUNTIME_DIR/nerea/ptt`) is the way in — `echo start|stop|toggle >` it, or
`packaging/nerea-talk.sh`.

Tier A is **read**, with one exception: `crates/nerea-core/src/tools` holds four read tools —
`read_file`, `list_dir`, `search` (with `deep: true` for the formats that need a parser) and
`find` — plus `remember`, which writes a knowledge card. No shell, ever, and `remember` takes no
path: the file name comes from a slug, so it can only ever land in `cards/`. They are declared to the API as real tools
(`tools::specs()`), so the model returns a `tool_calls` field instead of writing a format into
its prose. That is deliberate: with a JSON block in the prompt a flash model would announce
"voy a buscar" and stop, and the JSON could leak into the chat. The API cannot half-say a call.

- Text and code are read as-is; PDF (text layer, no OCR), `.docx`, `.odt`, EPUB and HTML are
  unpacked first. Anything with a NUL byte is refused, not dumped.
- `tools::guard` resolves `~` and symlinks **before** applying the rules, so a link cannot walk
  around them. `~/.ssh`, `~/.gnupg`, `~/.pki`, the browser profiles, her own data and config
  dirs, `/etc/shadow`, `/proc`, `/sys` and credential-shaped names (`id_rsa`, `.env`, `*.pem`,
  `*.key`) are never readable.
- `write_file` and `edit_file` do **not** write: they stage a proposal in `~/.local/share/nerea/pending/`
  and the change waits for a click. `apply_change` is the only path to a real file, it is a Tauri
  command the page calls, and the model has no route to it. The original is copied to
  `~/.local/share/nerea/backups/` first; an edit whose fragment matches twice is refused; a target
  that changed under the proposal is refused.
- `run_command` is the only way to run a program, and it is not a shell: `{program, args[]}`,
  validated against `~/.config/nerea/tools.toml` and against these rules, which live in code:
  `sudo`/`pkexec` never run; every path argument is checked (a credential file is refused, an
  output must land inside the declared roots); deleting a root or an ancestor of one is refused.
  The turn **blocks** on his click — a card he cannot see is a turn that hangs, so the chat opens.
- `docs` speaks MCP to context7 over streamable HTTP, and exposes **one** tool instead of the
  server's two: every schema is paid on every request, and the window is 8 000 tokens.
- `processes`, `system`, `services` and `logs` read the machine's vitals: `/proc` directly,
  plus `systemctl --failed` and `journalctl` with fixed arguments. Read-only, no shell, no command
  assembled from model text. `processes` sees every process; `limit` decides how many come back.
- `read_file` takes `numbers: true` and then prints `52:linea`, the same shape `grep -n` uses.
  `list_dir` marks each row `d`, `l` or `f`, because a link listed as a file is a lie.
- Every call appends one line to `~/.local/share/nerea/audit.jsonl`. One observation is clipped at
  8k characters and one message may read at most 16k in total (`tools::OBSERVATION_BUDGET`); past
  that a call is answered with "ya tenés material para contestar" instead of more text.
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
- **An assistant tool round must carry the words she said.** `push_tool_round` takes that round's
  prose; with `content: ""` the next round reads as if she went silent and only called tools, and
  she answers with an apology instead of an answer. Measured, not guessed.
- **Observations have to be folded between rounds.** A full observation is a quarter of the
  context window: three untouched ones push the user's own question out of `trimmed`, and a model
  that cannot see the question keeps calling tools forever. `compact_observations(400)` runs
  before each new round.
- **A card nobody can find is a card that does not exist.** Selection scores tags 3, title 2,
  body 1, and the floor is one point: an earlier floor of two made every fact written by `remember`
  unreachable, because such a card has no tags until you add them. A card created without tags is
  indexed by its topic for the same reason.
- **`pw-record` is broken on this machine.** Its native PipeWire capture returned a signal
  pinned at full scale (RMS 18 800, peak 32 768) for a silent room, at every gain and every input
  source; the same microphone through `arecord -D default` lands at RMS 360. A saturated signal is
  not noise to filter — the waveform is already destroyed. Record with `arecord`, and keep the
  peak in the log: "she can't hear me" is undebuggable without it.
- **Signals in this process crash it.** `SIGUSR1` gave a SIGSEGV inside `libjavascriptcoregtk`:
  WebKit keeps its own handlers, and `signal-hook-registry` chains to them from an
  async-signal-safe context. Use the named pipe.
- **The microphone is at 18 %.** The app normalises a quiet clip to a 9 000 peak before sending
  it, so the system volume does not have to be right for her to hear you.
- **Never build the provider while holding the session lock.** `ensure_provider` used to call
  `factory::build_chain` (a blocking keyring round-trip) with the session mutex in hand: one stall
  there froze every later message at `begin_turn`, before the turn even reached the transcript. The
  build now runs on a blocking thread, without the lock, with a 15 s limit.
- **A streaming body with no idle timeout hangs the app.** The SSE request has no total timeout on
  purpose, but a provider that stops sending used to freeze the turn forever — and the stop button
  did nothing, because the loop was blocked waiting for a byte. Ninety seconds of silence now ends
  the turn with a message.
- **A launcher that copies `Exec=` verbatim fails on flatpak.** The exported entry carries
  `--file-forwarding … @@u %U @@`; with no files those markers make `flatpak` complain. Flatpak
  entries are rebuilt as `flatpak run <id>`, and field codes are dropped everywhere else.
- **A missing function in the UI looks exactly like a hung turn.** `setBusy` called
  `setInterval(checkApproval, …)` before `checkApproval` existed: the ReferenceError aborted
  `send()` before `invoke`, so the chat said "pensando" while nothing reached Rust — no log, no
  transcript, nothing. A `window.onerror` handler now prints a JavaScript failure in the chat.
- **An event that must arrive is a hang waiting to happen.** The approval card is rendered from a
  poll (`pending_approval` every 900 ms while a turn is busy), not from `nerea://approve` alone: the
  event was emitted, the listener was registered, and the card still never appeared.
- **A blocked turn looks like a hang.** `run_command` waits up to 180 s for a click; when the panel
  was closed, the card existed nowhere visible. The approval emits `nerea://panel` first.
- **`cargo build` does not always re-embed `ui/`.** A UI change can compile green and never reach the
  binary; `cargo clean -p nerea-desktop` forces it. The assets are compressed, so `strings` will not
  tell you either — test the page, not the file.
- **A Python `replace` that finds nothing is silent.** Twice in one session a UI patch was "applied"
  and never existed: the anchor text belonged to another file. Assert the change (`grep` the result)
  after every scripted edit.
- **Never restrict the tools to force an answer.** With an empty `tools` array, and even with
  `tool_choice: "none"`, this model writes its own `<｜DSML｜ …>` call syntax into the prose and
  it reaches the chat (both measured on `deepseek-v4.1-flash`). Keep the tools declared and free;
  the last round of a turn simply ignores any call it still asks for.
- **`git pull --ff-only` refuses on a diverged branch** — the updater reports it instead of merging.

## Where state lives

| path | what |
|---|---|
| `~/.config/nerea/config.toml` | provider, fallback, `[voice] speak` |
| `~/.local/share/nerea/conversation.jsonl` | one JSON line per turn, the source of truth |
| `~/.local/share/nerea/memory.json` | running summary + the timestamp it covers |
| `~/.local/share/nerea/cards/<topic>.md` | one knowledge card per topic: front matter, then the facts as bullets |
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
| `NEREA_CARDS_QUERY="…"` | print the cards a message would carry |
| `~/.config/nerea/tools.toml` `trust` | commands approved with "Siempre", by exact fingerprint |
| `NEREA_AUTO_APPROVE=1` | skips the approval card for `run_command`. **Never in normal use** |
| `NEREA_MCP_URL=…` | override the context7 endpoint |
| `NEREA_LISTEN_MS=3000` | one push-to-talk turn by itself: record 3 s, transcribe, answer |
| `NEREA_MIC=default` | capture device for `arecord` |
| `NEREA_STT_MODEL=small` | whisper model; `base` is faster and worse in Spanish |
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

## Backlog

**The work queue is `odd/tasks/work-queue.md`**: one task per session, each with its acceptance line.
The table below is the summary; the queue is what you actually pick from.

One row per idea, with the state as of the last release. Update the row when an item is touched;
a row that still says `proposed` was never started, and that is the whole point of the table.

| item | state | document |
|---|---|---|
| identity core — `persona.rs` on every turn | **shipped 0.5.7** | `odd/tasks/memory-layers.md` |
| rolling summary — `memory.json`, 350 words | **shipped 0.3.0** | `odd/tasks/memory-layers.md` |
| read-only tools — `read_file`, `list_dir`, `search` | **shipped 0.6.2** | `odd/tasks/system-access-tier-a.md` |
| native function calling for tools | **shipped 0.6.2** | `odd/tasks/system-access-tier-a.md` |
| knowledge cards (tier 2) | **shipped 0.9.0** | `odd/tasks/memory-layers.md`, L4 |
| `remember` — cards only on request | **shipped 0.9.0** | `odd/tasks/memory-layers.md`, L6 |
| `search` inside `.docx` / `.odt` / `.pdf` | **shipped 0.7.0** | `odd/tasks/system-access-tiers.md`, L2.1 |
| `find` a file by name | **shipped 0.7.0** | `odd/tasks/system-access-tiers.md`, L2.2 |
| per-turn observation budget | **shipped 0.7.0** | `odd/tasks/system-access-tiers.md`, L2.3 |
| line numbers in `read_file` | proposed | `odd/tasks/system-access-tiers.md`, L2.4 |
| audit visible in the history window | proposed | `odd/tasks/system-access-tiers.md`, L2.5 |
| tier B — commands behind a confirmation | undesigned | `odd/tasks/system-access-tiers.md`, L3 |
| tier C — write, clipboard, notifications, media | undesigned | `odd/tasks/system-access-tiers.md`, L4 |

Two rules for this table. An item marked *shipped* is not reopened for a rewrite: it is improved in
place. And tier B stays undesigned until the user shapes the permission — building the command
first and guarding it later means it ran unguarded once.
