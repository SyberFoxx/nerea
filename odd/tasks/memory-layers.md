# memory-layers (proposed, not authorized)

Layered memory for Nerea: identity that never changes, facts that live in cards,
and a rolling summary of what already happened. Recorded as a proposal — no task
starts until the user picks the write policy (see "Open decision").

## Specs (proposed)

S1. The prompt is composed of three separately budgeted blocks — identity,
    summary, selected cards — instead of one prompt where everything competes for
    the same space.
S2. Durable facts live in small Markdown cards, one per topic, in
    `~/.local/share/nerea/cards/`, each capped at 2 KB.
S3. On every turn only the 2–3 cards that score against the user's message are
    injected; nothing is injected when nothing scores.
S4. A card can be created or updated by her through an explicit tool, with the
    change visible in the audit log and bounded to her own directory.
S5. The selection is a pure function, unit-tested: given cards and a message it
    returns the same result every time.

## Log

### L1 — the advice, verbatim

> Las mejores prácticas para asistentes persistentes no consisten en guardar todo, sino en
> organizar la memoria en capas y enseñarle a buscar solo lo que necesita en cada momento.
>
> **Un "Núcleo de Identidad" siempre presente (Tier 1):** No cargues solo el system prompt con las
> 400 líneas de personalidad. Crea un archivo pequeño (unos 2KB) que se inyecte siempre y que
> contenga lo esencial: quién es Nerea, quién eres tú (Lioner), y los hechos inmutables de vuestra
> relación.
>
> **"Tarjetas de Conocimiento" (Tier 2):** ... guarda los hechos importantes en archivos pequeños
> por tema: gustos_lioner.md, proyectos_actuales.md, lore_novela.md, eventos_clave.md. ... cargar
> solo las 2 o 3 que sean relevantes para el tema actual.
>
> **Un "Resumen Rodante" (Tier 3):** ... pídele a un modelo ... que resuma los turnos antiguos en
> un párrafo.

### L2 — what already exists (measured, 0.6.2)

Two of the three tiers are already built. Recording this so the next session does
not rebuild them:

| tier | state | evidence |
|---|---|---|
| 1 — identity | **exists** | `crates/nerea-core/src/persona.rs`, 5 673 chars (~1 400 tokens), injected on every request by `state::system_prompt`, capped by a size test |
| 3 — rolling summary | **exists** | `memory::plan` folds old turns once `pending > TRIGGER`; `state::summarize_if_due` writes a 350-word summary into `memory.json`, injected via `request_with_context` |
| 2 — knowledge cards | **missing** | `recall::terms` scores the *transcript* lexically; it returns old conversation fragments, not facts filed by topic |

Budget today: `CONTEXT_BUDGET_TOKENS = 8_000`. Identity ~1 400 tokens,
summary ~450, recall snippets ~300 when they hit. Cards would take ~300–600.

### L3 — what a card is that a recalled fragment is not

- A card is **edited**, a fragment is appended. "proyectos_actuales" changes when
  the project changes; a turn from March cannot.
- A card survives **independently of when it was said**. Recall can only find what
  was already written in a message.
- A card is **readable and fixable by the user**. When she is wrong, he opens the
  file and corrects it — no prompt archaeology.

That difference is the whole feature. Card 2 is not a bigger index.

### L4 — design

**Storage.** `~/.local/share/nerea/cards/<topic>.md`, plain Markdown with four
front-matter lines: `topic`, `title`, `tags` (comma-separated), `updated`.
Files, not a database: the user can read them, `read_file`/`search` already work
on them, `git` can back them up, and there is no migration. 2 KB cap per card,
enforced by a test like the persona's.

**Index.** `cards/index.json`: topic, title, tags, mtime, size. Rebuilt when an
mtime changes; cheap enough to check on boot and after a write.

**Selection.** `begin_turn` scores cards against the user's message with the
existing `recall::terms` machinery: tag hits weigh more than title, title more
than body. Take the top 3 above a floor; inject as a block in the system message:
"Lo que sabés sobre esto:". Nothing scores, nothing is injected — the common case
must cost zero tokens.

**Write path, two options:**

- *A — explicit tool* `remember {topic, fact}`. She writes when asked ("acordate
  que..."). Predictable, auditable, one new write path into her own directory.
- *B — automatic extraction* after each summary: a second small call proposes card
  updates from the new turns. Learns on its own, but invents noise when the model
  is bored, and every write needs a review story.

**Prompt composition.** Three blocks with separate budgets, assembled in
`state::system_prompt` / `request_with_context`: identity (always), summary
(always), cards (selected). Identity stops being the only thing that is always
there, and the summary stops sharing its budget with facts.

**Semantic similarity — honest tradeoff.** Embeddings mean another endpoint and
key, another latency per turn, or a local model measured in gigabytes on a machine
that already carries 11 GB of voice weights. Recommendation: lexical plus
hand-curated `tags` first, which is what a human curator does anyway; measure the
misses for a week; only then consider embeddings. A wrong card is worse than no
card, and the floor in S3 is what keeps that true.

**Risks.** Cards rot → `updated` in the prompt, she says the fact may be old past
30 days, and the user can fix the file. Card bloat → 2 KB cap. Wrong card →
top 3 only, above a floor, and a test with fixtures that must not be pulled.

### L5 — phases

1. **Cards read-only** (~200 lines): directory, index, selection, prompt block,
   tests with three fixture cards. No behaviour change until the first card exists.
2. **`remember` tool** (~80 lines): create/update, 2 KB cap, audit entry, never
   deletes. Verifiable end to end: he tells her a fact, restarts, she recalls it.
3. **Automatic extraction** (optional, off by default): only if phases 1–2 prove
   they earn their tokens.

Acceptance for phase 1: asking about the novel pulls `lore_novela.md` and not
`proyectos_actuales.md`; asking "hola" injects no card at all.

### L6 — open decision

Does she write cards on her own initiative, or only when the user asks? Phase 2 is
the cheap answer; phase 3 is the ambitious one. Everything else in this document
is independent of that choice.
