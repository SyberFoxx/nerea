# nerea

An AI desktop companion for Linux: a Rust core, a Tauri shell, and a static-image avatar.
Conceptually inspired by [CapriCore](https://github.com/CapriChooms/CapriCore), deliberately not
its stack — no Electron, no WebGL, no JavaScript in the brain.

**Status: phase 4 in progress.** `cargo run -p nerea-desktop` opens the chat window plus a small
transparent avatar overlay that seats itself on the focused window.

---

## Phases

| # | Phase | State | Deliverable |
|---|-------|-------|-------------|
| 1 | Core + providers | `done` | Workspace, `AiProvider` trait, shared OpenAI-compatible transport, keyring, CLI harness |
| 2 | Conversation | `done` | Turn history with a prompt budget, config file, provider failover |
| 3 | Tauri shell | `done` | Chat window + avatar panel, IPC commands, streaming events |
| 4 | Static-image avatar | `wip` | Overlay window, frame manifest, state machine, docking |
| 5 | Memory | `pending` | SQLite turn store plus rolling summaries |

### Phase 1 — Core + providers

- [x] Cargo workspace: `nerea-core` (UI-agnostic) + `nerea-cli` (headless harness)
- [x] `AiProvider` trait, `dyn`-compatible via `async-trait` + `BoxStream`
- [x] One OpenAI-compatible transport shared by every provider
- [x] Own SSE parser: split chunks, CRLF boundaries, unterminated final event
- [x] API keys in the OS keyring (Secret Service over D-Bus on Linux)
- [x] Wire parsing pinned by an in-process mock server — no network in tests

### Phase 2 — Conversation

- [x] `Conversation`: history, system prompt, oldest-first trimming under a token budget
- [x] `Config` at `~/.config/nerea/config.toml`, written atomically; keys never live there
- [x] `FallbackProvider`: fails over on network, 401/403/408/429/5xx
- [x] Streaming fails over only before the first *visible* token
- [x] `ChatResponse.provider` makes failover observable to the UI
- [x] A configured fallback without a key degrades to a warning, not a dead end
- [x] `nerea config show|path|init` and an interactive `chat` REPL
- [x] Gateway contract: own `User-Agent` plus a per-conversation `x-opencode-session` header
- [x] Verified end to end: real streaming and non-streaming calls against `opencode-go`

### Phase 3 — Tauri shell

- [x] `src-tauri` workspace member + `ui/` frontend (plain HTML/CSS/JS, no bundler)
- [x] Commands: `status`, `send_message`, `cancel_stream`, `reset_conversation`
- [x] Streaming to the frontend as `nerea://delta` events
- [x] State locks are never held across a stream, so cancel and status stay responsive
- [x] `nerea-core` stays free of any Tauri dependency
- [ ] `thinking` indicator before the first token (currently the bubble simply stays empty)

### Phase 4 — Static-image avatar

- [x] Second window: transparent, undecorated, always on top, 176 × 256
- [x] Frame manifest (`idle`, `thinking`, `talking`, `error` → frame lists)
- [x] State machine driven by the turn lifecycle: thinking → talking → idle, error on failure
- [x] Docking: seats on the bottom-right corner of the focused window (`hyprctl clients -j`)
- [x] Placeholder frames with a visible fallback if art fails to load
- [ ] Composer bubble attached to the avatar, so the chat window becomes optional
- [ ] Re-dock automatically when the focused window changes
- [ ] Real art

## Avatar art

Frames live in `ui/avatar/` and are listed in `ui/avatar/manifest.json`. Keep the state names and
the canvas, drop in your own files, nothing else changes:

```json
"talking": { "fps": 6, "frames": ["avatar/talking-0.png", "avatar/talking-1.png"] }
```

The pose should be a seated half-body anchored to the bottom edge: the window sits on the corner of
the active window, so feet and seat line read as "sitting on it".

On Hyprland the overlay needs a rule to stay pinned across workspaces. Both windows share the
`nerea-desktop` class, so match on the title:

```conf
windowrulev2 = pin,title:^(nerea-avatar)$
windowrulev2 = noborder,title:^(nerea-avatar)$
windowrulev2 = noshadow,title:^(nerea-avatar)$
windowrulev2 = noanim,title:^(nerea-avatar)$
```

### Phase 5 — Memory

- [ ] SQLite store of turns, keyed by conversation
- [ ] Rolling summary so context survives trimming

---

## Architecture

```
crates/nerea-core     providers · secrets · config · conversation · factory   (no UI, no tokio)
crates/nerea-cli      headless harness                                        (tokio, clap)
src-tauri             Tauri 2 shell: IPC commands, event streaming
ui                    static frontend: HTML/CSS/JS, no bundler
```

A message travels like this:

1. The caller pushes the new turn into `Conversation`.
2. `Conversation::trimmed()` returns the system prompt plus the newest turns that fit the budget.
3. `FallbackProvider` sends to the primary provider; transient failures move to the secondary.
4. `OpenAiCompatProvider` builds the HTTP request and normalizes the wire format.
5. Streaming deltas come back as `Result<String, ProviderError>`; the reply is pushed into history.

Nothing above layer 2 knows which provider answered, and nothing in `nerea-core` knows a window
exists. That is what keeps the Tauri shell swappable.

## Providers

| id | Base URL | Allowed models |
|----|----------|----------------|
| `opencode-go` | `https://opencode.ai/inference/go/openai/v1` | `deepseek-v4.1-flash` **only** |
| `deepseek` | `https://api.deepseek.com` | `deepseek-v4-flash`, `deepseek-v4-pro`, `deepseek-flash` |

`opencode-go` is pinned on purpose: this project uses exactly one model through it. An unknown
`--model` fails locally, before any request is billed. DeepSeek direct is the fallback path.

OpenCode Go requires two things beyond an API key: a client-specific `User-Agent`
(`nerea/<version>`) and a stable session id in `x-opencode-session`, one per conversation. Requests
without them are rejected with `400`. Both are covered by tests that fail if either header changes.

## Config

`~/.config/nerea/config.toml` — nothing here is secret.

```toml
provider = "opencode-go"

[fallback]
provider = "deepseek"
```

A typo'd key is an error, not a silent default. A missing file means defaults. Writes go through a
staging file and `rename`, so a crash cannot truncate an existing config.

## Commands

```bash
cargo run -p nerea-cli -- providers              # providers, allowed models, key state
cargo run -p nerea-cli -- key set opencode-go    # or export OPENCODE_API_KEY
cargo run -p nerea-cli -- chat hola              # one shot
cargo run -p nerea-cli -- chat --stream hola     # one shot, streamed
cargo run -p nerea-cli -- chat                   # REPL (:quit, :reset)
cargo run -p nerea-cli -- config show

cargo run -p nerea-desktop                      # the window
```

## Verification

```bash
cargo test                      # 41 tests, no network required
cargo clippy --all-targets      # clean
```

Tests never touch the internet: the transport is exercised against an in-process `TcpListener`, and
failover is exercised both with mock providers and with two real transports wired together.

## Design decisions

- **`async-trait` + boxed stream, not `impl Stream` in the trait.** A registry of heterogeneous
  providers needs `dyn`; `impl Trait` in return position is not dyn-compatible.
- **Normalized types at the boundary.** `ChatResponse` is ours; each provider's wire format stays
  private to its adapter.
- **Keys in the keyring, never in config.** `keyring 4` with the `v1` feature resolves to Secret
  Service over `zbus` on Linux — no `libdbus`, no `libsecret` build dependency.
- **`reqwest 0.13` defaults to rustls.** No OpenSSL on the build host, no `rustls-tls` flag needed.
- **No total timeout on streaming requests.** `connect_timeout` on the client, per-request timeout
  only for non-streaming calls.
- **Failover is stream-aware.** It only reissues a request before the first *visible* token; empty
  role chunks do not count as content, and a partial answer is never re-sent.
- **Token budget is an estimate.** Four characters per token with margin beats a per-model
  tokenizer dependency while the budget is a safety net, not an accounting system.
- **`nerea-core` does not depend on tokio.** It is generic over runtime; only the harness and tests
  pick one.
- **One factory, two front ends.** `factory::build_chain` takes an injectable `KeySource`, so the CLI
  and the Tauri shell share provider construction and the tests run without a keyring.
- **No bundler.** Tauri serves `ui/` as static files; there is no node build step to keep green.
- **Gateways get what they ask for.** OpenCode Go rejects requests without its own `User-Agent` and a
  stable `x-opencode-session`. `ChatRequest` carries the session id, and the preset decides which
  header receives it — a provider that ignores it never sees one.

## Known gaps

- `println!` in the CLI panics when stdout is closed (`nerea providers | head -1`).
- No cap on the SSE buffer if a provider never sends an event boundary.
- API keys are not zeroized when dropped.
- One turn in flight at a time; the input is disabled while streaming.
- The frontend has no automated test — it is verified by opening the window.
- Bundle packaging (`tauri build`, `.deb`/AppImage) is untested.

## Conventions

- No `unwrap()` / `expect()` outside tests.
- Provider wire types stay private to their adapter.
- Nothing in `nerea-core` may depend on `tauri`.
- A new provider is one entry in `ai/presets.rs`, not a new transport.
- Artifacts (code, comments, docs) in English; conversation in the user's language.
