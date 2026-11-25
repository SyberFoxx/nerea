const tauri = window.__TAURI__;

// An exception inside a handler used to leave the chat saying "pensando" with no
// explanation. Now it says what broke, in the chat, where it can be read.
window.addEventListener("error", (event) => {
  const message = event?.message ?? "error desconocido";
  const log = document.getElementById("log");
  if (!log) return;
  const line = document.createElement("p");
  line.className = "bubble failed";
  line.textContent = `⚠ ${message}`;
  log.append(line);
});

const stage = document.getElementById("stage");
const sprite = document.getElementById("sprite");
const grip = document.getElementById("grip");
const panel = document.getElementById("panel");
const talk = document.getElementById("talk");
const pendingBlock = document.getElementById("pending");
const pendingList = document.getElementById("pending-list");
const approveBlock = document.getElementById("approve");
const approveBody = document.getElementById("approve-body");
const log = document.getElementById("log");
const form = document.getElementById("composer");
const prompt = document.getElementById("prompt");
const sendButton = document.getElementById("send");
const stopButton = document.getElementById("stop");
const menuButton = document.getElementById("menu-button");
const menu = document.getElementById("menu");
const closeButton = document.getElementById("close-button");
const subtitle = document.getElementById("subtitle");

const IDLE_AFTER_MS = 4 * 60 * 1000;
const SUBTITLE_TAIL_MS = 500;
/** How far the pointer may travel before a press counts as a drag. */
const PRESS_SLOP_PX = 6;
const RECENT_MESSAGES = 40;

/** Which frame a phrase-bank category shows while it is spoken. */
const CATEGORY_FRAME = {
  boot: "hello",
  resume: "hello",
  timeofday: "hello",
  idle: "idle",
  back: "back",
  thinking: "thinking",
  talking: "talking",
  micro: "agree",
  agree: "agree",
  surprise: "surprise",
  laugh: "laugh",
  tease: "tease",
  flirt: "flirt",
  jealous: "jealous",
  affection: "affection",
  complain: "complain",
  goodbye: "goodbye",
};

let frames = new Map();
let turnFrame = "idle";
let speakingTimer = null;
let speaking = false;
let toolNoteTimer = null;
let idleTimer = null;
let panelOpen = false;
let listenTimer = null;
let listening = false;
let replyTimer = null;
let workingLine = null;
let approvalPoll = null;

const LISTEN_ERRORS = {
  "muy corto": "Muy corto: mantené un segundo más.",
  silencio: "No hubo sonido.",
  "no te escuché": "No te escuché, probá otra vez.",
};
let busy = false;
let streamingBubble = null;
let press = null;
let dragSession = null;
let armReset = null;
let armUpdate = null;
let updateEntry = null;
let voiceEntry = null;

function invoke(command, args) {
  return tauri.core.invoke(command, args);
}

function show(name) {
  const source = frames.get(name) ?? frames.get("idle");
  if (!source) return;
  if (sprite.src !== source) sprite.src = source;
  sprite.dataset.state = name;
}

function scrollToEnd() {
  log.scrollTop = log.scrollHeight;
}

/// The card that asks for a click. The argv is the whole point: "allow pandoc"
/// without seeing the arguments is a permission nobody reviewed.
function showApproval(request) {
  if (!approveBlock || !approveBody || !request) return;

  approveBody.replaceChildren();

  const head = document.createElement("header");
  head.className = "change-head";

  const command = document.createElement("strong");
  command.textContent = [request.program, ...(request.args ?? [])].join(" ");
  head.append(command);

  const cwd = document.createElement("p");
  cwd.className = "approve-cwd";
  cwd.textContent = `en ${request.cwd ?? "?"}`;

  const actions = document.createElement("div");
  actions.className = "change-actions";

  const allow = document.createElement("button");
  allow.type = "button";
  allow.className = "change-apply";
  allow.textContent = "Permitir";

  const deny = document.createElement("button");
  deny.type = "button";
  deny.className = "change-discard";
  deny.textContent = "Denegar";

  const answer = async (approved) => {
    allow.disabled = true;
    deny.disabled = true;
    approveBlock.hidden = true;
    setWorking(approved ? "ejecutando…" : "denegado");
    await invoke("answer_approval", { id: request.id, approved }).catch(() => {});
  };

  allow.addEventListener("click", () => answer(true));
  deny.addEventListener("click", () => answer(false));

  actions.append(allow, deny);
  approveBody.append(head, cwd);

  if (request.warning) {
    const warning = document.createElement("p");
    warning.className = "approve-warning";
    warning.textContent = request.warning;
    approveBody.append(warning);
  }

  approveBody.append(actions);
  approveBlock.hidden = false;
}

