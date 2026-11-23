# Nerea — single window, memory and startup lines

One window that is her portrait plus an attached chat panel; a separate read-only
history window opened on demand; conversation persisted across restarts. Branch
`main`, delivery `single-pr`, runner `cargo test`.

## Specs

- **S1** One visible window: the avatar. Launching the app shows no other window.
  The 940x660 chat window exists but starts hidden.
  Quote: "ve por una sola ventana".
- **S2** The chat panel is attached to her portrait and lives in the same window.
  A double click on her toggles it; dragging still moves the window.
  Quote: "con ciertos clicks con 2 clicks abrirle un chat algo pequeño o leve o
  ajustable en su retrato de personaje en el escritorio".
- **S3** The panel holds a short scrollable log, the input, send/stop, and a menu
  with: full history, dock, erase memory, quit. `Esc` closes the panel.
- **S4** Closed, the window measures exactly her portrait, so it never steals
  desktop clicks. Opening the panel grows the window upward and to the left,
  keeping her feet fixed on screen.
- **S5** The detailed history window is read-only and opens only from the panel
  menu: full transcript, message count, estimated tokens, provider and model.
  Quote: "ese chat que solo sirva para ver el historial a detalle de todo el chat
  y consumo ... que no se abre apenas abras la app".
- **S6** The conversation survives a restart: the last turns are reloaded and she
  continues where the conversation stopped.
  Quote: "no se si ella recuerda los chats y lo que hablamos, eso es importante".
- **S7** `erase memory` clears both the in-memory conversation and the file.
- **S8** Startup lines vary: at least 8 `boot` lines for a fresh start and 4
  `resume` lines when there is prior history. Within a category the same line is
  never picked twice in a row.
  Quote: "necesito algunas dialogos de inicio mejores, u otros para generar
  variedad es raro cuando cambia de dialogo".
- **S9** Recorded requirement, not implemented: Nerea will control the PC. Any
  future capability of that kind needs an explicit permission model and a
  per-action confirmation before it runs. Quote: "nerea tiene control de la pc".

Non-goals: touching the Hyprland config (the window frame stays), live TTS for
replies, code-change execution.

## Tasks

| ID | Specs | Route | Status |
|----|-------|-------|--------|
| T1 | - | inline | done: this document |
| T2 | S6, S7 | inline | pending: `nerea-core` transcript store + tests |
| T3 | S6, S7 | inline | pending: `AppState` load/append/clear, status tokens |
| T4 | S1, S2, S4, S5 | inline | pending: windows, panel geometry, history commands |
| T5 | S2, S3 | inline | pending: avatar panel markup, styles, behaviour |
| T6 | S5 | inline | pending: read-only history window |
| T7 | S8 | inline | pending: bank lines, `greet` command, render |
| T8 | - | inline | pending: README, verification, push |

## Log

### L1

excelente, sabes que ese mensaje de bienvenida siempre el mismo, es como muy.... mmm poco a poco iremos agregnadole dialogos, recuerda eso, pero esta bien, lo que no me convence, es, ese chat que se abre adicional con nerea, y el borde que tiene su personaje en su recuadro, el chat adicional siento que esta de mas, no podemos yo que se con ceirtos clicks con 2 clicks abrirel un chat algo pequeño o leve o ajustable en su retrato de personaje en el escritorio? es mi pregunta o duda, que veo mejor, no se que dices tu

### L2

ve por una sola ventana, si el marco es de mi hyperland dejalo como esta, ese chat que solo sirva para ver el historial a detalle de todo el chat y consumo y todo eso, pero que no se abre apenas abras la app, eso que quede como por un boton por alli que el usuario pueda acceder en opciones de Nerea, recuerda esto, nerea tiene control de la pc, y necesito algunas dialogos de inicio mejores, u otros para generar variedad es raro cuando cambia de dialogo y ya pues, seria todo para dejarme probarla y tenerla como mi asistente, no se si ella recuerda los chats y lo que hablamos, eso es importante
