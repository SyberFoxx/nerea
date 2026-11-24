# system-access-tiers (inventory + proposal)

What she can do with the machine today, what is worth improving before anything
new is built, and what is still missing to reach tiers B and C. Recorded so the
next session starts from the real state instead of guessing.

## Log

### L1 — inventory as of 0.6.2

| tool | what it does | limits | where |
|---|---|---|---|
| `read_file` | text, code, Markdown, JSON, PDF (text layer), `.docx`, `.odt`, EPUB, HTML; windows of lines with `offset`/`limit` | 300 lines by default, 1 200 max; 12 000 chars per observation; 64 MB per file; refuses a binary | `tools/mod.rs`, `tools/extract.rs` |
| `list_dir` | name, kind, size | 200 entries | `tools/mod.rs` |
| `search` | literal, case-insensitive, inside the files of a directory | **plain text only**; 2 MB per file; 4 000 files; depth 6; skips `.git`, `target`, `node_modules`, `.venv`, `venv`, `dist`, `build` | `tools/mod.rs` |

Around them:

- **Native function calling** — specs in `tools`, calls in `tool_calls`, results as
  `role: "tool"` bound by `tool_call_id`. Three rounds per message, at most four
  calls per round.
- **Guard** — `~/.ssh`, `~/.gnupg`, `~/.pki`, `~/.password-store`, browser
  profiles, her own data and config dirs, the keyring, `/etc/shadow`,
  `/etc/sudoers*`, `/proc`, `/sys`, and credential-shaped names, all checked
  *after* resolving symlinks.
- **Audit** — one JSON line per call in `~/.local/share/nerea/audit.jsonl`.
- **Desktop only** — `nerea-cli` builds its conversation without tools.

### L2 — improve what exists before adding anything

1. **`search` cannot see inside a `.docx`, `.odt` or `.pdf`.** This is the
   expensive one: the novel is `.docx`, so "buscá «Nerea» en la novela" can only
   find it by `list_dir` and `read_file`, never by content. Proposal: a
   `deep: true` flag on `search` that runs `extract::extract` on those formats,
   capped at ~40 files and 2 MB each, stopping at the first `limit` hits. Plain
   text stays the default because it is ten times cheaper.
2. **No way to find a file by name.** She used `list_dir` three times in a row to
   locate something. Proposal: `find {pattern, path}`, a case-insensitive glob on
   the file name, same walk caps as `search`.
3. **Rounds are bounded, tokens are not.** Three rounds × four calls × 12 000
   chars is up to ~36 000 tokens in one message, more than the context budget.
   Proposal: a per-turn observation budget (for example 24 000 chars total) and
   the remaining calls refused with a clear message.
4. **`read_file` returns no line numbers.** She can say "líneas 40-60" but cannot
   cite `52: texto`. Cheap to add and it makes her answers checkable.
5. **The audit is invisible.** Today it is `tail -f`. Proposal: a read-only tab in
   the history window showing the last calls and their outcome. That is the
   feature that makes "she can touch my machine" tolerable to live with.
6. **`read_file` cannot see an image, audio or video.** Say it in the tool
   description so she stops trying, and keep OCR out unless it is asked for.
7. **`list_dir` does not distinguish a symlink from a file.** Small, but it is the
   kind of thing that makes a listing lie.

### L3 — tier B: commands (missing, undesigned on purpose)

The shape of the permission is the user's decision, so nothing here is built. What
it needs, in the order it matters:

1. **`run_command { program, args[], cwd? }`** — argv, never a shell string, so
   `;`, `|` and `$(...)` cannot exist as concepts.
2. **A binary allowlist** in `~/.config/nerea/tools.toml`, empty by default.
3. **A confirmation card** showing the exact argv, the cwd and a fingerprint:
   *allow once*, *allow always this fingerprint*, *deny*. Allow-always is per
   fingerprint, never per tool — otherwise one click is a permanent shell.
4. **Timeout of 10 s**, the process tree killed, stdout and stderr capped.
5. **A scrubbed environment** — no API key in the child.
6. **No `sudo`**, and no path that reaches a credential file.
7. **The decision in the audit**, next to the call it authorised.
8. **Cancel** while it runs; the existing `take_cancel` is per turn, not per call.

### L4 — tier C: the rest

- `write_file` / `edit_file`, same guard, diff preview in the confirmation.
- `remember` — a memory card (see `memory-layers.md`), the only write that needs
  no confirmation because the user's request *is* the consent.
- Clipboard read and write.
- Notifications (`libnotify`).
- Window control: the Hyprland command socket already exists for her own window.
- Media: `playerctl` play/pause/next.

### L5 — order I would defend

1. **Deep search + `find`** — cheap, removes real friction, no new permission.
2. **Audit in the history window** — makes everything else trustworthy to use.
3. **The permission model and its confirmation card** — the load-bearing piece.
4. **`run_command` behind that model.**
5. **`remember`**, then `write_file`.
6. Clipboard, notifications, media.

The permission model comes before the command, not after: a tool built first and
guarded later is a tool that ran unguarded once.

### L6 — decisions needed

- Which binaries go in the initial allowlist (`ls`, `cat`, `rg`, `git status`?).
- Does the confirmation card grow over her own window or appear as its own?
- Does "allow always" survive a restart, and where does it live?