/// Asks the app whether a command is waiting for a click. The event usually
/// arrives; this is what makes the card impossible to miss.
async function checkApproval() {
  const pending = await invoke("pending_approval").catch((error) => {
    setWorking(`sondeo falló: ${describe(error)}`);
    return null;
  });
  if (!pending) return;
  if (approveBlock && !approveBlock.hidden) return;
  showApproval(pending);
}

/// One line that says what is happening right now: thinking, running a program,
/// or waiting for his click. Without it a blocked turn looks like a dead chat.
function setWorking(text) {
  if (!text) {
    workingLine?.remove();
    workingLine = null;
    return;
  }

  if (!workingLine) {
    workingLine = document.createElement("p");
    workingLine.className = "working";
    log.append(workingLine);
  }
  workingLine.textContent = text;
  scrollToEnd();
}

function addBubble(role, text = "") {
  log.querySelector(".empty")?.remove();

  const article = document.createElement("article");
  article.className = `bubble ${role}`;

  const body = document.createElement("p");
  body.textContent = text;
  article.append(body);
  log.append(article);
  scrollToEnd();

  return body;
}

function clearLog(message) {
  log.replaceChildren();
  if (message) {
    const empty = document.createElement("p");
    empty.className = "empty";
    empty.textContent = message;
    log.append(empty);
  }
}

/* ---------------------------------------------------------------- speech */

function endSpeech() {
  clearTimeout(speakingTimer);
  clearTimeout(toolNoteTimer);
  speakingTimer = null;
  toolNoteTimer = null;
  speaking = false;
  subtitle.hidden = true;
  show(turnFrame);
}

/**
 * Says what she is reading, without speaking it.
 *
 * Reading a file takes a round trip, and a silent window looks broken. The note
 * clears itself, and the real answer clears it sooner.
 */
function showToolNote(text) {
  if (!text) return;

  subtitle.textContent = text;
  subtitle.hidden = false;

  clearTimeout(toolNoteTimer);
  toolNoteTimer = setTimeout(() => {
    toolNoteTimer = null;
    if (!speaking) subtitle.hidden = true;
  }, 6000);
}

function clearToolNote() {
  if (!toolNoteTimer) return;

  clearTimeout(toolNoteTimer);
  toolNoteTimer = null;
  if (!speaking) subtitle.hidden = true;
}

/// Her answer where he is looking: the chat if it is open, and otherwise the
/// box over her, clamped. Expanding it is the chat's job.
function showReply(text) {
  const answer = (text ?? "").trim();
  if (!answer || panelOpen) return;

  subtitle.textContent = answer;
  subtitle.hidden = false;
  clearTimeout(replyTimer);
  replyTimer = setTimeout(() => {
    if (!speaking) subtitle.hidden = true;
  }, 11_000);
}

function playLine(line) {
  if (!line) return;

  clearTimeout(speakingTimer);
  speaking = true;

  show(CATEGORY_FRAME[line.category] ?? "idle");
  subtitle.textContent = line.es;
  subtitle.hidden = false;

  const hold = Math.min((line.duration_ms ?? 2000) + SUBTITLE_TAIL_MS, 12_000);
  speakingTimer = setTimeout(endSpeech, hold);

  if (panelOpen) addBubble("murmur", line.es);
}

function noteActivity() {
  clearTimeout(idleTimer);
  idleTimer = setTimeout(() => {
    if (!speaking && !busy && tauri?.core) invoke("say", { category: "idle" }).catch(() => {});
  }, IDLE_AFTER_MS);
}

/* ----------------------------------------------------------------- panel */

async function loadRecent() {
  if (busy) return;

  const entries = await invoke("transcript", { limit: RECENT_MESSAGES }).catch(() => []);
  clearLog(entries.length === 0 ? "Todavía no hablamos de nada." : "");
  for (const entry of entries) {
    addBubble(entry.role === "user" ? "user" : "nerea", entry.content);
  }
  scrollToEnd();
}

/**
 * Resolves once a frame with the current styles has been painted.
 *
 * Hiding her and resizing the window are two different processes; without this
 * gate the move reaches the compositor before the page paints the hide, and she
 * is seen sliding to the corner the resize is about to land on.
 */
