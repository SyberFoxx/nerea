# work-queue

The order to work in, one task per day a session exists. Each task carries its own
acceptance criteria so a session can start without re-deciding anything. States:
`proposed` → `in progress` → `shipped <version>`. A task is only `shipped` when its
acceptance line was observed, not when the code compiled.

Source documents: `system-access-tiers.md` (tool surface and gaps) and
`memory-layers.md` (memory). This file is the queue; those files are the reasoning.

## Queue

| id | task | why it matters | size | state |
|---|---|---|---|---|
| T1 | `search` deep inside `.docx` / `.odt` / `.pdf` | the novel is a `.docx`: today it cannot be searched by content, only found by listing | S | **shipped 0.7.0** |
| T2 | `find` a file by name | she used `list_dir` three times in a row to locate one file | S | **shipped 0.7.0** |
| T3 | a per-turn observation budget | rounds are bounded, tokens are not: worst case ~36k tokens against an 8k budget | S | **shipped 0.7.0** |
| T4 | `numbers` flag on `read_file` | she can say "líneas 40-60" but cannot cite `52: texto` | S | proposed |
| T5 | the audit, visible in the history window | today it is `tail -f`; without this, "she can touch my machine" is not livable | M | proposed |
| T6 | `list_dir` distinguishes symlink, hidden and file | a listing that lies is worse than a short one | S | proposed |
| T7 | honest tool descriptions for images, audio and video | she should stop trying instead of failing twice | S | proposed |
| T8 | knowledge cards + `remember` (memory tier 2) | the only real memory gap; details in `memory-layers.md` | M | proposed |
| T9 | tier B: permission model + `run_command` | the load-bearing piece of PC control; waits on the user's decisions | L | undesigned |
| T10 | tier C: write, clipboard, notifications, media | only after B exists and is trusted | L | undesigned |

## Acceptance, per task

**T1 — deep search.** `search {"query":"Nerea","path":"~/Documentos/novela","deep":true}`
returns the line inside `Nerea.docx`. Plain text is still the default and still
skipped for those formats. Caps: 40 extracted files, 2 MB each, stop at `limit`.
The audit line records `deep: true`. A malformed document is skipped, never fatal.

**T2 — find.** `find {"pattern":"nerea","path":"~/Documentos"}` returns every path
whose file name contains it, case-insensitively, cap 200, same walk skips as
`search`. One call replaces the three `list_dir` calls.

**T3 — observation budget.** *Revised while building T1:* 24 000 characters was measured
against the 8 000-token window and does not fit — two full observations already ate
three quarters of it and the prompt lost the question. Shipped as 8 000 characters per
observation (12k before) and 16 000 per message, plus `compact_observations(400)` before
each new round: older readings keep their first lines so the question survives.
A test drives two full observations and asserts the third is refused.

**T4 — line numbers.** `read_file {"path":…,"numbers":true}` prefixes every line
with its number, and a line the user can check with `grep -n` matches exactly.
Default stays `false`: 300 numbered lines cost ~1.8k characters of budget.

**T5 — audit in the history window.** A read-only section lists the last 50 calls:
time, tool, summary, ok, bytes. It reads `audit.jsonl` through one command, never
parses it in JavaScript. A call made from the chat appears without touching a
terminal.

**T6 — honest listing.** Each row starts with its kind: `d` directory, `l` symlink,
`f` file, and a leading `.` name is visible as such. Existing tests keep passing.

**T7 — honest descriptions.** The `read_file` description says images, audio and
video are not readable, and the refusal message says so once instead of a binary
detection message. No behaviour change beyond the words.

**T8 — cards.** Phase 1 (read-only cards, selection, prompt block) then phase 2
(`remember`), as written in `memory-layers.md`. Acceptance there: asking about the
novel pulls `lore_novela.md` and not `proyectos_actuales.md`; "hola" injects no
card; a remembered fact survives a restart.

**T9 — tier B.** Not startable until the user answers: which binaries are allowed
at first, whether the confirmation card grows over her window or gets its own, and
whether "allow always" survives a restart. Everything else is specified in
`system-access-tiers.md`, L3.

**T10 — tier C.** Depends on T9: every write tool reuses the same confirmation and
fingerprint. `remember` is the exception (the request is the consent).

## Known debt (not features, but they are the errors)

| id | problem | impact | state |
|---|---|---|---|
| D1 | a brief slide is still perceived when the window closes | cosmetic; she always lands in the right place | known, accepted |
| D2 | the TTS engine is not restarted inside its 3-minute cooldown | the first line after a long pause stays silent | known |
| D3 | an installed build does not update itself | a release exists and the running app does not know | known |
| D4 | CI builds artifacts but runs no tests | a broken release can ship green | known |
| D5 | recall is lexical only | a paraphrase does not find the old turn | known; cards (T8) cover the facts |
| D6 | the desktop app has tools, the CLI has none | the headless harness cannot be used to test them | known |
| D7 | restricting the tools makes this model write its DSML call syntax into the prose | an empty `tools` array and `tool_choice: "none"` both leak `<｜DSML｜ …>` into the chat | known; avoided by never restricting, worth a sanitizer if a provider changes |
| D8 | she sometimes opens a turn in English before switching to Spanish | cosmetic, but it breaks the voice | known |

Debt is recorded, not scheduled: it gets picked when it hurts more than the next
feature, and the row moves to the queue when that happens.

## Shipped

| version | tasks | evidence |
|---|---|---|
| `0.7.0` | T1, T2, T3 | `search` with `deep: true` found "Capítulo 1" at line 1 inside `Nerea.docx`; `find` located both copies by name; 120 tests, clippy clean |
