const sprite = document.getElementById("sprite");
const stage = document.getElementById("stage");
const subtitle = document.getElementById("subtitle");

const tauri = window.__TAURI__;
const IDLE_AFTER_MS = 4 * 60 * 1000;
const SUBTITLE_TAIL_MS = 500;

/** Which frame a phrase-bank category shows while it is spoken. */
const CATEGORY_FRAME = {
  boot: "hello",
  timeofday: "hello",
  idle: "idle",
  back: "back",
  thinking: "thinking",
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
let current = "idle";
let turnFrame = "idle";
let speaking = null;
let speakingTimer = null;
let idleTimer = null;

function show(name) {
  const source = frames.get(name) ?? frames.get("idle");
  if (!source) return;
  current = name;
  if (sprite.src !== source) sprite.src = source;
  sprite.dataset.state = name;
}

function endSpeech() {
  clearTimeout(speakingTimer);
  speakingTimer = null;
  speaking = null;
  subtitle.hidden = true;
  show(turnFrame);
}

function playLine(line) {
  if (!line) return;

  clearTimeout(speakingTimer);
  speaking = line.id;

  show(CATEGORY_FRAME[line.category] ?? "idle");
  subtitle.textContent = line.es;
  subtitle.hidden = false;

  const hold = Math.min((line.duration_ms ?? 2000) + SUBTITLE_TAIL_MS, 12_000);
  speakingTimer = setTimeout(endSpeech, hold);
}

function noteActivity() {
  clearTimeout(idleTimer);
  idleTimer = setTimeout(() => {
    if (!speaking && tauri?.core) tauri.core.invoke("say", { category: "idle" });
  }, IDLE_AFTER_MS);
}

async function loadManifest() {
  const response = await fetch("avatar/manifest.json");
  if (!response.ok) throw new Error(`manifest ${response.status}`);
  return response.json();
}

async function redock() {
  if (!tauri?.core) return;
  try {
    await tauri.core.invoke("dock");
  } catch {
    /* no hyprctl on this compositor: stay where the window manager put us */
  }
}

async function start() {
  const manifest = await loadManifest();
  frames = new Map(Object.entries(manifest.states));

  sprite.addEventListener("error", () => {
    stage.dataset.fallback = "true";
  });

  if (tauri?.core) {
    await tauri.event.listen("nerea://state", (event) => {
      turnFrame = event.payload.state;
      noteActivity();
      if (!speaking) show(turnFrame);
    });

    await tauri.event.listen("nerea://say", (event) => playLine(event.payload));
  }

  stage.addEventListener("dblclick", redock);

  stage.addEventListener("contextmenu", (event) => {
    event.preventDefault();
    if (tauri?.core) tauri.core.invoke("open_composer");
  });

  show("idle");
  noteActivity();
  await redock();

  if (tauri?.core) {
    try {
      await tauri.core.invoke("say", { category: "boot" });
    } catch {
      /* the bank is optional */
    }
  }
}

start().catch((error) => {
  console.error("avatar failed to start", error);
  document.body.dataset.error = "true";
});