function painted() {
  return new Promise((resolve) => {
    let done = false;
    const finish = () => {
      if (done) return;
      done = true;
      resolve();
    };
    requestAnimationFrame(() => requestAnimationFrame(finish));
    setTimeout(finish, 200);
  });
}

/// What she proposed and nobody approved. The diff is the whole point: a card
/// with a path and a button would be a card nobody read.
async function loadPending() {
  if (!pendingBlock || !pendingList) return;

  const changes = await invoke("pending_changes").catch(() => []);
  pendingList.replaceChildren();
  pendingBlock.hidden = changes.length === 0;
  if (changes.length === 0) return;

  for (const change of changes) {
    const card = document.createElement("article");
    card.className = "change";

    const head = document.createElement("header");
    head.className = "change-head";

    const name = document.createElement("strong");
    name.title = change.path;
    name.textContent = change.path.split("/").slice(-2).join("/");

    const kind = document.createElement("span");
    kind.className = "change-kind";
    kind.textContent = `${change.kind} · +${change.added} −${change.removed}`;

    head.append(name, kind);

    const diff = document.createElement("pre");
    diff.className = "change-diff";
    diff.textContent = change.preview;

    const actions = document.createElement("div");
    actions.className = "change-actions";

    const apply = document.createElement("button");
    apply.type = "button";
    apply.className = "change-apply";
    apply.textContent = "Aplicar";

    const discard = document.createElement("button");
    discard.type = "button";
    discard.className = "change-discard";
    discard.textContent = "Descartar";

    apply.addEventListener("click", async () => {
      apply.disabled = true;
      const said = await invoke("apply_change", { id: change.id }).catch((error) => `⚠ ${describe(error)}`);
      addBubble("murmur", said);
      await loadPending();
    });

    discard.addEventListener("click", async () => {
      discard.disabled = true;
      await invoke("discard_change", { id: change.id }).catch(() => {});
      await loadPending();
    });

    actions.append(apply, discard);
    card.append(head, diff, actions);
    pendingList.append(card);
  }
}

/** Opens the chat: the window grows around her, she does not move. */
async function openPanel() {
  if (panelOpen) return;
  panelOpen = true;

  // Her reply lives in the chat now; a box over it would clip its own tail.
  subtitle.hidden = true;

  stage.style.visibility = "hidden";
  panel.hidden = false;
  stage.dataset.panel = "open";
  await painted();
  await invoke("open_panel").catch(() => {});
  stage.style.visibility = "visible";

  await loadRecent();
  await loadPending();
  await askAboutVoice();
  prompt.focus();
  noteActivity();
}

/** Back to her portrait. Closing the chat is not leaving, so nothing is said. */
async function closePanel() {
  if (!panelOpen) return;
  panelOpen = false;

  hideMenu();

  stage.style.visibility = "hidden";
  panel.hidden = true;
  stage.dataset.panel = "closed";
  await painted();
  await invoke("close_panel").catch(() => {});
  stage.style.visibility = "visible";
}

async function refreshUpdate() {
  if (!updateEntry) return;

  const status = await invoke("update_status").catch(() => null);
  if (!status || !status.repo) {
    updateEntry.hidden = true;
    return;
  }

  if (status.behind > 0) {
    updateEntry.hidden = false;
    updateEntry.disabled = status.dirty;
    updateEntry.textContent = status.dirty
      ? `Actualizar (${status.behind}) — hay cambios sin commitear`
      : `Actualizar (${status.behind} nuevo${status.behind === 1 ? "" : "s"})`;
    return;
  }

  updateEntry.hidden = true;
}

const VOICE_QUESTION = {
  title: "¿Querés que hable?",
  body:
    "Puedo decir cada respuesta en voz alta, con mi voz clonada. Usa el motor local: " +
    "unos 3.6 GB de RAM y 2.8 GB de VRAM mientras está encendido. Se levanta solo cuando " +
    "hace falta y se apaga solo tras quince minutos sin uso. Si decís que no, sigo " +
    "respondiendo por texto y no gasto nada.",
};

async function voicePreference() {
  return invoke("voice_preference").catch(() => null);
}

