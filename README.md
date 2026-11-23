# nerea

AI desktop companion for Linux. Rust core, Tauri shell, static-image avatar.
Conceptually inspired by [CapriCore](https://github.com/CapriChooms/CapriCore), but not its
stack: no Electron, no WebGL. The brain is a plain Rust crate that knows nothing about windows.

**Status: phase 2 of 5 — conversation layer**

## Phases

| # | Phase | State | Deliverable |
|---|-------|-------|-------------|
| 1 | Core + providers | done | Workspace, `AiProvider` trait, OpenAI-compatible transport, keyring, CLI harness |
| 2 | Conversation | done | Turn history with prompt budget, config file, provider failover |
| 3 | Tauri shell | next | Chat window + avatar panel, IPC commands, streaming events |
| 4 | Static-image avatar | pending | Frame manifest and a state machine (`idle` / `thinking` / `talking` / `error`) |
| 5 | Memory | pending | SQLite turn store plus rolling summaries |

### Phase 1 — Core + providers

- [x] Cargo workspace: `nerea-core` (UI-agnostic) + `nerea-cli` (harness)
- [x] `AiProvider` trait, `dyn`-compatible via `async-trait` + `BoxStream`
- [x] Shared OpenAI-compatible transport, SSE streaming with own buffering
- [x] API keys in the OS keyring (Secret Service over D-Bus on Linux)
- [x] Wire parsing pinned by an in-process mock server, no network in tests

### Phase 2 — Conversation

- [x] `Conversation`: history, system prompt, oldest-first trimming under a token budget
- [x] `Config` at `~/.config/nerea/config.toml`; keys never live there
- [x] `FallbackProvider`: fails over on network errors, 401/403/408/429/5xx only
- [x] Streaming fails over only before the first emitted token
- [x] `nerea config show|path|init` and an interactive `chat` REPL

### Phase 3 — Tauri shell

- [ ] `src-tauri` member, `tauri.conf.json`, `ui/` frontend
- [ ] Commands: `list_providers`, `send_message`, `cancel_stream`
- [ ] Streaming to the frontend over Tauri events
- [ ] Keep `nerea-core` free of any Tauri dependency

### Phase 4 — Static-image avatar

- [ ] Frame manifest format (`idle`, `thinking`, `talking`, `error` → frame lists)
- [ ] State machine driven by turn lifecycle, not by timers alone
- [ ] Placeholder art only until the real designs land

### Phase 5 — Memory

- [ ] SQLite store for turns, keyed by conversation
- [ ] Rolling summary so old context survives trimming

## Layout

```
crates/nerea-core   providers, secrets, config, conversation   (no UI, no tokio)
crates/nerea-cli    headless harness: providers / key / config / chat
```

## Commands

```bash
cargo run -p nerea-cli -- providers              # providers, allowed models, key state
cargo run -p nerea-cli -- key set opencode-go    # or export OPENCODE_API_KEY
cargo run -p nerea-cli -- chat hola              # one shot, non-streaming
cargo run -p nerea-cli -- chat --stream hola     # one shot, streaming
cargo run -p nerea-cli -- chat                   # interactive REPL (:quit, :reset)
cargo run -p nerea-cli -- config show
```

## Providers

| id | Base URL | Allowed models |
|----|----------|----------------|
| `opencode-go` | `https://opencode.ai/inference/go/openai/v1` | `deepseek-v4.1-flash` **only** |
| `deepseek` | `https://api.deepseek.com` | `deepseek-v4-flash`, `deepseek-v4-pro`, `deepseek-flash` |

`opencode-go` is pinned on purpose: this project uses that one model and nothing else through it.
An unknown `--model` fails locally, before any request is billed. DeepSeek direct is the fallback.

## Config

`~/.config/nerea/config.toml` — defaults shown by `nerea config show`:

```toml
provider = "opencode-go"

[fallback]
provider = "deepseek"
```

API keys are never written here; they live in the system keyring.

## Checks

```bash
cargo test          # 28 tests, no network required
cargo clippy --all-targets
```

## Rules that are not negotiable

- No `unwrap()` / `expect()` outside tests.
- Provider wire formats stay private to their adapter; only normalized types cross the boundary.
- Nothing in `nerea-core` may depend on `tauri`.
- A new provider is one entry in `ai/presets.rs`, not a new transport.
