# system-access-tier-a

Nerea can read the machine: any text, code, PDF, Word and ODT file, with a
read-only registry of tools, a path guard and an audit log. Branch:
`main`. Delivery: work-unit commits, one PR-less push per version. Runner:
`cargo test` + `cargo clippy --all-targets -- -D warnings` + `node --check ui/avatar.js`.

## Specs

S1. She can read any text-like file — code, Markdown, JSON, TOML, YAML, logs,
    HTML, CSV — at any path the guard allows. A file longer than the window is
    returned in pieces: `offset`/`limit` in lines, and the answer says how many
    lines are left and with which `offset` to continue.

S2. She can extract the text of a PDF (text layer; **no OCR**), a Word `.docx`
    and an OpenDocument `.odt`. Also EPUB and HTML, which cost the same code.

S3. She can list a directory (name, kind, size) and search text inside the
    files of a directory, returning `path:line: text`.

S4. A binary file is refused with a clear message instead of dumping bytes.

S5. Paths are resolved (symlinks included) **before** the guard runs, and these
    are never readable: `~/.ssh`, `~/.gnupg`, `~/.pki`, `~/.password-store`,
    the browser profiles, the nerea data and config dirs, the keyring,
    `/etc/shadow`, `/etc/sudoers*`, `/proc`, `/sys`, and any file named like a
    credential (`id_rsa`, `id_ed25519`, `.env`, `.netrc`, `*.pem`, `*.key`,
    `*.p12`, `*.pfx`, `*.kdbx`).

S6. Every tool call appends one JSON line to `~/.local/share/nerea/audit.jsonl`:
    when, which tool, its arguments, whether it worked, how many bytes came back.

S7. At most 3 rounds per user message (each round may ask for up to 4 files), and every observation is capped at
    12 000 characters, because each one is a round trip that costs context.

S8. The tool block never reaches the chat: if the model answered with a tool
    call, the user sees nothing until the real answer arrives.

S9. While she reads, the subtitle says what she is doing.

S10. The build cache is not a product: deleting it frees ~50 GB and a rebuild
     must not leave the 874 MB static archive behind again.

## Tasks

| id | spec | route | status | commit |
|---|---|---|---|---|
| T1 | S10 | inline | done | (this commit) |
| T2 | S1 S2 S4 S5 S6 | inline | done | (this commit) |
| T3 | S7 S8 | inline | done | (this commit) |
| T4 | S9 | inline | done | (this commit) |
| T5 | all | inline | done | 111 tests, clippy clean | |

## Log

### L1 — the user's request, verbatim

> arranca por el nivel A, que pueda leer archivos sea de cualquier tipo que pueda leer, codigo, texto como world, pdf y todo eso porfa y con respecto al espacio, si es mejor limpiar cache del proyecto para libera como 60g hazlo por favor, no importa que ucando la ejecute tarde un poco pero que no me genere 14141848 cosa que no usare y que solo me ocupan espacio, necesito mi espacio excelente

### L2 — evidence: what the 70 GB were

`du` on the repository: 72 GB total, of which `target/` was 62 GB
(`debug/build` 49 GB, `debug/incremental` 11 GB, `release` 2.5 GB) and
`tools/voice` 11 GB (the cloned voice: venv + GPT-SoVITS weights). Her actual
memory was 15 KB (`~/.local/share/nerea/conversation.jsonl`).

The 49 GB came from `crate-type = ["staticlib", "cdylib", "rlib"]`: each rebuild
left a new `build/nerea-desktop/<hash>/out` holding `.a` 874 MB + `.so` 238 MB +
`.rlib` 231 MB, and Cargo never prunes old ones. 196 dirs, ~29 of them full.
`staticlib` is only for Android/iOS. Removed; the debug artifacts and the
incremental cache were deleted afterwards.

### L3 — evidence: the cache, measured

Before: `target/` 67 GB, `df` 163 GB used. `cargo clean` reported 69.3 GiB removed and `df`
recovered **26 GB** — the rest were hardlinked and btrfs-compressed copies, which is why the
directory looked bigger than the space it really held. After one full debug build: `target/` is
3.3 GB (`target/debug/nerea-desktop` 333 MB, `build/` 3.0 GB in 4 dirs instead of 196), and the
build takes 51 s from scratch.

### L5 — the block leaked into the chat (0.6.1)

The user pasted her own message: prose first ("Voy a empezar por el principio…"), then the
fenced block. The gate only held a reply that *started* with a fence, so the first character of
prose released the whole stream, JSON included.

Fixed: `fence_start` finds the last fence anywhere in the buffer, `Undecided` holds it until the
tag is readable, `trailing_backticks` holds at most two backticks so a fence split across chunks
is still caught. Her prose is still streamed; the block never is. Three tests cover it.

### L6 — native function calling replaces the protocol (0.6.2)

`GET` on the provider proved it: with a `tools` array the endpoint answers
`finish_reason: "tool_calls"` and a real `tool_calls` field, `content` empty. So the whole
fenced-JSON protocol, the gate and its tests were removed and replaced by the native shape:
`ToolSpec` → `tools`, `ToolCall` → `tool_calls`, results as `role: "tool"` messages bound by
`tool_call_id`. Verified with a probe that mirrors the app's request: the model answered with
`search {"query":"Nerea","path":"/home/ramses/Documentos/novela"}` and `list_dir` in one round.

Consequence worth recording: streaming and tools coexist, because the calls arrive in their own
field and never inside the text. Nothing has to be held back, and nothing can be half-said.

### L5b — she announces and does not act

After the fix she still said "Voy a buscar «Nerea» en los dos documentos." and emitted no block:
the audit log has no `search` for that turn. Allowing a sentence before the block made it easier
for a flash model to treat the sentence as the whole message. Next step: native function calling
if the provider exposes it, otherwise the block goes back to being the only content allowed.

### L4 — design decisions

- No shell. Three named read-only tools with typed arguments; the model asks
  with a fenced `nerea-tool` block, the app runs it. This keeps the provider
  contract untouched (a new provider stays one entry in `presets.rs`) instead of
  depending on native function calling that `deepseek-v4.1-flash` may not expose.
- The tool block is held back while streaming, so a tool call never flickers in
  the chat. A normal reply is released on its first character.
- Observations are `user` turns in the live conversation but never in the
  transcript: she keeps the context of what she read, memory stays a
  conversation.