/** The question is asked once, in the chat, and the answer is remembered. */
async function askAboutVoice() {
  const already = await voicePreference();
  if (already !== null && already !== undefined) return;

  const card = document.createElement("article");
  card.className = "bubble question";

  const title = document.createElement("strong");
  title.textContent = VOICE_QUESTION.title;

  const body = document.createElement("p");
  body.textContent = VOICE_QUESTION.body;

  const actions = document.createElement("div");
  actions.className = "question-actions";

  const yes = document.createElement("button");
  yes.type = "button";
  yes.className = "yes";
  yes.textContent = "Sí, que hable";

  const no = document.createElement("button");
  no.type = "button";
  no.textContent = "No, solo texto";

  actions.append(yes, no);
  card.append(title, body, actions);
  log.append(card);
  scrollToEnd();

  const answer = async (on) => {
    await invoke("voice_set_preference", { on }).catch(() => {});
    card.remove();
    addBubble(
      "murmur",
      on
        ? "Listo. Hablo en la próxima respuesta; si querés apagarme, está en el menú."
        : "Listo, sigo por texto. Podés cambiar de idea desde el menú.",
    );
    await refreshVoice();
  };

  yes.addEventListener("click", () => answer(true));
  no.addEventListener("click", () => answer(false));
}

async function refreshVoice() {
  if (!voiceEntry) return;

  const [preference, state] = await Promise.all([
    voicePreference(),
    invoke("voice_state").catch(() => null),
  ]);

  if (preference === null || preference === undefined) {
    voiceEntry.textContent = "Voz: preguntarme";
    voiceEntry.disabled = false;
    return;
  }

  if (!preference) {
    voiceEntry.textContent = "Voz: apagada · encender";
    voiceEntry.disabled = state === "unavailable";
    return;
  }

  const labels = {
    ready: "Voz: encendida · apagar",
    starting: "Voz: calentando…",
    off: "Voz: encendida · motor dormido",
    unavailable: "Voz: sin motor en esta máquina",
  };

  voiceEntry.textContent = labels[state] ?? `Voz: ${state}`;
  voiceEntry.disabled = state === "unavailable" || state === "starting";
}

function hideMenu() {
  menu.hidden = true;
  menuButton.setAttribute("aria-expanded", "false");
}

function toggleMenu() {
  const open = menu.hidden;
  menu.hidden = !open;
  menuButton.setAttribute("aria-expanded", String(open));
}

/* -------------------------------------------------------------- sending */

function setBusy(value) {
  busy = value;
  clearInterval(approvalPoll);
  if (value) approvalPoll = setInterval(checkApproval, 900);
  // One button, two shapes: while she answers there is nothing to send.
  sendButton.hidden = value;
  stopButton.hidden = !value;
  prompt.disabled = value;
  if (!value) prompt.focus();
}

function describe(error) {
  return typeof error === "string" ? error : (error?.message ?? "algo salió mal");
}

async function send(text) {
  addBubble("user", text);
  streamingBubble = addBubble("nerea");
  // The class belongs to the bubble, not to the paragraph inside it: the rule
  // that draws "pensando" is on the bubble, and it was never matching.
  streamingBubble.parentElement.classList.add("thinking");
  setWorking("pensando…");
  setBusy(true);

  try {
    await invoke("send_message", { text });
  } catch (error) {
    streamingBubble.parentElement.classList.remove("thinking");
    streamingBubble.textContent = `⚠ ${describe(error)}`;
    streamingBubble.parentElement.classList.add("failed");
  } finally {
    streamingBubble = null;
    setBusy(false);
  }
}

/* --------------------------------------------------------------- events */

async function loadManifest() {
  const response = await fetch("avatar/manifest.json");
  if (!response.ok) throw new Error(`manifest ${response.status}`);
  return response.json();
}

async function redock() {
  await invoke("dock").catch(() => {});
}

async function start() {
  const manifest = await loadManifest();
  frames = new Map(Object.entries(manifest.states));

  sprite.addEventListener("error", () => {
    stage.dataset.fallback = "true";
  });

  if (!tauri?.core) {
    stage.dataset.fallback = "true";
    return;
  }

  await tauri.event.listen("nerea://state", (event) => {
    turnFrame = event.payload.state;
    noteActivity();
    if (!speaking) show(turnFrame);
  });

  await tauri.event.listen("nerea://delta", (event) => {
    clearToolNote();
    if (!streamingBubble) return;
    streamingBubble.parentElement.classList.remove("thinking");
    streamingBubble.textContent += event.payload.text;
    scrollToEnd();
  });

  // A message that came in through the microphone never passed through the
  // composer, so the chat has to be told about both halves of it.
  await tauri.event.listen("nerea://heard", (event) => {
    const said = event.payload?.text ?? "";
    if (!said) return;
    addBubble("user", said);
    streamingBubble = addBubble("nerea");
    streamingBubble.parentElement.classList.add("thinking");
    setBusy(true);
    scrollToEnd();
  });

  await tauri.event.listen("nerea://reply", async (event) => {
    const answer = event.payload?.text ?? "";
    if (streamingBubble) {
      streamingBubble.parentElement.classList.remove("thinking");
      if (answer) streamingBubble.textContent = answer;
      streamingBubble = null;
    }
    setBusy(false);

    // A tool may have staged a proposal during this turn; the cards are the only
    // place it can be approved, so they are refreshed on every answer.
    await loadPending();

    // He asked for the answer in the chat, not in a box: a message that came
    // through the microphone opens it, and the transcript now holds both turns.
    if (!panelOpen) {
      showReply(answer);
      await openPanel();
    }
  });

  await tauri.event.listen("nerea://approve", (event) => showApproval(event.payload));

  await tauri.event.listen("nerea://pending", () => loadPending());

  await tauri.event.listen("nerea://say", (event) => playLine(event.payload));

  await tauri.event.listen("nerea://tool", (event) => {
    setWorking(event.payload.text);
    showToolNote(event.payload.text);
  });

  // The microphone: "Te escucho…" while the key is down, the reason when there
  // was nothing to hear, and out of the way the rest of the time.
  await tauri.event.listen("nerea://listen", (event) => {
    if (talk) talk.textContent = event.payload?.listening ? "Escucho… (tocá para enviar)" : "Hablar";

    const payload = event.payload ?? {};
    clearTimeout(listenTimer);

    listening = false;
    if (payload.error) {
      subtitle.textContent = LISTEN_ERRORS[payload.error] ?? payload.error;
      subtitle.hidden = false;
      listenTimer = setTimeout(() => {
        if (!speaking) subtitle.hidden = true;
      }, 4200);
      return;
    }

    listening = Boolean(payload.listening);
    if (payload.listening) {
      subtitle.textContent = "Te escucho…";
      subtitle.hidden = false;
      // The recorder stops itself after 30 s; the label has to stop too.
      listenTimer = setTimeout(() => {
        if (!speaking) subtitle.hidden = true;
      }, 35_000);
      return;
    }

    if (!speaking) subtitle.hidden = true;
  });

  await tauri.event.listen("nerea://recall", () => {
    addBubble("murmur", "Me acordé de algo que hablamos hace un tiempo…");
  });

  await tauri.event.listen("nerea://memory", () => {
    addBubble("murmur", "Guardé en memoria lo importante de lo que hablamos.");
  });

  await tauri.event.listen("nerea://update", (event) => {
    const state = event.payload;
    const line =
      state === "build"
        ? "Compilando la versión nueva…"
        : state === "restarting"
          ? "Listo. Reiniciando."
          : typeof state === "string"
            ? state
            : "";
    if (line) addBubble("murmur", line);
  });

  // Development aid: the shell can ask for the panel without a pointer.
  await tauri.event.listen("nerea://panel", (event) => {
    if (event.payload) openPanel();
    else closePanel();
  });

  /* One press on her opens the chat; a press that travels drags her.
     Wayland forbids a client from moving its own window, and the native drag
     region grabs the pointer on the first press, which makes every later press
     unreachable — a click and a drag cannot share that path. So the drag is
     handed to the compositor while the pointer moves; see src-tauri/src/hypr.rs. */
  grip.addEventListener("mousedown", (event) => {
    if (event.button !== 0) return;
    press = { x: event.clientX, y: event.clientY, dragging: false };
    noteActivity();
  });

  window.addEventListener("mousemove", (event) => {
    if (!press) return;

    if (!press.dragging) {
      const far =
        Math.abs(event.clientX - press.x) > PRESS_SLOP_PX ||
        Math.abs(event.clientY - press.y) > PRESS_SLOP_PX;
      if (!far) return;

      press.dragging = true;
      dragSession = { queued: false, inFlight: true };
      invoke("drag_begin")
        .catch(() => {})
        .finally(() => {
          if (!dragSession) return;
          dragSession.inFlight = false;
          followPointer();
        });
      return;
    }

    followPointer();
  });

  /** Asks the compositor to put her where the pointer is; one call at a time. */
  function followPointer() {
    const session = dragSession;
    if (!session) return;

    if (session.inFlight) {
      session.queued = true;
      return;
    }

    session.inFlight = true;
    invoke("drag_step")
      .catch(() => {})
      .finally(() => {
        session.inFlight = false;
        if (session.queued) {
          session.queued = false;
          followPointer();
        }
      });
  }

  window.addEventListener("mouseup", (event) => {
    if (!press) return;
    const { dragging } = press;
    press = null;

    if (dragging) {
      dragSession = null;
      invoke("drag_end").catch(() => {});
      return;
    }

    // A press without travel opens the chat. It never closes it: two clicks in
    // a row are only a double click, and the ✕ is the way out.
    if (event.button === 0) openPanel();
  });

  // A release outside the window never reaches us: forget the press.
  window.addEventListener("blur", () => {
    press = null;
    if (dragSession) {
      dragSession = null;
      invoke("drag_end").catch(() => {});
    }
  });

  grip.addEventListener("contextmenu", (event) => {
    event.preventDefault();
    if (panelOpen) toggleMenu();
    else openPanel();
  });

  updateEntry = menu.querySelector('[data-action="update"]');
  voiceEntry = menu.querySelector('[data-action="voice"]');

  menuButton.addEventListener("click", toggleMenu);
  menuButton.addEventListener("click", refreshVoice);
  closeButton.addEventListener("click", closePanel);

  menu.addEventListener("click", async (event) => {
    const action = event.target?.dataset?.action;
    if (!action) return;

    if (action === "reset") {
      // Two steps, no modal dialog: the second press has to come quickly.
      if (!armReset) {
        const label = event.target.textContent;
        armReset = setTimeout(() => {
          armReset = null;
          event.target.textContent = label;
        }, 4000);
        event.target.textContent = "¿Borrar todo? Volvé a tocar";
        return;
      }
      clearTimeout(armReset);
      armReset = null;
      event.target.textContent = "Borrar memoria";
      await invoke("reset_conversation").catch(() => {});
      clearLog("Todavía no hablamos de nada.");
      hideMenu();
      return;
    }

    hideMenu();

    if (action === "voice") {
      hideMenu();

      const preference = await voicePreference();
      if (preference === null || preference === undefined) {
        await askAboutVoice();
        return;
      }

      const next = !preference;
      await invoke("voice_set_preference", { on: next }).catch(() => {});
      addBubble(
        "murmur",
        next
          ? "Voz encendida. El motor arranca cuando lo necesite."
          : "Voz apagada. El motor ya está detenido.",
      );
      await refreshVoice();
      return;
    }

    if (action === "update") {
      if (!armUpdate) {
        const label = event.target.textContent;
        armUpdate = setTimeout(() => {
          armUpdate = null;
          event.target.textContent = label;
        }, 4000);
        event.target.textContent = "¿Actualizar y reiniciar? Otra vez";
        return;
      }
      clearTimeout(armUpdate);
      armUpdate = null;
      addBubble("murmur", "Trayendo los cambios del repo…");
      try {
        await invoke("update_apply");
      } catch (error) {
        addBubble("murmur", `No pude actualizar: ${describe(error)}`);
      }
      return;
    }

    // The microphone, without a compositor keybind: one press to listen, the
    // next to send. The label says which one the next press will do.
    if (action === "talk") {
      hideMenu();
      if (listening) await invoke("listen_stop").catch(() => {});
      else await invoke("listen_start").catch(() => {});
      return;
    }

    if (action === "history") await invoke("open_history").catch(() => {});
    if (action === "dock") await invoke("dock").catch(() => {});
    if (action === "quit") await invoke("quit").catch(() => {});
  });

  form.addEventListener("submit", (event) => {
    event.preventDefault();
    const text = prompt.value.trim();
    if (!text || busy) return;
    prompt.value = "";
    send(text);
  });

  stopButton.addEventListener("click", () => invoke("cancel_stream").catch(() => {}));

  document.addEventListener("keydown", (event) => {
    if (event.key !== "Escape") return;
    if (!menu.hidden) hideMenu();
    else closePanel();
  });

  show("idle");
  noteActivity();
  refreshUpdate();

  await redock().catch(() => {});
  await invoke("greet").catch(() => {});
}

start().catch((error) => {
  // Her picture is only replaced when the picture itself is missing.
  console.error("avatar failed to start", error);
});
